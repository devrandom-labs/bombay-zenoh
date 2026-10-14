use super::{get_client_cert_common_name, get_server_cert_common_name, LinkUnicastTls};
use crate::utils::{TlsClientConfig, TlsServerConfig};
use std::{
    future::{poll_fn, ready, Future},
    io::{Error, ErrorKind},
    path::Path,
    pin::pin,
    sync::Arc,
    task::Poll,
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_rustls::{rustls::pki_types::ServerName, TlsAcceptor, TlsConnector, TlsStream};
use zenoh_link_commons::LinkUnicastTrait;
use zenoh_protocol::core::EndPoint;
use zenoh_result::ZResult;
use zenoh_runtime::ZRuntime;

// Explicit finite test guard; no native/production IO default.
const TLS_IO_WAIT: Duration = Duration::from_secs(5);

async fn connected_tls_links() -> ZResult<(LinkUnicastTls, LinkUnicastTls)> {
    let certificates = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tls-ownership");
    let ca = certificates.join("root-ca.pem");
    let certificate = certificates.join("localhost-certificate.pem");
    let key = certificates.join("localhost-private-key.pem");
    let configuration = format!(
        "root_ca_certificate_file={};listen_certificate_file={};listen_private_key_file={};connect_certificate_file={};connect_private_key_file={};enable_mtls=true;verify_name_on_connect=true;use_public_pki=false",
        ca.display(), certificate.display(), key.display(), certificate.display(), key.display()
    );
    let endpoint = EndPoint::new("tls", "localhost:0", "", configuration)?;
    let supplied = endpoint.config();
    let client = TlsClientConfig::new(&supplied).await?;
    let server = TlsServerConfig::new(&supplied).await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (outgoing, incoming) = tokio::join!(TcpStream::connect(address), listener.accept());
    let outgoing = outgoing?;
    let (incoming, _) = incoming?;
    let client_source = outgoing.local_addr()?;
    let client_target = outgoing.peer_addr()?;
    let server_source = incoming.local_addr()?;
    let server_target = incoming.peer_addr()?;
    let connector = TlsConnector::from(Arc::new(client.client_config));
    let acceptor = TlsAcceptor::from(Arc::new(server.server_config));
    let (connected, accepted) = tokio::join!(
        connector.connect(ServerName::try_from("localhost")?, outgoing),
        acceptor.accept(incoming)
    );
    let connected = connected?;
    let accepted = accepted?;
    let client_auth = get_server_cert_common_name(connected.get_ref().1)?;
    let server_auth = get_client_cert_common_name(accepted.get_ref().1)?;
    Ok((
        LinkUnicastTls::new(
            TlsStream::Client(connected),
            client_source,
            client_target,
            client_auth.into(),
            None,
        ),
        LinkUnicastTls::new(
            TlsStream::Server(accepted),
            server_source,
            server_target,
            server_auth.into(),
            None,
        ),
    ))
}

#[test]
fn pending_read_preserves_write_progress_and_borrowed_wait_resumption() {
    ZRuntime::Application.block_on(async {
        let startup = timeout(TLS_IO_WAIT, connected_tls_links()).await;
        let (link, peer) = startup.expect("pair guard").expect("actual mTLS pair");
        let mut received = [0_u8; 3];
        let mut peer_received = [0_u8; 3];
        let (first, borrowed, written, flushed, peer_read, peer_written, peer_flushed, resumed) = {
            let mut reading = pin!(link.read_exact(&mut received, None));
            let mut acquired = None;
            let first = poll_fn(|cx| {
                Poll::Ready(match reading.as_mut().poll(cx) {
                    Poll::Ready(original) => {
                        acquired = Some(original);
                        Poll::Ready(())
                    }
                    Poll::Pending => Poll::Pending,
                })
            })
            .await;
            let borrowed = {
                let mut wait = pin!(poll_fn(|cx| {
                    if acquired.is_some() {
                        return Poll::Ready(());
                    }
                    match reading.as_mut().poll(cx) {
                        Poll::Ready(original) => {
                            acquired = Some(original);
                            Poll::Ready(())
                        }
                        Poll::Pending => Poll::Pending,
                    }
                }));
                poll_fn(|cx| Poll::Ready(wait.as_mut().poll(cx))).await
            }; // Only the borrowed wait is disposed; reading remains the SAME operation.
            let written = timeout(TLS_IO_WAIT, link.write_all(b"out", None)).await;
            // Native TLS writes can buffer; flush is an explicit private-owner test effect.
            let flushed = timeout(TLS_IO_WAIT, async {
                link.writer.lock().await.flush().await
            })
            .await;
            let peer_read = timeout(TLS_IO_WAIT, peer.read_exact(&mut peer_received, None)).await;
            let peer_written = timeout(TLS_IO_WAIT, peer.write_all(b"in!", None)).await;
            let peer_flushed = timeout(TLS_IO_WAIT, async {
                peer.writer.lock().await.flush().await
            })
            .await;
            let resumed = match acquired {
                Some(original) => timeout(TLS_IO_WAIT, ready(original)).await,
                None => timeout(TLS_IO_WAIT, reading).await,
            };
            (
                first,
                borrowed,
                written,
                flushed,
                peer_read,
                peer_written,
                peer_flushed,
                resumed,
            )
        };
        let link_close = timeout(TLS_IO_WAIT, LinkUnicastTrait::close(&link)).await;
        let mut peer_end = [0_u8; 1];
        let peer_eof = timeout(TLS_IO_WAIT, peer.read(&mut peer_end, None)).await;
        let peer_close = timeout(TLS_IO_WAIT, LinkUnicastTrait::close(&peer)).await;
        let mut link_end = [0_u8; 1];
        let link_eof = timeout(TLS_IO_WAIT, link.read(&mut link_end, None)).await;
        assert!(matches!(&first, Poll::Pending));
        assert!(matches!(&borrowed, Poll::Pending));
        assert!(matches!(&written, Ok(Ok(()))) && matches!(&flushed, Ok(Ok(()))));
        assert!(matches!(&peer_written, Ok(Ok(()))) && matches!(&peer_flushed, Ok(Ok(()))));
        assert!(matches!(&peer_read, Ok(Ok(()))) && matches!(&resumed, Ok(Ok(()))));
        assert_eq!(peer_received, *b"out");
        assert_eq!(received, *b"in!");
        assert!(matches!(&link_close, Ok(Ok(()))) && matches!(&peer_close, Ok(Ok(()))));
        assert!(matches!(&peer_eof, Ok(Ok(0))) && matches!(&link_eof, Ok(Ok(0))));
    });
}

#[test]
fn disposed_read_operation_releases_lane_and_abandonment_remains_native_eof_error() {
    ZRuntime::Application.block_on(async {
        let startup = timeout(TLS_IO_WAIT, connected_tls_links()).await;
        let (link, peer) = startup.expect("pair guard").expect("actual mTLS pair");
        let mut cancelled_bytes = [0_u8; 3];
        let mut cancelled_result = None;
        let pending = {
            let mut reading = pin!(link.read_exact(&mut cancelled_bytes, None));
            poll_fn(|cx| {
                Poll::Ready(match reading.as_mut().poll(cx) {
                    Poll::Ready(original) => {
                        cancelled_result = Some(original);
                        Poll::Ready(())
                    }
                    Poll::Pending => Poll::Pending,
                })
            })
            .await
        }; // The actual read operation is disposed before any application bytes are sent.
        let written = timeout(TLS_IO_WAIT, peer.write_all(b"new", None)).await;
        let flushed = timeout(TLS_IO_WAIT, async {
            peer.writer.lock().await.flush().await
        })
        .await;
        let mut received = [0_u8; 3];
        let replacement_read = timeout(TLS_IO_WAIT, link.read_exact(&mut received, None)).await;
        drop(link); // Exact entire native owner surrender; NOT graceful close_notify.
        let mut peer_end = [0_u8; 1];
        let abandoned = timeout(TLS_IO_WAIT, peer.read(&mut peer_end, None)).await;
        let peer_close = timeout(TLS_IO_WAIT, LinkUnicastTrait::close(&peer)).await;
        // Preserve the actual native cause; the cleanup attempt remains owned to scope exit.
        let native_error = abandoned
            .as_ref()
            .ok()
            .and_then(|native| native.as_ref().err())
            .and_then(|cause| {
                cause.downcast_ref::<Error>().or_else(|| {
                    cause
                        .source()
                        .and_then(|source| source.downcast_ref::<Error>())
                })
            });
        let native_kind = native_error.map(Error::kind);
        let close_error = peer_close
            .as_ref()
            .ok()
            .and_then(|native| native.as_ref().err())
            .and_then(|cause| {
                cause.downcast_ref::<Error>().or_else(|| {
                    cause
                        .source()
                        .and_then(|source| source.downcast_ref::<Error>())
                })
            });
        assert!(matches!(&pending, Poll::Pending));
        assert!(cancelled_result.is_none());
        assert_eq!(cancelled_bytes, [0_u8; 3]);
        assert!(matches!(&written, Ok(Ok(()))) && matches!(&flushed, Ok(Ok(()))));
        assert!(matches!(&replacement_read, Ok(Ok(()))));
        assert_eq!(received, *b"new");
        assert!(matches!(&abandoned, Ok(Err(_))));
        assert_eq!(native_kind, Some(ErrorKind::UnexpectedEof));
        assert!(
            peer_close.is_ok(),
            "actual peer cleanup must finish within fixture guard"
        );
        if let Ok(Err(_)) = &peer_close {
            assert!(
                close_error.is_some(),
                "abandoned-peer close keeps its actual native IO source"
            );
        }
    });
}
