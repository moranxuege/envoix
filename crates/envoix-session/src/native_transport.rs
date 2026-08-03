//! TLS-protected Manifest v2 framing over a platform-owned byte stream.
//!
//! Apple and Android own Wi-Fi Aware discovery, pairing, network selection,
//! and the raw TCP socket. This module keeps encryption, channel binding,
//! Envoix authentication, and all protocol framing in Rust.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use envoix_error::CoreError;
use envoix_protocol::manifest_v2_frames::{
    ManifestV2Frame, ManifestV2FrameCodecError, ManifestV2FrameConnection,
};
use envoix_protocol::{
    Frame, FrameConnection, ProtocolError, flush_frame_writer, read_frame, read_manifest_v2_frame,
    write_frame, write_manifest_v2_frame,
};
use rcgen::generate_simple_self_signed;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, DigitallySignedStruct, Error as TlsError, ServerConfig, SignatureScheme,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::task::JoinHandle;
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

use crate::TransferCancelToken;

pub const NATIVE_TRANSPORT_IO_CHUNK_BYTES: u32 = 64 * 1024;
const NATIVE_TRANSPORT_BRIDGE_BYTES: usize = 256 * 1024;
const NATIVE_TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const NATIVE_LOCAL_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const NATIVE_PEER_CLOSE_TIMEOUT: Duration = Duration::from_secs(10);
const NATIVE_TLS_SERVER_NAME: &str = "wifi-aware.envoix.invalid";
const NATIVE_TLS_CERTIFICATE_NAME: &str = "Envoix Wi-Fi Aware session";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTransportRole {
    Client,
    Server,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTransportRead {
    pub bytes: Vec<u8>,
    pub end_of_stream: bool,
}

#[async_trait]
pub trait PlatformDuplexTransport: Send + Sync {
    async fn send(&self, bytes: Vec<u8>) -> Result<(), CoreError>;

    async fn receive(&self, max_bytes: u32) -> Result<NativeTransportRead, CoreError>;

    async fn close(&self) -> Result<(), CoreError>;
}

struct NativeTransportBridge {
    transport: Arc<dyn PlatformDuplexTransport>,
    inbound: JoinHandle<()>,
    outbound: JoinHandle<()>,
    failure: Arc<Mutex<Option<String>>>,
}

impl NativeTransportBridge {
    async fn close(self) -> Result<(), CoreError> {
        self.inbound.abort();
        self.outbound.abort();
        tokio::time::timeout(NATIVE_LOCAL_CLOSE_TIMEOUT, self.transport.close())
            .await
            .map_err(|_| CoreError::Transport("native platform close timed out".into()))?
    }

    fn failure(&self) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

pub struct NativeFrameConnection {
    stream: TlsStream<DuplexStream>,
    bridge: Option<NativeTransportBridge>,
}

impl fmt::Debug for NativeFrameConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeFrameConnection")
            .field("transport", &"platform_duplex")
            .finish_non_exhaustive()
    }
}

impl NativeFrameConnection {
    pub async fn connect(
        transport: Arc<dyn PlatformDuplexTransport>,
        role: NativeTransportRole,
        cancel: &TransferCancelToken,
    ) -> Result<Self, CoreError> {
        let (stream, bridge) = bridge_transport(transport);
        let handshake = async {
            match role {
                NativeTransportRole::Client => connect_tls_client(stream).await,
                NativeTransportRole::Server => accept_tls_server(stream).await,
            }
        };
        let result = tokio::select! {
            result = tokio::time::timeout(NATIVE_TLS_HANDSHAKE_TIMEOUT, handshake) => {
                result
                    .map_err(|_| CoreError::Transport("native TLS handshake timed out".into()))?
            }
            () = cancel.cancelled() => Err(CoreError::Cancelled),
        };
        match result {
            Ok(stream) => Ok(Self {
                stream,
                bridge: Some(bridge),
            }),
            Err(error) => {
                let bridge_failure = bridge.failure();
                let _ = bridge.close().await;
                Err(bridge_failure.map(CoreError::Transport).unwrap_or(error))
            }
        }
    }

