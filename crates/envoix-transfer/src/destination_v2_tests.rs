use super::*;
use crate::test_support::{FILE_BYTES, offer};
use tempfile::tempdir;

fn request(target_directory: PathBuf) -> DestinationRequestV2 {
    DestinationRequestV2 {
        target_directory,
        copy_staging_directory: None,
        decision: DestinationDecisionV2::UseDirectSave,
        target_allocatable_bytes: Some(POST_SAVE_RESERVE_BYTES * 4),
        staging_allocatable_bytes: None,
        stable_object_identity: true,
        exceptional_transfer_approved: false,
        preplanned_root_names: None,
    }
}

#[tokio::test]
async fn planning_reserves_keep_both_name_without_overwriting() {
    let temporary = tempdir().unwrap();
    let target = temporary.path().join("received");
    fs::create_dir_all(&target).await.unwrap();
    fs::write(target.join("report.txt"), b"different")
        .await
        .unwrap();

    let offer = offer();
    let plan = DestinationWritePlanV2::create(&offer, request(target.clone()))
        .await
        .unwrap();
    assert_eq!(plan.mode, DestinationModeV2::DirectSave);
    assert_eq!(plan.root_plans[0].planned_name, "report (1).txt");
    let target = fs::canonicalize(target).await.unwrap();
    assert_eq!(
        plan.target_path_for_root(0).unwrap(),
        target.join("report (1).txt")
    );
    let accept = plan.create_initial_accept(&offer).unwrap();
    assert_eq!(
        accept.entry_plans[0].disposition,
        EntryDispositionV2::ReceivePayload
    );
}

#[tokio::test]
async fn platform_preplanned_root_name_is_frozen_into_accept() {
    let temporary = tempdir().unwrap();
    let target = temporary.path().join("received");
    fs::create_dir_all(&target).await.unwrap();
    let offer = offer();
    let mut destination = request(target);
    destination.preplanned_root_names = Some(vec![RootPlanV2 {
        root_id: 0,
        planned_name: "report (7).txt".into(),
    }]);

    let plan = DestinationWritePlanV2::create(&offer, destination)
        .await
        .unwrap();
    let accept = plan.create_initial_accept(&offer).unwrap();

    assert_eq!(plan.root_plans, accept.root_plans);
    assert_eq!(accept.root_plans[0].planned_name, "report (7).txt");
}

#[tokio::test]
async fn late_collision_fails_without_changing_the_accepted_name() {
    let temporary = tempdir().unwrap();
    let target = temporary.path().join("received");
    let offer = offer();
    let plan = DestinationWritePlanV2::create(&offer, request(target.clone()))
        .await
        .unwrap();
    let accepted_name = plan.root_plans[0].planned_name.clone();
    let mut destination = LocalDestinationProviderV2::new(plan.clone(), offer.manifest.clone())
        .await
        .unwrap();
    let entry = &offer.manifest.entries[0];
    let digest = ContentDigestV2(*blake3::hash(FILE_BYTES).as_bytes());
    destination
        .begin_entry(
            entry,
            EntryStartV2 {
                identity: JobGenerationV2 {
                    job_id: offer.manifest.job_id,
                    generation: offer.manifest.generation,
                },
                entry_id: entry.entry_id,
                encoding: envoix_protocol::manifest_v2_frames::EntryEncodingV2::Identity,
                plaintext_block_bytes: FILE_BYTES.len() as u32,
            },
            0,
        )
        .await
        .unwrap();
    destination
        .write_block(
            entry,
            &EntryBlockV2 {
                identity: JobGenerationV2 {
                    job_id: offer.manifest.job_id,
                    generation: offer.manifest.generation,
                },
                entry_id: entry.entry_id,
                block_index: 0,
                plaintext_offset: 0,
                plaintext_length: FILE_BYTES.len() as u32,
                encoded_bytes: FILE_BYTES.to_vec(),
            },
        )
        .await
        .unwrap();
    destination.verify_payload(entry, digest).await.unwrap();

    fs::write(target.join(&accepted_name), b"external")
        .await
        .unwrap();
    let error = destination
        .commit_job(
            &offer.manifest,
            &[VerifiedEntryV2 {
                entry_id: entry.entry_id,
                final_digest: Some(digest),
                completion_choice: EntryCompletionChoiceV2::PayloadComplete,
            }],
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        ManifestV2DataError::Destination(DestinationPlanErrorV2::DestinationContended)
    ));
    assert_eq!(destination.plan().plan_revision, plan.plan_revision);
    assert_eq!(destination.plan().root_plans, plan.root_plans);
    assert_eq!(
        fs::read(target.join(accepted_name)).await.unwrap(),
        b"external"
    );
}

#[tokio::test]
async fn identical_root_file_is_advertised_as_stable_reuse() {
    let temporary = tempdir().unwrap();
    let target = temporary.path().join("received");
    fs::create_dir_all(&target).await.unwrap();
    fs::write(target.join("report.txt"), FILE_BYTES)
        .await
        .unwrap();
    let offer = offer();
    let plan = DestinationWritePlanV2::create(&offer, request(target))
        .await
        .unwrap();
    let accept = plan.create_initial_accept(&offer).unwrap();
    assert_eq!(
        accept.entry_plans[0].disposition,
        EntryDispositionV2::ReuseExisting
    );
}

#[tokio::test]
async fn durable_plan_round_trips_and_rejects_changed_destination() {
    let temporary = tempdir().unwrap();
    let target = temporary.path().join("received");
    let other = temporary.path().join("other");
    fs::create_dir_all(&other).await.unwrap();
    let offer = offer();
    let plan = DestinationWritePlanV2::create(&offer, request(target))
        .await
        .unwrap();
    let store = DestinationPlanStoreV2::new(temporary.path().join("plans"));
    store.save(&plan).await.unwrap();
    assert_eq!(
        store.load(plan.job_id, plan.generation).await.unwrap(),
        Some(plan.clone())
    );
    assert!(matches!(
        plan.validate_resume_request(&offer, &request(other)).await,
        Err(DestinationPlanErrorV2::InvalidEntryState)
    ));
}

#[tokio::test]
async fn capacity_must_be_known_and_leave_post_save_reserve() {
    let temporary = tempdir().unwrap();
    let offer = offer();
    let target = temporary.path().join("received");
    let mut unknown = request(target.clone());
    unknown.target_allocatable_bytes = None;
    assert!(matches!(
        DestinationWritePlanV2::create(&offer, unknown).await,
        Err(DestinationPlanErrorV2::UnknownCapacity)
    ));

    let mut insufficient = request(target);
    insufficient.target_allocatable_bytes = Some(POST_SAVE_RESERVE_BYTES);
    assert!(matches!(
        DestinationWritePlanV2::create(&offer, insufficient).await,
        Err(DestinationPlanErrorV2::InsufficientSpace { .. })
    ));
}

#[test]
fn keep_both_suffix_preserves_file_extensions_but_not_directory_dots() {
    assert_eq!(
        component_with_suffix("report.txt", 1, true),
        "report (1).txt"
    );
    assert_eq!(
        component_with_suffix("Folder.v1", 1, false),
        "Folder.v1 (1)"
    );
    assert_eq!(
        component_with_suffix(".gitignore", 1, true),
        ".gitignore (1)"
    );
    assert!(component_with_suffix(&"界".repeat(85), 9, false).len() <= 255);
}
