use std::future::Future;
use std::pin::Pin;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};

use crate::{
    ControlFrame, ControlLimits, IoOperation, RendezvousError, read_control, write_control,
};

pub(crate) type BoxWriter = Box<dyn AsyncWrite + Send + Unpin>;
pub(crate) type BoxReader = Box<dyn AsyncRead + Send + Unpin>;

pub trait CloseWaiter: Send {
    fn wait_closed(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

impl CloseWaiter for () {
    fn wait_closed(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(std::future::ready(()))
    }
}

pub(crate) enum ParkedActivity {
    Data,
    Closed,
}

pub struct PeerConn {
    writer: BoxWriter,
    reader: BufReader<BoxReader>,
    close: Box<dyn CloseWaiter>,
}

impl PeerConn {
    pub fn new(
        writer: impl AsyncWrite + Send + Unpin + 'static,
        reader: impl AsyncRead + Send + Unpin + 'static,
        close: impl CloseWaiter + 'static,
    ) -> Self {
        Self {
            writer: Box::new(writer),
            reader: BufReader::new(Box::new(reader)),
            close: Box::new(close),
        }
    }

    pub(crate) async fn read_control(
        &mut self,
        limits: ControlLimits,
    ) -> Result<ControlFrame, RendezvousError> {
        read_control(&mut self.reader, limits).await
    }

    pub(crate) async fn write_control(
        &mut self,
        frame: &ControlFrame,
        limits: ControlLimits,
    ) -> Result<(), RendezvousError> {
        write_control(&mut self.writer, frame, limits).await
    }

    pub(crate) async fn probe_while_parked(&mut self) -> Result<ParkedActivity, RendezvousError> {
        let closed = self
            .reader
            .fill_buf()
            .await
            .map_err(|_| RendezvousError::Io {
                operation: IoOperation::ReadControl,
            })?
            .is_empty();
        Ok(if closed {
            ParkedActivity::Closed
        } else {
            ParkedActivity::Data
        })
    }

    pub(crate) fn into_parts(self) -> (BoxWriter, BoxReader, Box<dyn CloseWaiter>) {
        (self.writer, Box::new(self.reader), self.close)
    }
}