    fn map_io_error(&self, error: impl fmt::Display) -> CoreError {
        self.bridge
            .as_ref()
            .and_then(NativeTransportBridge::failure)
            .map(CoreError::Transport)
            .unwrap_or_else(|| CoreError::Transport(error.to_string()))
    }

    fn export_tls_keying_material(
        &self,
        label: &[u8],
        context: &[u8],
    ) -> Result<[u8; 32], CoreError> {
        let mut output = [0_u8; 32];
        let result = match &self.stream {
            TlsStream::Client(stream) => {
                stream
                    .get_ref()
                    .1
                    .export_keying_material(&mut output, label, Some(context))
            }
            TlsStream::Server(stream) => {
                stream
                    .get_ref()
                    .1
                    .export_keying_material(&mut output, label, Some(context))
            }
        };
        result.map_err(|_| CoreError::Transport("native TLS exporter failed".into()))?;
        Ok(output)
    }

    async fn close_transport(&mut self) -> Result<(), CoreError> {
        let shutdown_result =
            tokio::time::timeout(NATIVE_LOCAL_CLOSE_TIMEOUT, self.stream.shutdown())
                .await
                .map_err(|_| CoreError::Transport("native TLS shutdown timed out".into()))
                .and_then(|result| result.map_err(|error| self.map_io_error(error)));
        let bridge_result = match self.bridge.take() {
            Some(bridge) => bridge.close().await,
            None => Ok(()),
        };
        shutdown_result.and(bridge_result)
    }

    pub(crate) async fn await_peer_close(&mut self) {
        let _ = tokio::time::timeout(NATIVE_LOCAL_CLOSE_TIMEOUT, self.stream.shutdown()).await;
        let mut byte = [0_u8; 1];
        let _ = tokio::time::timeout(NATIVE_PEER_CLOSE_TIMEOUT, self.stream.read(&mut byte)).await;
        if let Some(bridge) = self.bridge.take() {
            let _ = bridge.close().await;
        }
    }
}

#[async_trait]
impl FrameConnection for NativeFrameConnection {
    async fn send_frame(&mut self, frame: Frame) -> Result<(), ProtocolError> {
        write_frame(&mut self.stream, &frame)
            .await
            .map_err(|error| self.map_io_error(error))?;
        flush_frame_writer(&mut self.stream)
            .await
            .map_err(|error| self.map_io_error(error))
    }

    async fn recv_frame(&mut self) -> Result<Frame, ProtocolError> {
        read_frame(&mut self.stream)
            .await
            .map_err(|error| self.map_io_error(error))
    }

    fn export_keying_material(
        &self,
        label: &[u8],
        context: &[u8],
    ) -> Result<[u8; 32], ProtocolError> {
        self.export_tls_keying_material(label, context)
    }

    async fn close(&mut self) -> Result<(), ProtocolError> {
        self.close_transport().await
    }
}

#[async_trait]
impl ManifestV2FrameConnection for NativeFrameConnection {
    async fn send_manifest_v2_frame(
        &mut self,
        frame: ManifestV2Frame,
    ) -> Result<(), ProtocolError> {
        write_manifest_v2_frame(&mut self.stream, &frame)
            .await
            .map_err(|error| self.map_io_error(manifest_codec_error(error)))
    }

    async fn recv_manifest_v2_frame(&mut self) -> Result<ManifestV2Frame, ProtocolError> {
        read_manifest_v2_frame(&mut self.stream)
            .await
            .map_err(|error| self.map_io_error(manifest_codec_error(error)))
    }

    fn export_keying_material(
        &self,
        label: &[u8],
        context: &[u8],
    ) -> Result<[u8; 32], ProtocolError> {
        self.export_tls_keying_material(label, context)
    }

    async fn close(&mut self) -> Result<(), ProtocolError> {
        self.close_transport().await
    }
}

