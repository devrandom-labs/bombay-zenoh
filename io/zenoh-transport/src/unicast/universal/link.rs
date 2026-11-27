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
#[cfg(all(
    feature = "uring",
    target_os = "linux",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "loongarch64",
        target_arch = "powerpc64"
    )
))]
use std::fmt::Debug;
use std::{
    any::Any,
    fmt,
    future::poll_fn,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock, Weak,
    },
    task::Poll,
    time::{Duration, Instant},
};

use futures::{future::select_all, task::AtomicWaker};
use tokio::{sync::OwnedSemaphorePermit, task::JoinHandle};
use tokio_util::sync::CancellationToken;
#[cfg(all(
    feature = "uring",
    target_os = "linux",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "loongarch64",
        target_arch = "powerpc64"
    )
))]
use zenoh_buffers::{
    buffer::Buffer,
    reader::{BacktrackableReader, HasReader},
};
use zenoh_buffers::{ZSlice, ZSliceBuffer};
use zenoh_link::Link;
#[cfg(feature = "transport_tls")]
use zenoh_link::LinkAuthId;
use zenoh_protocol::{
    core::Priority,
    transport::{KeepAlive, TransportMessage},
};
use zenoh_result::{bail, zerror, ZResult};
use zenoh_sync::RecyclingObjectPool;
#[cfg(feature = "unstable")]
use zenoh_sync::{event, Notifier, Waiter};
use zenoh_task::TaskController;
#[cfg(all(
    feature = "uring",
    target_os = "linux",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "loongarch64",
        target_arch = "powerpc64"
    )
))]
use zenoh_uring::api::reader::{
    fragmented_batch::{DefragmentationState, FragmentedBatch},
    rx_buffer::RxBuffer,
};

use super::transport::TransportUnicastUniversal;
use crate::{
    common::{
        batch::{BatchConfig, RBatch},
        pipeline::{
            PipelineConsumer, TransmissionPipeline, TransmissionPipelineConf,
            TransmissionPipelineConsumer, TransmissionPipelineProducer,
        },
        priority::TransportPriorityTx,
    },
    manager::ReceiveBudget,
    unicast::link::{TransportLinkUnicast, TransportLinkUnicastRx, TransportLinkUnicastTx},
};

#[derive(Clone)]
pub(super) struct TransportLinkUnicastUniversal {
    // The underlying link
    pub(super) link: TransportLinkUnicast,
    // The transmission pipeline
    pub(super) pipeline: TransmissionPipelineProducer,
    // The task handling substruct
    task_controller: TaskController,
    #[cfg(feature = "unstable")]
    // Notifier for a BlockFirst message to be ready to be sent
    // (after the previous one has been sent)
    pub block_first_notifiers: [Notifier; Priority::NUM],
    #[cfg(feature = "unstable")]
    // Waiter for a BlockFirst message to be ready to be sent
    pub block_first_waiters: [Waiter; Priority::NUM],
    #[cfg(feature = "stats")]
    pub(super) stats: zenoh_stats::LinkStats,
}

impl TransportLinkUnicastUniversal {
    pub(super) fn new(
        transport: &TransportUnicastUniversal,
        link: TransportLinkUnicast,
        priority_tx: &[TransportPriorityTx],
    ) -> (Self, TransmissionPipelineConsumer) {
        assert!(!priority_tx.is_empty());

        let config = TransmissionPipelineConf {
            batch: BatchConfig {
                mtu: link.config.batch.mtu,
                is_streamed: link.link.is_streamed(),
                #[cfg(feature = "transport_compression")]
                is_compression: link.config.batch.is_compression,
            },
            queue_size: transport.manager.config.queue_size,
            wait_before_drop: transport.manager.config.wait_before_drop,
            max_wait_before_drop_fragments: transport.manager.config.max_wait_before_drop_fragments,
            wait_before_close: transport.manager.config.wait_before_close,
            batching_enabled: transport.manager.config.batching,
            batching_time_limit: transport.manager.config.queue_backoff,
            queue_alloc: transport.manager.config.queue_alloc,
            #[cfg(feature = "transport_tls")]
            tls_link: (transport.manager.tls_connections.is_some()
                && matches!(link.link.get_auth_id(), LinkAuthId::Tls(_)))
            .then(|| link.link.clone()),
        };

        // The pipeline
        let (producer, consumer) =
            TransmissionPipeline::make(config, priority_tx, link.link.supports_priorities());

        // Use the complete src and dest locators including parameters
        #[cfg(feature = "stats")]
        let link_unicast = link.link();
        #[cfg(feature = "stats")]
        let stats = transport
            .stats
            .link_stats(&link_unicast.src, &link_unicast.dst);

        #[cfg(feature = "unstable")]
        let mut block_first_notifiers = Vec::new();
        #[cfg(feature = "unstable")]
        let mut block_first_waiters = Vec::new();
        #[cfg(feature = "unstable")]
        for _ in 0..Priority::NUM {
            let (notifier, waiter) = event::new();
            // notify to be make the BlockFirst "slot" available
            notifier.notify().unwrap();
            block_first_notifiers.push(notifier);
            block_first_waiters.push(waiter);
        }

        let result = Self {
            link,
            pipeline: producer,
            task_controller: TaskController::default(),
            #[cfg(feature = "unstable")]
            block_first_notifiers: block_first_notifiers.try_into().ok().unwrap(),
            #[cfg(feature = "unstable")]
            block_first_waiters: block_first_waiters.try_into().ok().unwrap(),
            #[cfg(feature = "stats")]
            stats,
        };

        (result, consumer)
    }

