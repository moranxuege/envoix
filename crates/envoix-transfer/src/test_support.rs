use envoix_protocol::manifest_v2::{
    CompressionPolicyV2, ContentDigestV2, EntryContentDigestV2, JobIdV2, ManifestEntryKindV2,
    ManifestEntryV2, ManifestOfferV2, ManifestRootV2, ManifestTotalsV2, ManifestV2,
    SourceCompletenessV2, build_manifest_offer_v2,
};
use envoix_protocol::manifest_v2_frames::{
    EntryDispositionV2, EntryPlanV2, EntryResultKindV2, EntryResultV2, JobGenerationV2,
    ManifestAcceptV2, ProofCapabilityV2, RootPlanV2,
};

pub(crate) const FILE_BYTES: &[u8] = b"manifest-v2";

pub(crate) fn offer() -> ManifestOfferV2 {
    let digest = ContentDigestV2(*blake3::hash(FILE_BYTES).as_bytes());
    build_manifest_offer_v2(ManifestV2 {
        job_id: JobIdV2([0x21; 16]),
        generation: 1,
        selection_revision: 1,
        compression_policy: CompressionPolicyV2::Never,
        roots: vec![ManifestRootV2 {
            root_id: 0,
            root_entry_id: 0,
            requested_name: "report.txt".into(),
            completeness: SourceCompletenessV2::Complete,
        }],
        entries: vec![ManifestEntryV2 {
            entry_id: 0,
            root_id: 0,
            parent_entry_id: None,
            component: "report.txt".into(),
            kind: ManifestEntryKindV2::RegularFile,
            plaintext_size: FILE_BYTES.len() as u64,
            content_digest: EntryContentDigestV2::Known(digest),
        }],
        totals: ManifestTotalsV2 {
            file_count: 1,
            directory_count: 0,
            total_plaintext_bytes: FILE_BYTES.len() as u64,
        },
    })
    .unwrap()
}

pub(crate) fn accept(offer: &ManifestOfferV2) -> ManifestAcceptV2 {
    ManifestAcceptV2 {
        identity: JobGenerationV2 {
            job_id: offer.manifest.job_id,
            generation: offer.manifest.generation,
        },
        manifest_digest: offer.structural_digest,
        accept_nonce: [0x31; 32],
        proof_capability: ProofCapabilityV2([0x41; 32]),
        plan_revision: 1,
        root_plans: vec![RootPlanV2 {
            root_id: 0,
            planned_name: "report.txt".into(),
        }],
        entry_plans: vec![EntryPlanV2 {
            entry_id: 0,
            disposition: EntryDispositionV2::ReceivePayload,
            next_plaintext_block: 0,
        }],
    }
}

pub(crate) fn entry_results(offer: &ManifestOfferV2) -> Vec<EntryResultV2> {
    vec![EntryResultV2 {
        identity: JobGenerationV2 {
            job_id: offer.manifest.job_id,
            generation: offer.manifest.generation,
        },
        entry_id: 0,
        result: EntryResultKindV2::Saved,
        final_size: FILE_BYTES.len() as u64,
        final_digest: Some(ContentDigestV2(*blake3::hash(FILE_BYTES).as_bytes())),
        final_component_override: None,
    }]
}
