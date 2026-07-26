//! iroh custom transport backed by a platform-owned connected datagram channel.

use std::fmt;
use std::io::{self, IoSliceMut};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use envoix_error::CoreError;
use iroh::endpoint::presets;
use iroh::endpoint::transports::{
    CustomEndpoint, CustomSender, CustomTransport, RecvInfo, Transmit,
};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr};
use iroh_base::CustomAddr;
use n0_watcher::Watchable;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::PollSender;

use crate::endpoint::fixed_mtu_data_transport_config;
use crate::{
    BoundEndpoint, CandidateFilter, SessionError, TransferCancelToken, TransferProtocol,
    interrupted_error,
};

/// Private iroh transport identifier derived from the ASCII bytes `envoix`.
pub(crate) const WIFI_AWARE_TRANSPORT_ID: u64 = 0x65_6e_76_6f_69_78;

const BOOTSTRAP_MAGIC: &[u8; 8] = b"ENVXWA02";
const BOOTSTRAP_CLIENT_HELLO: u8 = 1;
const BOOTSTRAP_SERVER_HELLO: u8 = 2;
const BOOTSTRAP_FRAME_BYTES: usize = BOOTSTRAP_MAGIC.len() + 1 + 32 + 32 + 2;
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(30);
const BOOTSTRAP_RETRY_INTERVAL: Duration = Duration::from_millis(300);
const BOOTSTRAP_SERVER_REPLY_COUNT: usize = 5;
const BOOTSTRAP_SERVER_REPLY_INTERVAL: Duration = Duration::from_millis(50);
const DATAGRAM_QUEUE_CAPACITY: usize = 256;
const MAX_PLATFORM_DATAGRAM_BYTES: u32 = u16::MAX as u32;
const MIN_QUIC_DATAGRAM_BYTES: u16 = 1_200;
const PLATFORM_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// A connected, message-preserving datagram channel owned by the platform.
#[async_trait]
pub trait PlatformDatagramTransport: Send + Sync {
    async fn send_datagram(&self, bytes: Vec<u8>) -> Result<(), SessionError>;

    async fn receive_datagram(&self, max_bytes: u32) -> Result<Vec<u8>, SessionError>;