    pub(super) fn start_tx(
        &mut self,
        transport: TransportUnicastUniversal,
        consumer: TransmissionPipelineConsumer,
        keep_alive: Duration,
    ) {
        // Spawn the TX task
        let mut tx = self.link.tx();
        #[cfg(feature = "stats")]
        let stats = self.stats.clone();
        let ct = self.task_controller.get_cancellation_token();
        let task = async move {
            let res = tx_task(
                consumer,
                &mut tx,
                keep_alive,
                ct,
                #[cfg(feature = "stats")]
                stats,
            )
            .await;

            if let Err(e) = res {
                tracing::debug!("TX task failed: {}", e);
                // Spawn a task to avoid a deadlock waiting for this same task
                // to finish in the close() joining its handle
                // TODO(yuyuan): do more study to check which ZRuntime should be used or refine the
                // termination
                zenoh_runtime::ZRuntime::Net
                    .spawn(async move { transport.del_link(tx.inner.link()).await });
            }
        };
        self.task_controller
            .spawn_with_rt(zenoh_runtime::ZRuntime::TX, task);
    }

    pub(super) fn start_rx(&mut self, transport: TransportUnicastUniversal, lease: Duration) {
        let priorities = self.link.config.priorities.clone();
        let reliability = self.link.config.reliability;
        let mut rx = self.link.rx();
        let cancellation_token = self.task_controller.get_cancellation_token();
        #[cfg(feature = "stats")]
        let stats = self.stats.clone();
        let task = async move {
            // Start the consume task
            let res = rx_task(
                &mut rx,
                transport.clone(),
                lease,
                transport.manager.config.link_rx_buffer_size,
                cancellation_token,
                #[cfg(feature = "stats")]
                stats,
            )
            .await;

            // TODO(yuyuan): improve this callback
            if let Err(e) = res {
                // process error if task was not cancelled
                tracing::debug!("RX task failed: {}", e);

                // Spawn a task to avoid a deadlock waiting for this same task
                // to finish in the close() joining its handle
                // WARN: Must be spawned on RX

                zenoh_runtime::ZRuntime::RX.spawn(async move {
                    transport
                        .del_link(Link::new_unicast(&rx.link, priorities, reliability))
                        .await
                });

                // // WARN: This ZRuntime blocks
                // zenoh_runtime::ZRuntime::Net
                //     .spawn(async move { transport.del_link((&rx.link).into()).await });

                // // WARN: This cloud block
                // transport.del_link((&rx.link).into()).await;
            }
        };
        // WARN: If this is on ZRuntime::TX, a deadlock would occur.
        self.task_controller
            .spawn_with_rt(zenoh_runtime::ZRuntime::RX, task);
    }

    pub(super) async fn close(self) -> ZResult<()> {
        tracing::trace!("{}: closing", self.link);
        self.task_controller.terminate_all_async().await;
        self.pipeline.disable();

        self.link.close(None).await
    }
}

