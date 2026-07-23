use super::*;
use tempfile::tempdir;

#[tokio::test]
async fn multiple_roots_are_paginated_hashed_and_sealed_immutably() {
    let temporary = tempdir().unwrap();
    let first_root = temporary.path().join("First");
    let second_root = temporary.path().join("Second");
    fs::create_dir_all(first_root.join("Empty")).await.unwrap();
    fs::create_dir_all(second_root.join("Nested"))
        .await
        .unwrap();
    fs::write(first_root.join("alpha.txt"), b"alpha")
        .await
        .unwrap();
    fs::write(second_root.join("Nested/beta.txt"), b"beta")
        .await
        .unwrap();

    let mut job = CanonicalTransferJob::new(CompressionPolicyV2::Smart).unwrap();
    job.add_local_path(first_root.clone()).await.unwrap();
    job.add_local_path(second_root).await.unwrap();
    job.prepare_all().await.unwrap();

    assert_eq!(
        job.inventory_summary(),
        InventorySummary {
            root_count: 2,
            file_count: 2,
            directory_count: 4,
            total_plaintext_bytes: 9,
            warning_count: 0,
        }
    );
    let roots = job.list_roots();
    assert_eq!(
        roots
            .iter()
            .map(|root| root.name.as_str())
            .collect::<Vec<_>>(),
        ["First", "Second"]
    );

    let first_page = job.list_children(roots[0].item_id, None, 1).unwrap();
    assert_eq!(first_page.items.len(), 1);
    let cursor = first_page.next_cursor.unwrap();
    assert_eq!(
        job.list_children(roots[0].item_id, Some(cursor), 8)
            .unwrap()
            .items
            .len(),
        1
    );

    let file_ids = job
        .inventory
        .iter()
        .filter(|entry| entry.kind == ManifestEntryKindV2::RegularFile)
        .map(|entry| entry.item_id)
        .collect::<Vec<_>>();
    for item_id in file_ids {
        job.hash_entry(item_id).await.unwrap();
    }
    assert!(matches!(
        job.list_children(roots[0].item_id, Some(cursor), 8),
        Err(TransferJobError::StaleInventoryCursor { .. })
    ));

    let manifest = job.seal_for_send().unwrap().clone();
    assert_eq!(manifest.roots.len(), 2);
    assert!(
        manifest
            .entries
            .iter()
            .filter(|entry| entry.kind == ManifestEntryKindV2::RegularFile)
            .all(|entry| matches!(entry.content_digest, EntryContentDigestV2::Known(_)))
    );
    assert!(matches!(
        job.set_compression_policy(CompressionPolicyV2::Always),
        Err(TransferJobError::SealedMutation)
    ));

    let store = TransferJobStore::new(temporary.path().join("jobs"));
    store.save(&job).await.unwrap();
    let restored = store.load(job.job_id()).await.unwrap().unwrap();
    assert_eq!(restored.manifest(), Some(&manifest));

    let alpha_entry = manifest
        .entries
        .iter()
        .find(|entry| entry.component == "alpha.txt")
        .unwrap();
    let source = restored
        .source_for_sealed_entry(alpha_entry.entry_id)
        .unwrap();
    fs::write(first_root.join("alpha.txt"), b"changed")
        .await
        .unwrap();
    assert!(matches!(
        source.verify_unchanged().await,
        Err(TransferJobError::SourceChanged)
    ));
}

