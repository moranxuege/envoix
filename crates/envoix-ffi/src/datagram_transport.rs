//! Foreign-owned connected datagram channel used by platform Wi-Fi Aware adapters.

use std::sync::Arc;

use async_trait::async_trait;
use envoix_client::api::{PlatformDatagramTransport, SessionError};

use crate::FfiNativeTransportError;

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiNativeDatagram {
    pub bytes: Vec<u8>,
}

/// Apple implements this interface with a connected Wi-Fi Aware UDP channel.
/// iroh QUIC, authentication, and Manifest v2 remain in Rust.
#[uniffi::export(with_foreign)]
#[async_trait]
pub trait FfiNativeDatagramTransport: Send + Sync {
    async fn send_datagram(&self, bytes: Vec<u8>) -> Result<(), FfiNativeTransportError>;

    async fn receive_datagram(
        &self,
        max_bytes: u32,
    ) -> Result<FfiNativeDatagram, FfiNativeTransportError>;

    async fn shutdown(&self) -> Result<(), FfiNativeTransportError>;
}

pub(crate) fn core_datagram_transport(
    transport: Arc<dyn FfiNativeDatagramTransport>,
) -> Arc<dyn PlatformDatagramTransport> {
    Arc::new(ForeignDatagramTransport { transport })
}

struct ForeignDatagramTransport {
    transport: Arc<dyn FfiNativeDatagramTransport>,
}

#[async_trait]
impl PlatformDatagramTransport for ForeignDatagramTransport {
    async fn send_datagram(&self, bytes: Vec<u8>) -> Result<(), SessionError> {
        self.transport
            .send_datagram(bytes)
            .await
            .map_err(platform_error)
    }

    async fn receive_datagram(&self, max_bytes: u32) -> Result<Vec<u8>, SessionError> {
        let datagram = self
            .transport
            .receive_datagram(max_bytes)
            .await
            .map_err(platform_error)?;
        if datagram.bytes.len() > max_bytes as usize {
            return Err(SessionError::Transport(
                "platform Wi-Fi Aware datagram exceeded its receive bound".into(),
            ));
        }
        Ok(datagram.bytes)
    }

    async fn close(&self) -> Result<(), SessionError> {
        self.transport.shutdown().await.map_err(platform_error)
    }
}

fn platform_error(error: FfiNativeTransportError) -> SessionError {
    SessionError::Transport(format!(
        "platform Wi-Fi Aware datagram transport failed: {error}"
    ))
}