/*************************************/
/*              TASKS                */
/*************************************/
async fn tx_task(
    pipeline: TransmissionPipelineConsumer,
    link: &mut TransportLinkUnicastTx,
    keep_alive: Duration,
    cancellation_token: CancellationToken,
    #[cfg(feature = "stats")] stats: zenoh_stats::LinkStats,
) -> ZResult<()> {
    let keep_alive_tracker = TimeoutTracker::new(keep_alive);
    if link.inner.link.supports_priorities() {
        let (res, _, _) = select_all(pipeline.split().into_iter().map(|pipeline| {
            let mut link = link.clone();
            let cancellation_token = cancellation_token.clone();
            let keep_alive_tracker = keep_alive_tracker.clone();
            #[cfg(feature = "stats")]
            let stats = stats.clone();
            zenoh_runtime::ZRuntime::TX.spawn(async move {
                write_loop(
                    Some(pipeline.priority()),
                    pipeline,
                    &mut link,
                    keep_alive_tracker,
                    cancellation_token,
                    #[cfg(feature = "stats")]
                    stats,
                )
                .await
            })
        }))
        .await;
        res.unwrap()?;
    } else {
        write_loop(
            None,
            pipeline,
            link,
            keep_alive_tracker,
            cancellation_token,
            #[cfg(feature = "stats")]
            stats,
        )
        .await?;
    }
    Ok(())
}

async fn write_loop(
    write_priority: Option<Priority>,
    mut pipeline: impl PipelineConsumer,
    link: &mut TransportLinkUnicastTx,
    keep_alive_tracker: TimeoutTracker,
    cancellation_token: CancellationToken,
    #[cfg(feature = "stats")] stats: zenoh_stats::LinkStats,
) -> ZResult<()> {
    let task = async {
        loop {
            tokio::select! {
                pull = pipeline.pull() => {
                    let Some((mut batch, priority)) = pull else {
                        // The queue has been disabled: break the tx loop, drain the queue, and exit
                        break
                    };
                    debug_assert!(write_priority.is_none() || write_priority == Some(priority));
                    link.send_batch(&mut batch, write_priority).await?;
                    // inform the latest message tracker that a message has been sent
                    keep_alive_tracker.reset();

                    #[cfg(feature = "stats")]
                    {
                        stats.inc_bytes(zenoh_stats::Tx, batch.len() as u64);
                        stats.inc_transport_message(zenoh_stats::Tx, batch.stats.t_msgs as u64);
                    }

                    // Reinsert the batch into the queue
                    pipeline.refill(batch, priority);
                },
                _ = keep_alive_tracker.wait_if(write_priority.unwrap_or(Priority::Control) == Priority::Control) => {
                    // A timeout occurred, no control/data messages have been sent during
                    // the keep_alive period, we need to send a KeepAlive message
                    let message: TransportMessage = KeepAlive.into();

                    #[allow(unused_variables)] // Used when stats feature is enabled
                    let n = link.send(&message, Some(Priority::Control)).await?;

                    #[cfg(feature = "stats")]
                    {
                        stats.inc_bytes(zenoh_stats::Tx, n as u64);
                        stats.inc_transport_message(zenoh_stats::Tx, 1);
                    }
                }
            }
        }
        ZResult::Ok(())
    };
    if let Some(result) = cancellation_token.run_until_cancelled(task).await {
        result?;
    }

    // Drain the transmission pipeline and write remaining bytes on the wire
    let mut batches = pipeline.drain();
    for (mut b, _) in batches.drain(..) {
        tokio::time::timeout(
            keep_alive_tracker.timeout(),
            link.send_batch(&mut b, write_priority),
        )
        .await
        .map_err(|_| {
            zerror!(
                "{link}: flush failed after {} ms",
                keep_alive_tracker.timeout().as_millis()
            )
        })??;

        #[cfg(feature = "stats")]
        {
            stats.inc_bytes(zenoh_stats::Tx, b.len() as u64);
            stats.inc_transport_message(zenoh_stats::Tx, b.stats.t_msgs as u64);
        }
    }

    Ok(())
}

async fn rx_task(
    link: &mut TransportLinkUnicastRx,
    transport: TransportUnicastUniversal,
    lease: Duration,
    rx_buffer_size: usize,
    cancellation_token: CancellationToken,
    #[cfg(feature = "stats")] stats: zenoh_stats::LinkStats,
) -> ZResult<()> {
    #[cfg(all(
        feature = "uring",
        target_os = "linux",
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "riscv64",
            target_arch = "loongarch64",
            target_arch = "powerpc64"
        )
    ))]
    if transport.manager.unicast_receive.is_none()
        && transport.manager.state.uring.is_some()
        && link.link.get_fd().is_ok()
    {
        return rx_task_uring(
            link,
            transport.clone(),
            lease,
            rx_buffer_size,
            #[cfg(feature = "stats")]
            stats,
            cancellation_token,
        )
        .await;
    }

    cancellation_token
        .run_until_cancelled(rx_task_non_uring(
            link,
            transport.clone(),
            lease,
            rx_buffer_size,
            cancellation_token.clone(),
            #[cfg(feature = "stats")]
            stats,
        ))
        .await
        .unwrap_or(Ok(()))
}