    async fn close(&self) -> Result<(), SessionError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DatagramTransportRole {
    Client,
    Server,
}

pub(crate) struct BoundDatagramEndpoint {
    pub(crate) bound_endpoint: BoundEndpoint,
    pub(crate) peer_addr: EndpointAddr,
    pub(crate) bridge: DatagramTransportBridge,
}

pub(crate) async fn bind_datagram_endpoint(
    transport: Arc<dyn PlatformDatagramTransport>,
    role: DatagramTransportRole,
    maximum_datagram_size: u32,
    window: u32,
    cancel: &TransferCancelToken,
) -> Result<BoundDatagramEndpoint, SessionError> {
    let maximum_datagram_size = validate_maximum_datagram_size(maximum_datagram_size)?;
    let secret_key = SecretKey::generate();
    let local_id = secret_key.public();
    let (remote_id, negotiated_datagram_size) = match exchange_endpoint_ids(
        transport.clone(),
        role,
        local_id,
        maximum_datagram_size,
        cancel,
    )
    .await
    {
        Ok(exchange) => exchange,
        Err(error) => {
            close_platform_transport(transport).await;
            return Err(error);
        }
    };
    let local_addr = custom_addr(local_id);
    let remote_addr = custom_addr(remote_id);
    let (custom_transport, bridge) = DatagramCustomTransport::start(
        transport,
        role,
        local_addr.clone(),
        remote_addr.clone(),
        maximum_datagram_size,
    );
    let endpoint = match Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .alpns(vec![TransferProtocol::ManifestV2.alpn().to_vec()])
        .transport_config(fixed_mtu_data_transport_config(
            window,
            negotiated_datagram_size,
        ))
        .clear_address_lookup()
        .clear_ip_transports()
        .clear_relay_transports()
        .add_custom_transport(custom_transport)
        .bind()
        .await
    {
        Ok(endpoint) => endpoint,
        Err(error) => {
            bridge.close().await;
            return Err(CoreError::Transport(error.to_string()));
        }
    };
    let peer_addr = EndpointAddr::from_parts(remote_id, [TransportAddr::Custom(remote_addr)]);
    Ok(BoundDatagramEndpoint {
        bound_endpoint: BoundEndpoint {
            local_endpoint: endpoint,
            candidates: CandidateFilter::default(),
        },
        peer_addr,
        bridge,
    })
}

fn custom_addr(endpoint_id: EndpointId) -> CustomAddr {
    CustomAddr::from_parts(WIFI_AWARE_TRANSPORT_ID, endpoint_id.as_bytes())
}

async fn exchange_endpoint_ids(
    transport: Arc<dyn PlatformDatagramTransport>,
    role: DatagramTransportRole,
    local_id: EndpointId,
    maximum_datagram_size: u16,
    cancel: &TransferCancelToken,
) -> Result<(EndpointId, u16), SessionError> {
    tokio::select! {
        result = tokio::time::timeout(
            BOOTSTRAP_TIMEOUT,
            exchange_endpoint_ids_inner(transport, role, local_id, maximum_datagram_size),
        ) => {
            result.map_err(|_| {
                CoreError::Transport("Wi-Fi Aware datagram bootstrap timed out".into())
            })?
        }
        () = cancel.cancelled() => Err(interrupted_error(cancel)),
    }
}

async fn exchange_endpoint_ids_inner(
    transport: Arc<dyn PlatformDatagramTransport>,
    role: DatagramTransportRole,
    local_id: EndpointId,
    maximum_datagram_size: u16,
) -> Result<(EndpointId, u16), SessionError> {
    match role {
        DatagramTransportRole::Client => {
            let hello = encode_bootstrap(
                BOOTSTRAP_CLIENT_HELLO,
                local_id,
                None,
                maximum_datagram_size,
            );
            let mut retry = tokio::time::interval(BOOTSTRAP_RETRY_INTERVAL);
            retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut receive = Box::pin(receive_platform_datagram(transport.clone()));
            loop {
                tokio::select! {
                    received = &mut receive => {
                        let bytes = match received {
                            Ok(bytes) => bytes,
                            Err(error) => return Err(error),
                        };
                        let frame = match parse_bootstrap(&bytes) {
                            Ok(Some(frame)) => frame,
                            Ok(None) => {
                                receive.set(receive_platform_datagram(transport.clone()));
                                continue;
                            }
                            Err(error) => return Err(error),
                        };
                        if frame.kind == BOOTSTRAP_SERVER_HELLO
                            && frame.peer_id == Some(local_id)
                        {
                            return Ok((
                                frame.sender_id,
                                maximum_datagram_size.min(frame.maximum_datagram_size),
                            ));
                        }
                        receive.set(receive_platform_datagram(transport.clone()));
                    }
                    _ = retry.tick() => {
                        transport.send_datagram(hello.clone()).await?;
                    }
                }
            }
        }
        DatagramTransportRole::Server => loop {
            let bytes = transport
                .receive_datagram(MAX_PLATFORM_DATAGRAM_BYTES)
                .await?;
            let Some(frame) = parse_bootstrap(&bytes)? else {
                continue;
            };
            if frame.kind != BOOTSTRAP_CLIENT_HELLO || frame.peer_id.is_some() {
                continue;
            }
            let reply = encode_bootstrap(
                BOOTSTRAP_SERVER_HELLO,
                local_id,
                Some(frame.sender_id),
                maximum_datagram_size,
            );
            for index in 0..BOOTSTRAP_SERVER_REPLY_COUNT {
                if let Err(error) = transport.send_datagram(reply.clone()).await {
                    if index == 0 {
                        return Err(error);
                    }
                    break;
                }
                if index + 1 < BOOTSTRAP_SERVER_REPLY_COUNT {
                    tokio::time::sleep(BOOTSTRAP_SERVER_REPLY_INTERVAL).await;
                }
            }
            return Ok((
                frame.sender_id,
                maximum_datagram_size.min(frame.maximum_datagram_size),
            ));
        },
    }
}

async fn receive_platform_datagram(
    transport: Arc<dyn PlatformDatagramTransport>,
) -> Result<Vec<u8>, SessionError> {
    transport
        .receive_datagram(MAX_PLATFORM_DATAGRAM_BYTES)
        .await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BootstrapFrame {
    kind: u8,
    sender_id: EndpointId,
    peer_id: Option<EndpointId>,
    maximum_datagram_size: u16,
}

fn encode_bootstrap(
    kind: u8,
    sender_id: EndpointId,
    peer_id: Option<EndpointId>,
    maximum_datagram_size: u16,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(BOOTSTRAP_FRAME_BYTES);
    bytes.extend_from_slice(BOOTSTRAP_MAGIC);
    bytes.push(kind);
    bytes.extend_from_slice(sender_id.as_bytes());
    bytes.extend_from_slice(
        peer_id
            .map(|id| *id.as_bytes())
            .unwrap_or([0_u8; 32])
            .as_slice(),
    );
    bytes.extend_from_slice(&maximum_datagram_size.to_be_bytes());
    bytes
}

fn parse_bootstrap(bytes: &[u8]) -> Result<Option<BootstrapFrame>, SessionError> {
    if !bytes.starts_with(BOOTSTRAP_MAGIC) {
        return Ok(None);
    }
    if bytes.len() != BOOTSTRAP_FRAME_BYTES {
        return Err(CoreError::Protocol(
            "invalid Wi-Fi Aware datagram bootstrap frame length".into(),
        ));
    }
    let sender_bytes: &[u8; 32] = bytes[9..41]
        .try_into()
        .map_err(|_| CoreError::Protocol("invalid bootstrap sender ID".into()))?;
    let peer_bytes: &[u8; 32] = bytes[41..73]
        .try_into()
        .map_err(|_| CoreError::Protocol("invalid bootstrap peer ID".into()))?;
    let sender_id = EndpointId::from_bytes(sender_bytes)
        .map_err(|_| CoreError::Protocol("invalid bootstrap sender key".into()))?;
    let peer_id = if peer_bytes.iter().all(|byte| *byte == 0) {
        None
    } else {
        Some(
            EndpointId::from_bytes(peer_bytes)
                .map_err(|_| CoreError::Protocol("invalid bootstrap peer key".into()))?,
        )
    };
    let maximum_datagram_size = u16::from_be_bytes(
        bytes[73..75]
            .try_into()
            .map_err(|_| CoreError::Protocol("invalid bootstrap datagram size".into()))?,
    );
    if maximum_datagram_size < MIN_QUIC_DATAGRAM_BYTES {
        return Err(CoreError::Protocol(
            "Wi-Fi Aware datagram size is below the QUIC minimum".into(),
        ));
    }
    Ok(Some(BootstrapFrame {
        kind: bytes[8],
        sender_id,
        peer_id,
        maximum_datagram_size,
    }))
}

fn validate_maximum_datagram_size(maximum_datagram_size: u32) -> Result<u16, SessionError> {
    let maximum_datagram_size = u16::try_from(maximum_datagram_size).map_err(|_| {
        CoreError::InvalidInput("platform maximum datagram size exceeds u16".into())
    })?;
    if maximum_datagram_size < MIN_QUIC_DATAGRAM_BYTES {
        return Err(CoreError::InvalidInput(format!(
            "platform maximum datagram size must be at least {MIN_QUIC_DATAGRAM_BYTES}"
        )));
    }
    Ok(maximum_datagram_size)
}

fn is_bootstrap(bytes: &[u8]) -> bool {
    bytes.starts_with(BOOTSTRAP_MAGIC)
}

struct DatagramCustomTransport {
    local_addr: CustomAddr,
    remote_addr: CustomAddr,
    local_addrs: Watchable<Vec<CustomAddr>>,
    inbound: Mutex<Option<mpsc::Receiver<Vec<u8>>>>,
    outbound: mpsc::Sender<Vec<u8>>,
    failure: Arc<Mutex<Option<String>>>,
}

impl fmt::Debug for DatagramCustomTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DatagramCustomTransport")
            .field("transport_id", &WIFI_AWARE_TRANSPORT_ID)
            .finish_non_exhaustive()
    }
}

