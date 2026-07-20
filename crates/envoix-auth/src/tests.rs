use super::*;
use async_trait::async_trait;
use envoix_protocol::{Chunk, Frame, ProtocolError, Ready};
use envoix_types::TransferId;
use tokio::sync::mpsc;

const TOKEN: &str = "abcdefghijkl";

#[test]
fn accepts_ascii_token_at_minimum_length() {
    let config = PairingConfig::spake2_shared_token(TOKEN).unwrap();

    assert_eq!(
        config,
        PairingConfig::Spake2SharedToken {
            token: TOKEN.into()
        }
    );
}

#[test]
fn rejects_short_token() {
    let error = PairingConfig::spake2_shared_token("short").unwrap_err();

    assert!(matches!(error, CoreError::InvalidInput(_)));
}

#[test]
fn rejects_non_ascii_token() {
    let error = PairingConfig::spake2_shared_token("abcdefghijklé").unwrap_err();

    assert!(matches!(error, CoreError::InvalidInput(_)));
}

#[tokio::test]
async fn matching_tokens_complete_auth() {
    let (mut sender, mut receiver) = memory_connection_pair([7_u8; 32], [7_u8; 32]);
    let sender_config = PairingConfig::spake2_shared_token(TOKEN).unwrap();
    let receiver_config = PairingConfig::spake2_shared_token(TOKEN).unwrap();

    let receiver_task =
        tokio::spawn(
            async move { authenticate_receiver(&mut receiver, &receiver_config).await },
        );

    authenticate_sender(&mut sender, &sender_config)
        .await
        .unwrap();
    receiver_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn mismatched_tokens_fail_confirmation() {
    let (mut sender, mut receiver) = memory_connection_pair([7_u8; 32], [7_u8; 32]);
    let sender_config = PairingConfig::spake2_shared_token(TOKEN).unwrap();
    let receiver_config = PairingConfig::spake2_shared_token("mnopqrstuvwx").unwrap();

    let receiver_task =
        tokio::spawn(
            async move { authenticate_receiver(&mut receiver, &receiver_config).await },
        );

    let sender_result = authenticate_sender(&mut sender, &sender_config).await;
    let receiver_result = receiver_task.await.unwrap();

    assert!(sender_result.is_err() || receiver_result.is_err());
}

#[tokio::test]
async fn different_channel_bindings_fail_confirmation() {
    let (mut sender, mut receiver) = memory_connection_pair([1_u8; 32], [2_u8; 32]);
    let sender_config = PairingConfig::spake2_shared_token(TOKEN).unwrap();
    let receiver_config = PairingConfig::spake2_shared_token(TOKEN).unwrap();

    let receiver_task =
        tokio::spawn(
            async move { authenticate_receiver(&mut receiver, &receiver_config).await },
        );

    let sender_result = authenticate_sender(&mut sender, &sender_config).await;
    let receiver_result = receiver_task.await.unwrap();

    assert!(sender_result.is_err() || receiver_result.is_err());
}

#[test]
fn confirmation_proofs_are_role_separated() {
    let transcript = ConfirmationTranscript {
        sender_nonce: &[1_u8; NONCE_LEN],
        receiver_nonce: &[2_u8; NONCE_LEN],
        sender_message: b"sender message",
        receiver_message: b"receiver message",
        exporter: &[3_u8; 32],
    };
    let key = b"shared key";

    let sender_proof = confirmation_proof(key, &transcript, SENDER_CONFIRM_LABEL);
    let receiver_proof = confirmation_proof(key, &transcript, RECEIVER_CONFIRM_LABEL);

    assert_ne!(sender_proof, receiver_proof);
}

struct MemoryFrameConnection {
    tx: mpsc::Sender<Frame>,
    rx: mpsc::Receiver<Frame>,
    exporter: [u8; 32],
}

fn memory_connection_pair(
    sender_exporter: [u8; 32],
    receiver_exporter: [u8; 32],
) -> (MemoryFrameConnection, MemoryFrameConnection) {
    let (sender_tx, receiver_rx) = mpsc::channel(16);
    let (receiver_tx, sender_rx) = mpsc::channel(16);

    (
        MemoryFrameConnection {
            tx: sender_tx,
            rx: sender_rx,
            exporter: sender_exporter,
        },
        MemoryFrameConnection {
            tx: receiver_tx,
            rx: receiver_rx,
            exporter: receiver_exporter,
        },
    )
}

#[async_trait]
impl FrameConnection for MemoryFrameConnection {
    async fn send_frame(&mut self, frame: Frame) -> Result<(), ProtocolError> {
        self.tx
            .send(frame)
            .await
            .map_err(|error| CoreError::Transport(error.to_string()))
    }

    async fn send_chunk(
        &mut self,
        transfer_id: &TransferId,
        index: u64,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), ProtocolError> {
        self.send_frame(Frame::Chunk(Chunk {
            transfer_id: transfer_id.clone(),
            index,
            offset,
            bytes: bytes.to_vec(),
        }))
        .await
    }

    async fn recv_frame(&mut self) -> Result<Frame, ProtocolError> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| CoreError::Transport("memory connection closed".into()))
    }

    fn export_keying_material(
        &self,
        _label: &[u8],
        _context: &[u8],
    ) -> Result<[u8; 32], ProtocolError> {
        Ok(self.exporter)
    }

    async fn close(&mut self) -> Result<(), ProtocolError> {
        Ok(())
    }
}

#[tokio::test]
async fn default_channel_binding_failure_rejects_auth() {
    struct NoBindingConnection;

    #[async_trait]
    impl FrameConnection for NoBindingConnection {
        async fn send_frame(&mut self, _frame: Frame) -> Result<(), ProtocolError> {
            Ok(())
        }

        async fn send_chunk(
            &mut self,
            _transfer_id: &TransferId,
            _index: u64,
            _offset: u64,
            _bytes: &[u8],
        ) -> Result<(), ProtocolError> {
            Ok(())
        }

        async fn recv_frame(&mut self) -> Result<Frame, ProtocolError> {
            Ok(Frame::Ready(Ready))
        }

        async fn close(&mut self) -> Result<(), ProtocolError> {
            Ok(())
        }
    }

    let mut connection = NoBindingConnection;
    let config = PairingConfig::spake2_shared_token(TOKEN).unwrap();
    let error = authenticate_sender(&mut connection, &config)
        .await
        .unwrap_err();

    assert!(matches!(error, CoreError::Transport(_)));
}