async fn rx_task_non_uring(
    link: &mut TransportLinkUnicastRx,
    transport: TransportUnicastUniversal,
    lease: Duration,
    rx_buffer_size: usize,
    cancellation_token: CancellationToken,
    #[cfg(feature = "stats")] stats: zenoh_stats::LinkStats,
) -> ZResult<()> {
    // The pool of buffers
    let mtu = link.config.batch.mtu as usize;
    let mut n = rx_buffer_size / mtu;
    if n == 0 {
        tracing::debug!("RX configured buffer of {rx_buffer_size} bytes is too small for {link} that has an MTU of {mtu} bytes. Defaulting to {mtu} bytes for RX buffer.");
        n = 1;
    }
    let pool = match transport.manager.unicast_receive.as_ref() {
        Some(budget) => ReceivePool::Charged(Arc::new(ReceiveReserve::new(n, mtu, budget)?)),
        None => ReceivePool::Recycling(RecyclingObjectPool::new(n, move || {
            vec![0_u8; mtu].into_boxed_slice()
        })),
    };

    let lease_tracker = TimeoutTracker::new(lease);
    if link.link.supports_priorities() {
        let (res, _, _) = select_all((Priority::MAX as u8..=Priority::MIN as u8).map(|prio| {
            let mut link = link.clone();
            let transport = transport.clone();
            let cancellation_token = cancellation_token.clone();
            let lease_tracker = lease_tracker.clone();
            #[cfg(feature = "stats")]
            let stats = stats.clone();
            let pool = pool.clone();
            zenoh_runtime::ZRuntime::RX.spawn(async move {
                cancellation_token
                    .run_until_cancelled(read_loop(
                        Some(Priority::try_from(prio).unwrap()),
                        &mut link,
                        transport,
                        lease_tracker,
                        #[cfg(feature = "stats")]
                        stats,
                        &pool,
                    ))
                    .await
            })
        }))
        .await;
        res.unwrap().transpose()?;
        Ok(())
    } else {
        read_loop(
            None,
            link,
            transport,
            lease_tracker,
            #[cfg(feature = "stats")]
            stats,
            &pool,
        )
        .await
    }
}

#[derive(Clone)]
enum ReceivePool<F: Fn() -> Box<[u8]>> {
    Recycling(RecyclingObjectPool<Box<[u8]>, F>),
    Charged(Arc<ReceiveReserve>),
}

struct ReceiveReserve {
    returning: flume::Sender<ChargedBytes>,
    available: flume::Receiver<ChargedBytes>,
}

impl ReceiveReserve {
    fn new(slots: usize, mtu: usize, budget: &ReceiveBudget) -> ZResult<Self> {
        let mut reservation = budget.charge(slots * mtu)?;
        let (returning, available) = flume::bounded(slots);
        for _ in 0..slots {
            let charge = reservation.split(mtu).expect("reserve was charged in full");
            let buffer = ChargedBytes::new(mtu, charge);
            let returned = returning.try_send(buffer);
            assert!(returned.is_ok(), "reserve has one slot per buffer");
        }
        Ok(Self {
            returning,
            available,
        })
    }

    fn loan(self: &Arc<Self>, mtu: usize, budget: &ReceiveBudget) -> ZResult<ReceiveBacking> {
        // Unlike the legacy pool's try-lock checkout, an empty channel means
        // there really is no free buffer. Lock contention is not pressure.
        let (buffer, reserve) = match self.available.try_recv() {
            Ok(buffer) => (buffer, Arc::downgrade(self)),
            Err(flume::TryRecvError::Empty) => {
                let charge = budget.charge(mtu)?;
                (ChargedBytes::new(mtu, charge), Weak::new())
            }
            Err(flume::TryRecvError::Disconnected) => {
                unreachable!("receive reserve owns both ends")
            }
        };
        Ok(ReceiveBacking {
            buffer: Some(buffer),
            reserve,
        })
    }
}