impl DatagramCustomTransport {
    fn start(
        transport: Arc<dyn PlatformDatagramTransport>,
        role: DatagramTransportRole,
        local_addr: CustomAddr,
        remote_addr: CustomAddr,
        maximum_datagram_size: u16,
    ) -> (Arc<Self>, DatagramTransportBridge) {
        let (inbound_tx, inbound_rx) = mpsc::channel(DATAGRAM_QUEUE_CAPACITY);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(DATAGRAM_QUEUE_CAPACITY);
        let failure = Arc::new(Mutex::new(None));

        let outbound_transport = transport.clone();
        let outbound_failure = failure.clone();
        let outbound_task = tokio::spawn(async move {
            while let Some(bytes) = outbound_rx.recv().await {
                if let Err(error) = outbound_transport.send_datagram(bytes).await {
                    store_failure(&outbound_failure, error.to_string());
                    break;
                }
            }
        });

        let inbound_transport = transport.clone();
        let inbound_failure = failure.clone();
        let local_id = endpoint_id_from_custom(&local_addr)
            .expect("local custom address is constructed from an endpoint ID");
        let remote_id = endpoint_id_from_custom(&remote_addr)
            .expect("remote custom address is constructed from an endpoint ID");
        let inbound_task = tokio::spawn(async move {
            loop {
                let bytes = match inbound_transport
                    .receive_datagram(MAX_PLATFORM_DATAGRAM_BYTES)
                    .await
                {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        store_failure(&inbound_failure, error.to_string());
                        break;
                    }
                };
                if is_bootstrap(&bytes) {
                    if role == DatagramTransportRole::Server
                        && matches!(
                            parse_bootstrap(&bytes),
                            Ok(Some(BootstrapFrame {
                                kind: BOOTSTRAP_CLIENT_HELLO,
                                sender_id,
                                peer_id: None,
                                ..
                            })) if sender_id == remote_id
                        )
                    {
                        let reply = encode_bootstrap(
                            BOOTSTRAP_SERVER_HELLO,
                            local_id,
                            Some(remote_id),
                            maximum_datagram_size,
                        );
                        if let Err(error) = inbound_transport.send_datagram(reply).await {
                            store_failure(&inbound_failure, error.to_string());
                            break;
                        }
                    }
                    continue;
                }
                if inbound_tx.send(bytes).await.is_err() {
                    break;
                }
            }
        });

