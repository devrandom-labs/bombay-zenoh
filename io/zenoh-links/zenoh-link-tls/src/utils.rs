//
// Copyright (c) 2024 ZettaScale Technology
//
// This program and the accompanying materials are made available under the
// terms of the Eclipse Public License 2.0 which is available at
// http://www.eclipse.org/legal/epl-2.0, or the Apache License, Version 2.0
// which is available at https://www.apache.org/licenses/LICENSE-2.0.
//
// SPDX-License-Identifier: EPL-2.0 OR Apache-2.0
//
// Contributors:
//   ZettaScale Zenoh Team, <zenoh@zettascale.tech>
//
use std::{
    convert::TryFrom,
    fs::File,
    io::{self, BufReader, Cursor},
    net::SocketAddr,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer, TrustAnchor},
    server::WebPkiClientVerifier,
    version::TLS13,
    ClientConfig, RootCertStore, ServerConfig,
};
use rustls_pki_types::{
    pem::{self, ReadIter},
    PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, PrivateSec1KeyDer, ServerName,
};
use secrecy::ExposeSecret;
use webpki::anchor_from_trusted_cert;
use zenoh_config::Config as ZenohConfig;
use zenoh_link_commons::{
    parse_dscp,
    tcp::TcpSocketConfig,
    tls::{
        config::{self, *},
        WebPkiVerifierAnyServerName,
    },
    ConfigurationInspector, BIND_INTERFACE, BIND_SOCKET, TCP_SO_RCV_BUF, TCP_SO_SND_BUF,
};
use zenoh_protocol::core::{
    endpoint::{Address, Config},
    parameters,
};
use zenoh_result::{bail, zerror, ZError, ZResult};

#[derive(Default, Clone, Copy, Debug)]
pub struct TlsConfigurator;

impl ConfigurationInspector<ZenohConfig> for TlsConfigurator {
    fn inspect_config(&self, config: &ZenohConfig) -> ZResult<String> {
        let mut ps: Vec<(&str, &str)> = vec![];

        let c = config.transport().link().tls();

        match (c.root_ca_certificate(), c.root_ca_certificate_base64()) {
            (Some(_), Some(_)) => {
                bail!("Only one between 'root_ca_certificate' and 'root_ca_certificate_base64' can be present!")
            }
            (Some(ca_certificate), None) => {
                ps.push((TLS_ROOT_CA_CERTIFICATE_FILE, ca_certificate));
            }
            (None, Some(ca_certificate)) => {
                ps.push((
                    TLS_ROOT_CA_CERTIFICATE_BASE64,
                    ca_certificate.expose_secret(),
                ));
            }
            _ => {}
        }

        match (c.listen_private_key(), c.listen_private_key_base64()) {
            (Some(_), Some(_)) => {
                bail!("Only one between 'listen_private_key' and 'listen_private_key' can be present!")
            }
            (Some(server_private_key), None) => {
                ps.push((TLS_LISTEN_PRIVATE_KEY_FILE, server_private_key));
            }
            (None, Some(server_private_key)) => {
                ps.push((
                    TLS_LISTEN_PRIVATE_KEY_BASE64,
                    server_private_key.expose_secret(),
                ));
            }
            _ => {}
        }

        match (c.listen_certificate(), c.listen_certificate_base64()) {
            (Some(_), Some(_)) => {
                bail!("Only one between 'listen_certificate' and 'listen_certificate_base64' can be present!")
            }
            (Some(server_certificate), None) => {
                ps.push((TLS_LISTEN_CERTIFICATE_FILE, server_certificate));
            }
            (None, Some(server_certificate)) => {
                ps.push((
                    TLS_LISTEN_CERTIFICATE_BASE64,
                    server_certificate.expose_secret(),
                ));
            }
            _ => {}
        }

        match c.enable_mtls().unwrap_or(TLS_ENABLE_MTLS_DEFAULT) {
            true => ps.push((TLS_ENABLE_MTLS, "true")),
            false => ps.push((TLS_ENABLE_MTLS, "false")),
        }

        match (c.connect_private_key(), c.connect_private_key_base64()) {
            (Some(_), Some(_)) => {
                bail!("Only one between 'connect_private_key' and 'connect_private_key_base64' can be present!")
            }
            (Some(client_private_key), None) => {
                ps.push((TLS_CONNECT_PRIVATE_KEY_FILE, client_private_key));
            }
            (None, Some(client_private_key)) => {
                ps.push((
                    TLS_CONNECT_PRIVATE_KEY_BASE64,
                    client_private_key.expose_secret(),
                ));
            }
            _ => {}
        }

        match (c.connect_certificate(), c.connect_certificate_base64()) {
            (Some(_), Some(_)) => {
                bail!("Only one between 'connect_certificate' and 'connect_certificate_base64' can be present!")
            }
            (Some(client_certificate), None) => {
                ps.push((TLS_CONNECT_CERTIFICATE_FILE, client_certificate));
            }
            (None, Some(client_certificate)) => {
                ps.push((
                    TLS_CONNECT_CERTIFICATE_BASE64,
                    client_certificate.expose_secret(),
                ));
            }
            _ => {}
        }

        match c
            .verify_name_on_connect()
            .unwrap_or(TLS_VERIFY_NAME_ON_CONNECT_DEFAULT)
        {
            true => ps.push((TLS_VERIFY_NAME_ON_CONNECT, "true")),
            false => ps.push((TLS_VERIFY_NAME_ON_CONNECT, "false")),
        };

        match c
            .close_link_on_expiration()
            .unwrap_or(TLS_CLOSE_LINK_ON_EXPIRATION_DEFAULT)
        {
            true => ps.push((TLS_CLOSE_LINK_ON_EXPIRATION, "true")),
            false => ps.push((TLS_CLOSE_LINK_ON_EXPIRATION, "false")),
        }

        match c.use_public_pki().unwrap_or(TLS_USE_PUBLIC_PKI_DEFAULT) {
            true => ps.push((TLS_USE_PUBLIC_PKI, "true")),
            false => ps.push((TLS_USE_PUBLIC_PKI, "false")),
        }

        let rx_buffer_size;
        if let Some(size) = c.so_rcvbuf() {
            rx_buffer_size = size.to_string();
            ps.push((TCP_SO_RCV_BUF, &rx_buffer_size));
        }

        let tx_buffer_size;
        if let Some(size) = c.so_sndbuf() {
            tx_buffer_size = size.to_string();
            ps.push((TCP_SO_SND_BUF, &tx_buffer_size));
        }

        Ok(parameters::from_iter(ps.drain(..)))
    }
}

