use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use envoix_protocol::{DecodeError, HEADER_LEN, encoded_frame_len, identifiers::DATA_ALPN};
use iroh::endpoint::{
    BindOpts, Connection, PathEvent, QuicTransportConfig, RecvStream, SendStream, VarInt, presets,
};
use iroh::{Endpoint, EndpointAddr, RelayMap, RelayMode, RelayUrl, TransportAddr};
use n0_future::StreamExt;
use noq_proto::congestion::Bbr3Config;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use zeroize::Zeroizing;

use crate::config::{
    CongestionControl, SessionEndpointConfig, SessionTimeouts, SessionTransportConfig, WaitKind,
};
use crate::error::{SessionError, SessionOperation};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PathObservation {
    Direct { addr: SocketAddr },
    Relay { url: RelayUrl },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseOrdering {
    /// This side sent the final application frame and must let its peer close first.
    AwaitPeer,
    /// This side consumed the final application frame, or is failing the session.
    Active,
}

pub struct ExportedSecret(Zeroizing<[u8; 32]>);

impl ExportedSecret {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub fn into_bytes(self) -> [u8; 32] {
        *self.0
    }
}

impl fmt::Debug for ExportedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExportedSecret([redacted])")
    }
}

#[derive(Clone, Debug)]
pub struct SessionCancellation(watch::Sender<bool>);

impl SessionCancellation {
    pub fn new() -> Self {
        let (sender, _) = watch::channel(false);
        Self(sender)
    }

    pub fn cancel(&self) {
        self.0.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    pub async fn cancelled(&self) {
        let mut receiver = self.0.subscribe();
        while !*receiver.borrow() {
            if receiver.changed().await.is_err() {
                break;
            }
        }
    }
}

impl Default for SessionCancellation {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
pub trait SessionLink: Send {
    async fn send_packet(&mut self, packet: &[u8]) -> Result<(), SessionError>;
    async fn receive_packet(&mut self, maximum_payload: usize) -> Result<Vec<u8>, SessionError>;

    fn export_keying_material(
        &self,
        label: &[u8],
        context: &[u8],
    ) -> Result<ExportedSecret, SessionError>;

    /// Transfers ownership of the session's observation stream to the caller.
    fn take_path_observations(&mut self) -> mpsc::UnboundedReceiver<PathObservation>;

    async fn close(
        &mut self,
        ordering: CloseOrdering,
        timeouts: SessionTimeouts,
    ) -> Result<(), SessionError>;
}

pub struct IrohListener {
    endpoint: Endpoint,
}

impl IrohListener {
    pub async fn bind(
        config: SessionEndpointConfig,
        cancellation: &SessionCancellation,
        timeouts: SessionTimeouts,
    ) -> Result<Self, SessionError> {
        let endpoint = controlled(
            build_endpoint(config, true),
            cancellation,
            timeouts.bind(),
            WaitKind::Bind,
        )
        .await??;
        Ok(Self { endpoint })
    }

    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    /// Accepts one candidate without consuming the listener, allowing a bounded
    /// authentication-failure loop to discard a bad candidate and accept again.
    pub async fn accept_candidate(
        &self,
        cancellation: &SessionCancellation,
        timeouts: SessionTimeouts,
    ) -> Result<IrohSessionLink, SessionError> {
        let incoming = controlled(
            self.endpoint.accept(),
            cancellation,
            timeouts.connect(),
            WaitKind::Connect,
        )
        .await?
        .ok_or(SessionError::PeerClosed)?;
        let connection = controlled(
            async move { incoming.await },
            cancellation,
            timeouts.connect(),
            WaitKind::Connect,
        )
        .await?
        .map_err(|_| SessionError::operation(SessionOperation::Accept))?;
        let (send, recv) = match controlled(
            connection.accept_bi(),
            cancellation,
            timeouts.stream(),
            WaitKind::Stream,
        )
        .await
        {
            Ok(Ok(streams)) => streams,
            Ok(Err(_)) => {
                connection.close(VarInt::from_u32(0), b"stream");
                return Err(SessionError::operation(SessionOperation::OpenStream));
            }
            Err(error) => {
                connection.close(VarInt::from_u32(0), b"stream");
                return Err(error);
            }
        };
        Ok(IrohSessionLink::new(None, connection, send, recv))
    }

    /// Moves the listener endpoint into the authenticated candidate so teardown
    /// closes the one-shot endpoint together with the session.
    pub fn promote(self, mut candidate: IrohSessionLink) -> IrohSessionLink {
        candidate.endpoint = Some(self.endpoint);
        candidate
    }