        let custom_transport = Arc::new(Self {
            local_addr: local_addr.clone(),
            remote_addr,
            local_addrs: Watchable::new(vec![local_addr]),
            inbound: Mutex::new(Some(inbound_rx)),
            outbound: outbound_tx,
            failure,
        });
        let bridge = DatagramTransportBridge {
            transport,
            inbound_task: Some(inbound_task),
            outbound_task: Some(outbound_task),
        };
        (custom_transport, bridge)
    }
}

impl CustomTransport for DatagramCustomTransport {
    fn bind(&self) -> io::Result<Box<dyn CustomEndpoint>> {
        let inbound = self
            .inbound
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or_else(|| io::Error::other("Wi-Fi Aware custom transport was already bound"))?;
        Ok(Box::new(DatagramCustomEndpoint {
            local_addr: self.local_addr.clone(),
            remote_addr: self.remote_addr.clone(),
            local_addrs: self.local_addrs.clone(),
            inbound,
            outbound: self.outbound.clone(),
            failure: self.failure.clone(),
        }))
    }
}

struct DatagramCustomEndpoint {
    local_addr: CustomAddr,
    remote_addr: CustomAddr,
    local_addrs: Watchable<Vec<CustomAddr>>,
    inbound: mpsc::Receiver<Vec<u8>>,
    outbound: mpsc::Sender<Vec<u8>>,
    failure: Arc<Mutex<Option<String>>>,
}

impl fmt::Debug for DatagramCustomEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DatagramCustomEndpoint")
            .field("transport_id", &WIFI_AWARE_TRANSPORT_ID)
            .finish_non_exhaustive()
    }
}

impl CustomEndpoint for DatagramCustomEndpoint {
    fn watch_local_addrs(&self) -> n0_watcher::Direct<Vec<CustomAddr>> {
        self.local_addrs.watch()
    }

