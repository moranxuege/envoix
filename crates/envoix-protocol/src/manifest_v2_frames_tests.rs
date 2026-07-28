use super::*;
use crate::manifest_v2::{
    CompressionPolicyV2, EntryContentDigestV2, ManifestEntryKindV2, ManifestEntryV2,
    ManifestRootV2, ManifestTotalsV2, ManifestV2, SourceCompletenessV2, build_manifest_offer_v2,
};

fn offer() -> ManifestOfferV2 {
    build_manifest_offer_v2(ManifestV2 {
        job_id: JobIdV2([0x11; 16]),
        generation: 3,
        selection_revision: 7,
        compression_policy: CompressionPolicyV2::Smart,
        roots: vec![ManifestRootV2 {
            root_id: 0,
            root_entry_id: 0,
            requested_name: "Photos".into(),
            completeness: SourceCompletenessV2::Complete,
        }],
        entries: vec![
            ManifestEntryV2 {
                entry_id: 0,
                root_id: 0,
                parent_entry_id: None,
                component: "Photos".into(),
                kind: ManifestEntryKindV2::Directory,
                plaintext_size: 0,
                content_digest: EntryContentDigestV2::Deferred,
            },
            ManifestEntryV2 {
                entry_id: 1,
                root_id: 0,
                parent_entry_id: Some(0),
                component: "image.jpg".into(),
                kind: ManifestEntryKindV2::RegularFile,
                plaintext_size: 3,
                content_digest: EntryContentDigestV2::Known(ContentDigestV2([0x22; 32])),
            },
        ],
        totals: ManifestTotalsV2 {
            file_count: 1,
            directory_count: 1,
            total_plaintext_bytes: 3,
        },
    })
    .unwrap()
}

fn identity() -> JobGenerationV2 {
    JobGenerationV2 {
        job_id: JobIdV2([0x11; 16]),
        generation: 3,
    }
}

fn entry_result(entry_id: u32) -> EntryResultV2 {
    EntryResultV2 {
        identity: identity(),
        entry_id,
        result: EntryResultKindV2::Saved,
        final_size: if entry_id == 0 { 0 } else { 3 },
        final_digest: (entry_id == 1).then_some(ContentDigestV2([0x22; 32])),
        final_component_override: None,
    }
}

fn frames() -> Vec<ManifestV2Frame> {
    let offer = offer();
    let accept = ManifestAcceptV2 {
        identity: identity(),
        manifest_digest: offer.structural_digest,
        accept_nonce: [0x33; 32],
        proof_capability: ProofCapabilityV2([0x44; 32]),
        plan_revision: 1,
        root_plans: vec![RootPlanV2 {
            root_id: 0,
            planned_name: "Photos (1)".into(),
        }],
        entry_plans: vec![
            EntryPlanV2 {
                entry_id: 0,
                disposition: EntryDispositionV2::ReceivePayload,
                next_plaintext_block: 0,
            },
            EntryPlanV2 {
                entry_id: 1,
                disposition: EntryDispositionV2::ReceivePayload,
                next_plaintext_block: 0,
            },
        ],
    };
    let accept_body_digest =
        canonical_manifest_v2_frame_body_digest(&ManifestV2Frame::Accept(accept.clone())).unwrap();
    vec![
        ManifestV2Frame::Offer(offer.clone()),
        ManifestV2Frame::Accept(accept),
        ManifestV2Frame::EntryStart(EntryStartV2 {
            identity: identity(),
            entry_id: 1,
            encoding: EntryEncodingV2::Identity,
            plaintext_block_bytes: 4,
        }),
        ManifestV2Frame::EntryContentDigest(EntryContentDigestFrameV2 {
            identity: identity(),
            entry_id: 1,
            digest: ContentDigestV2([0x22; 32]),
            decision: EntryDigestDecisionV2::ContinuePayload,
        }),
        ManifestV2Frame::EntryBlock(EntryBlockV2 {
            identity: identity(),
            entry_id: 1,
            block_index: 0,
            plaintext_offset: 0,
            plaintext_length: 3,
            encoded_bytes: vec![1, 2, 3],
        }),
        ManifestV2Frame::EntryComplete(EntryCompleteV2 {
            identity: identity(),
            entry_id: 1,
            final_size: 3,
            final_digest: ContentDigestV2([0x22; 32]),
            completion_choice: EntryCompletionChoiceV2::PayloadComplete,
        }),
        ManifestV2Frame::EntryResult(entry_result(1)),
        ManifestV2Frame::JobComplete(JobCompleteV2 {
            identity: identity(),
            sender_completion_set_digest: ContentDigestV2([0x55; 32]),
        }),
        ManifestV2Frame::DeliveryProof(DeliveryProofV2 {
            identity: identity(),
            manifest_digest: offer.structural_digest,
            result_set_digest: ContentDigestV2([0x66; 32]),
            proof_nonce: [0x77; 32],
            proof_mac: [0x88; 32],
        }),
        ManifestV2Frame::ResumeRequest(ResumeRequestV2 {
            identity: identity(),
            offer,
            accept_body_digest,
            sender_checkpoint_digest: ContentDigestV2([0x99; 32]),
            challenge_nonce: [0xaa; 32],
        }),
        ManifestV2Frame::ResumeStatus(ResumeStatusV2 {
            identity: identity(),
            accept_body_digest,
            plan_revision: 1,
            entries: vec![
                ResumeEntryV2 {
                    entry_id: 0,
                    arbiter: EntryArbiterV2::PayloadCompleteChosen,
                    next_plaintext_block: 0,
                    content_digest: None,
                    entry_result: Some(entry_result(0)),
                },
                ResumeEntryV2 {
                    entry_id: 1,
                    arbiter: EntryArbiterV2::PayloadOpen,
                    next_plaintext_block: 1,
                    content_digest: Some(ContentDigestV2([0x22; 32])),
                    entry_result: None,
                },
            ],
            challenge_nonce: [0xaa; 32],
            challenge_mac: [0xbb; 32],
        }),
        ManifestV2Frame::Cancel(CancelV2 {
            identity: identity(),
            scope: CancelScopeV2::Entry,
            entry_id: Some(1),
            failure_code: 42,
        }),
        ManifestV2Frame::Error(ManifestErrorV2 {
            identity: identity(),
            failure_code: 43,
            phase: ManifestFailurePhaseV2::Save,
            entry_id: Some(1),
        }),
    ]
}

