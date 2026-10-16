//
// Copyright (c) 2023 ZettaScale Technology
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
use crate::TransportManager;
use rand::SeedableRng;
use zenoh_config::TransportUnicastConf;
use zenoh_crypto::PseudoRng;

#[cfg(target_family = "unix")]
#[path = "../../tests/fixtures/deliveries.rs"]
mod deliveries;
#[cfg(target_family = "unix")]
use crate::{
    multicast::TransportMulticast, unicast::TransportUnicast, DummyTransportEventHandler,
    TransportEventHandler, TransportMulticastEventHandler, TransportPeer,
    TransportPeerEventHandler,
};
#[cfg(target_family = "unix")]
use deliveries::Deliveries;
#[cfg(target_family = "unix")]
use std::{
    collections::HashSet,
    future::{poll_fn, Future},
    path::Path,
    pin::pin,
    sync::{atomic::Ordering, Arc},
    task::Poll,
    time::{Duration, Instant},
};
#[cfg(target_family = "unix")]
use tokio::{task::yield_now, time::timeout};
#[cfg(target_family = "unix")]
use zenoh_link::{tls::LinkManagerUnicastTls, Link};
#[cfg(target_family = "unix")]
use zenoh_link_commons::{LinkAuthId, LinkManagerUnicastTrait, TCP_SO_SND_BUF};
#[cfg(target_family = "unix")]
use zenoh_protocol::{
    core::{CongestionControl, EndPoint, Priority, Reliability, WhatAmI, ZenohIdProto},
    network::{push::ext::QoSType, NetworkMessage, NetworkMessageMut, Push},
};
#[cfg(target_family = "unix")]
use zenoh_result::ZResult;
#[cfg(target_family = "unix")]
use zenoh_runtime::ZRuntime;

#[test]
fn maximum_accepted_link_capacity_is_rejected_before_runtime() {
    let mut randomness = PseudoRng::seed_from_u64(0);
    let original = TransportManager::config_unicast()
        .accept_pending(usize::MAX)
        .build(&mut randomness);
    match &original {
        Ok(params) => assert_eq!(params.config.accept_pending, usize::MAX),
        Err(cause) => {
            let prefix = format!(
                "'accept_pending' must be smaller than usize::MAX ({}) at ",
                usize::MAX
            );
            let diagnostic = cause.to_string();
            assert!(diagnostic.starts_with(&prefix));
            assert!(diagnostic.contains("src/unicast/manager.rs:"));
        }
    }
    assert!(
        original.is_err(),
        "unsupported native queue capacity was accepted"
    );
}

#[test]
fn supported_accepted_link_capacities_preserve_configured_values() {
    let configured_default = *TransportUnicastConf::default().accept_pending();
    let mut randomness = PseudoRng::seed_from_u64(0);
    let defaults = TransportManager::config_unicast().build(&mut randomness);
    let mut originals = Vec::new();
    for limit in [0, 1, configured_default, usize::MAX - 1] {
        let original = TransportManager::config_unicast()
            .accept_pending(limit)
            .build(&mut randomness);
        originals.push((limit, original));
    }
    let defaults = defaults.expect("ordinary native defaults remain supported");
    assert_eq!(defaults.config.accept_pending, configured_default);
    for (limit, original) in originals {
        let params = original.expect("supported original native input remains accepted");
        assert_eq!(params.config.accept_pending, limit);
    }
}

#[cfg(target_family = "unix")]
const QUEUE_IO_WAIT: Duration = Duration::from_secs(5);
#[cfg(target_family = "unix")]
const QUEUE_TRACE_WAIT: Duration = Duration::from_secs(40);
#[cfg(target_family = "unix")]
const QUEUE_ACCEPT_WAIT: Duration = Duration::from_secs(90);

#[cfg(target_family = "unix")]
fn tls_endpoint() -> EndPoint {
    let certificates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../zenoh-links/zenoh-link-tls/tests/fixtures/tls-ownership");
    let ca = certificates.join("root-ca.pem");
    let certificate = certificates.join("localhost-certificate.pem");
    let key = certificates.join("localhost-private-key.pem");
    let configuration = format!(
        "root_ca_certificate_file={};listen_certificate_file={};listen_private_key_file={};connect_certificate_file={};connect_private_key_file={};enable_mtls=true;verify_name_on_connect=true;use_public_pki=false;close_link_on_expiration=false",
        ca.display(), certificate.display(), key.display(), certificate.display(), key.display()
    );
    let mut endpoint = EndPoint::new("tls", "localhost:0", "", configuration).unwrap();
    endpoint
        .config_mut()
        .extend_from_iter([(TCP_SO_SND_BUF, "4096")].into_iter())
        .unwrap();
    endpoint
}