pub(crate) struct TlsServerConfig<'a> {
    pub(crate) server_config: ServerConfig,
    pub(crate) tls_handshake_timeout: Duration,
    pub(crate) tls_close_link_on_expiration: bool,
    pub(crate) tcp_socket_config: TcpSocketConfig<'a>,
}

impl<'a> TlsServerConfig<'a> {
    pub async fn new(config: &'a Config<'_>) -> ZResult<Self> {
        let tls_server_client_auth: bool = match config.get(TLS_ENABLE_MTLS) {
            Some(s) => s
                .parse()
                .map_err(|_| zerror!("Unknown enable mTLS argument: {}", s))?,
            None => TLS_ENABLE_MTLS_DEFAULT,
        };
        let tls_close_link_on_expiration: bool = match config.get(TLS_CLOSE_LINK_ON_EXPIRATION) {
            Some(s) => s
                .parse()
                .map_err(|_| zerror!("Unknown close on expiration argument: {}", s))?,
            None => TLS_CLOSE_LINK_ON_EXPIRATION_DEFAULT,
        };
        let tls_server_private_key = TlsServerConfig::load_tls_private_key(config).await?;
        let tls_server_certificate = TlsServerConfig::load_tls_certificate(config).await?;

        let certs: Vec<CertificateDer> =
            ReadIter::<_, CertificateDer<'static>>::new(&mut Cursor::new(&tls_server_certificate))
                .collect::<Result<_, _>>()
                .map_err(|err| zerror!(err => "Error processing server certificate"))?;

        let mut keys = preferred_private_keys(&tls_server_private_key)
            .map_err(|err| zerror!(err => "Error processing server key"))?;

        if keys.is_empty() {
            bail!("No private key found for TLS server.");
        }

        // Install ring based rustls CryptoProvider.
        rustls::crypto::ring::default_provider()
            // This can be called successfully at most once in any process execution.
            // Call this early in your process to configure which provider is used for the provider.
            // The configuration should happen before any use of ClientConfig::builder() or ServerConfig::builder().
            .install_default()
            // Ignore the error here, because `rustls::crypto::ring::default_provider().install_default()` will inevitably be executed multiple times
            // when there are multiple quic links, and all but the first execution will fail.
            .ok();

        let sc = if tls_server_client_auth {
            let root_cert_store = load_trust_anchors(config)?.map_or_else(
                || Err(zerror!("Missing root certificates while mTLS is enabled.")),
                Ok,
            )?;
            let client_auth = WebPkiClientVerifier::builder(root_cert_store.into()).build()?;
            ServerConfig::builder_with_protocol_versions(&[&TLS13])
                .with_client_cert_verifier(client_auth)
                .with_single_cert(certs, keys.remove(0))
                .map_err(|e| zerror!(e))?
        } else {
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, keys.remove(0))
                .map_err(|e| zerror!(e))?
        };

        let tls_handshake_timeout = Duration::from_millis(
            config
                .get(config::TLS_HANDSHAKE_TIMEOUT_MS)
                .map(u64::from_str)
                .transpose()?
                .unwrap_or(config::TLS_HANDSHAKE_TIMEOUT_MS_DEFAULT),
        );

        let mut tcp_rx_buffer_size = None;
        if let Some(size) = config.get(TCP_SO_RCV_BUF) {
            tcp_rx_buffer_size = Some(
                size.parse()
                    .map_err(|_| zerror!("Unknown TCP read buffer size argument: {}", size))?,
            );
        };
        let mut tcp_tx_buffer_size = None;
        if let Some(size) = config.get(TCP_SO_SND_BUF) {
            tcp_tx_buffer_size = Some(
                size.parse()
                    .map_err(|_| zerror!("Unknown TCP write buffer size argument: {}", size))?,
            );
        };
        let mut bind_socket = None;
        if let Some(bind_socket_str) = config.get(BIND_SOCKET) {
            bind_socket = Some(get_tls_addr(&Address::from(bind_socket_str)).await?);
        };

        Ok(TlsServerConfig {
            server_config: sc,
            tls_handshake_timeout,
            tls_close_link_on_expiration,
            tcp_socket_config: TcpSocketConfig::new(
                tcp_tx_buffer_size,
                tcp_rx_buffer_size,
                config.get(BIND_INTERFACE),
                bind_socket,
                parse_dscp(config)?,
            ),
        })
    }

    async fn load_tls_private_key(config: &Config<'_>) -> ZResult<Vec<u8>> {
        load_tls_key(
            config,
            TLS_LISTEN_PRIVATE_KEY_RAW,
            TLS_LISTEN_PRIVATE_KEY_FILE,
            TLS_LISTEN_PRIVATE_KEY_BASE64,
        )
        .await
    }

    async fn load_tls_certificate(config: &Config<'_>) -> ZResult<Vec<u8>> {
        load_tls_certificate(
            config,
            TLS_LISTEN_CERTIFICATE_RAW,
            TLS_LISTEN_CERTIFICATE_FILE,
            TLS_LISTEN_CERTIFICATE_BASE64,
        )
        .await
    }
}

pub(crate) struct TlsClientConfig<'a> {
    pub(crate) client_config: ClientConfig,
    pub(crate) tls_close_link_on_expiration: bool,
    pub(crate) tcp_socket_config: TcpSocketConfig<'a>,
}

