use super::{
    accept_task, get_client_cert_common_name, get_server_cert_common_name, LinkUnicastTls,
};
use crate::utils::{TlsClientConfig, TlsServerConfig};
use std::{
    fmt::Debug,
    future::{poll_fn, ready, Future},
    io::{Cursor, Error, ErrorKind},
    path::Path,
    pin::{pin, Pin},
    sync::{Arc, Mutex},
    task::Poll,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_rustls::{
    rustls::{pki_types::ServerName, ClientConnection},
    TlsAcceptor, TlsConnector, TlsStream,
};
use tokio_util::sync::CancellationToken;
use tracing::{
    dispatcher::{self, Dispatch},
    field::Field,
    span::{Attributes, Id, Record},
    Event, Level, Metadata, Subscriber,
};
use zenoh_link_commons::{LinkAuthId, LinkUnicastTrait};
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

const HANDSHAKE_HOLD: Duration = Duration::from_secs(1);
const HANDSHAKE_WAIT: Duration = Duration::from_secs(10);
const TRACE_WAIT: Duration = Duration::from_secs(30);

enum HandshakeObservation {
    SecondPeer,
    ListenerCancellation,
}

#[test]
fn first_handshake_defers_second_until_original_peer_release() {
    observe_first_handshake(HandshakeObservation::SecondPeer);
}

#[test]
fn listener_cancellation_retires_original_pending_handshake() {
    observe_first_handshake(HandshakeObservation::ListenerCancellation);
}

#[expect(
    clippy::too_many_lines,
    reason = "one real handshake observation and complete native cleanup trace"
)]
fn observe_first_handshake(observation: HandshakeObservation) {
    let token = CancellationToken::new();
    let _listener_stop = token.clone().drop_guard();
    ZRuntime::Application.block_on(async {
        timeout(TRACE_WAIT, async {
            let certificates = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tls-ownership");
            let ca = certificates.join("root-ca.pem");
            let certificate = certificates.join("localhost-certificate.pem");
            let key = certificates.join("localhost-private-key.pem");
            let configuration = format!(
                "root_ca_certificate_file={};listen_certificate_file={};listen_private_key_file={};connect_certificate_file={};connect_private_key_file={};enable_mtls=true;verify_name_on_connect=true;use_public_pki=false",
                ca.display(), certificate.display(), key.display(), certificate.display(), key.display()
            );
            let endpoint = EndPoint::new("tls", "localhost:0", "", configuration).unwrap();
            let supplied = endpoint.config();
            let client = TlsClientConfig::new(&supplied).await.unwrap();
            let server = TlsServerConfig::new(&supplied).await.unwrap();
            let client = Arc::new(client.client_config);
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (publication, incoming) = flume::unbounded();
            let mut listening = ZRuntime::Acceptor.spawn(accept_task(
                listener,
                TlsAcceptor::from(Arc::new(server.server_config)),
                token.clone(),
                publication,
                HANDSHAKE_WAIT,
                server.tls_close_link_on_expiration,
            ));
            let first_started = Instant::now();
            let mut first = TcpStream::connect(address).await.unwrap();
            let mut first_tls = ClientConnection::new(
                Arc::clone(&client), ServerName::try_from("localhost").unwrap()
            ).unwrap();
            let mut hello = Vec::new();
            let hello_length = first_tls.write_tls(&mut hello);
            let hello_sent = timeout(TLS_IO_WAIT, first.write_all(&hello)).await;
            let mut flights = Vec::new();
            let mut parsed = Vec::new();
            let first_flight = timeout(TLS_IO_WAIT, async {
                while first_tls.is_handshaking() {
                    let mut header = [0_u8; 5];
                    first.read_exact(&mut header).await?;
                    let size = usize::from(u16::from_be_bytes([header[3], header[4]]));
                    let mut flight = Vec::from(header);
                    flight.resize(5 + size, 0);
                    first.read_exact(&mut flight[5..]).await?;
                    first_tls.read_tls(&mut Cursor::new(&flight))?;
                    flights.push(flight);
                    let original = first_tls.process_new_packets();
                    match original {
                        Ok(state) => parsed.push(Ok(state)),
                        Err(cause) => { parsed.push(Err(cause)); break; }
                    }
                }
                Ok::<(), Error>(())
            }).await;
            // Retain client authentication/Finished bytes locally; do not write them.
            if let HandshakeObservation::ListenerCancellation = observation {
                let mut before_cancel = [0_u8; 1];
                let first_open = first.try_read(&mut before_cancel);
                token.cancel();
                let mut stopped = None;
                let observed = timeout(HANDSHAKE_HOLD, poll_fn(|context| {
                    match Pin::new(&mut listening).poll(context) {
                        Poll::Pending => Poll::Pending,
                        Poll::Ready(original) => {
                            stopped = Some(original);
                            Poll::Ready(())
                        }
                    }
                })).await;
                let mut peer_end = [0_u8; 1];
                let peer_eof = if stopped.is_some() {
                    Some(timeout(TLS_IO_WAIT, first.read(&mut peer_end)).await)
                } else {
                    None
                };
                let mut after_cancel = [0_u8; 1];
                let still_open = first.try_read(&mut after_cancel);
                let held_for = first_started.elapsed();
                let first_release = first.shutdown().await;
                drop(first);
                let resumed = if stopped.is_none() {
                    Some(timeout(TLS_IO_WAIT, &mut listening).await)
                } else {
                    None
                };
                let mut extras = Vec::new();
                let exhausted = loop {
                    match incoming.try_recv() {
                        Ok(link) => {
                            let original = timeout(TLS_IO_WAIT, link.close()).await;
                            extras.push((link, original));
                        }
                        Err(original) => break original,
                    }
                };
                hello_length.expect("the real ClientHello is encoded");
                hello_sent.expect("ClientHello write guard").expect("actual ClientHello sent");
                first_flight.expect("first server-flight guard").expect("actual first server flight");
                assert!(!flights.is_empty());
                for original in parsed { original.expect("actual server flight authenticates"); }
                assert!(!first_tls.is_handshaking());
                assert!(first_tls.wants_write(), "actual client Finished remains withheld");
                assert!(held_for < HANDSHAKE_WAIT, "native first-handshake deadline had not expired");
                assert!(matches!(&first_open, Err(cause) if cause.kind() == ErrorKind::WouldBlock));
                first_release.expect("first TCP shutdown before explicit native owner surrender");
                if let Some(original) = &resumed {
                    assert!(matches!(original, Ok(Ok(Ok(())))));
                } else {
                    assert!(matches!(&stopped, Some(Ok(Ok(())))));
                }
                assert!(extras.is_empty());
                assert!(matches!(&exhausted, flume::TryRecvError::Disconnected));
                if observed.is_ok() {
                    assert!(matches!(&peer_eof, Some(Ok(Ok(0) | Err(_)))));
                } else {
                    assert!(matches!(&still_open, Err(cause) if cause.kind() == ErrorKind::WouldBlock));
                }
                assert!(observed.is_ok(), "listener remained joined-pending while its held peer was unreleased");
                return;
            }
            let outgoing = TcpStream::connect(address).await.unwrap();
            let second_address = outgoing.local_addr().unwrap();
            let connector = TlsConnector::from(client);
            let mut second = pin!(connector.connect(ServerName::try_from("localhost").unwrap(), outgoing));
            let mut acquired = None;
            let held = timeout(HANDSHAKE_HOLD, poll_fn(|context| {
                match second.as_mut().poll(context) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(original) => {
                        acquired = Some(original);
                        Poll::Ready(())
                    }
                }
            })).await;
            let mut accepted = None;
            if acquired.as_ref().is_some_and(Result::is_ok) {
                accepted = Some(timeout(TLS_IO_WAIT, incoming.recv_async()).await);
            }
            let before_release = accepted.as_ref().and_then(|native| {
                native.as_ref().ok().and_then(|native| native.as_ref().ok())
                    .map(|link| link.get_dst().address().to_string())
            });
            let held_for = first_started.elapsed();
            let mut first_unread = [0_u8; 1];
            let first_open = first.try_read(&mut first_unread);
            let first_release = first.shutdown().await;
            drop(first); // Deliberate abandonment releases the held handshake, not graceful TLS close.
            let resumed = if acquired.is_none() {
                Some(timeout(TLS_IO_WAIT, poll_fn(|context| {
                    match second.as_mut().poll(context) {
                        Poll::Pending => Poll::Pending,
                        Poll::Ready(original) => {
                            acquired = Some(original);
                            Poll::Ready(())
                        }
                    }
                })).await)
            } else { None };
            let mut written = None;
            let mut flushed = None;
            let mut read = None;
            let mut link_close = None;
            let mut peer_eof = None;
            let mut peer_close = None;
            let mut link_eof = None;
            let mut received = [0_u8; 3];
            let mut peer_end = [0_u8; 1];
            let mut link_end = [0_u8; 1];
            if let Some(Ok(peer)) = acquired.as_mut() {
                if accepted.is_none() {
                    accepted = Some(timeout(TLS_IO_WAIT, incoming.recv_async()).await);
                }
                if let Some(Ok(Ok(link))) = accepted.as_ref() {
                    written = Some(timeout(TLS_IO_WAIT, peer.write_all(b"one")).await);
                    flushed = Some(timeout(TLS_IO_WAIT, peer.flush()).await);
                    read = Some(timeout(TLS_IO_WAIT, link.read_exact(&mut received, None)).await);
                    link_close = Some(timeout(TLS_IO_WAIT, link.close()).await);
                    peer_eof = Some(timeout(TLS_IO_WAIT, peer.read(&mut peer_end)).await);
                    peer_close = Some(timeout(TLS_IO_WAIT, peer.shutdown()).await);
                    link_eof = Some(timeout(TLS_IO_WAIT, link.read(&mut link_end, None)).await);
                } else {
                    peer_close = Some(timeout(TLS_IO_WAIT, peer.shutdown()).await);
                }
            }
            token.cancel();
            let stopped = timeout(TLS_IO_WAIT, &mut listening).await;
            let aborted = if stopped.is_err() {
                listening.abort();
                Some(timeout(TLS_IO_WAIT, &mut listening).await)
            } else { None };
            let mut extras = Vec::new();
            let exhausted = loop {
                match incoming.try_recv() {
                    Ok(link) => {
                        let original = timeout(TLS_IO_WAIT, link.close()).await;
                        extras.push((link, original));
                    }
                    Err(original) => break original,
                }
            };
            hello_length.expect("the real ClientHello is encoded");
            hello_sent.expect("ClientHello write guard").expect("actual ClientHello sent");
            first_flight.expect("first server-flight guard").expect("actual first server flight");
            assert!(!flights.is_empty());
            for original in parsed { original.expect("actual server flight authenticates"); }
            assert!(!first_tls.is_handshaking());
            assert!(first_tls.wants_write(), "actual client completion bytes remain withheld");
            assert!(held_for < HANDSHAKE_WAIT, "first handshake native deadline had not expired");
            assert!(matches!(&first_open, Err(cause) if cause.kind() == ErrorKind::WouldBlock),
                "the first raw peer remains open without new server bytes before release");
            first_release.expect("explicit first TCP abandonment");
            if let Some(original) = resumed { original.expect("same second operation resumes"); }
            acquired.expect("actual second completion").expect("valid second mTLS connection");
            let link = accepted.expect("actual native handoff").expect("native handoff guard")
                .expect("original accepted second link");
            assert_eq!(link.get_dst().address().to_string(), second_address.to_string());
            assert!(matches!(&written, Some(Ok(Ok(())))));
            assert!(matches!(&flushed, Some(Ok(Ok(())))));
            assert!(matches!(&read, Some(Ok(Ok(())))));
            assert_eq!(received, *b"one");
            assert!(matches!(&link_close, Some(Ok(Ok(())))));
            assert!(matches!(&peer_eof, Some(Ok(Ok(0)))));
            assert!(matches!(&peer_close, Some(Ok(Ok(())))));
            assert!(matches!(&link_eof, Some(Ok(Ok(0)))));
            assert!(matches!(&stopped, Ok(Ok(Ok(())))));
            assert!(aborted.is_none());
            assert!(extras.is_empty());
            assert!(matches!(&exhausted, flume::TryRecvError::Disconnected));
            if held.is_ok() {
                assert_eq!(before_release.as_deref(), Some(second_address.to_string().as_str()),
                    "early second completion also reached the actual native receiver");
            } else {
                assert!(before_release.is_none());
            }
            assert!(held.is_err(), "the second mTLS operation completed while the first handshake was held");
        }).await
    }).expect("complete handshake trace timed out; setup/cleanup failure, not comparison evidence");
}