struct ChargedBytes {
    bytes: Box<[u8]>,
    _charge: OwnedSemaphorePermit,
}

impl ChargedBytes {
    fn new(mtu: usize, charge: OwnedSemaphorePermit) -> Self {
        Self {
            bytes: vec![0_u8; mtu].into_boxed_slice(),
            _charge: charge,
        }
    }
}

struct ReceiveBacking {
    buffer: Option<ChargedBytes>,
    reserve: Weak<ReceiveReserve>,
}

impl Drop for ReceiveBacking {
    fn drop(&mut self) {
        if let Some(reserve) = self.reserve.upgrade() {
            let buffer = self.buffer.take().expect("receive backing owns its buffer");
            let _returned = reserve.returning.try_send(buffer);
        }
    }
}

impl fmt::Debug for ReceiveBacking {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReceiveBacking")
            .field("len", &self.as_slice().len())
            .finish()
    }
}

impl AsMut<[u8]> for ReceiveBacking {
    fn as_mut(&mut self) -> &mut [u8] {
        self.buffer
            .as_mut()
            .expect("receive backing owns its buffer")
            .bytes
            .as_mut()
    }
}

impl ZSliceBuffer for ReceiveBacking {
    fn as_slice(&self) -> &[u8] {
        self.buffer
            .as_ref()
            .expect("receive backing owns its buffer")
            .bytes
            .as_ref()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod receive_budget {
    use std::{num::NonZeroU32, sync::Arc};

    use tokio::sync::TryAcquireError;

    use super::{ReceiveBudget, ReceiveReserve};

    #[test]
    fn reserves_and_extra_allocations_share_one_limit() {
        let batch = 4_096;
        let budget = ReceiveBudget::new(NonZeroU32::new(12_288).unwrap());
        let first = Arc::new(ReceiveReserve::new(1, batch, &budget).unwrap());
        let second = Arc::new(ReceiveReserve::new(1, batch, &budget).unwrap());
        let first_loan = first.loan(batch, &budget).unwrap();
        let extra = first.loan(batch, &budget).unwrap();
        // A free reserved batch can be checked out when new allocation is full.
        let second_loan = second.loan(batch, &budget).unwrap();
        let refused = second.loan(batch, &budget).unwrap_err();
        assert!(matches!(
            refused.downcast_ref(),
            Some(TryAcquireError::NoPermits)
        ));
        assert_eq!(budget.charged_bytes(), 12_288);
        assert_eq!(budget.refusals(), 1);

        drop(extra);
        let replacement = second.loan(batch, &budget).unwrap();
        assert_eq!(budget.charged_bytes(), 12_288);
        drop(second_loan);
        assert_eq!(budget.charged_bytes(), 12_288);
        let reused = second.loan(batch, &budget).unwrap();
        assert_eq!(budget.charged_bytes(), 12_288);
        drop(first);
        drop(second);
        assert_eq!(budget.charged_bytes(), 12_288);
        drop(first_loan);
        drop(replacement);
        drop(reused);
        assert_eq!(budget.charged_bytes(), 0);
        assert_eq!(budget.refusals(), 1);

        let closing = Arc::new(ReceiveReserve::new(2, batch, &budget).unwrap());
        let held = closing.loan(batch, &budget).unwrap();
        assert_eq!(budget.charged_bytes(), 8_192);
        drop(closing);
        assert_eq!(budget.charged_bytes(), 4_096);
        drop(held);
        assert_eq!(budget.charged_bytes(), 0);
    }
}

#[cfg(all(test, feature = "transport_tls"))]
mod transmit_retirement {
    use std::{num::NonZeroUsize, path::Path, sync::Arc, time::Duration};

    use tokio::{sync::Mutex, time::timeout};
    use zenoh_config::Config;
    use zenoh_link::{tls::LinkManagerUnicastTls, LinkManagerUnicastTrait};
    use zenoh_protocol::{
        core::{EndPoint, Field, Priority, WhatAmI},
        transport::{init::ext::PatchType, KeepAlive},
    };
    use zenoh_runtime::ZRuntime;

    use super::{
        super::transport::ClosableCallback, BatchConfig, TransportLinkUnicast,
        TransportLinkUnicastUniversal, TransportPriorityTx, TransportUnicastUniversal,
    };
    use crate::{
        unicast::{
            link::{TransportLinkUnicastConfig, TransportLinkUnicastDirection},
            transport_unicast_inner::TransportStatus,
            TransportConfigUnicast,
        },
        DummyTransportEventHandler, TransportManager,
    };

    fn endpoint(address: &str) -> EndPoint {
        let certificates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../zenoh-links/zenoh-link-tls/tests/fixtures/tls-ownership");
        let ca = certificates.join("root-ca.pem");
        let certificate = certificates.join("localhost-certificate.pem");
        let key = certificates.join("localhost-private-key.pem");
        let configuration = format!(
            "root_ca_certificate_file={};listen_certificate_file={};listen_private_key_file={};connect_certificate_file={};connect_private_key_file={};enable_mtls=true;verify_name_on_connect=true;use_public_pki=false",
            ca.display(), certificate.display(), key.display(), certificate.display(), key.display()
        );
        EndPoint::new("tls", address, "", configuration).unwrap()
    }

    #[test]
    fn retired_producer_keeps_its_tls_slot_until_the_queue_is_released() {
        ZRuntime::Application.block_on(async {
            timeout(Duration::from_secs(10), async {
                let mut config = Config::default();
                config
                    .transport
                    .link
                    .tls
                    .set_max_connections(Some(NonZeroUsize::MIN))
                    .unwrap();
                let native = TransportManager::builder()
                    .from_config(&config.expanded())
                    .await
                    .unwrap()
                    .build(Arc::new(DummyTransportEventHandler))
                    .unwrap();
                let slots = native.tls_connections.as_ref().unwrap().clone();
                let resolution = native.config.resolution.get(Field::FrameSN);
                let transmit = TransportUnicastUniversal {
                    manager: Arc::new(native),
                    config: Arc::new(TransportConfigUnicast {
                        zid: Default::default(),
                        whatami: WhatAmI::Peer,
                        region_name: None,
                        bound: None,
                        sn_resolution: resolution,
                        tx_initial_sn: 0,
                        is_qos: false,
                        is_lowlatency: false,
                        patch: PatchType::CURRENT,
                    }),
                    priority_tx: Arc::new([TransportPriorityTx::make(resolution).unwrap()]),
                    priority_rx: Arc::new([]),
                    links: Default::default(),
                    callback: Arc::new(ClosableCallback::new()),
                    status: Arc::new(Mutex::new(TransportStatus::Uninitialized)),
                };
                let (publication, incoming) = flume::bounded(1);
                let listener = LinkManagerUnicastTls::new(publication.clone());
                let caller =
                    LinkManagerUnicastTls::with_connection_limit(publication, slots.clone());
                let locator = listener
                    .new_listener(endpoint("localhost:0"))
                    .await
                    .unwrap();
                let address = locator.address();
                let port = address.as_str().rsplit(':').next().unwrap();
                let peer = endpoint(&format!("localhost:{port}"));

                let idle = slots.available_permits();
                let outgoing = caller.new_link(peer.clone()).await.unwrap();
                let accepted = incoming.recv_async().await.unwrap();
                let active = slots.available_permits();
                let link = TransportLinkUnicast::new(
                    outgoing,
                    TransportLinkUnicastConfig {
                        direction: TransportLinkUnicastDirection::Outbound,
                        batch: BatchConfig {
                            mtu: transmit.manager.config.batch_size,
                            is_streamed: true,
                        },
                        priorities: None,
                        reliability: None,
                    },
                );
                let (link, consumer) =
                    TransportLinkUnicastUniversal::new(&transmit, link, &transmit.priority_tx);
                let producer = link.pipeline.clone();
                let queued = producer.push_transport_message(KeepAlive.into(), Priority::DEFAULT);

                let closed = tokio::join!(link.close(), accepted.close());
                drop((consumer, accepted));
                let held = slots.available_permits();
                drop(producer);
                let released = slots.available_permits();

                let replacement = caller.new_link(peer.clone()).await.unwrap();
                let accepted = incoming.recv_async().await.unwrap();
                let replaced = slots.available_permits();
                let closed_replacement = tokio::join!(replacement.close(), accepted.close());
                drop((replacement, accepted));
                let retired = slots.available_permits();
                let closed_listener = listener.del_listener(&peer).await;
                transmit.manager.close().await;

                closed.0.unwrap();
                closed.1.unwrap();
                closed_replacement.0.unwrap();
                closed_replacement.1.unwrap();
                closed_listener.unwrap();
                assert!(queued);
                assert_eq!(
                    [idle, active, held, released, replaced, retired],
                    [1, 0, 0, 1, 0, 1]
                );
            })
            .await
            .expect("the TLS connection and retirement complete");
        });
    }
}

async fn read_loop<F: Fn() -> Box<[u8]>>(
    priority: Option<Priority>,
    link: &mut TransportLinkUnicastRx,
    transport: TransportUnicastUniversal,
    lease_tracker: TimeoutTracker,
    #[cfg(feature = "stats")] stats: zenoh_stats::LinkStats,
    pool: &ReceivePool<F>,
) -> ZResult<()> {
    async fn read<F: Fn() -> Box<[u8]>>(
        link: &mut TransportLinkUnicastRx,
        priority: Option<Priority>,
        pool: &ReceivePool<F>,
        budget: Option<&ReceiveBudget>,
    ) -> ZResult<RBatch<ZSlice>> {
        match pool {
            ReceivePool::Recycling(buffers) => {
                link.recv_batch(
                    || buffers.try_take().unwrap_or_else(|| buffers.alloc()),
                    priority,
                )
                .await
            }
            ReceivePool::Charged(reserve) => {
                let budget = budget.expect("charged reserve has a receive budget");
                let buffer = reserve.loan(link.config.batch.mtu as usize, budget)?;
                link.recv_charged_batch(buffer, priority).await
            }
        }
    }

    loop {
        tokio::select! {
            batch = read(link, priority, pool, transport.manager.unicast_receive.as_deref()) => {
                let batch = batch?;
                lease_tracker.reset();
                #[cfg(feature = "stats")]
                {
                    let header_bytes = if link.link.is_streamed() { 2 } else { 0 };
                    stats.inc_bytes(zenoh_stats::Rx, header_bytes + batch.len() as u64);
                }
                transport.read_messages(batch, link, #[cfg(feature = "stats")] &stats)?;
            }
            _ = lease_tracker.wait_if(priority.unwrap_or(Priority::Control) == Priority::Control) => {
                bail!("{link}: expired after {} milliseconds", lease_tracker.timeout().as_millis());
            }
        }
    }
}

struct TimeoutTrackerInner {
    timeout: Duration,
    waker: AtomicWaker,
    has_timed_out: AtomicBool,
    latest_reset: Mutex<Instant>,
    task: OnceLock<JoinHandle<()>>,
}

#[derive(Clone)]
struct TimeoutTracker(Arc<TimeoutTrackerInner>);

impl TimeoutTracker {
    fn new(timeout: Duration) -> TimeoutTracker {
        let now = Instant::now();
        let inner = Arc::new(TimeoutTrackerInner {
            timeout,
            waker: AtomicWaker::new(),
            has_timed_out: AtomicBool::new(false),
            latest_reset: Mutex::new(now),
            task: OnceLock::new(),
        });
        let tracker = Arc::downgrade(&inner);
        let task = tokio::spawn(async move {
            let mut latest_reset = now;
            loop {
                tokio::time::sleep_until((latest_reset + timeout).into()).await;
                let prev = latest_reset;
                let Some(tracker) = tracker.upgrade() else {
                    break;
                };
                latest_reset = *tracker.latest_reset.lock().unwrap();
                if latest_reset <= prev {
                    latest_reset = Instant::now();
                    tracker.has_timed_out.store(true, Ordering::Release);
                    tracker.waker.wake();
                }
            }
        });
        inner.task.set(task).unwrap();
        Self(inner)
    }

    fn timeout(&self) -> Duration {
        self.0.timeout
    }

    fn reset(&self) {
        *self.0.latest_reset.lock().unwrap() = Instant::now();
    }

    async fn wait_if(&self, predicate: bool) {
        poll_fn(|cx| {
            if !predicate {
                return Poll::Pending;
            }
            self.0.waker.register(cx.waker());
            if self.0.has_timed_out.load(Ordering::Acquire) {
                self.0.has_timed_out.store(false, Ordering::Release);
                return Poll::Ready(());
            }
            Poll::Pending
        })
        .await
    }
}

impl Drop for TimeoutTracker {
    fn drop(&mut self) {
        self.0.task.get().unwrap().abort();
    }
}

#[cfg(all(
    feature = "uring",
    target_os = "linux",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "loongarch64",
        target_arch = "powerpc64"
    )
))]
async fn rx_task_uring(
    link: &mut TransportLinkUnicastRx,
    transport: TransportUnicastUniversal,
    lease: Duration,
    rx_buffer_size: usize,
    #[cfg(feature = "stats")] stats: zenoh_stats::LinkStats,
    cancellation_token: CancellationToken,
) -> ZResult<()> {
    // The pool of buffers
    let mtu = link.config.batch.mtu as usize;
    let mut n = rx_buffer_size / mtu;
    if n == 0 {
        tracing::debug!("RX configured buffer of {rx_buffer_size} bytes is too small for {link} that has an MTU of {mtu} bytes. Defaulting to {mtu} bytes for RX buffer.");
        n = 1;
    }

    let pool = RecyclingObjectPool::new(n, move || vec![0_u8; mtu].into_boxed_slice());

    let c_link = link.clone();

    let batch_config = link.config.batch;

    let r = transport
        .manager
        .state
        .uring
        .as_ref()
        .expect("uring is Some: checked by caller")
        .reader
        .clone();

    let lease_tracker = TimeoutTracker::new(lease);

    fn read_batch<TBuffer: BacktrackableReader + Buffer + Debug>(
        transport: &TransportUnicastUniversal,
        link: &TransportLinkUnicastRx,
        batch: RBatch<TBuffer>,
        #[cfg(feature = "stats")] stats: &zenoh_stats::LinkStats,
    ) -> ZResult<()> {
        #[cfg(feature = "stats")]
        {
            let header_bytes = 2;
            stats.inc_bytes(zenoh_stats::Rx, header_bytes + batch.len() as u64);
        }
        transport.read_messages(
            batch,
            link,
            #[cfg(feature = "stats")]
            stats,
        )
    }

    let mut uring_read_task = {
        match link.link.is_streamed() {
            true => {
                let lt = lease_tracker.clone();
                let ring_cb = move |data: FragmentedBatch| {
                    lt.reset();
                    match data.defragment()? {
                        DefragmentationState::Single(slice) => {
                            let mut batch = RBatch::new(batch_config, slice);
                            batch.initialize_uring(|| {
                                pool.try_take().unwrap_or_else(|| pool.alloc())
                            })?;
                            read_batch(
                                &transport,
                                &c_link,
                                batch,
                                #[cfg(feature = "stats")]
                                &stats,
                            )
                        }
                        DefragmentationState::Fragmented(buf) => {
                            let mut batch = RBatch::new(batch_config, buf.reader());
                            match batch.initialize_uring(|| {
                                pool.try_take().unwrap_or_else(|| pool.alloc())
                            })? {
                                Some(decompressed_batch) => read_batch(
                                    &transport,
                                    &c_link,
                                    decompressed_batch,
                                    #[cfg(feature = "stats")]
                                    &stats,
                                ),
                                None => read_batch(
                                    &transport,
                                    &c_link,
                                    batch,
                                    #[cfg(feature = "stats")]
                                    &stats,
                                ),
                            }
                        }
                    }
                };

                r.setup_fragmented_read(link.link.get_fd()?, ring_cb)
                    .await?
            }
            false => {
                let lt = lease_tracker.clone();
                let ring_cb = move |data: Arc<RxBuffer>| {
                    lt.reset();
                    let slice: ZSlice = data.into();
                    let mut batch = RBatch::new(batch_config, slice);
                    batch.initialize_uring(|| pool.try_take().unwrap_or_else(|| pool.alloc()))?;

                    read_batch(
                        &transport,
                        &c_link,
                        batch,
                        #[cfg(feature = "stats")]
                        &stats,
                    )
                };

                r.setup_read(link.link.get_fd()?, ring_cb).await?
            }
        }
    };

    let result = tokio::select! {
        e = uring_read_task.read_error() => {
            tracing::debug!("Uring RX task stopped by uring task error event");
            Err(e)
        },
        finished = r.wait_finished() => {
            tracing::debug!("Uring RX task stopped by uring finished event: {:?}", finished);
            finished
        }
        _ = lease_tracker.wait_if(true) => {
            tracing::debug!("Uring RX task stopped by lease timeout");
            bail!("{link}: expired after {} milliseconds", lease.as_millis());
        }
        _ = cancellation_token.cancelled() => {
            tracing::debug!("Uring RX task stopped by cancellation event");
            Ok(())
        }
    };

    uring_read_task.stop().await;

    result
}
