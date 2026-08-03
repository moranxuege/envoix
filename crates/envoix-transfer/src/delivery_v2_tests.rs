use std::collections::VecDeque;

use async_trait::async_trait;
use envoix_protocol::ProtocolError;
use envoix_protocol::manifest_v2::{
    CompressionPolicyV2, EntryContentDigestV2, ManifestEntryKindV2, build_manifest_offer_v2,
};
use envoix_protocol::manifest_v2_frames::canonical_manifest_v2_frame_body_digest;
use serde_json::json;
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
async fn sender_store_round_trips_frozen_entry_encodings() {
    let temporary = tempdir().unwrap();
    let mut offer = offer();
    offer.manifest.compression_policy = CompressionPolicyV2::Smart;
    offer.manifest.entries[0].component = "REPORT.TXT".into();
    offer.manifest.roots[0].requested_name = "REPORT.TXT".into();
    let offer = build_manifest_offer_v2(offer.manifest).unwrap();
    let record = SenderDeliveryRecordV2::new(&offer);
    assert_eq!(
        record.frozen_entry_encoding(0).unwrap(),
        EntryEncodingV2::Zstd
    );

    let store = SenderDeliveryStoreV2::new(temporary.path());
    store.save(&record).await.unwrap();
    let restored = store.load(record.identity()).await.unwrap().unwrap();
    restored.validate_offer(&offer).unwrap();
    assert_eq!(
        restored.frozen_entry_encoding(0).unwrap(),
        EntryEncodingV2::Zstd
    );
}

#[test]
fn sender_record_rejects_invalid_encoding_length_and_directory_encoding() {
    let mut offer = offer();
    offer.manifest.compression_policy = CompressionPolicyV2::Smart;
    let offer = build_manifest_offer_v2(offer.manifest).unwrap();
    let record = SenderDeliveryRecordV2::new(&offer);

    let mut missing = serde_json::to_value(&record).unwrap();
    missing["entry_encodings"] = json!([]);
    let missing: SenderDeliveryRecordV2 = serde_json::from_value(missing).unwrap();
    assert!(matches!(
        missing.validate_offer(&offer),
        Err(DeliveryAuthorityErrorV2::InvalidRecord)
    ));

    let mut directory_offer = crate::test_support::offer();
    directory_offer.manifest.compression_policy = CompressionPolicyV2::Smart;
    directory_offer.manifest.entries[0].kind = ManifestEntryKindV2::Directory;
    directory_offer.manifest.entries[0].plaintext_size = 0;
    directory_offer.manifest.entries[0].content_digest = EntryContentDigestV2::Deferred;
    directory_offer.manifest.totals.file_count = 0;
    directory_offer.manifest.totals.directory_count = 1;
    directory_offer.manifest.totals.total_plaintext_bytes = 0;
    let directory_offer = build_manifest_offer_v2(directory_offer.manifest).unwrap();
    let directory = SenderDeliveryRecordV2::new(&directory_offer);
    let mut invalid = serde_json::to_value(directory).unwrap();
    invalid["entry_encodings"][0] = json!("Zstd");
    let invalid: SenderDeliveryRecordV2 = serde_json::from_value(invalid).unwrap();
    assert!(matches!(
        invalid.validate_offer(&directory_offer),
        Err(DeliveryAuthorityErrorV2::InvalidRecord)
    ));
}

#[tokio::test]
async fn schema_two_waiting_record_remains_readable_without_encoding_payload() {
    let temporary = tempdir().unwrap();
    let offer = offer();
    let accept = accept(&offer);
    let accept_digest =
        canonical_manifest_v2_frame_body_digest(&ManifestV2Frame::Accept(accept.clone())).unwrap();
    let mut sender = SenderDeliveryRecordV2::new(&offer);
    sender.commit_accept(&accept, accept_digest).unwrap();
    sender
        .commit_results(&SenderDataPlaneSummaryV2 {
            identity: accept.identity,
            accept_body_digest: accept_digest,
            sender_completion_set_digest: ContentDigestV2([0x91; 32]),
            entry_results: entry_results(&offer),
        })
        .unwrap();

    let mut legacy = serde_json::to_value(sender).unwrap();
    legacy["schema_version"] = json!(LEGACY_SENDER_DELIVERY_SCHEMA_VERSION);
    legacy.as_object_mut().unwrap().remove("entry_encodings");
    let legacy: SenderDeliveryRecordV2 = serde_json::from_value(legacy).unwrap();
    legacy.validate_offer(&offer).unwrap();

    let store = SenderDeliveryStoreV2::new(temporary.path());
    store.save(&legacy).await.unwrap();
    let restored = store.load(legacy.identity()).await.unwrap().unwrap();
    assert_eq!(
        restored.phase(),
        SenderTransferPhaseV2::WaitingForReceiverSave
    );
    assert!(restored.requires_entry_encoding_migration());
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