impl<'a> TlsClientConfig<'a> {
    pub async fn new(config: &'a Config<'_>) -> ZResult<Self> {
        let tls_client_server_auth: bool = match config.get(TLS_ENABLE_MTLS) {
            Some(s) => s
                .parse()
                .map_err(|_| zerror!("Unknown enable mTLS auth argument: {}", s))?,
            None => TLS_ENABLE_MTLS_DEFAULT,
        };

        let tls_server_name_verification: bool = match config.get(TLS_VERIFY_NAME_ON_CONNECT) {
            Some(s) => s
                .parse()
                .map_err(|_| zerror!("Unknown server name verification argument: {}", s))?,
            None => TLS_VERIFY_NAME_ON_CONNECT_DEFAULT,
        };
        if !tls_server_name_verification {
            tracing::warn!("Skipping name verification of TLS server");
        }

        let tls_close_link_on_expiration: bool = match config.get(TLS_CLOSE_LINK_ON_EXPIRATION) {
            Some(s) => s
                .parse()
                .map_err(|_| zerror!("Unknown close on expiration argument: {}", s))?,
            None => TLS_CLOSE_LINK_ON_EXPIRATION_DEFAULT,
        };

        let root_cert_store = outgoing_trust_anchors(config)?;

        // Install ring based rustls CryptoProvider.
        rustls::crypto::ring::default_provider()
            // This can be called successfully at most once in any process execution.
            // Call this early in your process to configure which provider is used for the provider.
            // The configuration should happen before any use of ClientConfig::builder() or ServerConfig::builder().
            .install_default()
            // Ignore the error here, because `rustls::crypto::ring::default_provider().install_default()` will inevitably be executed multiple times
            // when there are multiple quic links, and all but the first execution will fail.
            .ok();

        let cc = if tls_client_server_auth {
            tracing::debug!("Loading client authentication key and certificate...");
            let tls_client_private_key = TlsClientConfig::load_tls_private_key(config).await?;
            let tls_client_certificate = TlsClientConfig::load_tls_certificate(config).await?;

            let certs: Vec<CertificateDer> = ReadIter::<_, CertificateDer<'static>>::new(
                &mut Cursor::new(&tls_client_certificate),
            )
            .collect::<Result<_, _>>()
            .map_err(|err| zerror!(err => "Error processing client certificate"))?;

            let mut keys = preferred_private_keys(&tls_client_private_key)
                .map_err(|err| zerror!(err => "Error processing client key"))?;

            if keys.is_empty() {
                bail!("No private key found for TLS client.");
            }

            let builder = ClientConfig::builder_with_protocol_versions(&[&TLS13]);

            if tls_server_name_verification {
                builder
                    .with_root_certificates(root_cert_store)
                    .with_client_auth_cert(certs, keys.remove(0))
            } else {
                builder
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(WebPkiVerifierAnyServerName::new(
                        root_cert_store,
                    )))
                    .with_client_auth_cert(certs, keys.remove(0))
            }
            .map_err(|e| zerror!("Bad certificate/key: {}", e))?
        } else {
            let builder = ClientConfig::builder();
            if tls_server_name_verification {
                builder
                    .with_root_certificates(root_cert_store)
                    .with_no_client_auth()
            } else {
                builder
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(WebPkiVerifierAnyServerName::new(
                        root_cert_store,
                    )))
                    .with_no_client_auth()
            }
        };

        let mut tcp_rx_buffer_size = None;
        if let Some(size) = config.get(TCP_SO_RCV_BUF) {
            tcp_rx_buffer_size = Some(
                size.parse()
                    .map_err(|_| zerror!("Unknown TCP read buffer size argument: {}", size))?,
            );
        };
        let mut tcp_tx_buffer_size = None;
        if let Some(size) = config.get(TCP_SO_SND_BUF) {
            tcp_tx_buffer_size = Some(
                size.parse()
                    .map_err(|_| zerror!("Unknown TCP write buffer size argument: {}", size))?,
            );
        };
        let mut bind_socket = None;
        if let Some(bind_socket_str) = config.get(BIND_SOCKET) {
            bind_socket = Some(get_tls_addr(&Address::from(bind_socket_str)).await?);
        };

        Ok(TlsClientConfig {
            client_config: cc,
            tls_close_link_on_expiration,
            tcp_socket_config: TcpSocketConfig::new(
                tcp_tx_buffer_size,
                tcp_rx_buffer_size,
                config.get(BIND_INTERFACE),
                bind_socket,
                parse_dscp(config)?,
            ),
        })
    }

    async fn load_tls_private_key(config: &Config<'_>) -> ZResult<Vec<u8>> {
        load_tls_key(
            config,
            TLS_CONNECT_PRIVATE_KEY_RAW,
            TLS_CONNECT_PRIVATE_KEY_FILE,
            TLS_CONNECT_PRIVATE_KEY_BASE64,
        )
        .await
    }

    async fn load_tls_certificate(config: &Config<'_>) -> ZResult<Vec<u8>> {
        load_tls_certificate(
            config,
            TLS_CONNECT_CERTIFICATE_RAW,
            TLS_CONNECT_CERTIFICATE_FILE,
            TLS_CONNECT_CERTIFICATE_BASE64,
        )
        .await
    }
}

fn preferred_private_keys(pem: &[u8]) -> Result<Vec<PrivateKeyDer<'static>>, pem::Error> {
    let mut keys = ReadIter::<_, PrivatePkcs1KeyDer<'static>>::new(&mut Cursor::new(pem))
        .map(|key| key.map(PrivateKeyDer::from))
        .collect::<Result<Vec<_>, _>>()?;
    if keys.is_empty() {
        keys = ReadIter::<_, PrivatePkcs8KeyDer<'static>>::new(&mut Cursor::new(pem))
            .map(|key| key.map(PrivateKeyDer::from))
            .collect::<Result<_, _>>()?;
    }
    if keys.is_empty() {
        keys = ReadIter::<_, PrivateSec1KeyDer<'static>>::new(&mut Cursor::new(pem))
            .map(|key| key.map(PrivateKeyDer::from))
            .collect::<Result<_, _>>()?;
    }
    Ok(keys)
}