#[tokio::test]
async fn overlapping_user_selections_collapse_to_one_canonical_root() {
    let temporary = tempdir().unwrap();
    let folder = temporary.path().join("Folder");
    let child = folder.join("file.txt");
    fs::create_dir_all(&folder).await.unwrap();
    fs::write(&child, b"content").await.unwrap();

    let mut child_first = CanonicalTransferJob::new(CompressionPolicyV2::Never).unwrap();
    let child_id = child_first
        .add_local_path(child.clone())
        .await
        .unwrap()
        .root_item_id;
    let parent = child_first.add_local_path(folder.clone()).await.unwrap();
    assert_eq!(parent.removed_covered_roots, vec![child_id]);
    child_first.prepare_all().await.unwrap();
    assert_eq!(child_first.inventory_summary().root_count, 1);

    let mut parent_first = CanonicalTransferJob::new(CompressionPolicyV2::Never).unwrap();
    let parent = parent_first.add_local_path(folder).await.unwrap();
    let folded = parent_first.add_local_path(child).await.unwrap();
    assert!(folded.folded_into_existing_selection);
    assert_eq!(folded.root_item_id, parent.root_item_id);
}

#[tokio::test]
async fn store_owned_staging_is_deleted_only_after_remove_is_persisted() {
    let temporary = tempdir().unwrap();
    let source = temporary.path().join("share.bin");
    fs::write(&source, b"shared").await.unwrap();
    let store = TransferJobStore::new(temporary.path().join("jobs"));
    let mut job = CanonicalTransferJob::new(CompressionPolicyV2::Smart).unwrap();
    let added = store
        .import_staged_file(
            &mut job,
            &source,
            "share.bin".into(),
            LocalSourceOrigin::ShareStaging,
        )
        .await
        .unwrap();
    let staged = job.selections[0].path.clone();
    assert!(fs::try_exists(&staged).await.unwrap());
    job.prepare_all().await.unwrap();
    store.save(&job).await.unwrap();

    store
        .apply_source_decision(
            &mut job,
            added.root_item_id,
            SourceDecision::RemoveSelection,
        )
        .await
        .unwrap();
    assert!(!fs::try_exists(&staged).await.unwrap());
    assert!(
        store
            .load(job.job_id())
            .await
            .unwrap()
            .unwrap()
            .source_selections()
            .is_empty()
    );
}

#[tokio::test]
async fn inaccessible_source_requires_an_explicit_user_decision() {
    let temporary = tempdir().unwrap();
    let missing = temporary.path().join("missing.txt");
    let mut job = CanonicalTransferJob::new(CompressionPolicyV2::Never).unwrap();
    let root = job
        .add_local_path(missing.clone())
        .await
        .unwrap()
        .root_item_id;
    job.prepare_all().await.unwrap();
    assert_eq!(job.lifecycle(), JobLifecycle::NeedsSourceDecision);
    assert!(matches!(
        job.seal_for_send(),
        Err(TransferJobError::UnresolvedSourceDecision)
    ));

    fs::write(&missing, b"restored").await.unwrap();
    job.resolve_source_decision(
        root,
        SourceDecision::Reauthorize {
            local_path: missing,
        },
    )
    .unwrap();
    job.prepare_selection(root).await.unwrap();
    assert_eq!(job.lifecycle(), JobLifecycle::ReadyToSend);
}

#[tokio::test]
async fn decomposed_filesystem_names_are_sealed_as_unicode_nfc() {
    let temporary = tempdir().unwrap();
    let folder = temporary.path().join("Unicode");
    fs::create_dir_all(&folder).await.unwrap();
    let decomposed = "re\u{301}sume\u{301}.txt";
    let normalized = "résumé.txt";
    fs::write(folder.join(decomposed), b"unicode content")
        .await
        .unwrap();

    let mut job = CanonicalTransferJob::new(CompressionPolicyV2::Never).unwrap();
    job.add_local_path(folder).await.unwrap();
    job.prepare_all().await.unwrap();
    let entry_id = {
        let manifest = job.seal_for_send().unwrap();
        let entry = manifest
            .entries
            .iter()
            .find(|entry| entry.kind == ManifestEntryKindV2::RegularFile)
            .unwrap();
        assert_eq!(entry.component, normalized);
        assert!(unicode_normalization::is_nfc(&entry.component));
        entry.entry_id
    };
    job.source_for_sealed_entry(entry_id)
        .unwrap()
        .verify_unchanged()
        .await
        .unwrap();
}
