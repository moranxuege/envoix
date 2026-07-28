use std::collections::VecDeque;

use async_trait::async_trait;
use envoix_protocol::ProtocolError;
use envoix_protocol::manifest_v2_frames::canonical_manifest_v2_frame_body_digest;
use tempfile::tempdir;

use super::*;
use crate::test_support::{accept, entry_results, offer};

#[derive(Default)]
struct MockConnection {
    sent: Vec<ManifestV2Frame>,
    incoming: VecDeque<ManifestV2Frame>,
}

#[async_trait]
impl ManifestV2FrameConnection for MockConnection {
    async fn send_manifest_v2_frame(
        &mut self,
        frame: ManifestV2Frame,
    ) -> Result<(), ProtocolError> {
        self.sent.push(frame);
        Ok(())
    }

    async fn recv_manifest_v2_frame(&mut self) -> Result<ManifestV2Frame, ProtocolError> {
        Ok(self.incoming.pop_front().expect("test frame"))
    }

    fn export_keying_material(
        &self,
        _label: &[u8],
        _context: &[u8],
    ) -> Result<[u8; 32], ProtocolError> {
        Ok([0x51; 32])
    }

    async fn close(&mut self) -> Result<(), ProtocolError> {
        Ok(())
    }
}

#[tokio::test]
async fn delivery_is_terminal_only_after_receiver_proof_is_verified() {
    let temporary = tempdir().unwrap();
    let offer = offer();
    let accept = accept(&offer);
    let accept_digest =
        canonical_manifest_v2_frame_body_digest(&ManifestV2Frame::Accept(accept.clone())).unwrap();
    let results = entry_results(&offer);
    let completion_digest = ContentDigestV2([0x61; 32]);

    let sender_store = SenderDeliveryStoreV2::new(temporary.path().join("sender"));
    let receiver_store = ReceiverDeliveryStoreV2::new(temporary.path().join("receiver"));
    let mut sender = SenderDeliveryRecordV2::new(&offer);
    sender.commit_accept(&accept, accept_digest).unwrap();
    sender
        .commit_results(&SenderDataPlaneSummaryV2 {
            identity: accept.identity,
            accept_body_digest: accept_digest,
            sender_completion_set_digest: completion_digest,
            entry_results: results.clone(),
        })
        .unwrap();
    assert_eq!(
        sender.phase(),
        SenderTransferPhaseV2::WaitingForReceiverSave
    );

    let mut receiver = ReceiverDeliveryRecordV2::new(
        &offer,
        &accept,
        &ReceiverDataPlaneSummaryV2 {
            identity: accept.identity,
            sender_completion_set_digest: completion_digest,
            entry_results: results,
        },
    )
    .unwrap();
    let mut receiver_connection = MockConnection::default();
    let proof = ManifestV2DeliveryAuthority::receiver_send_proof(
        &mut receiver,
        &receiver_store,
        &mut receiver_connection,
    )
    .await
    .unwrap();
    assert_eq!(
        receiver_connection.sent,
        vec![ManifestV2Frame::DeliveryProof(proof)]
    );

    let mut sender_connection = MockConnection {
        sent: Vec::new(),
        incoming: VecDeque::from([ManifestV2Frame::DeliveryProof(proof)]),
    };
    ManifestV2DeliveryAuthority::sender_confirm_delivery(
        &mut sender,
        &sender_store,
        &mut sender_connection,
    )
    .await
    .unwrap();
    assert_eq!(sender.phase(), SenderTransferPhaseV2::Delivered);
    assert_eq!(
        sender_store
            .load(sender.identity())
            .await
            .unwrap()
            .unwrap()
            .phase(),
        SenderTransferPhaseV2::Delivered
    );
    assert_eq!(
        receiver_store
            .load(sender.identity())
            .await
            .unwrap()
            .unwrap()
            .delivery_proof(),
        Some(proof)
    );
}

#[test]
fn resume_challenge_is_bound_to_identity_nonce_and_capability() {
    let offer = offer();
    let accept = accept(&offer);
    let nonce = [0x71; 32];
    let response = ManifestV2DeliveryAuthority::answer_resume_challenge(
        accept.identity,
        nonce,
        accept.proof_capability,
    );
    ManifestV2DeliveryAuthority::verify_resume_challenge(
        accept.identity,
        nonce,
        response,
        accept.proof_capability,
    )
    .unwrap();
    assert!(matches!(
        ManifestV2DeliveryAuthority::verify_resume_challenge(
            accept.identity,
            [0x72; 32],
            response,
            accept.proof_capability,
        ),
        Err(DeliveryAuthorityErrorV2::InvalidProof)
    ));
    assert!(matches!(
        ManifestV2DeliveryAuthority::verify_resume_challenge(
            accept.identity,
            nonce,
            response,
            ProofCapabilityV2([0x73; 32]),
        ),
        Err(DeliveryAuthorityErrorV2::InvalidProof)
    ));
}

#[test]
fn committed_accept_and_results_cannot_be_rewritten() {
    let offer = offer();
    let accept = accept(&offer);
    let accept_digest =
        canonical_manifest_v2_frame_body_digest(&ManifestV2Frame::Accept(accept.clone())).unwrap();
    let mut sender = SenderDeliveryRecordV2::new(&offer);
    sender.commit_accept(&accept, accept_digest).unwrap();

    let mut changed_accept = accept.clone();
    changed_accept.proof_capability = ProofCapabilityV2([0x81; 32]);
    assert!(matches!(
        sender.commit_accept(&changed_accept, accept_digest),
        Err(DeliveryAuthorityErrorV2::CapabilityMismatch)
    ));

    let summary = SenderDataPlaneSummaryV2 {
        identity: accept.identity,
        accept_body_digest: accept_digest,
        sender_completion_set_digest: ContentDigestV2([0x82; 32]),
        entry_results: entry_results(&offer),
    };
    sender.commit_results(&summary).unwrap();
    let mut changed = summary;
    changed.entry_results[0].final_size += 1;
    assert!(matches!(
        sender.commit_results(&changed),
        Err(DeliveryAuthorityErrorV2::ResultMismatch)
    ));
}

#[test]
fn delivery_debug_output_does_not_expose_proof_capability() {
    let offer = offer();
    let accept = accept(&offer);
    let sender = SenderDeliveryRecordV2::new(&offer);
    let debug = format!("{sender:?} {accept:?}");
    assert!(!debug.contains(&"41".repeat(32)));
    assert!(debug.contains("<redacted>"));
}