#[cfg(target_family = "unix")]
#[test]
fn peer_accepted_queue_preserves_live_delivery_and_recovers_after_pressure() {
    ZRuntime::Application.block_on(observe_accepted_queue(WhatAmI::Peer, WhatAmI::Peer));
}

#[cfg(target_family = "unix")]
#[test]
fn router_accepted_queue_preserves_live_client_delivery_and_recovers_after_pressure() {
    ZRuntime::Application.block_on(observe_accepted_queue(WhatAmI::Router, WhatAmI::Client));
}

#[cfg(target_family = "unix")]
#[expect(
    clippy::too_many_lines,
    reason = "one actual queue pressure and full native cleanup trace"
)]
async fn observe_accepted_queue(listener_role: WhatAmI, caller_role: WhatAmI) {
    timeout(QUEUE_TRACE_WAIT, async {
        let listener_id = ZenohIdProto::try_from([1]).unwrap();
        let live_id = ZenohIdProto::try_from([2]).unwrap();
        let fresh_id = ZenohIdProto::try_from([3]).unwrap();
        let received = Arc::new(Deliveries::default());
        let listener = TransportManager::builder()
            .zid(listener_id)
            .whatami(listener_role)
            .unicast(
                TransportManager::config_unicast()
                    .accept_pending(1)
                    .accept_timeout(QUEUE_ACCEPT_WAIT)
                    .open_timeout(QUEUE_IO_WAIT),
            )
            .build_test(received.clone())
            .unwrap();
        let live = TransportManager::builder()
            .zid(live_id)
            .whatami(caller_role)
            .build_test(Arc::new(DummyTransportEventHandler))
            .unwrap();
        let fresh = TransportManager::builder()
            .zid(fresh_id)
            .whatami(caller_role)
            .build_test(Arc::new(DummyTransportEventHandler))
            .unwrap();
        let supplied = tls_endpoint();
        let bound = timeout(QUEUE_IO_WAIT, listener.add_listener(supplied.clone()))
            .await
            .unwrap()
            .unwrap();
        let endpoint = EndPoint::new(
            bound.protocol(),
            bound.address(),
            bound.metadata(),
            supplied.config(),
        )
        .unwrap();
        let live_transport = timeout(QUEUE_IO_WAIT, live.open_transport_unicast(endpoint.clone()))
            .await
            .unwrap()
            .unwrap();
        let live_membership = timeout(QUEUE_IO_WAIT, async {
            while listener.get_transports_unicast().await.len() != 1
                || listener.state.unicast.incoming.load(Ordering::SeqCst) != 0
            {
                yield_now().await;
            }
        })
        .await;
        let outgoing = LinkManagerUnicastTls::new(listener.new_unicast_link_sender.clone());
        let genuine = timeout(QUEUE_IO_WAIT, outgoing.new_link(endpoint.clone()))
            .await
            .unwrap()
            .unwrap();
        let genuine_started = Instant::now();
        let occupied = timeout(QUEUE_IO_WAIT, async {
            while listener.state.unicast.incoming.load(Ordering::SeqCst) != 1 {
                yield_now().await;
            }
        })
        .await;
        let (control_publication, control_receiving) = flume::unbounded();
        let control_listener = LinkManagerUnicastTls::new(control_publication);
        let control_bound = timeout(
            QUEUE_IO_WAIT,
            control_listener.new_listener(supplied.clone()),
        )
        .await
        .unwrap()
        .unwrap();
        let control_endpoint = EndPoint::new(
            control_bound.protocol(),
            control_bound.address(),
            control_bound.metadata(),
            supplied.config(),
        )
        .unwrap();
        let control_peer = timeout(QUEUE_IO_WAIT, outgoing.new_link(control_endpoint.clone()))
            .await
            .unwrap()
            .unwrap();
        let control_original = timeout(QUEUE_IO_WAIT, control_receiving.recv_async())
            .await
            .unwrap()
            .unwrap();
        let control_description = (
            control_original.get_src().clone(),
            control_original.get_dst().clone(),
            control_original.get_auth_id().clone(),
        );
        let control_peer_description = (
            control_peer.get_src().clone(),
            control_peer.get_dst().clone(),
            control_peer.get_auth_id().clone(),
        );
        let writer = control_original.clone(); // Sole independent native writer; never closes a spare.
        let mut payload = vec![31_u8; 512 * 1024];
        payload[..16].copy_from_slice(b"dispatcher-held!");
        let mut writing = Box::pin(writer.write_all(&payload, None));
        let mut written = None;
        let mut prefix = [0_u8; 16];
        let mut prefix_read = None;
        let mut write_poll = None;
        let writer_qualified = {
            let mut reading = pin!(control_peer.read_exact(&mut prefix, None));
            timeout(
                QUEUE_IO_WAIT,
                poll_fn(|context| {
                    if written.is_none() {
                        match writing.as_mut().poll(context) {
                            Poll::Pending => write_poll = Some(Poll::<()>::Pending),
                            Poll::Ready(original) => {
                                written = Some(original);
                                write_poll = Some(Poll::Ready(()));
                            }
                        }
                    }
                    if prefix_read.is_none() {
                        if let Poll::Ready(original) = reading.as_mut().poll(context) {
                            prefix_read = Some(original);
                        }
                    }
                    if prefix_read.is_some() || written.is_some() {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                }),
            )
            .await
        };
        let enqueued = listener.new_unicast_link_sender.try_send(control_original);
        let consumed = timeout(QUEUE_IO_WAIT, async {
            while !listener.new_unicast_link_sender.is_empty() {
                yield_now().await;
            }
        })
        .await;
        let pressure_first = timeout(QUEUE_IO_WAIT, outgoing.new_link(endpoint.clone()))
            .await
            .unwrap()
            .unwrap();
        let first_queued = timeout(QUEUE_IO_WAIT, async {
            while listener.new_unicast_link_sender.len() != 1 {
                yield_now().await;
            }
        })
        .await;
        let pressure_second = timeout(QUEUE_IO_WAIT, outgoing.new_link(endpoint.clone()))
            .await
            .unwrap()
            .unwrap();
        let mut second_end = [0_u8; 1];
        let mut second_reading = Box::pin(pressure_second.read(&mut second_end, None));
        let mut second_eof = None;
        let mut second_before_release = None;
        let mut queued = None;
        let pressure_observed = timeout(
            QUEUE_IO_WAIT,
            poll_fn(|context| {
                if second_eof.is_none() {
                    match second_reading.as_mut().poll(context) {
                        Poll::Pending => second_before_release = Some(Poll::<()>::Pending),
                        Poll::Ready(original) => {
                            second_eof = Some(original);
                            second_before_release = Some(Poll::Ready(()));
                        }
                    }
                }
                let actual = listener.new_unicast_link_sender.len();
                if actual >= 2 || second_eof.is_some() {
                    queued = Some(actual);
                    Poll::Ready(())
                } else {
                    context.waker().wake_by_ref();
                    Poll::Pending
                }
            }),
        )
        .await;
        let capacity = listener.new_unicast_link_sender.capacity();
        let actual_incoming = listener.state.unicast.incoming.load(Ordering::SeqCst);
        let actual_membership = listener
            .get_transports_unicast()
            .await
            .into_iter()
            .map(|transport| transport.get_peer())
            .collect::<Vec<_>>();
        let mut message = NetworkMessage::from(Push {
            wire_expr: "queue/live".into(),
            ext_qos: QoSType::new(Priority::default(), CongestionControl::Block, false),
            ..Push::from(vec![29_u8])
        });
        message.reliability = Reliability::Reliable;
        let scheduled = live_transport.schedule(message.as_mut());
        let live_delivered = timeout(QUEUE_IO_WAIT, async {
            while received.get_count() != 1 {
                yield_now().await;
            }
        })
        .await;
        let count_while_held = received.get_count();
        let genuine_held_for = genuine_started.elapsed();
        // Release by successful completion of the SAME original native write, never disposal.
        let mut remainder = vec![0_u8; payload.len() - prefix.len()];
        let mut drained = None;
        let released = timeout(QUEUE_IO_WAIT, async {
            tokio::join!(
                async {
                    if written.is_none() {
                        poll_fn(|context| match writing.as_mut().poll(context) {
                            Poll::Pending => Poll::Pending,
                            Poll::Ready(original) => {
                                written = Some(original);
                                Poll::Ready(())
                            }
                        })
                        .await;
                    }
                },
                async {
                    drained = Some(control_peer.read_exact(&mut remainder, None).await);
                }
            );
        })
        .await;
        drop(writing);
        drop(writer);
        let mut control_end = [0_u8; 1];
        let control_eof = timeout(QUEUE_IO_WAIT, control_peer.read(&mut control_end, None)).await;
        let mut first_end = [0_u8; 1];
        let first_eof = timeout(QUEUE_IO_WAIT, pressure_first.read(&mut first_end, None)).await;
        let second_resumed = if second_eof.is_none() {
            Some(
                timeout(
                    QUEUE_IO_WAIT,
                    poll_fn(|context| match second_reading.as_mut().poll(context) {
                        Poll::Pending => Poll::Pending,
                        Poll::Ready(original) => {
                            second_eof = Some(original);
                            Poll::Ready(())
                        }
                    }),
                )
                .await,
            )
        } else {
            None
        };
        drop(second_reading);
        let drained_queue = timeout(QUEUE_IO_WAIT, async {
            while !listener.new_unicast_link_sender.is_empty() {
                yield_now().await;
            }
        })
        .await;
        let incoming_after_drain = listener.state.unicast.incoming.load(Ordering::SeqCst);
        let control_close = timeout(QUEUE_IO_WAIT, control_peer.close()).await;
        let first_close = timeout(QUEUE_IO_WAIT, pressure_first.close()).await;
        let second_close = timeout(QUEUE_IO_WAIT, pressure_second.close()).await;
        let rejected_close = match &enqueued {
            Err(flume::TrySendError::Full(link) | flume::TrySendError::Disconnected(link)) => {
                Some(timeout(QUEUE_IO_WAIT, link.close()).await)
            }
            Ok(()) => None,
        };
        let genuine_close = timeout(QUEUE_IO_WAIT, genuine.close()).await;
        let genuine_retired = timeout(QUEUE_IO_WAIT, async {
            while listener.state.unicast.incoming.load(Ordering::SeqCst) != 0 {
                yield_now().await;
            }
        })
        .await;
        let fresh_open = timeout(
            QUEUE_IO_WAIT,
            fresh.open_transport_unicast(endpoint.clone()),
        )
        .await;
        let fresh_peer = match &fresh_open {
            Ok(Ok(transport)) => Some(transport.get_peer()),
            _ => None,
        };
        let fresh_membership = timeout(QUEUE_IO_WAIT, async {
            if matches!(&fresh_open, Ok(Ok(_))) {
                while listener.get_transports_unicast().await.len() != 2 {
                    yield_now().await;
                }
            }
        })
        .await;
        let fresh_members = listener
            .get_transports_unicast()
            .await
            .into_iter()
            .map(|transport| transport.get_peer())
            .collect::<Vec<_>>();
        let live_peer = live_transport.get_peer();
        let fresh_close = match &fresh_open {
            Ok(Ok(transport)) => Some(timeout(QUEUE_IO_WAIT, transport.close()).await),
            _ => None,
        };
        let live_close = timeout(QUEUE_IO_WAIT, live_transport.close()).await;
        let retired = timeout(QUEUE_IO_WAIT, async {
            while !listener.get_transports_unicast().await.is_empty() {
                yield_now().await;
            }
        })
        .await;
        let control_listener_close = timeout(
            QUEUE_IO_WAIT,
            control_listener.del_listener(&control_endpoint),
        )
        .await;
        let control_listeners = control_listener.get_listeners().await;
        let outgoing_listeners = outgoing.get_listeners().await;
        drop(control_listener);
        let mut extras = Vec::new();
        let control_exhausted = loop {
            match control_receiving.try_recv() {
                Ok(link) => {
                    let original = timeout(QUEUE_IO_WAIT, link.close()).await;
                    extras.push((link, original));
                }
                Err(original) => break original,
            }
        };
        let listener_join = timeout(QUEUE_IO_WAIT, listener.close()).await;
        let live_join = timeout(QUEUE_IO_WAIT, live.close()).await;
        let fresh_join = timeout(QUEUE_IO_WAIT, fresh.close()).await;
        let empty = [
            (
                listener.get_listeners().await,
                listener.get_transports_unicast().await,
            ),
            (
                live.get_listeners().await,
                live.get_transports_unicast().await,
            ),
            (
                fresh.get_listeners().await,
                fresh.get_transports_unicast().await,
            ),
        ];
        drop(genuine);
        drop(control_peer);
        drop(pressure_first);
        drop(pressure_second);
        drop(outgoing);
        let (extra_links, extra_results): (Vec<_>, Vec<_>) = extras.into_iter().unzip();
        drop(extra_links);
        live_membership.expect("actual live native membership");
        occupied.expect("genuine incomplete native acceptance");
        writer_qualified.expect("actual pending native writer prefix guard");
        assert!(matches!(&prefix_read, Some(Ok(()))));
        assert_eq!(prefix, *b"dispatcher-held!");
        assert!(matches!(&write_poll, Some(Poll::Pending)));
        assert!(enqueued.is_ok());
        consumed.expect("actual dispatcher took the original control link");
        first_queued.expect("first ordinary TLS original queued");
        pressure_observed.expect("actual second queue/refusal observation");
        let queued = queued.expect("actual queue count before release");
        match (&second_before_release, &second_resumed) {
            (Some(Poll::Ready(())), None) => assert_eq!(queued, 1),
            (Some(Poll::Pending), Some(Ok(()))) => assert_eq!(queued, 2),
            other => panic!("unexpected actual native refusal/resumption: {other:?}"),
        }
        if let Some(capacity) = capacity {
            assert_eq!(capacity, listener.config.unicast.accept_pending);
        }
        assert_eq!(actual_incoming, 1);
        assert_eq!(actual_membership.len(), 1);
        let peer = actual_membership[0].as_ref().unwrap();
        assert_eq!(
            (peer.zid, peer.whatami, peer.links.len()),
            (live_id, caller_role, 1)
        );
        assert!(matches!(&scheduled, Ok(true)));
        live_delivered.expect("live native Push during real queue pressure");
        assert_eq!(count_while_held, 1);
        assert!(genuine_held_for < QUEUE_ACCEPT_WAIT);
        released.expect("same native writer/draining peer completion guard");
        assert!(matches!(&written, Some(Ok(()))));
        assert!(matches!(&drained, Some(Ok(()))));
        assert_eq!(remainder, payload[16..]);
        assert_eq!(control_description.0, control_peer_description.1);
        assert_eq!(control_description.1, control_peer_description.0);
        assert_eq!(
            control_description.2,
            LinkAuthId::Tls(Some("localhost".to_owned()))
        );
        assert_eq!(
            control_peer_description.2,
            LinkAuthId::Tls(Some("localhost".to_owned()))
        );
        assert!(matches!(&control_eof, Ok(Ok(0))));
        assert!(matches!(&first_eof, Ok(Ok(0))));
        assert!(matches!(&second_eof, Some(Ok(0))));
        drained_queue.expect("actual overloaded queue drained");
        assert_eq!(incoming_after_drain, 1);
        for original in [&control_close, &first_close, &second_close, &genuine_close] {
            assert!(matches!(original, Ok(Ok(()))));
        }
        assert!(rejected_close.is_none());
        genuine_retired.expect("genuine accept finished before fresh full open");
        fresh_membership.expect("actual fresh native membership");
        let peer = fresh_peer
            .as_ref()
            .expect("actual fresh original open")
            .as_ref()
            .unwrap();
        assert_eq!(
            (peer.zid, peer.whatami, peer.links.len()),
            (listener_id, listener_role, 1)
        );
        let live_peer = live_peer.unwrap();
        assert_eq!(
            (live_peer.zid, live_peer.whatami),
            (listener_id, listener_role)
        );
        assert_eq!(fresh_members.len(), 2);
        let peers = fresh_members
            .iter()
            .map(|original| original.as_ref().unwrap().zid)
            .collect::<HashSet<_>>();
        assert_eq!(peers, HashSet::from([live_id, fresh_id]));
        for original in &fresh_members {
            let peer = original.as_ref().unwrap();
            assert_eq!((peer.whatami, peer.links.len()), (caller_role, 1));
        }
        assert!(matches!(&fresh_close, Some(Ok(Ok(())))));
        assert!(matches!(&live_close, Ok(Ok(()))));
        retired.expect("real native transports retired");
        assert!(matches!(&control_listener_close, Ok(Ok(()))));
        assert!(control_listeners.is_empty());
        assert!(outgoing_listeners.is_empty());
        assert!(matches!(
            control_exhausted,
            flume::TryRecvError::Disconnected
        ));
        assert!(extra_results.is_empty());
        for original in [&listener_join, &live_join, &fresh_join] {
            original.as_ref().unwrap();
        }
        for (listeners, transports) in empty {
            assert!(listeners.is_empty());
            assert!(transports.is_empty());
        }
        assert!(listener.new_unicast_link_sender.is_disconnected());
        assert!(
            queued <= listener.config.unicast.accept_pending,
            "actual accepted queue exceeded its configured native bound"
        );
    })
    .await
    .expect("whole native queue trace guard is setup/cleanup failure, not law evidence");
}

#[cfg(target_family = "unix")]
#[test]
fn zero_pending_rejects_peer_transport_without_admission() {
    ZRuntime::Application.block_on(observe_zero_pending(WhatAmI::Peer, WhatAmI::Peer));
}

#[cfg(target_family = "unix")]
#[test]
fn zero_pending_rejects_router_client_transport_without_admission() {
    ZRuntime::Application.block_on(observe_zero_pending(WhatAmI::Router, WhatAmI::Client));
}

#[cfg(target_family = "unix")]
async fn observe_zero_pending(listener_role: WhatAmI, caller_role: WhatAmI) {
    timeout(QUEUE_TRACE_WAIT, async {
        let listener = TransportManager::builder()
            .whatami(listener_role)
            .unicast(TransportManager::config_unicast().accept_pending(0))
            .build_test(Arc::new(DummyTransportEventHandler))
            .unwrap();
        let caller = TransportManager::builder()
            .whatami(caller_role)
            .build_test(Arc::new(DummyTransportEventHandler))
            .unwrap();
        let supplied = tls_endpoint();
        let bound = timeout(QUEUE_IO_WAIT, listener.add_listener(supplied.clone()))
            .await
            .unwrap()
            .unwrap();
        let endpoint = EndPoint::new(
            bound.protocol(),
            bound.address(),
            bound.metadata(),
            supplied.config(),
        )
        .unwrap();
        let outgoing = LinkManagerUnicastTls::new(listener.new_unicast_link_sender.clone());
        let tls_probe = timeout(QUEUE_IO_WAIT, outgoing.new_link(endpoint.clone())).await;
        let mut end = [0_u8; 1];
        let mut tls_eof = None;
        let mut tls_close = None;
        let tls_auth = match &tls_probe {
            Ok(Ok(link)) => {
                tls_eof = Some(timeout(QUEUE_IO_WAIT, link.read(&mut end, None)).await);
                tls_close = Some(timeout(QUEUE_IO_WAIT, link.close()).await);
                Some(link.get_auth_id().clone())
            }
            _ => None,
        };
        let original = timeout(QUEUE_IO_WAIT, caller.open_transport_unicast(endpoint)).await;
        let listener_members = listener.get_transports_unicast().await;
        let caller_members = caller.get_transports_unicast().await;
        let incoming = listener.state.unicast.incoming.load(Ordering::SeqCst);
        let unexpected_close = match &original {
            Ok(Ok(transport)) => Some(timeout(QUEUE_IO_WAIT, transport.close()).await),
            _ => None,
        };
        let listener_join = timeout(QUEUE_IO_WAIT, listener.close()).await;
        let caller_join = timeout(QUEUE_IO_WAIT, caller.close()).await;
        let empty = [
            (
                listener.get_listeners().await,
                listener.get_transports_unicast().await,
            ),
            (
                caller.get_listeners().await,
                caller.get_transports_unicast().await,
            ),
        ];
        drop(tls_probe);
        drop(outgoing);
        listener_join.expect("zero-input native listener actually joined");
        caller_join.expect("zero-input native caller actually joined");
        for (listeners, transports) in empty {
            assert!(listeners.is_empty());
            assert!(transports.is_empty());
        }
        assert_eq!(
            tls_auth,
            Some(LinkAuthId::Tls(Some("localhost".to_owned())))
        );
        assert!(matches!(&tls_eof, Some(Ok(Ok(0)))));
        assert!(matches!(&tls_close, Some(Ok(Ok(())))));
        assert_eq!(incoming, 0);
        assert!(listener_members.is_empty());
        assert!(caller_members.is_empty());
        if let Some(original) = unexpected_close {
            assert!(matches!(original, Ok(Ok(()))));
        }
        let original = original.expect("zero-admission actual native open guard");
        assert!(
            original.is_err(),
            "zero active input admitted a full native transport"
        );
    })
    .await
    .expect("whole zero-admission guard supplies no law evidence");
}
