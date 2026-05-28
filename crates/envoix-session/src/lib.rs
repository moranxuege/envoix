//! Session orchestration for transfer setup and concrete implementation wiring.

use std::net::SocketAddr;
use std::path::PathBuf;

use envoix_crypto::InsecureNoopCryptoProvider;
use envoix_error::CoreError;
use envoix_transfer::TransferEngine;
pub use envoix_transfer::{
    DEFAULT_CHUNK_SIZE, EventSink, NoopEventSink, TransferEvent, TransferSummary,
};
use envoix_transport::{ConnectionCandidate, TransportDialer, TransportListener};
use envoix_transport_quic::{QuicDialer, QuicListener};
use envoix_transport_tcp::{TcpIpv6Dialer, TcpIpv6Listener};
pub use envoix_types::TransferDirection;

pub type SessionError = CoreError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionConfig {
    pub chunk_size: usize,
    pub protocol: TransportProtocol,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportProtocol {
    Quic,
    Tcp,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            protocol: TransportProtocol::Quic,
        }
    }
}

pub async fn send_file_manual_ipv6(
    peer_addr: SocketAddr,
    file_path: PathBuf,
    config: SessionConfig,
    events: Box<dyn EventSink>,
) -> Result<TransferSummary, SessionError> {
    let mut connection = match config.protocol {
        TransportProtocol::Quic => {
            let dialer = QuicDialer;
            dialer
                .dial(ConnectionCandidate::Quic { addr: peer_addr })
                .await?
        }
        TransportProtocol::Tcp => {
            let dialer = TcpIpv6Dialer;
            dialer
                .dial(ConnectionCandidate::Tcp { addr: peer_addr })
                .await?
        }
    };
    let engine = TransferEngine::new(InsecureNoopCryptoProvider, config.chunk_size);

    engine
        .send_file(&mut *connection, file_path, events.as_ref())
        .await
}

pub async fn receive_file_ipv6(
    listen_addr: SocketAddr,
    output_dir: PathBuf,
    config: SessionConfig,
    events: Box<dyn EventSink>,
) -> Result<TransferSummary, SessionError> {
    let mut connection = match config.protocol {
        TransportProtocol::Quic => {
            let listener = QuicListener::bind(listen_addr)?;
            listener.accept().await?
        }
        TransportProtocol::Tcp => {
            let listener = TcpIpv6Listener::bind(listen_addr).await?;
            listener.accept().await?
        }
    };
    let engine = TransferEngine::new(InsecureNoopCryptoProvider, config.chunk_size);

    engine
        .receive_file(&mut *connection, output_dir, events.as_ref())
        .await
}
