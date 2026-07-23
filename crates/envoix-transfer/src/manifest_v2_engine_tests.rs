use std::sync::Mutex;

use async_trait::async_trait;
use envoix_protocol::ProtocolError;
use envoix_protocol::manifest_v2::{CompressionPolicyV2, build_manifest_offer_v2};
use tempfile::tempdir;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::*;
use crate::test_support::{accept, offer};
use crate::{
    DestinationDecisionV2, DestinationRequestV2, DestinationWritePlanV2,
    LocalDestinationProviderV2, POST_SAVE_RESERVE_BYTES,
};

struct ChannelConnection {
    outgoing: UnboundedSender<ManifestV2Frame>,
    incoming: UnboundedReceiver<ManifestV2Frame>,
}

fn connection_pair() -> (ChannelConnection, ChannelConnection) {
    let (left_to_right, right_incoming) = unbounded_channel();
    let (right_to_left, left_incoming) = unbounded_channel();
    (
        ChannelConnection {
            outgoing: left_to_right,
            incoming: left_incoming,
        },
        ChannelConnection {
            outgoing: right_to_left,
            incoming: right_incoming,
        },
    )
}

#[async_trait]
impl ManifestV2FrameConnection for ChannelConnection {
    async fn send_manifest_v2_frame(
        &mut self,
        frame: ManifestV2Frame,
    ) -> Result<(), ProtocolError> {
        self.outgoing.send(frame).expect("peer remains open");
        Ok(())
    }

    async fn recv_manifest_v2_frame(&mut self) -> Result<ManifestV2Frame, ProtocolError> {
        Ok(self.incoming.recv().await.expect("peer frame"))
    }

    fn export_keying_material(
        &self,
        _label: &[u8],
        _context: &[u8],
    ) -> Result<[u8; 32], ProtocolError> {
        Ok([0x91; 32])
    }

    async fn close(&mut self) -> Result<(), ProtocolError> {
        Ok(())
    }
}

#[derive(Default)]
struct RecordingProgress {
    phases: Mutex<Vec<ManifestV2ProgressPhase>>,
}

impl ManifestV2ProgressSink for RecordingProgress {
    fn on_progress(&self, _completed_plaintext_bytes: u64, _total_plaintext_bytes: u64) {}

    fn on_phase(&self, phase: ManifestV2ProgressPhase) {
        self.phases.lock().unwrap().push(phase);
    }
}

#[tokio::test]
async fn receiver_ledger_round_trips_and_authenticates_resume_identity() {
    let offer = offer();
    let accept = accept(&offer);
    let ledger = ReceiverDataPlaneLedgerV2::new(&offer, accept.clone()).unwrap();
    assert!(!ledger.requires_authenticated_resume());
    ledger.validate(&offer.manifest).unwrap();

    let temporary = tempdir().unwrap();
    let store = ReceiverDataPlaneStoreV2::new(temporary.path());
    store.save(&ledger).await.unwrap();
    let restored = store.load(accept.identity).await.unwrap().unwrap();
    assert_eq!(restored.resume_status(), ledger.resume_status());

    let valid = ResumeRequestV2 {
        identity: accept.identity,
        offer: offer.clone(),
        accept_body_digest: ledger.accept_body_digest(),
        sender_checkpoint_digest: sender_checkpoint_digest(
            offer.structural_digest,
            ledger.accept_body_digest(),
            SenderResumeIntentV2::ContinueData,
        ),
        challenge_nonce: [0xa1; 32],
    };
    restored.validate_resume_request(&valid).unwrap();
    assert_eq!(
        sender_resume_intent(offer.structural_digest, ledger.accept_body_digest(), &valid).unwrap(),
        SenderResumeIntentV2::ContinueData
    );
    let mut wrong = valid;
    wrong.accept_body_digest = ContentDigestV2([0xa2; 32]);
    assert!(matches!(
        restored.validate_resume_request(&wrong),
        Err(ManifestV2DataError::AcceptMismatch)
    ));
}

#[test]
fn ledger_rejects_tampered_accept_and_invalid_sender_checkpoint() {
    let offer = offer();
    let accept = accept(&offer);
    let mut ledger = ReceiverDataPlaneLedgerV2::new(&offer, accept.clone()).unwrap();
    ledger.accept.plan_revision += 1;
    assert!(matches!(
        ledger.validate(&offer.manifest),
        Err(ManifestV2DataError::InvalidLedger(_))
    ));

    let request = ResumeRequestV2 {
        identity: accept.identity,
        offer: offer.clone(),
        accept_body_digest: ContentDigestV2([0xa3; 32]),
        sender_checkpoint_digest: ContentDigestV2([0xa4; 32]),
        challenge_nonce: [0xa5; 32],
    };
    assert!(matches!(
        sender_resume_intent(
            offer.structural_digest,
            request.accept_body_digest,
            &request
        ),
        Err(ManifestV2DataError::InvalidLedger(_))
    ));
}