fn process_pem(pem: &mut dyn io::BufRead) -> ZResult<Vec<TrustAnchor<'static>>> {
    let certs: Vec<CertificateDer> = ReadIter::<_, CertificateDer<'static>>::new(pem)
        .map(|result| result.map_err(|err| zerror!(err => "Error processing PEM certificates")))
        .collect::<Result<Vec<CertificateDer>, ZError>>()?;

    let trust_anchors: Vec<TrustAnchor> = certs
        .into_iter()
        .map(|cert| {
            anchor_from_trusted_cert(&cert)
                .map_err(|err| zerror!("Error processing trust anchor: {err}."))
                .map(|trust_anchor| trust_anchor.to_owned())
        })
        .collect::<Result<Vec<TrustAnchor>, ZError>>()?;

    Ok(trust_anchors)
}

async fn load_tls_key(
    config: &Config<'_>,
    tls_private_key_raw_config_key: &str,
    tls_private_key_file_config_key: &str,
    tls_private_key_base64_config_key: &str,
) -> ZResult<Vec<u8>> {
    if let Some(value) = config.get(tls_private_key_raw_config_key) {
        return Ok(value.as_bytes().to_vec());
    }

    if let Some(b64_key) = config.get(tls_private_key_base64_config_key) {
        return base64_decode(b64_key);
    }

    if let Some(value) = config.get(tls_private_key_file_config_key) {
        return Ok(tokio::fs::read(value)
            .await
            .map_err(|e| zerror!("Invalid TLS private key file: {}", e))?)
        .and_then(|result| {
            if result.is_empty() {
                Err(zerror!("Empty TLS key.").into())
            } else {
                Ok(result)
            }
        });
    }
    Err(zerror!("Missing TLS private key.").into())
}

async fn load_tls_certificate(
    config: &Config<'_>,
    tls_certificate_raw_config_key: &str,
    tls_certificate_file_config_key: &str,
    tls_certificate_base64_config_key: &str,
) -> ZResult<Vec<u8>> {
    if let Some(value) = config.get(tls_certificate_raw_config_key) {
        return Ok(value.as_bytes().to_vec());
    }

    if let Some(b64_certificate) = config.get(tls_certificate_base64_config_key) {
        return base64_decode(b64_certificate);
    }

    if let Some(value) = config.get(tls_certificate_file_config_key) {
        return Ok(tokio::fs::read(value)
            .await
            .map_err(|e| zerror!("Invalid TLS certificate file: {}", e))?);
    }
    Err(zerror!("Missing tls certificates.").into())
}

fn outgoing_trust_anchors(config: &Config<'_>) -> ZResult<RootCertStore> {
    let tls_use_public_pki: bool = match config.get(TLS_USE_PUBLIC_PKI) {
        Some(s) => s
            .parse()
            .map_err(|_| zerror!("Unknown use public pki argument: {}", s))?,
        None => TLS_USE_PUBLIC_PKI_DEFAULT,
    };

    let mut root_cert_store = RootCertStore::empty();
    // Allows mixed user-generated CA and webPKI CA
    if tls_use_public_pki {
        tracing::debug!("Loading default Web PKI certificates.");
        root_cert_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }

    if let Some(custom_root_cert) = load_trust_anchors(config)? {
        tracing::debug!("Loading user-generated certificates.");
        root_cert_store.extend(custom_root_cert.roots);
    }

    if root_cert_store.is_empty() {
        return Err(zerror!("No root CA certificates loaded: 'use_public_pki' is disabled and no root CA certificate was configured").into());
    }

    Ok(root_cert_store)
}

fn load_trust_anchors(config: &Config<'_>) -> ZResult<Option<RootCertStore>> {
    let mut root_cert_store = RootCertStore::empty();
    if let Some(value) = config.get(TLS_ROOT_CA_CERTIFICATE_RAW) {
        let mut pem = BufReader::new(value.as_bytes());
        let trust_anchors = process_pem(&mut pem)?;
        root_cert_store.extend(trust_anchors);
        return Ok(Some(root_cert_store));
    }

    if let Some(b64_certificate) = config.get(TLS_ROOT_CA_CERTIFICATE_BASE64) {
        let certificate_pem = base64_decode(b64_certificate)?;
        let mut pem = BufReader::new(certificate_pem.as_slice());
        let trust_anchors = process_pem(&mut pem)?;
        root_cert_store.extend(trust_anchors);
        return Ok(Some(root_cert_store));
    }

    if let Some(filename) = config.get(TLS_ROOT_CA_CERTIFICATE_FILE) {
        let mut pem = BufReader::new(File::open(filename)?);
        let trust_anchors = process_pem(&mut pem)?;
        root_cert_store.extend(trust_anchors);
        return Ok(Some(root_cert_store));
    }
    Ok(None)
}

pub fn base64_decode(data: &str) -> ZResult<Vec<u8>> {
    use base64::{engine::general_purpose, Engine};
    Ok(general_purpose::STANDARD
        .decode(data)
        .map_err(|e| zerror!("Unable to perform base64 decoding: {e:?}"))?)
}

pub async fn get_tls_addr(address: &Address<'_>) -> ZResult<SocketAddr> {
    match tokio::net::lookup_host(address.as_str()).await?.next() {
        Some(addr) => Ok(addr),
        None => bail!("Couldn't resolve TLS locator address: {}", address),
    }
}

pub fn get_tls_host<'a>(address: &'a Address<'a>) -> ZResult<&'a str> {
    Ok(address
        .as_str()
        .rsplit_once(':')
        .ok_or_else(|| zerror!("Invalid TLS address"))?
        .0)
}

pub fn get_tls_server_name<'a>(address: &'a Address<'a>) -> ZResult<ServerName<'a>> {
    Ok(ServerName::try_from(get_tls_host(address)?).map_err(|e| zerror!(e))?)
}

#[cfg(test)]
mod outgoing_tls_trust_tests {
    use std::io::Cursor;

