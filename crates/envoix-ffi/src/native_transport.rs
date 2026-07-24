//! Foreign-owned duplex byte stream used by platform Wi-Fi Aware adapters.

use std::sync::Arc;

use async_trait::async_trait;
use envoix_client::api::{NativeTransportRead, PlatformDuplexTransport, SessionError};

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiNativeTransportRead {
    pub bytes: Vec<u8>,
    pub end_of_stream: bool,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiNativeTransportError {
    #[error("{reason}")]
    Operation { reason: String },
}

/// Apple and Android implement this interface with their Wi-Fi Aware TCP
/// socket. Encryption, authentication, and framing remain in Rust.
#[uniffi::export(with_foreign)]
#[async_trait]
pub trait FfiNativeDuplexTransport: Send + Sync {
    async fn send(&self, bytes: Vec<u8>) -> Result<(), FfiNativeTransportError>;

    async fn receive(
        &self,
        max_bytes: u32,
    ) -> Result<FfiNativeTransportRead, FfiNativeTransportError>;

    async fn close(&self) -> Result<(), FfiNativeTransportError>;
}

pub(crate) fn core_native_transport(
    transport: Arc<dyn FfiNativeDuplexTransport>,
) -> Arc<dyn PlatformDuplexTransport> {
    Arc::new(ForeignNativeTransport { transport })
}

struct ForeignNativeTransport {
    transport: Arc<dyn FfiNativeDuplexTransport>,
}

#[async_trait]
impl PlatformDuplexTransport for ForeignNativeTransport {
    async fn send(&self, bytes: Vec<u8>) -> Result<(), SessionError> {
        self.transport.send(bytes).await.map_err(platform_error)
    }

    async fn receive(&self, max_bytes: u32) -> Result<NativeTransportRead, SessionError> {
        let read = self
            .transport
            .receive(max_bytes)
            .await
            .map_err(platform_error)?;
        if read.bytes.len() > max_bytes as usize {
            return Err(SessionError::Transport(
                "platform Wi-Fi Aware transport exceeded its read bound".into(),
            ));
        }
        Ok(NativeTransportRead {
            bytes: read.bytes,
            end_of_stream: read.end_of_stream,
        })
    }

    async fn close(&self) -> Result<(), SessionError> {
        self.transport.close().await.map_err(platform_error)
    }
}

fn platform_error(error: FfiNativeTransportError) -> SessionError {
    SessionError::Transport(format!("platform Wi-Fi Aware transport failed: {error}"))
}