#[test]
fn every_frozen_frame_round_trips_canonically() {
    for frame in frames() {
        let encoded = encode_manifest_v2_frame(&frame).unwrap();
        assert_eq!(decode_manifest_v2_frame(&encoded).unwrap(), frame);
        assert_eq!(
            canonical_manifest_v2_frame_body_digest(&frame).unwrap(),
            ContentDigestV2(*blake3::hash(&encoded[HEADER_BYTES..]).as_bytes())
        );
    }
}

#[tokio::test]
async fn framed_io_reads_exactly_one_frame() {
    let expected = frames()[4].clone();
    let (mut writer, mut reader) = tokio::io::duplex(1024);
    write_manifest_v2_frame(&mut writer, &expected)
        .await
        .unwrap();
    assert_eq!(read_manifest_v2_frame(&mut reader).await.unwrap(), expected);
}

#[test]
fn malformed_headers_and_noncanonical_fields_are_rejected() {
    let mut encoded = encode_manifest_v2_frame(&frames()[2]).unwrap();
    encoded[6..8].copy_from_slice(&99_u16.to_be_bytes());
    assert!(matches!(
        decode_manifest_v2_frame(&encoded),
        Err(ManifestV2FrameCodecError::UnknownFrameType(99))
    ));

    let invalid_cancel = ManifestV2Frame::Cancel(CancelV2 {
        identity: identity(),
        scope: CancelScopeV2::Job,
        entry_id: Some(1),
        failure_code: 1,
    });
    assert!(matches!(
        encode_manifest_v2_frame(&invalid_cancel),
        Err(ManifestV2FrameCodecError::InvalidCancelScope)
    ));

    let invalid_block = ManifestV2Frame::EntryBlock(EntryBlockV2 {
        identity: identity(),
        entry_id: 1,
        block_index: 0,
        plaintext_offset: 0,
        plaintext_length: 0,
        encoded_bytes: vec![1],
    });
    assert!(matches!(
        encode_manifest_v2_frame(&invalid_block),
        Err(ManifestV2FrameCodecError::InvalidBlock)
    ));
}

#[test]
fn debug_output_redacts_receiver_capability() {
    let accept = match &frames()[1] {
        ManifestV2Frame::Accept(accept) => accept.clone(),
        _ => unreachable!(),
    };
    let debug = format!("{accept:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains(&"44".repeat(32)));
}