    use zenoh_config::Config as ZenohConfig;
    use zenoh_link_commons::ConfigurationInspector;
    use zenoh_protocol::core::EndPoint;

    use super::{outgoing_trust_anchors, process_pem, TlsClientConfig, TlsConfigurator};

    const EMPTY_OUTGOING_TRUST: &str = "No root CA certificates loaded: 'use_public_pki' is disabled and no root CA certificate was configured";
    // Existing public minica CA fixture from the owning transport tests.
    // Embedded so these tests remain inside the published TLS package.
    pub(super) const DEPLOYMENT_CA: &str = "-----BEGIN CERTIFICATE-----
MIIDSzCCAjOgAwIBAgIIB42n1ZIkOakwDQYJKoZIhvcNAQELBQAwIDEeMBwGA1UE
AxMVbWluaWNhIHJvb3QgY2EgMDc4ZGE3MCAXDTIzMDMwNjE2MDMwN1oYDzIxMjMw
MzA2MTYwMzA3WjAgMR4wHAYDVQQDExVtaW5pY2Egcm9vdCBjYSAwNzhkYTcwggEi
MA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQDIuCq24O4P4Aep5vAVlrIQ7P8+
uWWgcHIFYa02TmhBUB/hjo0JANCQvAtpVNuQ8NyKPlqnnq1cttePbSYVeA0rrnOs
DcfySAiyGBEY9zMjFfHJtH1wtrPcJEU8XIEY3xUlrAJE2CEuV9dVYgfEEydnvgLc
8Ug0WXSiARjqbnMW3l8jh6bYCp/UpL/gSM4mxdKrgpfyPoweGhlOWXc3RTS7cqM9
T25acURGOSI6/g8GF0sNE4VZmUvHggSTmsbLeXMJzxDWO+xVehRmbQx3IkG7u++b
QdRwGIJcDNn7zHlDMHtQ0Z1DBV94fZNBwCULhCBB5g20XTGw//S7Fj2FPwyhAgMB
AAGjgYYwgYMwDgYDVR0PAQH/BAQDAgKEMB0GA1UdJQQWMBQGCCsGAQUFBwMBBggr
BgEFBQcDAjASBgNVHRMBAf8ECDAGAQH/AgEAMB0GA1UdDgQWBBTWfAmQ/BUIQm/9
/llJJs2jUMWzGzAfBgNVHSMEGDAWgBTWfAmQ/BUIQm/9/llJJs2jUMWzGzANBgkq
hkiG9w0BAQsFAAOCAQEAvtcZFAELKiTuOiAeYts6zeKxc+nnHCzayDeD/BDCbxGJ
e1n+xdHjLtWGd+/Anc+fvftSYBPTFQqCi84lPiUIln5z/rUxE+ke81hNPIfw2obc
yIg87xCabQpVyEh8s+MV+7YPQ1+fH4FuSi2Fck1FejxkVqN2uOZPvOYUmSTsaVr1
8SfRnwJNZ9UMRPM2bD4Jkvj0VcL42JM3QkOClOzYW4j/vll2cSs4kx7er27cIoo1
Ck0v2xSPAiVjg6w65rUQeW6uB5m0T2wyj+wm0At8vzhZPlgS1fKhcmT2dzOq3+oN
R+IdLiXcyIkg0m9N8I17p0ljCSkbrgGMD3bbePRTfg==
-----END CERTIFICATE-----";

    #[tokio::test]
    async fn configured_only_outgoing_tls_requires_a_trust_anchor() {
        for (selection, expected_certificate) in [
            ("use_public_pki=false", None),
            ("use_public_pki=false;root_ca_certificate_raw=", Some("")),
        ] {
            let endpoint = EndPoint::new("tls", "localhost:7447", "", selection).unwrap();
            let configuration = endpoint.config();
            assert_eq!(
                configuration.get("root_ca_certificate_raw"),
                expected_certificate
            );
            let construction = TlsClientConfig::new(&configuration).await;
            let refusal = match construction {
                Err(refusal) => refusal,
                Ok(_) => panic!("configured-only outgoing TLS silently loaded public roots"),
            };
            assert!(refusal.to_string().contains(EMPTY_OUTGOING_TRUST));
        }
    }

    #[tokio::test]
    async fn legacy_outgoing_tls_can_use_public_roots_without_a_private_ca() {
        for selection in ["", "use_public_pki=true"] {
            let endpoint = EndPoint::new("tls", "localhost:7447", "", selection).unwrap();
            let configuration = endpoint.config();
            let construction = TlsClientConfig::new(&configuration).await;
            assert!(construction.is_ok());
            let roots = outgoing_trust_anchors(&configuration).unwrap();
            assert!(roots.roots == webpki_roots::TLS_SERVER_ROOTS);
        }
    }

    #[tokio::test]
    async fn configured_only_outgoing_tls_preserves_exact_deployment_anchors() {
        let selection = format!("use_public_pki=false;root_ca_certificate_raw={DEPLOYMENT_CA}");
        let endpoint = EndPoint::new("tls", "localhost:7447", "", selection).unwrap();
        let configuration = endpoint.config();
        let expected = process_pem(&mut Cursor::new(DEPLOYMENT_CA)).unwrap();
        assert_eq!(expected.len(), 1);
        let roots = outgoing_trust_anchors(&configuration).unwrap();
        assert!(
            roots.roots == expected,
            "configured-only outgoing roots included another anchor"
        );
        let construction = TlsClientConfig::new(&configuration).await;
        assert!(construction.is_ok());
    }

    #[test]
    fn legacy_outgoing_tls_preserves_public_and_deployment_anchors() {
        let deployment = process_pem(&mut Cursor::new(DEPLOYMENT_CA)).unwrap();
        let mut expected = webpki_roots::TLS_SERVER_ROOTS.to_vec();
        expected.extend(deployment);
        for selection in [
            format!("root_ca_certificate_raw={DEPLOYMENT_CA}"),
            format!("use_public_pki=true;root_ca_certificate_raw={DEPLOYMENT_CA}"),
        ] {
            let endpoint = EndPoint::new("tls", "localhost:7447", "", selection).unwrap();
            let configuration = endpoint.config();
            let roots = outgoing_trust_anchors(&configuration).unwrap();
            assert!(roots.roots == expected);
        }
    }