#[test]
fn block_validation_rejects_offsets_and_partial_nonterminal_blocks() {
    let offer = offer();
    let accept = accept(&offer);
    let mut ledger = ReceiverDataPlaneLedgerV2::new(&offer, accept.clone()).unwrap();
    ledger.entries[0].start = Some(EntryStartV2 {
        identity: accept.identity,
        entry_id: 0,
        encoding: EntryEncodingV2::Identity,
        plaintext_block_bytes: 4,
    });
    let entry = &offer.manifest.entries[0];
    let wrong_offset = EntryBlockV2 {
        identity: accept.identity,
        entry_id: 0,
        block_index: 0,
        plaintext_offset: 1,
        plaintext_length: 4,
        encoded_bytes: vec![0; 4],
    };
    assert!(matches!(
        validate_block(entry, &ledger.entries[0], &wrong_offset),
        Err(ManifestV2DataError::BlockOrder)
    ));
}

#[tokio::test]
async fn compressed_data_plane_saves_before_returning_results() {
    let temporary = tempdir().unwrap();
    let source = temporary.path().join("report.txt");
    let source_bytes = vec![b'x'; 128 * 1024];
    fs::write(&source, &source_bytes).await.unwrap();
    let mut job = CanonicalTransferJob::new(CompressionPolicyV2::Always).unwrap();
    job.add_local_path(source).await.unwrap();
    job.prepare_all().await.unwrap();
    let file_id = job.list_roots()[0].item_id;
    job.hash_entry(file_id).await.unwrap();
    let manifest = job.seal_for_send().unwrap().clone();
    let offer = build_manifest_offer_v2(manifest).unwrap();

    let target = temporary.path().join("received");
    let plan = DestinationWritePlanV2::create(
        &offer,
        DestinationRequestV2 {
            target_directory: target.clone(),
            copy_staging_directory: None,
            decision: DestinationDecisionV2::UseDirectSave,
            target_allocatable_bytes: Some(POST_SAVE_RESERVE_BYTES * 4),
            staging_allocatable_bytes: None,
            stable_object_identity: true,
            exceptional_transfer_approved: false,
            preplanned_root_names: None,
        },
    )
    .await
    .unwrap();
    let final_path = plan.target_path_for_root(0).unwrap();
    let accept = plan.create_initial_accept(&offer).unwrap();
    let mut destination = LocalDestinationProviderV2::new(plan, offer.manifest.clone())
        .await
        .unwrap();
    let mut receiver_ledger = ReceiverDataPlaneLedgerV2::new(&offer, accept.clone()).unwrap();
    let receiver_store = ReceiverDataPlaneStoreV2::new(temporary.path().join("receiver-state"));
    let sender_store = SenderDeliveryStoreV2::new(temporary.path().join("sender-state"));
    let mut sender_record = SenderDeliveryRecordV2::new(&offer);
    let sender_progress = RecordingProgress::default();
    let receiver_progress = RecordingProgress::default();
    let (mut sender_connection, mut receiver_connection) = connection_pair();

    let sender = ManifestV2DataPlane::send(
        &job,
        &mut sender_record,
        &sender_store,
        &mut sender_connection,
        &sender_progress,
    );
    let receiver = async {
        assert_eq!(
            receiver_connection.recv_manifest_v2_frame().await.unwrap(),
            ManifestV2Frame::Offer(offer.clone())
        );
        receiver_connection
            .send_manifest_v2_frame(ManifestV2Frame::Accept(accept))
            .await
            .unwrap();
        ManifestV2DataPlane::receive(
            &offer,
            &mut receiver_ledger,
            &receiver_store,
            &mut destination,
            &mut receiver_connection,
            &receiver_progress,
            &NoopManifestV2ResultGate,
        )
        .await
    };
    let (sender_result, receiver_result) = tokio::join!(sender, receiver);
    let sender_result = sender_result.unwrap();
    let receiver_result = receiver_result.unwrap();

    assert_eq!(sender_result.entry_results, receiver_result.entry_results);
    assert_eq!(fs::read(final_path).await.unwrap(), source_bytes);
    assert_eq!(
        sender_record.phase(),
        SenderTransferPhaseV2::WaitingForReceiverSave
    );
    assert!(
        receiver_progress
            .phases
            .lock()
            .unwrap()
            .contains(&ManifestV2ProgressPhase::Saving)
    );
    assert_eq!(
        receiver_progress.phases.lock().unwrap().last(),
        Some(&ManifestV2ProgressPhase::FinalizingDelivery)
    );
}

#[test]
fn sender_rejects_a_result_digest_that_differs_from_entry_complete() {
    let offer = offer();
    let identity = JobGenerationV2 {
        job_id: offer.manifest.job_id,
        generation: offer.manifest.generation,
    };
    let entry = &offer.manifest.entries[0];
    let completion = EntryCompleteV2 {
        identity,
        entry_id: entry.entry_id,
        final_size: entry.plaintext_size,
        final_digest: ContentDigestV2([0x11; 32]),
        completion_choice: EntryCompletionChoiceV2::PayloadComplete,
    };
    let result = EntryResultV2 {
        identity,
        entry_id: entry.entry_id,
        result: EntryResultKindV2::Saved,
        final_size: entry.plaintext_size,
        final_digest: Some(ContentDigestV2([0x22; 32])),
        final_component_override: None,
    };

    assert!(matches!(
        validate_entry_result(entry, identity, completion, &result),
        Err(ManifestV2DataError::FinalMismatch)
    ));
}