#[derive(Default)]
struct AcceptedTls {
    messages: Mutex<Vec<String>>,
}

impl Subscriber for AcceptedTls {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.is_event()
            && metadata.target() == "zenoh_link_tls::unicast"
            && metadata.level() == &Level::DEBUG
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        unreachable!("the accepted-peer observer enables no spans")
    }

    fn record(&self, _: &Id, _: &Record<'_>) {
        unreachable!("the accepted-peer observer creates no span identities")
    }

    fn record_follows_from(&self, _: &Id, _: &Id) {
        unreachable!("the accepted-peer observer creates no span identities")
    }

    fn event(&self, event: &Event<'_>) {
        let mut messages = self.messages.lock().expect("accepted-peer observations");
        event.record(&mut |field: &Field, value: &dyn Debug| {
            if field.name() == "message" {
                messages.push(format!("{value:?}"));
            }
        });
    }

    fn enter(&self, _: &Id) {
        unreachable!("the accepted-peer observer creates no span identities")
    }

    fn exit(&self, _: &Id) {
        unreachable!("the accepted-peer observer creates no span identities")
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one native handoff and complete cleanup trace"
)]
fn listener_stop_retires_full_handoff_and_preserves_queued_original() {
    ZRuntime::Application.block_on(async {
        timeout(TRACE_WAIT, async {
            let certificates = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/tls-ownership");
            let ca = certificates.join("root-ca.pem");
            let certificate = certificates.join("localhost-certificate.pem");
            let key = certificates.join("localhost-private-key.pem");
            let configuration = format!(
                "root_ca_certificate_file={};listen_certificate_file={};listen_private_key_file={};connect_certificate_file={};connect_private_key_file={};enable_mtls=true;verify_name_on_connect=true;use_public_pki=false;close_link_on_expiration=false",
                ca.display(), certificate.display(), key.display(), certificate.display(), key.display()
            );
            let endpoint = EndPoint::new("tls", "localhost:0", "", configuration).unwrap();
            let supplied = endpoint.config();
            let client = TlsClientConfig::new(&supplied).await.unwrap();
            let server = TlsServerConfig::new(&supplied).await.unwrap();
            let connector = TlsConnector::from(Arc::new(client.client_config));
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let token = CancellationToken::new();
            let _listener_stop = token.clone().drop_guard();
            let (publication, incoming) = flume::bounded(1);
            let accepted = Arc::new(AcceptedTls::default());
            let diagnostics = Dispatch::new(Arc::clone(&accepted));
            let mut listening = Box::pin(accept_task(
                listener, TlsAcceptor::from(Arc::new(server.server_config)),
                token.clone(), publication, HANDSHAKE_WAIT, server.tls_close_link_on_expiration,
            ));
            let mut listener_result = None;
            let mut first_client = None;
            let first_socket = TcpStream::connect(address).await.unwrap();
            let first_address = first_socket.local_addr().unwrap();
            let first_progress = {
                let mut connecting = pin!(connector.connect(
                    ServerName::try_from("localhost").unwrap(), first_socket,
                ));
                timeout(TLS_IO_WAIT, poll_fn(|context| {
                    if first_client.is_none() {
                        if let Poll::Ready(original) = connecting.as_mut().poll(context) {
                            first_client = Some(original);
                        }
                    }
                    if listener_result.is_none() {
                        if let Poll::Ready(original) = dispatcher::with_default(
                            &diagnostics, || listening.as_mut().poll(context),
                        ) {
                            listener_result = Some(original);
                        }
                    }
                    if first_client.as_ref().is_some_and(Result::is_ok) && incoming.len() == 1 {
                        Poll::Ready(())
                    } else { Poll::Pending }
                })).await
            };
            let second_socket = TcpStream::connect(address).await.unwrap();
            let second_address = second_socket.local_addr().unwrap();
            let second_event = format!(
                "Accepted TLS connection on {address:?}: {second_address:?}. Common Name: localhost."
            );
            let mut second_client = None;
            let mut handoff_pending = None;
            let mut full_at_handoff = None;
            let second_progress = {
                let mut connecting = pin!(connector.connect(
                    ServerName::try_from("localhost").unwrap(), second_socket,
                ));
                timeout(TLS_IO_WAIT, poll_fn(|context| {
                    if second_client.is_none() {
                        if let Poll::Ready(original) = connecting.as_mut().poll(context) {
                            second_client = Some(original);
                        }
                    }
                    if listener_result.is_some() { return Poll::Ready(()); }
                    let original = dispatcher::with_default(
                        &diagnostics, || listening.as_mut().poll(context),
                    );
                    match original {
                        Poll::Ready(original) => {
                            listener_result = Some(original);
                            Poll::Ready(())
                        }
                        Poll::Pending => {
                            let messages = accepted.messages.lock().unwrap();
                            if messages.iter().any(|message| message == &second_event)
                                && second_client.as_ref().is_some_and(Result::is_ok) {
                                handoff_pending = Some(Poll::<()>::Pending);
                                full_at_handoff = Some(incoming.len());
                                Poll::Ready(())
                            } else { Poll::Pending }
                        }
                    }
                })).await
            };
            let observations = accepted.messages.lock().unwrap().clone();
            // No other executor has polled this SAME future before cancellation.
            token.cancel();
            let post_cancel = Arc::new(Mutex::new(Vec::new()));
            let recorded_polls = Arc::clone(&post_cancel);
            let mut joining = if listener_result.is_none() {
                Some(ZRuntime::Acceptor.spawn(poll_fn(move |context| {
                    match listening.as_mut().poll(context) {
                        Poll::Pending => {
                            recorded_polls.lock().unwrap().push(Poll::Pending);
                            Poll::Pending
                        }
                        Poll::Ready(original) => {
                            recorded_polls.lock().unwrap().push(Poll::Ready(()));
                            Poll::Ready(original)
                        }
                    }
                })))
            } else { None };
            let mut stopped = None;
            let early_join = if let Some(joining) = joining.as_mut() {
                Some(timeout(HANDSHAKE_HOLD, poll_fn(|context| {
                    match Pin::new(&mut *joining).poll(context) {
                        Poll::Pending => Poll::Pending,
                        Poll::Ready(original) => {
                            stopped = Some(original);
                            Poll::Ready(())
                        }
                    }
                })).await)
            } else { None };
            // Even a failed early observation must release originals and join.
            let first = incoming.try_recv();
            let resumed = if stopped.is_none() {
                if let Some(joining) = joining.as_mut() {
                    Some(timeout(TLS_IO_WAIT, poll_fn(|context| {
                        match Pin::new(&mut *joining).poll(context) {
                            Poll::Pending => Poll::Pending,
                            Poll::Ready(original) => {
                                stopped = Some(original);
                                Poll::Ready(())
                            }
                        }
                    })).await)
                } else { None }
            } else { None };
            let second = incoming.try_recv();
            let mut extras = Vec::new();
            let exhausted = loop {
                match incoming.try_recv() {
                    Ok(link) => {
                        let original = timeout(TLS_IO_WAIT, link.close()).await;
                        extras.push((link, original));
                    }
                    Err(original) => break original,
                }
            };
            let first_description = first.as_ref().ok().map(|link| (
                link.get_src().to_string(), link.get_dst().to_string(), link.get_auth_id().clone(),
            ));
            let second_description = second.as_ref().ok().map(|link| (
                link.get_src().to_string(), link.get_dst().to_string(), link.get_auth_id().clone(),
            ));
            let first_auth = first_client.as_ref().and_then(|original| original.as_ref().ok())
                .map(|peer| get_server_cert_common_name(peer.get_ref().1));
            let second_auth = second_client.as_ref().and_then(|original| original.as_ref().ok())
                .map(|peer| get_server_cert_common_name(peer.get_ref().1));
            let mut written = None;
            let mut flushed = None;
            if let Some(Ok(peer)) = first_client.as_mut() {
                written = Some(timeout(TLS_IO_WAIT, peer.write_all(b"kept")).await);
                flushed = Some(timeout(TLS_IO_WAIT, peer.flush()).await);
            }
            let mut bytes = [0_u8; 4];
            let mut read = None;
            let mut return_written = None;
            let mut return_read = None;
            let mut returned = [0_u8; 4];
            if let Ok(link) = &first {
                read = Some(timeout(TLS_IO_WAIT, link.read_exact(&mut bytes, None)).await);
                return_written = Some(timeout(TLS_IO_WAIT, link.write_all(b"back", None)).await);
            }
            if let Some(Ok(peer)) = first_client.as_mut() {
                return_read = Some(timeout(TLS_IO_WAIT, peer.read_exact(&mut returned)).await);
            }
            let mut first_link_close = None;
            if let Ok(link) = &first {
                first_link_close = Some(timeout(TLS_IO_WAIT, link.close()).await);
            }
            let mut second_link_close = None;
            if let Ok(link) = &second {
                second_link_close = Some(timeout(TLS_IO_WAIT, link.close()).await);
            }
            let mut first_peer_eof = None;
            let mut first_peer_close = None;
            let mut second_peer_eof = None;
            let mut second_peer_close = None;
            let mut end = [0_u8; 1];
            if let Some(Ok(peer)) = first_client.as_mut() {
                first_peer_eof = Some(timeout(TLS_IO_WAIT, peer.read(&mut end)).await);
                first_peer_close = Some(timeout(TLS_IO_WAIT, peer.shutdown()).await);
            }
            if let Some(Ok(peer)) = second_client.as_mut() {
                second_peer_eof = Some(timeout(TLS_IO_WAIT, peer.read(&mut end)).await);
                second_peer_close = Some(timeout(TLS_IO_WAIT, peer.shutdown()).await);
            }
            let mut first_link_eof = None;
            let mut second_link_eof = None;
            if let Ok(link) = &first {
                first_link_eof = Some(timeout(TLS_IO_WAIT, link.read(&mut end, None)).await);
            }
            if let Ok(link) = &second {
                second_link_eof = Some(timeout(TLS_IO_WAIT, link.read(&mut end, None)).await);
            }
            let resident_after = incoming.len();
            let after_cancel = post_cancel.lock().unwrap().clone();
            drop(first);
            let second_refusal = match second {
                Ok(link) => { drop(link); None }
                Err(original) => Some(original),
            };
            let extra_results = extras.into_iter().map(|(link, original)| {
                drop(link);
                original
            }).collect::<Vec<_>>();
            drop(first_client);
            drop(second_client);
            drop(incoming);
            first_progress.expect("first actual mTLS handoff guard");
            second_progress.expect("second actual mTLS handoff guard");
            assert!(!server.tls_close_link_on_expiration);
            assert!(listener_result.is_none(), "no premature listener completion");
            assert!(matches!(&handoff_pending, Some(Poll::Pending)));
            assert_eq!(full_at_handoff, Some(1));
            assert!(observations.iter().any(|message| message == &second_event));
            assert!(matches!(&stopped, Some(Ok(Ok(())))));
            assert!(matches!(after_cancel.last(), Some(Poll::Ready(()))));
            if let Some(original) = resumed { original.expect("same listener actual join guard"); }
            assert_eq!(resident_after, 0);
            let first_description = first_description.expect("exact first queued original");
            assert_eq!(first_description, (
                format!("tls/{address}"), format!("tls/{first_address}"),
                LinkAuthId::Tls(Some("localhost".to_owned())),
            ));
            let first_auth = first_auth.expect("first client native authentication").unwrap();
            let second_auth = second_auth.expect("second client native authentication").unwrap();
            assert_eq!(first_auth.auth_value.as_deref(), Some("localhost"));
            assert_eq!(second_auth.auth_value.as_deref(), Some("localhost"));
            assert!(matches!(&written, Some(Ok(Ok(())))));
            assert!(matches!(&flushed, Some(Ok(Ok(())))));
            assert!(matches!(&read, Some(Ok(Ok(())))));
            assert_eq!(bytes, *b"kept");
            assert!(matches!(&return_written, Some(Ok(Ok(())))));
            assert!(matches!(&return_read, Some(Ok(Ok(_)))));
            assert_eq!(returned, *b"back");
            assert!(matches!(&first_link_close, Some(Ok(Ok(())))));
            assert!(matches!(&first_peer_eof, Some(Ok(Ok(0)))));
            assert!(matches!(&first_peer_close, Some(Ok(Ok(())))));
            assert!(matches!(&first_link_eof, Some(Ok(Ok(0)))));
            assert!(matches!(&second_peer_eof, Some(Ok(Ok(0)))));
            assert!(matches!(&second_peer_close, Some(Ok(Ok(())))));
            match second_description {
                Some(original) => {
                    assert_eq!(original, (
                        format!("tls/{address}"), format!("tls/{second_address}"),
                        LinkAuthId::Tls(Some("localhost".to_owned())),
                    ));
                    assert!(matches!(&second_link_close, Some(Ok(Ok(())))));
                    assert!(matches!(&second_link_eof, Some(Ok(Ok(0)))));
                    assert!(matches!(&early_join, Some(Err(_))));
                    assert!(matches!(after_cancel.first(), Some(Poll::Pending)),
                        "a real post-cancel native poll, not scheduling delay, must block");
                }
                None => {
                    assert!(matches!(&second_refusal, Some(flume::TryRecvError::Disconnected)));
                    assert!(second_link_close.is_none());
                }
            }
            assert!(matches!(&exhausted, flume::TryRecvError::Disconnected));
            assert!(extra_results.is_empty());
            assert!(matches!(&early_join, Some(Ok(()))),
                "listener stop remained pending behind the full original handoff");
        }).await
    }).expect("whole handoff guard is setup/cleanup failure, not law evidence");
}