fn bridge_transport(
    transport: Arc<dyn PlatformDuplexTransport>,
) -> (DuplexStream, NativeTransportBridge) {
    let (tls_stream, platform_stream) = tokio::io::duplex(NATIVE_TRANSPORT_BRIDGE_BYTES);
    let (mut platform_read, mut platform_write) = tokio::io::split(platform_stream);
    let failure = Arc::new(Mutex::new(None));

    let inbound_transport = transport.clone();
    let inbound_failure = failure.clone();
    let inbound = tokio::spawn(async move {
        loop {
            let read = match inbound_transport
                .receive(NATIVE_TRANSPORT_IO_CHUNK_BYTES)
                .await
            {
                Ok(read) => read,
                Err(error) => {
                    record_bridge_failure(&inbound_failure, error);
                    break;
                }
            };
            if read.bytes.len() > NATIVE_TRANSPORT_IO_CHUNK_BYTES as usize {
                record_bridge_failure(
                    &inbound_failure,
                    CoreError::Transport("platform transport exceeded its read bound".into()),
                );
                break;
            }
            if read.bytes.is_empty() && !read.end_of_stream {
                record_bridge_failure(
                    &inbound_failure,
                    CoreError::Transport(
                        "platform transport returned an empty non-EOF read".into(),
                    ),
                );
                break;
            }
            if let Err(error) = platform_write.write_all(&read.bytes).await {
                record_bridge_failure(&inbound_failure, error);
                break;
            }
            if read.end_of_stream {
                let _ = platform_write.shutdown().await;
                break;
            }
        }
    });

    let outbound_transport = transport.clone();
    let outbound_failure = failure.clone();
    let outbound = tokio::spawn(async move {
        let mut buffer = vec![0_u8; NATIVE_TRANSPORT_IO_CHUNK_BYTES as usize];
        loop {
            match platform_read.read(&mut buffer).await {
                Ok(0) => break,
                Ok(count) => {
                    if let Err(error) = outbound_transport.send(buffer[..count].to_vec()).await {
                        record_bridge_failure(&outbound_failure, error);
                        break;
                    }
                }
                Err(error) => {
                    record_bridge_failure(&outbound_failure, error);
                    break;
                }
            }
        }
    });

    (
        tls_stream,
        NativeTransportBridge {
            transport,
            inbound,
            outbound,
            failure,
        },
    )
}

fn record_bridge_failure(failure: &Mutex<Option<String>>, error: impl fmt::Display) {
    let mut failure = failure
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if failure.is_none() {
        *failure = Some(error.to_string());
    }
}

async fn connect_tls_client(stream: DuplexStream) -> Result<TlsStream<DuplexStream>, CoreError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PakeBoundServerVerifier {
        algorithms: provider.clone(),
    });
    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_setup_error)?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    let server_name = ServerName::try_from(NATIVE_TLS_SERVER_NAME)
        .map_err(tls_setup_error)?
        .to_owned();
    TlsConnector::from(Arc::new(config))
        .connect(server_name, stream)
        .await
        .map(TlsStream::from)
        .map_err(tls_io_error)
}

async fn accept_tls_server(stream: DuplexStream) -> Result<TlsStream<DuplexStream>, CoreError> {
    let certified = generate_simple_self_signed([NATIVE_TLS_CERTIFICATE_NAME.to_string()])
        .map_err(tls_setup_error)?;
    let certificate = certified.cert.der().clone();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        certified.signing_key.serialize_der(),
    ));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_setup_error)?
        .with_no_client_auth()
        .with_single_cert(vec![certificate], key)
        .map_err(tls_setup_error)?;
    TlsAcceptor::from(Arc::new(config))
        .accept(stream)
        .await
        .map(TlsStream::from)
        .map_err(tls_io_error)
}