    #[test]
    fn malformed_public_trust_selection_is_refused() {
        let endpoint =
            EndPoint::new("tls", "localhost:7447", "", "use_public_pki=malformed").unwrap();
        let configuration = endpoint.config();
        let refusal = outgoing_trust_anchors(&configuration).unwrap_err();
        assert!(refusal
            .to_string()
            .starts_with("Unknown use public pki argument: malformed at "));
    }

    #[test]
    fn malformed_configured_anchor_never_falls_back_to_public_roots() {
        for selection in ["false", "true"] {
            let certificate = "-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----";
            let selection =
                format!("use_public_pki={selection};root_ca_certificate_raw={certificate}");
            let endpoint = EndPoint::new("tls", "localhost:7447", "", selection).unwrap();
            let configuration = endpoint.config();
            let refusal = outgoing_trust_anchors(&configuration).unwrap_err();
            assert!(refusal
                .to_string()
                .starts_with("Error processing trust anchor:"));
        }
    }

    #[test]
    fn deployment_only_selection_reaches_outgoing_endpoint() {
        let mut session = ZenohConfig::default();
        session
            .insert_json5("transport/link/tls/use_public_pki", "false")
            .unwrap();
        let selection = TlsConfigurator.inspect_config(&session).unwrap();
        let endpoint = EndPoint::new("tls", "localhost:7447", "", selection).unwrap();
        let configuration = endpoint.config();
        assert_eq!(configuration.get("use_public_pki"), Some("false"));
        let refusal = outgoing_trust_anchors(&configuration).unwrap_err();
        assert!(refusal.to_string().contains(EMPTY_OUTGOING_TRUST));
    }
}

#[cfg(test)]
mod tls_pem_tests {
    use std::{
        io::{self, BufReader, Cursor, Read},
        ptr,
    };

    use base64::{engine::general_purpose, Engine};
    use rustls_pki_types::{
        pem::{self, ReadIter},
        CertificateDer, PrivateKeyDer,
        PrivateKeyDer::{Pkcs1, Pkcs8, Sec1},
        TrustAnchor,
    };
    use zenoh_protocol::core::EndPoint;

    use super::{
        outgoing_tls_trust_tests::DEPLOYMENT_CA, preferred_private_keys, process_pem,
        TlsClientConfig, TlsServerConfig,
    };

    const RSA: &str = "-----BEGIN RSA PRIVATE KEY-----\nAQID\n-----END RSA PRIVATE KEY-----\n";
    const RSA_SECOND: &str =
        "-----BEGIN RSA PRIVATE KEY-----\nBAUG\n-----END RSA PRIVATE KEY-----\n";
    const PKCS8: &str = "-----BEGIN PRIVATE KEY-----\nBwgJ\n-----END PRIVATE KEY-----\n";
    const SEC1: &str = "-----BEGIN EC PRIVATE KEY-----\nCgsM\n-----END EC PRIVATE KEY-----\n";
    const CERTIFICATE: &str = "-----BEGIN CERTIFICATE-----\nDQ4P\n-----END CERTIFICATE-----\n";
    const CERTIFICATE_NEXT: &str = "-----BEGIN CERTIFICATE-----\nEBES\n-----END CERTIFICATE-----\n";
    const UNKNOWN: &str = "-----BEGIN UNKNOWN-----\nnot-base64!\n-----END UNKNOWN-----\n";
    const MALFORMED: &str = "-----BEGIN CERTIFICATE-----\n!\n-----END CERTIFICATE-----\n";
    const MATCHING_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgK7z8MHmX75qSSGh9
4wpqRXA0d3Bsb5tFp9kqHmmCYwuhRANCAAQhiWwnr2iHk/bV86RMYfuntz4o+gPH
ayFAA6DbVZnxvNz4MxY889I7HSrD5iqtO8iaxlQeavx9F1Ff73agYgBj
-----END PRIVATE KEY-----";
    const MATCHING_CERTIFICATE: &str = "-----BEGIN CERTIFICATE-----
MIICEjCCAbigAwIBAgIUK+gTxmEapjy9K2AOKy+FaWkjACkwCgYIKoZIzj0EAwMw
RTELMAkGA1UEBhMCQVUxEzARBgNVBAgMClNvbWUtU3RhdGUxITAfBgNVBAoMGElu
dGVybmV0IFdpZGdpdHMgUHR5IEx0ZDAgFw0yNjAyMDUxMjQ5MTNaGA8zMDI1MDYw
ODEyNDkxM1owRTELMAkGA1UEBhMCQVUxEzARBgNVBAgMClNvbWUtU3RhdGUxITAf
BgNVBAoMGEludGVybmV0IFdpZGdpdHMgUHR5IEx0ZDBZMBMGByqGSM49AgEGCCqG
SM49AwEHA0IABCGJbCevaIeT9tXzpExh+6e3Pij6A8drIUADoNtVmfG83PgzFjzz
0jsdKsPmKq07yJrGVB5q/H0XUV/vdqBiAGOjgYMwgYAwHQYDVR0OBBYEFITqkHg7
V7s6YEFEjayZ1x0nZ7yVMB8GA1UdIwQYMBaAFC1uOxD1e2c3O9JWFJpbYrk+VQ99
MAkGA1UdEwQCMAAwFAYDVR0RBA0wC4IJbG9jYWxob3N0MB0GA1UdJQQWMBQGCCsG
AQUFBwMBBggrBgEFBQcDAjAKBggqhkjOPQQDAwNIADBFAiARmqtIYRygkWrMZg6j
kYikJVUysUelvUMRfljBCbB1jgIhALzl57XtBhJ+QVy5jNMKjU6s9yfCvnzZPTpk
3Z0eRdp3
-----END CERTIFICATE-----";