    pub async fn close(self, timeouts: SessionTimeouts) -> Result<(), SessionError> {
        timeout(timeouts.endpoint_close(), self.endpoint.close())
            .await
            .map_err(|_| SessionError::DeadlineExceeded {
                wait: WaitKind::EndpointClose,
            })
    }
}

pub async fn dial(
    config: SessionEndpointConfig,
    target: EndpointAddr,
    cancellation: &SessionCancellation,
    timeouts: SessionTimeouts,
) -> Result<IrohSessionLink, SessionError> {
    let endpoint = controlled(
        build_endpoint(config, false),
        cancellation,
        timeouts.bind(),
        WaitKind::Bind,
    )
    .await??;
    let connection = match controlled(
        endpoint.connect(target, DATA_ALPN),
        cancellation,
        timeouts.connect(),
        WaitKind::Connect,
    )
    .await
    {
        Ok(Ok(connection)) => connection,
        Ok(Err(_)) => {
            close_endpoint(&endpoint, timeouts).await;
            return Err(SessionError::operation(SessionOperation::Connect));
        }
        Err(error) => {
            close_endpoint(&endpoint, timeouts).await;
            return Err(error);
        }
    };
    let (send, recv) = match controlled(
        connection.open_bi(),
        cancellation,
        timeouts.stream(),
        WaitKind::Stream,
    )
    .await
    {
        Ok(Ok(streams)) => streams,
        Ok(Err(_)) => {
            connection.close(VarInt::from_u32(0), b"stream");
            close_endpoint(&endpoint, timeouts).await;
            return Err(SessionError::operation(SessionOperation::OpenStream));
        }
        Err(error) => {
            connection.close(VarInt::from_u32(0), b"stream");
            close_endpoint(&endpoint, timeouts).await;
            return Err(error);
        }
    };
    Ok(IrohSessionLink::new(Some(endpoint), connection, send, recv))
}

pub struct IrohSessionLink {
    endpoint: Option<Endpoint>,
    connection: Connection,
    send: Option<SendStream>,
    recv: RecvStream,
    path_receiver: Option<mpsc::UnboundedReceiver<PathObservation>>,
    path_watcher: Option<JoinHandle<()>>,
    closed: bool,
}

impl fmt::Debug for IrohSessionLink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IrohSessionLink")
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

impl IrohSessionLink {
    fn new(
        endpoint: Option<Endpoint>,
        connection: Connection,
        send: SendStream,
        recv: RecvStream,
    ) -> Self {
        let (path_sender, path_receiver) = mpsc::unbounded_channel();
        let path_watcher = spawn_path_watcher(connection.clone(), path_sender);
        Self {
            endpoint,
            connection,
            send: Some(send),
            recv,
            path_receiver: Some(path_receiver),
            path_watcher: Some(path_watcher),
            closed: false,
        }
    }

    async fn close_endpoint(&mut self, timeouts: SessionTimeouts) -> Result<(), SessionError> {
        let Some(endpoint) = self.endpoint.take() else {
            return Ok(());
        };
        timeout(timeouts.endpoint_close(), endpoint.close())
            .await
            .map_err(|_| SessionError::DeadlineExceeded {
                wait: WaitKind::EndpointClose,
            })
    }
}

#[async_trait]
impl SessionLink for IrohSessionLink {
    async fn send_packet(&mut self, packet: &[u8]) -> Result<(), SessionError> {
        let send = self
            .send
            .as_mut()
            .ok_or_else(|| SessionError::operation(SessionOperation::WriteFrame))?;
        send.write_all(packet)
            .await
            .map_err(|_| SessionError::operation(SessionOperation::WriteFrame))
    }

    async fn receive_packet(&mut self, maximum_payload: usize) -> Result<Vec<u8>, SessionError> {
        let mut header = [0_u8; HEADER_LEN];
        self.recv
            .read_exact(&mut header)
            .await
            .map_err(|_| SessionError::PeerClosed)?;
        let total = encoded_frame_len(&header).map_err(|error| match error {
            DecodeError::UnsupportedVersion { .. } => SessionError::VersionMismatch,
            _ => SessionError::MalformedEnvelope,
        })?;
        let payload_len = total - HEADER_LEN;
        if payload_len > maximum_payload {
            return Err(SessionError::PayloadTooLarge {
                declared: payload_len,
                maximum: maximum_payload,
            });
        }
        let mut packet = vec![0_u8; total];
        packet[..HEADER_LEN].copy_from_slice(&header);
        self.recv
            .read_exact(&mut packet[HEADER_LEN..])
            .await
            .map_err(|_| SessionError::PeerClosed)?;
        Ok(packet)
    }

    fn export_keying_material(
        &self,
        label: &[u8],
        context: &[u8],
    ) -> Result<ExportedSecret, SessionError> {
        let mut output = [0_u8; 32];
        self.connection
            .export_keying_material(&mut output, label, context)
            .map_err(|_| SessionError::operation(SessionOperation::ExportBinding))?;
        Ok(ExportedSecret::new(output))
    }

    fn take_path_observations(&mut self) -> mpsc::UnboundedReceiver<PathObservation> {
        self.path_receiver.take().unwrap_or_else(|| {
            let (_sender, receiver) = mpsc::unbounded_channel();
            receiver
        })
    }

