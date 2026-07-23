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
