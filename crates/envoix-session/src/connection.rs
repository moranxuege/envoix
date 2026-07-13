use std::sync::Arc;
use std::time::Duration;

use envoix_error::CoreError;
use envoix_protocol::{
    Frame, FrameConnection, ProtocolError, flush_frame_writer, read_frame, write_chunk_frame,
    write_frame,
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
    ) -> Self {
        let path_watcher = spawn_path_watcher(connection.clone(), None);
        Self {
            _local_endpoint: local_endpoint,
            connection,
            send,
            recv,
            path_watcher,
        }
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
    /// The last frame of a transfer is the receiver's `CompleteAck`. Closing
    /// right after sending it races our `CONNECTION_CLOSE` against the peer
    /// reading that frame - QUIC may drop still-unread stream data on close, so
    /// the ack is lost and an otherwise-complete transfer looks failed. Letting
    /// the peer (which reads the ack) initiate the close keeps the stream open
    /// long enough for the ack to be delivered. Bounded so a peer that never
    /// closes cannot hang us; if it elapses we close ourselves.
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

    async fn send_chunk(
        &mut self,
        transfer_id: &envoix_types::TransferId,
        index: u64,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), ProtocolError> {
        write_chunk_frame(&mut self.send, transfer_id, index, offset, bytes).await?;
        flush_frame_writer(&mut self.send).await
    }

    async fn send_chunk_or_recv_frame(
        &mut self,
        transfer_id: &envoix_types::TransferId,
        index: u64,
        offset: u64,
        bytes: &[u8],
    ) -> Result<Option<Frame>, ProtocolError> {
        let send = &mut self.send;
        let recv = &mut self.recv;
        tokio::select! {
            biased;
            frame = read_frame(recv) => frame.map(Some),
            result = async {
                write_chunk_frame(send, transfer_id, index, offset, bytes).await?;
                flush_frame_writer(send).await
            } => result.map(|()| None),
        }
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

impl Drop for IrohFrameConnection {
    fn drop(&mut self) {
        // Stop the background path watcher when the connection goes away
        // (clean close or abrupt drop on interrupt).
        self.path_watcher.abort();
    }
}