    async fn close(
        &mut self,
        ordering: CloseOrdering,
        timeouts: SessionTimeouts,
    ) -> Result<(), SessionError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        if let Some(watcher) = self.path_watcher.take() {
            watcher.abort();
            let _ = watcher.await;
        }

        let mut send = self.send.take();
        if let Some(stream) = send.as_mut() {
            let _ = stream.finish();
        }

        match ordering {
            CloseOrdering::AwaitPeer => {
                if timeout(timeouts.peer_close(), self.connection.closed())
                    .await
                    .is_err()
                {
                    self.connection.close(VarInt::from_u32(0), b"done");
                }
            }
            CloseOrdering::Active => {
                if let Some(stream) = send.as_ref() {
                    let _ = timeout(timeouts.peer_close(), stream.stopped()).await;
                }
                self.connection.close(VarInt::from_u32(0), b"done");
            }
        }
        self.close_endpoint(timeouts).await
    }
}

impl Drop for IrohSessionLink {
    fn drop(&mut self) {
        if let Some(watcher) = self.path_watcher.take() {
            watcher.abort();
        }
        if !self.closed {
            self.connection.close(VarInt::from_u32(0), b"dropped");
        }
    }
}

async fn controlled<T>(
    future: impl Future<Output = T>,
    cancellation: &SessionCancellation,
    wait: std::time::Duration,
    kind: WaitKind,
) -> Result<T, SessionError> {
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(SessionError::Cancelled),
        result = timeout(wait, future) => result.map_err(|_| SessionError::DeadlineExceeded { wait: kind }),
    }
}

async fn build_endpoint(
    config: SessionEndpointConfig,
    accept_incoming: bool,
) -> Result<Endpoint, SessionError> {
    let mut builder = Endpoint::builder(presets::Minimal)
        .clear_address_lookup()
        .clear_ip_transports()
        .relay_mode(match config.relay {
            None => RelayMode::Disabled,
            Some(url) => RelayMode::Custom(RelayMap::from(url)),
        })
        .transport_config(data_transport_config(config.transport));
    if accept_incoming {
        builder = builder.alpns(vec![DATA_ALPN.to_vec()]);
    }
    builder = builder
        .bind_addr_with_opts(config.bind.ipv4, BindOpts::default().set_is_required(true))
        .map_err(|_| SessionError::operation(SessionOperation::Bind))?;
    if let Some(ipv6) = config.bind.ipv6 {
        builder = builder
            .bind_addr_with_opts(ipv6, BindOpts::default().set_is_required(false))
            .map_err(|_| SessionError::operation(SessionOperation::Bind))?;
    }
    builder
        .bind()
        .await
        .map_err(|_| SessionError::operation(SessionOperation::Bind))
}

fn data_transport_config(config: SessionTransportConfig) -> QuicTransportConfig {
    let window = config.flow_window.bytes();
    let mut builder = QuicTransportConfig::builder()
        .max_concurrent_bidi_streams(VarInt::from_u32(1))
        .max_concurrent_uni_streams(VarInt::from_u32(0))
        .stream_receive_window(VarInt::from_u32(window))
        .receive_window(VarInt::from_u32(window))
        .send_window(u64::from(window));
    if matches!(config.congestion, CongestionControl::Bbr3) {
        builder = builder.congestion_controller_factory(Arc::new(Bbr3Config::default()));
    }
    builder.build()
}

fn spawn_path_watcher(
    connection: Connection,
    sender: mpsc::UnboundedSender<PathObservation>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut last = selected_path(&connection);
        if let Some(path) = last.clone()
            && sender.send(path).is_err()
        {
            return;
        }
        let mut events = connection.path_events();
        while let Some(event) = events.next().await {
            let selected = match event {
                PathEvent::Selected { remote_addr, .. } => path_observation(&remote_addr),
                PathEvent::Lagged { .. } => selected_path(&connection),
                _ => None,
            };
            if let Some(path) = selected
                && last.as_ref() != Some(&path)
            {
                if sender.send(path.clone()).is_err() {
                    return;
                }
                last = Some(path);
            }
        }
    })
}

fn selected_path(connection: &Connection) -> Option<PathObservation> {
    connection
        .paths()
        .iter()
        .find(|path| path.is_selected())
        .and_then(|path| path_observation(path.remote_addr()))
}

fn path_observation(address: &TransportAddr) -> Option<PathObservation> {
    match address {
        TransportAddr::Ip(addr) => Some(PathObservation::Direct { addr: *addr }),
        TransportAddr::Relay(url) => Some(PathObservation::Relay { url: url.clone() }),
        _ => None,
    }
}

async fn close_endpoint(endpoint: &Endpoint, timeouts: SessionTimeouts) {
    let _ = timeout(timeouts.endpoint_close(), endpoint.close()).await;
}