/// The ephemeral TLS certificate supplies encryption, not Envoix identity.
/// Immediately after TLS, mutual SPAKE2 authenticates the explicit invitation
/// token and binds it to this connection's exporter. A MITM can terminate TLS,
/// but cannot complete Envoix authentication because each leg has a different
/// exporter and the attacker does not know the invitation secret.
#[derive(Debug)]
struct PakeBoundServerVerifier {
    algorithms: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PakeBoundServerVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(
            message,
            certificate,
            signature,
            &self.algorithms.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(
            message,
            certificate,
            signature,
            &self.algorithms.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn manifest_codec_error(error: ManifestV2FrameCodecError) -> CoreError {
    match error {
        ManifestV2FrameCodecError::Io(error) => CoreError::Transport(error.to_string()),
        other => CoreError::Protocol(other.to_string()),
    }
}

fn tls_setup_error(error: impl fmt::Display) -> CoreError {
    CoreError::Transport(format!("native TLS setup failed: {error}"))
}

fn tls_io_error(error: impl fmt::Display) -> CoreError {
    CoreError::Transport(format!("native TLS handshake failed: {error}"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use envoix_protocol::manifest_v2::CompressionPolicyV2;
    use envoix_transfer::{
        CanonicalTransferJob, DestinationDecisionV2, DestinationRequestV2, EventSink,
        POST_SAVE_RESERVE_BYTES, TransferEvent, TransferStage,
    };
    use tempfile::tempdir;
    use tokio::fs;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
    use tokio::sync::Mutex as AsyncMutex;

    use super::*;
    use crate::{
        PairingConfig, authenticate_receiver, authenticate_sender,
        receive_manifest_v2_offer_over_native_transport, send_manifest_v2_over_native_transport,
    };

    struct MemoryPlatformTransport {
        reader: AsyncMutex<ReadHalf<DuplexStream>>,
        writer: AsyncMutex<WriteHalf<DuplexStream>>,
        fragment_bytes: usize,
    }

    impl MemoryPlatformTransport {
        fn pair(fragment_bytes: usize) -> (Arc<Self>, Arc<Self>) {
            let (left, right) = tokio::io::duplex(NATIVE_TRANSPORT_BRIDGE_BYTES);
            let (left_reader, left_writer) = tokio::io::split(left);
            let (right_reader, right_writer) = tokio::io::split(right);
            (
                Arc::new(Self {
                    reader: AsyncMutex::new(left_reader),
                    writer: AsyncMutex::new(left_writer),
                    fragment_bytes,
                }),
                Arc::new(Self {
                    reader: AsyncMutex::new(right_reader),
                    writer: AsyncMutex::new(right_writer),
                    fragment_bytes,
                }),
            )
        }
    }

    #[async_trait]
    impl PlatformDuplexTransport for MemoryPlatformTransport {
        async fn send(&self, bytes: Vec<u8>) -> Result<(), CoreError> {
            let mut writer = self.writer.lock().await;
            for fragment in bytes.chunks(self.fragment_bytes) {
                writer
                    .write_all(fragment)
                    .await
                    .map_err(|error| CoreError::Transport(error.to_string()))?;
            }
            Ok(())
        }

        async fn receive(&self, max_bytes: u32) -> Result<NativeTransportRead, CoreError> {
            let length = usize::try_from(max_bytes)
                .unwrap_or(usize::MAX)
                .min(self.fragment_bytes);
            let mut bytes = vec![0_u8; length];
            let count = self
                .reader
                .lock()
                .await
                .read(&mut bytes)
                .await
                .map_err(|error| CoreError::Transport(error.to_string()))?;
            bytes.truncate(count);
            Ok(NativeTransportRead {
                bytes,
                end_of_stream: count == 0,
            })
        }

        async fn close(&self) -> Result<(), CoreError> {
            self.writer
                .lock()
                .await
                .shutdown()
                .await
                .map_err(|error| CoreError::Transport(error.to_string()))
        }
    }

    struct PendingPlatformTransport {
        closed: AtomicBool,
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

        fn stage_timings(&self) -> Vec<(TransferStage, u64, u64, u64)> {
            self.events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .filter_map(|event| match event {
                    TransferEvent::StageTiming {
                        attempt_id,
                        stage,
                        elapsed_us,
                        delta_us,
                        ..
                    } => Some((*stage, *attempt_id, *elapsed_us, *delta_us)),
                    _ => None,
                })
                .collect()
        }
    }

    #[async_trait]
    impl PlatformDuplexTransport for PendingPlatformTransport {
        async fn send(&self, _bytes: Vec<u8>) -> Result<(), CoreError> {
            std::future::pending().await
        }

        async fn receive(&self, _max_bytes: u32) -> Result<NativeTransportRead, CoreError> {
            std::future::pending().await
        }

        async fn close(&self) -> Result<(), CoreError> {
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn fragmented_transport_completes_tls_exporter_and_spake2() {
        let (client_transport, server_transport) = MemoryPlatformTransport::pair(7);
        let client_cancel = TransferCancelToken::new();
        let server_cancel = TransferCancelToken::new();
        let (client, server) = tokio::join!(
            NativeFrameConnection::connect(
                client_transport,
                NativeTransportRole::Client,
                &client_cancel,
            ),
            NativeFrameConnection::connect(
                server_transport,
                NativeTransportRole::Server,
                &server_cancel,
            ),
        );
        let mut client = client.unwrap();
        let mut server = server.unwrap();
        let label = b"envoix-native-test";
        let context = b"wifi-aware";
        assert_eq!(
            FrameConnection::export_keying_material(&client, label, context).unwrap(),
            FrameConnection::export_keying_material(&server, label, context).unwrap(),
        );

        let sender_pairing = PairingConfig::spake2_shared_token("native-transport-secret").unwrap();
        let receiver_pairing =
            PairingConfig::spake2_shared_token("native-transport-secret").unwrap();
        let (sender, receiver) = tokio::join!(
            authenticate_sender(&mut client, &sender_pairing),
            authenticate_receiver(&mut server, &receiver_pairing),
        );
        sender.unwrap();
        receiver.unwrap();

        let _ = FrameConnection::close(&mut client).await;
        let _ = FrameConnection::close(&mut server).await;
    }

    #[tokio::test]
    async fn mismatched_pairing_tokens_cannot_authenticate_native_tls() {
        let (client_transport, server_transport) = MemoryPlatformTransport::pair(11);
        let client_cancel = TransferCancelToken::new();
        let server_cancel = TransferCancelToken::new();
        let (client, server) = tokio::join!(
            NativeFrameConnection::connect(
                client_transport,
                NativeTransportRole::Client,
                &client_cancel,
            ),
            NativeFrameConnection::connect(
                server_transport,
                NativeTransportRole::Server,
                &server_cancel,
            ),
        );
        let mut client = client.unwrap();
        let mut server = server.unwrap();
        let sender_pairing = PairingConfig::spake2_shared_token("sender-secret").unwrap();
        let receiver_pairing = PairingConfig::spake2_shared_token("receiver-secret").unwrap();

        let authentication_error = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                result = authenticate_sender(&mut client, &sender_pairing) => result,
                result = authenticate_receiver(&mut server, &receiver_pairing) => result,
            }
        })
        .await
        .expect("mismatched PAKE must terminate");

        assert!(authentication_error.is_err());
        let _ = FrameConnection::close(&mut client).await;
        let _ = FrameConnection::close(&mut server).await;
    }

    #[tokio::test]
    async fn cancellation_closes_a_pending_platform_transport() {
        let transport = Arc::new(PendingPlatformTransport {
            closed: AtomicBool::new(false),
        });
        let cancel = TransferCancelToken::new();
        cancel.cancel();

        let error =
            NativeFrameConnection::connect(transport.clone(), NativeTransportRole::Client, &cancel)
                .await
                .unwrap_err();

        assert!(matches!(error, CoreError::Cancelled));
        assert!(transport.closed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn unsealed_job_is_rejected_before_opening_native_transport() {
        let transport = Arc::new(PendingPlatformTransport {
            closed: AtomicBool::new(false),
        });
        let job = CanonicalTransferJob::new(CompressionPolicyV2::Never).unwrap();
        let pairing = PairingConfig::spake2_shared_token("native-manifest-secret").unwrap();
        let temporary = tempdir().unwrap();

        let error = send_manifest_v2_over_native_transport(
            transport.clone(),
            &job,
            temporary.path().join("sender-state"),
            &pairing,
            Arc::new(RecordingEvents::default()),
            &TransferCancelToken::new(),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, CoreError::InvalidInput(_)));
        assert!(!transport.closed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn native_transport_completes_manifest_v2_delivery() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("wifi-aware.bin");
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
        let (client_transport, server_transport) = MemoryPlatformTransport::pair(17);
        let sender_pairing = PairingConfig::spake2_shared_token("native-manifest-secret").unwrap();
        let receiver_pairing =
            PairingConfig::spake2_shared_token("native-manifest-secret").unwrap();
        let sender_events = Arc::new(RecordingEvents::default());
        let receiver_events = Arc::new(RecordingEvents::default());
        let sender_cancel = TransferCancelToken::new();
        let receiver_cancel = TransferCancelToken::new();

        let sender = send_manifest_v2_over_native_transport(
            client_transport,
            &job,
            temporary.path().join("sender-state"),
            &sender_pairing,
            sender_events.clone(),
            &sender_cancel,
        );
        let receiver = async {
            receive_manifest_v2_offer_over_native_transport(
                server_transport,
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
        let expected_stages = [
            TransferStage::SessionStarted,
            TransferStage::ConnectionReady,
            TransferStage::AuthenticationStarted,
            TransferStage::AuthenticationComplete,
            TransferStage::ManifestOffer,
            TransferStage::ManifestAccepted,
            TransferStage::FirstPayload,
            TransferStage::PayloadComplete,
            TransferStage::DeliveryComplete,
        ];
        for timings in [
            sender_events.stage_timings(),
            receiver_events.stage_timings(),
        ] {
            assert_eq!(
                timings.iter().map(|(stage, ..)| *stage).collect::<Vec<_>>(),
                expected_stages
            );
            let attempt_id = timings[0].1;
            assert!(timings.iter().all(|timing| timing.1 == attempt_id));
            assert!(timings.windows(2).all(|pair| pair[0].2 <= pair[1].2));
            assert!(
                timings
                    .windows(2)
                    .all(|pair| pair[1].3 == pair[1].2.saturating_sub(pair[0].2))
            );
        }
    }

    #[tokio::test]
    async fn canceling_pending_native_offer_emits_one_canceled_terminal_stage() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("pending-cancel.bin");
        fs::write(&source, b"pending offer cancellation")
            .await
            .unwrap();

        let mut job = CanonicalTransferJob::new(CompressionPolicyV2::Never).unwrap();
        job.add_local_path(source).await.unwrap();
        job.prepare_all().await.unwrap();
        job.seal_for_send().unwrap();

        let (client_transport, server_transport) = MemoryPlatformTransport::pair(13);
        let sender_pairing = PairingConfig::spake2_shared_token("pending-cancel-secret").unwrap();
        let receiver_pairing = PairingConfig::spake2_shared_token("pending-cancel-secret").unwrap();
        let sender_events = Arc::new(RecordingEvents::default());
        let receiver_events = Arc::new(RecordingEvents::default());
        let sender_cancel = TransferCancelToken::new();
        let receiver_cancel = TransferCancelToken::new();

        let sender = send_manifest_v2_over_native_transport(
            client_transport,
            &job,
            temporary.path().join("sender-state"),
            &sender_pairing,
            sender_events,
            &sender_cancel,
        );
        let receiver = async {
            let pending = receive_manifest_v2_offer_over_native_transport(
                server_transport,
                &receiver_pairing,
                receiver_events.clone(),
                &receiver_cancel,
            )
            .await
            .unwrap();
            let close = pending.cancel();
            assert_eq!(
                receiver_events
                    .stage_timings()
                    .last()
                    .map(|(stage, ..)| *stage),
                Some(TransferStage::Canceled)
            );
            close.await;
        };
        let (sender, ()) = tokio::join!(sender, receiver);

        assert!(sender.is_err());
        let stages = receiver_events
            .stage_timings()
            .into_iter()
            .map(|(stage, ..)| stage)
            .collect::<Vec<_>>();
        assert_eq!(
            stages,
            vec![
                TransferStage::SessionStarted,
                TransferStage::ConnectionReady,
                TransferStage::AuthenticationStarted,
                TransferStage::AuthenticationComplete,
                TransferStage::ManifestOffer,
                TransferStage::Canceled,
            ]
        );
    }

    #[tokio::test]
    async fn failing_pending_native_offer_emits_one_failed_terminal_stage() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("pending-failure.bin");
        fs::write(&source, b"pending offer setup failure")
            .await
            .unwrap();

        let mut job = CanonicalTransferJob::new(CompressionPolicyV2::Never).unwrap();
        job.add_local_path(source).await.unwrap();
        job.prepare_all().await.unwrap();
        job.seal_for_send().unwrap();

        let (client_transport, server_transport) = MemoryPlatformTransport::pair(13);
        let sender_pairing = PairingConfig::spake2_shared_token("pending-failure-secret").unwrap();
        let receiver_pairing =
            PairingConfig::spake2_shared_token("pending-failure-secret").unwrap();
        let sender_events = Arc::new(RecordingEvents::default());
        let receiver_events = Arc::new(RecordingEvents::default());
        let sender_cancel = TransferCancelToken::new();
        let receiver_cancel = TransferCancelToken::new();

        let sender = send_manifest_v2_over_native_transport(
            client_transport,
            &job,
            temporary.path().join("sender-state"),
            &sender_pairing,
            sender_events,
            &sender_cancel,
        );
        let receiver = async {
            let pending = receive_manifest_v2_offer_over_native_transport(
                server_transport,
                &receiver_pairing,
                receiver_events.clone(),
                &receiver_cancel,
            )
            .await
            .unwrap();
            let close = pending.close_with_failure();
            assert_eq!(
                receiver_events
                    .stage_timings()
                    .last()
                    .map(|(stage, ..)| *stage),
                Some(TransferStage::Failed)
            );
            close.await;
        };
        let (sender, ()) = tokio::join!(sender, receiver);

        assert!(sender.is_err());
        let stages = receiver_events
            .stage_timings()
            .into_iter()
            .map(|(stage, ..)| stage)
            .collect::<Vec<_>>();
        assert_eq!(
            stages,
            vec![
                TransferStage::SessionStarted,
                TransferStage::ConnectionReady,
                TransferStage::AuthenticationStarted,
                TransferStage::AuthenticationComplete,
                TransferStage::ManifestOffer,
                TransferStage::Failed,
            ]
        );
    }

    #[tokio::test]
    async fn sender_local_delivery_store_failure_is_terminal() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("sender-preparation.bin");
        fs::write(&source, b"sender local setup failure")
            .await
            .unwrap();
        let invalid_state_directory = temporary.path().join("state-is-a-file");
        fs::write(&invalid_state_directory, b"not a directory")
            .await
            .unwrap();

        let mut job = CanonicalTransferJob::new(CompressionPolicyV2::Never).unwrap();
        job.add_local_path(source).await.unwrap();
        job.prepare_all().await.unwrap();
        job.seal_for_send().unwrap();

        let (client_transport, server_transport) = MemoryPlatformTransport::pair(13);
        let sender_pairing =
            PairingConfig::spake2_shared_token("sender-preparation-secret").unwrap();
        let receiver_pairing =
            PairingConfig::spake2_shared_token("sender-preparation-secret").unwrap();
        let sender_events = Arc::new(RecordingEvents::default());
        let receiver_events = Arc::new(RecordingEvents::default());
        let sender_cancel = TransferCancelToken::new();
        let receiver_cancel = TransferCancelToken::new();

        let sender = send_manifest_v2_over_native_transport(
            client_transport,
            &job,
            invalid_state_directory,
            &sender_pairing,
            sender_events.clone(),
            &sender_cancel,
        );
        let receiver = receive_manifest_v2_offer_over_native_transport(
            server_transport,
            &receiver_pairing,
            receiver_events,
            &receiver_cancel,
        );
        let (sender, receiver) = tokio::join!(sender, receiver);

        assert!(sender.is_err());
        assert!(receiver.is_err());
        assert_eq!(
            sender_events
                .stage_timings()
                .into_iter()
                .map(|(stage, ..)| stage)
                .collect::<Vec<_>>(),
            vec![
                TransferStage::SessionStarted,
                TransferStage::ConnectionReady,
                TransferStage::Failed,
            ]
        );
    }
}