    #[test]
    fn process_pem_preserves_complete_ordered_trust_anchors() {
        // Literal ASN.1 field contents independently extracted with OpenSSL from these fixtures.
        let expected = vec![
            TrustAnchor {
                subject: general_purpose::STANDARD.decode("MR4wHAYDVQQDExVtaW5pY2Egcm9vdCBjYSAwNzhkYTc=").unwrap().into(),
                subject_public_key_info: general_purpose::STANDARD.decode("MA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQDIuCq24O4P4Aep5vAVlrIQ7P8+uWWgcHIFYa02TmhBUB/hjo0JANCQvAtpVNuQ8NyKPlqnnq1cttePbSYVeA0rrnOsDcfySAiyGBEY9zMjFfHJtH1wtrPcJEU8XIEY3xUlrAJE2CEuV9dVYgfEEydnvgLc8Ug0WXSiARjqbnMW3l8jh6bYCp/UpL/gSM4mxdKrgpfyPoweGhlOWXc3RTS7cqM9T25acURGOSI6/g8GF0sNE4VZmUvHggSTmsbLeXMJzxDWO+xVehRmbQx3IkG7u++bQdRwGIJcDNn7zHlDMHtQ0Z1DBV94fZNBwCULhCBB5g20XTGw//S7Fj2FPwyhAgMBAAE=").unwrap().into(),
                name_constraints: None,
            },
            TrustAnchor {
                subject: general_purpose::STANDARD.decode("MQswCQYDVQQGEwJBVTETMBEGA1UECAwKU29tZS1TdGF0ZTEhMB8GA1UECgwYSW50ZXJuZXQgV2lkZ2l0cyBQdHkgTHRk").unwrap().into(),
                subject_public_key_info: general_purpose::STANDARD.decode("MBMGByqGSM49AgEGCCqGSM49AwEHA0IABCGJbCevaIeT9tXzpExh+6e3Pij6A8drIUADoNtVmfG83PgzFjzz0jsdKsPmKq07yJrGVB5q/H0XUV/vdqBiAGM=").unwrap().into(),
                name_constraints: None,
            },
        ];
        let input = format!("{DEPLOYMENT_CA}\n{MATCHING_CERTIFICATE}\n");
        let actual = process_pem(&mut Cursor::new(input)).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn process_pem_refuses_malformed_tail_after_complete_anchors() {
        let input = format!("{DEPLOYMENT_CA}\n{MATCHING_CERTIFICATE}\n{MALFORMED}");
        let refusal = process_pem(&mut Cursor::new(input)).unwrap_err();
        let cause = refusal
            .source()
            .unwrap()
            .downcast_ref::<pem::Error>()
            .unwrap();
        let pem::Error::Base64Decode(reason) = cause else {
            panic!("wrong native PEM cause: {cause}")
        };
        assert_eq!(reason, "InvalidCharacter(33)");
    }

    #[test]
    fn preferred_private_keys_preserves_complete_preferred_der() {
        for sections in [
            [RSA, PKCS8, SEC1],
            [RSA, SEC1, PKCS8],
            [PKCS8, RSA, SEC1],
            [PKCS8, SEC1, RSA],
            [SEC1, RSA, PKCS8],
            [SEC1, PKCS8, RSA],
        ] {
            let actual = preferred_private_keys(sections.concat().as_bytes()).unwrap();
            let expected = vec![Pkcs1(vec![1, 2, 3].into())];
            assert_eq!(actual, expected);
        }
        for (sections, expected) in [
            (vec![RSA], vec![Pkcs1(vec![1, 2, 3].into())]),
            (vec![PKCS8], vec![Pkcs8(vec![7, 8, 9].into())]),
            (
                vec![PKCS8, PKCS8],
                vec![Pkcs8(vec![7, 8, 9].into()), Pkcs8(vec![7, 8, 9].into())],
            ),
            (vec![SEC1], vec![Sec1(vec![10, 11, 12].into())]),
            (
                vec![SEC1, SEC1],
                vec![Sec1(vec![10, 11, 12].into()), Sec1(vec![10, 11, 12].into())],
            ),
            (vec![PKCS8, SEC1], vec![Pkcs8(vec![7, 8, 9].into())]),
            (vec![SEC1, PKCS8], vec![Pkcs8(vec![7, 8, 9].into())]),
            (
                vec![RSA_SECOND, SEC1, RSA],
                vec![Pkcs1(vec![4, 5, 6].into()), Pkcs1(vec![1, 2, 3].into())],
            ),
            (
                vec![UNKNOWN, CERTIFICATE, PKCS8],
                vec![Pkcs8(vec![7, 8, 9].into())],
            ),
        ] {
            let actual = preferred_private_keys(sections.concat().as_bytes()).unwrap();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn preferred_private_keys_refuses_malformed_sections_after_a_key() {
        for malformed in [MALFORMED, "-----BEGIN PRIVATE KEY----\n"] {
            for sections in [
                [malformed, RSA],
                [RSA, malformed],
                [PKCS8, malformed],
                [SEC1, malformed],
            ] {
                let actual = preferred_private_keys(sections.concat().as_bytes());
                assert!(actual.is_err(), "malformed recognized section was ignored");
            }
        }
        let actual = preferred_private_keys(b"-----BEGIN PRIVATE KEY-----\nAQID");
        assert!(actual.is_err(), "missing final footer was ignored");
        let input = ["-----BEGIN PRIVATE KEY-----\nAQID\n", RSA].concat();
        let actual = preferred_private_keys(input.as_bytes()).unwrap();
        assert_eq!(actual, vec![Pkcs1(vec![1, 2, 3, 1, 2, 3].into())]);
        for input in ["", "unrelated text", CERTIFICATE, UNKNOWN] {
            let actual = preferred_private_keys(input.as_bytes()).unwrap();
            assert_eq!(actual, Vec::<PrivateKeyDer>::new());
        }
        let actual = preferred_private_keys(b"-----BEGIN UNKNOWN-----\n!");
        assert!(actual.is_err());
    }

    #[test]
    fn typed_certificate_reader_preserves_complete_der() {
        let input = [CERTIFICATE, PKCS8, CERTIFICATE_NEXT, UNKNOWN].concat();
        let actual =
            ReadIter::<_, CertificateDer<'static>>::new(&mut Cursor::new(input.as_bytes()))
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
        let expected = vec![
            CertificateDer::from(vec![13, 14, 15]),
            CertificateDer::from(vec![16, 17, 18]),
        ];
        assert_eq!(actual, expected);
        let input = [CERTIFICATE, MALFORMED].concat();
        let actual =
            ReadIter::<_, CertificateDer<'static>>::new(&mut Cursor::new(input.as_bytes()))
                .collect::<Result<Vec<_>, _>>();
        assert!(actual.is_err());
    }

    #[tokio::test]
    async fn tls_constructors_preserve_key_selection_and_refusal_precedence() {
        for (key, certificate, refusals) in [
            (MATCHING_KEY.to_owned(), MATCHING_CERTIFICATE, None),
            (
                String::new(),
                MATCHING_CERTIFICATE,
                Some([
                    "No private key found for TLS server.",
                    "No private key found for TLS client.",
                ]),
            ),
            (
                MALFORMED.to_owned(),
                MALFORMED,
                Some([
                    "Error processing server certificate",
                    "Error processing client certificate",
                ]),
            ),
            (
                format!("{MATCHING_KEY}\n{MALFORMED}"),
                MATCHING_CERTIFICATE,
                Some(["Error processing server key", "Error processing client key"]),
            ),
            (
                format!("{MATCHING_KEY}\n{RSA}"),
                MATCHING_CERTIFICATE,
                Some(["private key", "Bad certificate/key"]),
            ),
        ] {
            let selection = format!("enable_mtls=true;use_public_pki=false;root_ca_certificate_raw={MATCHING_CERTIFICATE};listen_private_key_raw={key};listen_certificate_raw={certificate};connect_private_key_raw={key};connect_certificate_raw={certificate}");
            let endpoint = EndPoint::new("tls", "localhost:7447", "", &selection).unwrap();
            let configuration = endpoint.config();
            let server = TlsServerConfig::new(&configuration).await;
            let client = TlsClientConfig::new(&configuration).await;
            let outcomes = [server.map(drop), client.map(drop)];
            match refusals {
                None => {
                    for outcome in outcomes {
                        assert!(outcome.is_ok());
                    }
                }
                Some(expected) => {
                    for (outcome, prefix) in outcomes.into_iter().zip(expected) {
                        let refusal =
                            outcome.expect_err("TLS constructor changed its key/refusal selection");
                        assert!(refusal.to_string().contains(prefix), "{refusal}");
                    }
                }
            }
        }
        let selection = format!("enable_mtls=true;use_public_pki=false;root_ca_certificate_raw=;connect_private_key_raw={MALFORMED};connect_certificate_raw={MALFORMED}");
        let endpoint = EndPoint::new("tls", "localhost:7447", "", &selection).unwrap();
        let configuration = endpoint.config();
        let actual = TlsClientConfig::new(&configuration).await;
        let refusal = match actual {
            Err(refusal) => refusal,
            Ok(_) => panic!("empty private roots accepted"),
        };
        assert!(refusal
            .to_string()
            .contains("No root CA certificates loaded:"));
    }

    #[test]
    fn preferred_private_keys_preserves_native_parser_failures() {
        let missing = preferred_private_keys(b"-----BEGIN PRIVATE KEY-----\nAQID").unwrap_err();
        let pem::Error::MissingSectionEnd { end_marker } = missing else {
            panic!("wrong missing-footer cause: {missing}")
        };
        assert_eq!(end_marker, b"PRIVATE KEY");
        let illegal = preferred_private_keys(b"-----BEGIN PRIVATE KEY----").unwrap_err();
        let pem::Error::IllegalSectionStart { line } = illegal else {
            panic!("wrong malformed-BEGIN cause: {illegal}")
        };
        assert_eq!(line, b"-----BEGIN PRIVATE KEY----");
        let invalid = preferred_private_keys(MALFORMED.as_bytes()).unwrap_err();
        let pem::Error::Base64Decode(reason) = invalid else {
            panic!("wrong base64 cause: {invalid}")
        };
        assert_eq!(reason, "InvalidCharacter(33)");
    }

    struct CertificateReaderFailure(Option<io::Error>);
    impl Read for CertificateReaderFailure {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(self.0.take().expect("reader fault was consumed twice"))
        }
    }
    #[test]
    fn process_pem_preserves_original_failures() {
        let marker = Box::new(io::Error::other("original certificate read failure"));
        let original: *const io::Error = marker.as_ref();
        let failure = io::Error::new(io::ErrorKind::PermissionDenied, marker);
        let mut reader = BufReader::new(CertificateReaderFailure(Some(failure)));
        let refusal = process_pem(&mut reader).unwrap_err();
        let source = refusal.source().expect("native PEM cause was erased");
        let cause = source.downcast_ref::<pem::Error>().expect("PEM cause");
        let pem::Error::Io(error) = cause else {
            panic!("wrong reader cause: {cause}")
        };
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let source = error.get_ref().expect("original reader failure was erased");
        let marker = source.downcast_ref::<Box<io::Error>>().expect("I/O");
        assert!(ptr::eq(original, marker.as_ref()));
        assert_eq!(marker.to_string(), "original certificate read failure");
        let mut reader = Cursor::new(b"-----BEGIN CERTIFICATE-----\nAQID");
        let refusal = process_pem(&mut reader).unwrap_err();
        let source = refusal.source().expect("native PEM cause was erased");
        let cause = source.downcast_ref::<pem::Error>().expect("PEM cause");
        let pem::Error::MissingSectionEnd { end_marker } = cause else {
            panic!("wrong root certificate cause: {cause}")
        };
        assert_eq!(end_marker, b"CERTIFICATE");
    }
}