    fn create_sender(&self) -> Arc<dyn CustomSender> {
        Arc::new(DatagramCustomSender {
            local_addr: self.local_addr.clone(),
            remote_addr: self.remote_addr.clone(),
            outbound: Mutex::new(PollSender::new(self.outbound.clone())),
        })
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        metas: &mut [noq_udp::RecvMeta],
        recv_infos: &mut [RecvInfo],
    ) -> Poll<io::Result<usize>> {
        assert_eq!(bufs.len(), metas.len());
        assert_eq!(bufs.len(), recv_infos.len());
        if bufs.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut packets = Vec::with_capacity(bufs.len());
        match self.inbound.poll_recv_many(cx, &mut packets, bufs.len()) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(0) => {
                return Poll::Ready(Err(bridge_io_error(
                    &self.failure,
                    "Wi-Fi Aware datagram receive channel closed",
                )));
            }
            Poll::Ready(_) => {}
        }
        for (index, packet) in packets.iter().enumerate() {
            if packet.len() > bufs[index].len() {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "iroh receive buffer is smaller than a Wi-Fi Aware datagram",
                )));
            }
            bufs[index][..packet.len()].copy_from_slice(packet);
            metas[index].len = packet.len();
            metas[index].stride = packet.len();
            recv_infos[index] =
                RecvInfo::new(self.remote_addr.clone(), Some(self.local_addr.clone()));
        }
        Poll::Ready(Ok(packets.len()))
    }
}

struct DatagramCustomSender {
    local_addr: CustomAddr,
    remote_addr: CustomAddr,
    outbound: Mutex<PollSender<Vec<u8>>>,
}

impl fmt::Debug for DatagramCustomSender {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DatagramCustomSender")
            .field("transport_id", &WIFI_AWARE_TRANSPORT_ID)
            .finish_non_exhaustive()
    }
}

impl CustomSender for DatagramCustomSender {
    fn is_valid_send_addr(&self, addr: &CustomAddr) -> bool {
        addr == &self.remote_addr
    }

    fn poll_send(
        &self,
        cx: &mut Context<'_>,
        dst: &CustomAddr,
        src: Option<&CustomAddr>,
        transmit: &Transmit<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.is_valid_send_addr(dst) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unexpected Wi-Fi Aware custom destination",
            )));
        }
        if src.is_some_and(|source| source != &self.local_addr) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unexpected Wi-Fi Aware custom source",
            )));
        }
        if transmit
            .segment_size
            .is_some_and(|segment_size| segment_size < transmit.contents.len())
        {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Wi-Fi Aware custom transport does not support GSO",
            )));
        }
        let mut outbound = self
            .outbound
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match outbound.poll_reserve(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                error.to_string(),
            ))),
            Poll::Ready(Ok(())) => {
                let bytes = transmit.contents.to_vec();
                Poll::Ready(
                    outbound.send_item(bytes).map_err(|error| {
                        io::Error::new(io::ErrorKind::BrokenPipe, error.to_string())
                    }),
                )
            }
        }
    }
}

pub(crate) struct DatagramTransportBridge {
    transport: Arc<dyn PlatformDatagramTransport>,
    inbound_task: Option<JoinHandle<()>>,
    outbound_task: Option<JoinHandle<()>>,
}

impl fmt::Debug for DatagramTransportBridge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DatagramTransportBridge")
            .finish_non_exhaustive()
    }
}

impl DatagramTransportBridge {
    pub(crate) async fn close(mut self) {
        abort_task(&mut self.inbound_task).await;
        abort_task(&mut self.outbound_task).await;
        close_platform_transport(self.transport.clone()).await;
    }
}

impl Drop for DatagramTransportBridge {
    fn drop(&mut self) {
        if let Some(task) = self.inbound_task.take() {
            task.abort();
        }
        if let Some(task) = self.outbound_task.take() {
            task.abort();
        }
    }
}

async fn abort_task(task: &mut Option<JoinHandle<()>>) {
    if let Some(task) = task.take() {
        task.abort();
        let _ = task.await;
    }
}

async fn close_platform_transport(transport: Arc<dyn PlatformDatagramTransport>) {
    let _ = tokio::time::timeout(PLATFORM_CLOSE_TIMEOUT, transport.close()).await;
}

