use std::sync::Arc;
use std::time::Duration;

use envoix_error::CoreError;
use envoix_protocol::manifest_v2_frames::{ManifestV2Frame, ManifestV2FrameConnection};
use envoix_protocol::{
    Frame, FrameConnection, ProtocolError, TransferProtocol, flush_frame_writer, read_frame,
    read_manifest_v2_frame, write_frame, write_manifest_v2_frame,
};
use envoix_transfer::{EventSink, TransferEvent};
use envoix_types::DataPath;
use iroh::endpoint::{Connection, RecvStream, SendStream, VarInt};
use iroh::{Endpoint, TransportAddr};
use tokio::task::JoinHandle;

const STREAM_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
/// Cap on how long the side that sent the final frame waits for the peer to
/// close before closing itself, so a peer that never closes cannot hang us.
const PEER_CLOSE_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the background watcher samples the selected data path.
const PATH_WATCH_INTERVAL: Duration = Duration::from_millis(500);

pub(crate) struct IrohFrameConnection {
    pub(crate) _local_endpoint: Endpoint,
    pub(crate) connection: Connection,
    pub(crate) send: SendStream,
    pub(crate) recv: RecvStream,
    protocol: TransferProtocol,
    /// Watches the selected data path and reports it as soon as one is selected
    /// and again on every change, so the path is visible *during* the transfer
    /// rather than only at the end. Aborted when the connection is dropped.
    path_watcher: JoinHandle<()>,
}

/// The currently selected data path, or `None` if none is selected yet (still
/// establishing) or the connection is closing.
fn selected_path(connection: &Connection) -> Option<DataPath> {
    for path in connection.paths().iter() {
        if path.is_selected() {
            return Some(match path.remote_addr() {
                TransportAddr::Ip(addr) => DataPath::Direct { addr: *addr },
                TransportAddr::Relay(url) => DataPath::Relay {
                    url: url.to_string(),
                },
                other => DataPath::Other {
                    description: format!("{other:?}"),
                },
            });
        }
    }
    None
}

/// Spawn a task that reports the selected data path on first selection
/// (`Connected`) and on every change (`PathChanged`, e.g. a relay -> direct
/// upgrade after hole-punching), through `events` when given, and always at
/// debug level for library diagnostics.
fn spawn_path_watcher(
    connection: Connection,
    events: Option<Arc<dyn EventSink>>,
) -> JoinHandle<()> {
    // Inherit the caller's transfer span so the data-path lines correlate by
    // room / transfer_id like the rest of the transfer's logs.
    use tracing::Instrument as _;
    tokio::spawn(
        async move {
            let mut last: Option<DataPath> = None;
            loop {
                if let Some(path) = selected_path(&connection)
                    && last.as_ref() != Some(&path)
                {
                    tracing::debug!(target: "envoix", "data path: {path}");
                    if let Some(events) = &events {
                        events.on_event(match last {
                            None => TransferEvent::Connected { path: path.clone() },
                            Some(_) => TransferEvent::PathChanged { path: path.clone() },
                        });
                    }
                    last = Some(path);
                }
                tokio::time::sleep(PATH_WATCH_INTERVAL).await;
            }
        }
        .instrument(tracing::Span::current()),
    )
}

impl IrohFrameConnection {
    /// Wrap an established iroh connection + bidirectional stream, starting the
    /// background data-path watcher for its lifetime.
    pub(crate) fn new(
        local_endpoint: Endpoint,
        connection: Connection,
        send: SendStream,
        recv: RecvStream,
        protocol: TransferProtocol,
    ) -> Self {
        let path_watcher = spawn_path_watcher(connection.clone(), None);
        Self {
            _local_endpoint: local_endpoint,
            connection,
            send,
            recv,
            protocol,
            path_watcher,
        }
    }

    /// Returns the protocol selected by the QUIC/TLS ALPN handshake.
    pub(crate) fn protocol(&self) -> TransferProtocol {
        self.protocol
    }

    /// Restart the path watcher with an event sink, so path selection and
    /// changes surface as `Connected` / `PathChanged` events on the transfer's
    /// stream instead of only debug logs.
    pub(crate) fn watch_path(&mut self, events: Arc<dyn EventSink>) {
        self.path_watcher.abort();
        self.path_watcher = spawn_path_watcher(self.connection.clone(), Some(events));
    }

    /// Close as the side that sent the *final* frame: finish our stream, then
    /// wait (bounded) for the peer to close the connection instead of closing
    /// first.
    ///
    /// A receiver sends DeliveryProof last. It leaves the stream open until the
    /// sender validates, persists Delivered, and closes. The bound prevents a
    /// peer that never closes from pinning the receiver indefinitely.
    pub(crate) async fn await_peer_close(&mut self) {
        let _ = self.send.finish();
        if tokio::time::timeout(PEER_CLOSE_TIMEOUT, self.connection.closed())
            .await
            .is_err()
        {
            self.connection.close(VarInt::from_u32(0), b"done");
        }
    }
}

#[async_trait::async_trait]
impl FrameConnection for IrohFrameConnection {
    async fn send_frame(&mut self, frame: Frame) -> Result<(), ProtocolError> {
        write_frame(&mut self.send, &frame).await?;
        flush_frame_writer(&mut self.send).await
    }

    async fn recv_frame(&mut self) -> Result<Frame, ProtocolError> {
        read_frame(&mut self.recv).await
    }

    fn export_keying_material(
        &self,
        label: &[u8],
        context: &[u8],
    ) -> Result<[u8; 32], ProtocolError> {
        let mut output = [0_u8; 32];
        self.connection
            .export_keying_material(&mut output, label, context)
            .map_err(|_| CoreError::Transport("failed to export iroh keying material".into()))?;
        Ok(output)
    }

    async fn close(&mut self) -> Result<(), ProtocolError> {
        if self.send.finish().is_ok() {
            let _ = tokio::time::timeout(STREAM_CLOSE_TIMEOUT, self.send.stopped()).await;
        }
        self.connection.close(VarInt::from_u32(0), b"done");
        Ok(())
    }
}

#[async_trait::async_trait]
impl ManifestV2FrameConnection for IrohFrameConnection {
    async fn send_manifest_v2_frame(
        &mut self,
        frame: ManifestV2Frame,
    ) -> Result<(), ProtocolError> {
        write_manifest_v2_frame(&mut self.send, &frame).await?;
        Ok(())
    }

    async fn recv_manifest_v2_frame(&mut self) -> Result<ManifestV2Frame, ProtocolError> {
        read_manifest_v2_frame(&mut self.recv)
            .await
            .map_err(Into::into)
    }

    fn export_keying_material(
        &self,
        label: &[u8],
        context: &[u8],
    ) -> Result<[u8; 32], ProtocolError> {
        let mut output = [0_u8; 32];
        self.connection
            .export_keying_material(&mut output, label, context)
            .map_err(|_| CoreError::Transport("failed to export iroh keying material".into()))?;
        Ok(output)
    }

    async fn close(&mut self) -> Result<(), ProtocolError> {
        if self.send.finish().is_ok() {
            let _ = tokio::time::timeout(STREAM_CLOSE_TIMEOUT, self.send.stopped()).await;
        }
        self.connection.close(VarInt::from_u32(0), b"done");
        Ok(())
    }
}

impl Drop for IrohFrameConnection {
    fn drop(&mut self) {
        // Stop the background path watcher when the connection goes away
        // (clean close or abrupt drop on interrupt).
        self.path_watcher.abort();
    }
}