fn endpoint_id_from_custom(addr: &CustomAddr) -> Result<EndpointId, CoreError> {
    if addr.id() != WIFI_AWARE_TRANSPORT_ID {
        return Err(CoreError::InvalidInput(
            "unexpected custom transport ID".into(),
        ));
    }
    let bytes: &[u8; 32] = addr
        .data()
        .try_into()
        .map_err(|_| CoreError::InvalidInput("invalid custom endpoint ID length".into()))?;
    EndpointId::from_bytes(bytes)
        .map_err(|_| CoreError::InvalidInput("invalid custom endpoint key".into()))
}

fn store_failure(failure: &Mutex<Option<String>>, message: String) {
    let mut failure = failure
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if failure.is_none() {
        *failure = Some(message);
    }
}

fn bridge_io_error(failure: &Mutex<Option<String>>, fallback: &str) -> io::Error {
    let message = failure
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .unwrap_or_else(|| fallback.to_string());
    io::Error::other(message)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use envoix_protocol::manifest_v2::CompressionPolicyV2;
    use envoix_transfer::{
        CanonicalTransferJob, DestinationDecisionV2, DestinationRequestV2, EventSink,
        POST_SAVE_RESERVE_BYTES, TransferEvent,
    };
    use tempfile::tempdir;
    use tokio::fs;
    use tokio::sync::Mutex as AsyncMutex;

    use super::*;
    use crate::{
        PairingConfig, receive_manifest_v2_offer_over_datagram_transport,
        send_manifest_v2_over_datagram_transport,
    };

    struct MemoryDatagramTransport {
        outbound: mpsc::Sender<Vec<u8>>,
        inbound: AsyncMutex<mpsc::Receiver<Vec<u8>>>,
        sends: AtomicUsize,
        largest_send: AtomicUsize,
        maximum_datagram_size: usize,
        drop_first_send: bool,
        closed: AtomicBool,
    }

    impl MemoryDatagramTransport {
        fn pair(drop_first_client_send: bool) -> (Arc<Self>, Arc<Self>) {
            Self::pair_with_limits(
                drop_first_client_send,
                MIN_QUIC_DATAGRAM_BYTES,
                MIN_QUIC_DATAGRAM_BYTES,
            )
        }

        fn pair_with_limits(
            drop_first_client_send: bool,
            client_maximum_datagram_size: u16,
            server_maximum_datagram_size: u16,
        ) -> (Arc<Self>, Arc<Self>) {
            let (left_tx, left_rx) = mpsc::channel(DATAGRAM_QUEUE_CAPACITY);
            let (right_tx, right_rx) = mpsc::channel(DATAGRAM_QUEUE_CAPACITY);
            (
                Arc::new(Self {
                    outbound: right_tx,
                    inbound: AsyncMutex::new(left_rx),
                    sends: AtomicUsize::new(0),
                    largest_send: AtomicUsize::new(0),
                    maximum_datagram_size: usize::from(client_maximum_datagram_size),
                    drop_first_send: drop_first_client_send,
                    closed: AtomicBool::new(false),
                }),
                Arc::new(Self {
                    outbound: left_tx,
                    inbound: AsyncMutex::new(right_rx),
                    sends: AtomicUsize::new(0),
                    largest_send: AtomicUsize::new(0),
                    maximum_datagram_size: usize::from(server_maximum_datagram_size),
                    drop_first_send: false,
                    closed: AtomicBool::new(false),
                }),
            )
        }

        fn largest_send(&self) -> usize {
            self.largest_send.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl PlatformDatagramTransport for MemoryDatagramTransport {
        async fn send_datagram(&self, bytes: Vec<u8>) -> Result<(), SessionError> {
            if bytes.len() > self.maximum_datagram_size {
                return Err(CoreError::Transport(format!(
                    "memory datagram size {} exceeds platform maximum {}",
                    bytes.len(),
                    self.maximum_datagram_size
                )));
            }
            self.largest_send.fetch_max(bytes.len(), Ordering::SeqCst);
            let send_index = self.sends.fetch_add(1, Ordering::SeqCst);
            if self.drop_first_send && send_index == 0 {
                return Ok(());
            }
            self.outbound
                .send(bytes)
                .await
                .map_err(|_| CoreError::Transport("memory datagram peer closed".into()))
        }

        async fn receive_datagram(&self, max_bytes: u32) -> Result<Vec<u8>, SessionError> {
            let bytes = self
                .inbound
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| CoreError::Transport("memory datagram peer closed".into()))?;
            if bytes.len() > max_bytes as usize {
                return Err(CoreError::Transport(
                    "memory datagram exceeded receive bound".into(),
                ));
            }
            Ok(bytes)
        }

        async fn close(&self) -> Result<(), SessionError> {
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    struct CancellationLingeringDatagramTransport {
        inner: Arc<dyn PlatformDatagramTransport>,
        receive_active: Arc<AtomicBool>,
    }

    struct CancellationLingeringReceiveGuard {
        receive_active: Arc<AtomicBool>,
        completed: bool,
    }

    impl Drop for CancellationLingeringReceiveGuard {
        fn drop(&mut self) {
            if self.completed {
                self.receive_active.store(false, Ordering::SeqCst);
                return;
            }
            let receive_active = self.receive_active.clone();
            tokio::spawn(async move {
                tokio::time::sleep(BOOTSTRAP_RETRY_INTERVAL.saturating_mul(2)).await;
                receive_active.store(false, Ordering::SeqCst);
            });
        }
    }

    #[async_trait]
    impl PlatformDatagramTransport for CancellationLingeringDatagramTransport {
        async fn send_datagram(&self, bytes: Vec<u8>) -> Result<(), SessionError> {
            self.inner.send_datagram(bytes).await
        }

        async fn receive_datagram(&self, max_bytes: u32) -> Result<Vec<u8>, SessionError> {
            if self
                .receive_active
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return Err(CoreError::Transport(
                    "concurrent foreign datagram receive".into(),
                ));
            }
            let mut guard = CancellationLingeringReceiveGuard {
                receive_active: self.receive_active.clone(),
                completed: false,
            };
            let result = self.inner.receive_datagram(max_bytes).await;
            guard.completed = true;
            result
        }

        async fn close(&self) -> Result<(), SessionError> {
            self.inner.close().await
        }
    }

    #[derive(Default)]
    struct RecordingEvents {
        events: Mutex<Vec<TransferEvent>>,
    }

    impl EventSink for RecordingEvents {
        fn on_event(&self, event: TransferEvent) {
            self.events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
        }
    }

    impl RecordingEvents {
        fn selected_wifi_aware(&self) -> bool {
            self.events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .any(|event| {
                    matches!(
                        event,
                        TransferEvent::Connected {
                            path: envoix_types::DataPath::WifiAware
                        }
                    )
                })
        }
    }

    #[tokio::test]
    async fn bootstrap_retries_a_lost_first_datagram() {
        let (client, server) = MemoryDatagramTransport::pair(true);
        let client: Arc<dyn PlatformDatagramTransport> =
            Arc::new(CancellationLingeringDatagramTransport {
                inner: client,
                receive_active: Arc::new(AtomicBool::new(false)),
            });
        let client_id = SecretKey::generate().public();
        let server_id = SecretKey::generate().public();
        let client_cancel = TransferCancelToken::new();
        let server_cancel = TransferCancelToken::new();

        let (client_peer, server_peer) = tokio::join!(
            exchange_endpoint_ids(
                client,
                DatagramTransportRole::Client,
                client_id,
                MIN_QUIC_DATAGRAM_BYTES,
                &client_cancel,
            ),
            exchange_endpoint_ids(
                server,
                DatagramTransportRole::Server,
                server_id,
                MIN_QUIC_DATAGRAM_BYTES,
                &server_cancel,
            ),
        );

        assert_eq!(client_peer.unwrap(), (server_id, MIN_QUIC_DATAGRAM_BYTES));
        assert_eq!(server_peer.unwrap(), (client_id, MIN_QUIC_DATAGRAM_BYTES));
    }

    #[test]
    fn bootstrap_rejects_malformed_envoix_frames() {
        let error = parse_bootstrap(BOOTSTRAP_MAGIC).unwrap_err();
        assert!(matches!(error, CoreError::Protocol(_)));
        assert!(parse_bootstrap(b"unrelated datagram").unwrap().is_none());

        let endpoint_id = SecretKey::generate().public();
        let below_quic_minimum = encode_bootstrap(
            BOOTSTRAP_CLIENT_HELLO,
            endpoint_id,
            None,
            MIN_QUIC_DATAGRAM_BYTES - 1,
        );
        assert!(matches!(
            parse_bootstrap(&below_quic_minimum),
            Err(CoreError::Protocol(_))
        ));
    }

    #[test]
    fn maximum_datagram_size_validation_covers_bounds() {
        assert_eq!(
            validate_maximum_datagram_size(u32::from(MIN_QUIC_DATAGRAM_BYTES)).unwrap(),
            MIN_QUIC_DATAGRAM_BYTES
        );
        assert!(validate_maximum_datagram_size(u32::from(MIN_QUIC_DATAGRAM_BYTES) - 1).is_err());
        assert!(validate_maximum_datagram_size(u32::from(u16::MAX) + 1).is_err());
    }

    #[tokio::test]
    async fn datagram_transport_completes_manifest_v2_delivery() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("wifi-aware-quic.bin");
        let source_bytes = (0..128 * 1024)
            .map(|offset| (offset % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(&source, &source_bytes).await.unwrap();

        let mut job = CanonicalTransferJob::new(CompressionPolicyV2::Never).unwrap();
        job.add_local_path(source).await.unwrap();
        job.prepare_all().await.unwrap();
        job.seal_for_send().unwrap();

        let target = temporary.path().join("received");
        fs::create_dir_all(&target).await.unwrap();
        let destination = DestinationRequestV2 {
            target_directory: target,
            copy_staging_directory: None,
            decision: DestinationDecisionV2::UseDirectSave,
            target_allocatable_bytes: Some(POST_SAVE_RESERVE_BYTES * 4),
            staging_allocatable_bytes: None,
            stable_object_identity: true,
            exceptional_transfer_approved: false,
            preplanned_root_names: None,
        };
        let client_maximum_datagram_size = 1_402;
        let server_maximum_datagram_size = 1_452;
        let (client_transport, server_transport) = MemoryDatagramTransport::pair_with_limits(
            true,
            client_maximum_datagram_size,
            server_maximum_datagram_size,
        );
        let sender_pairing =
            PairingConfig::spake2_shared_token("datagram-manifest-secret").unwrap();
        let receiver_pairing =
            PairingConfig::spake2_shared_token("datagram-manifest-secret").unwrap();
        let sender_events = Arc::new(RecordingEvents::default());
        let receiver_events = Arc::new(RecordingEvents::default());
        let sender_cancel = TransferCancelToken::new();
        let receiver_cancel = TransferCancelToken::new();

        let sender = send_manifest_v2_over_datagram_transport(
            client_transport.clone(),
            u32::from(client_maximum_datagram_size),
            &job,
            temporary.path().join("sender-state"),
            &sender_pairing,
            sender_events.clone(),
            &sender_cancel,
        );
        let receiver = async {
            receive_manifest_v2_offer_over_datagram_transport(
                server_transport.clone(),
                u32::from(server_maximum_datagram_size),
                &receiver_pairing,
                receiver_events.clone(),
                &receiver_cancel,
            )
            .await
            .unwrap()
            .receive(
                destination,
                temporary.path().join("receiver-state"),
                &receiver_cancel,
            )
            .await
        };
        let (sender, receiver) = tokio::join!(sender, receiver);
        let sender = sender.unwrap();
        let receiver = receiver.unwrap();

        assert_eq!(sender.delivery_proof_digest, receiver.delivery_proof_digest);
        let received_path = receiver.destination_plan.target_path_for_root(0).unwrap();
        assert_eq!(fs::read(received_path).await.unwrap(), source_bytes);
        assert!(sender_events.selected_wifi_aware());
        assert!(receiver_events.selected_wifi_aware());
        assert!(client_transport.largest_send() <= usize::from(client_maximum_datagram_size));
        assert!(server_transport.largest_send() <= usize::from(client_maximum_datagram_size));
    }
}
