use super::*;

fn file(entry_id: u32, relative_path: &str, size: u64) -> ManifestEntryV1 {
    ManifestEntryV1 {
        entry_id,
        relative_path: relative_path.to_owned(),
        kind: ManifestEntryKind::RegularFile,
        size,
        hash: Some([entry_id as u8 + 1; 32]),
        modified_at_unix_ms: None,
    }
}

fn directory(entry_id: u32, relative_path: &str) -> ManifestEntryV1 {
    ManifestEntryV1 {
        entry_id,
        relative_path: relative_path.to_owned(),
        kind: ManifestEntryKind::Directory,
        size: 0,
        hash: None,
        modified_at_unix_ms: None,
    }
}

fn valid_manifest() -> ManifestV1 {
    ManifestV1 {
        manifest_id: ManifestId::new("manifest-1"),
        entries: vec![
            directory(0, "Photos"),
            file(1, "Photos/photo.jpg", 3),
            file(2, "notes.txt", 5),
        ],
        file_count: 2,
        directory_count: 1,
        root_count: 2,
        total_bytes: 8,
        hash_algorithm: ManifestHashAlgorithm::Blake3_256,
    }
}

#[test]
fn transfer_protocol_selection_never_falls_back_silently() {
    assert_eq!(
        TransferProtocol::required_for_shape(1, 0),
        Some(TransferProtocol::SingleFileV1)
    );
    assert_eq!(
        TransferProtocol::required_for_shape(2, 0),
        Some(TransferProtocol::ManifestV1)
    );
    assert_eq!(
        TransferProtocol::required_for_shape(0, 1),
        Some(TransferProtocol::ManifestV1)
    );
    assert_eq!(TransferProtocol::required_for_shape(0, 0), None);
    assert_eq!(
        TransferProtocol::from_alpn(SINGLE_FILE_V1_ALPN),
        Some(TransferProtocol::SingleFileV1)
    );
    assert_eq!(
        TransferProtocol::from_alpn(MANIFEST_V1_ALPN),
        Some(TransferProtocol::ManifestV1)
    );
    assert_eq!(TransferProtocol::from_alpn(b"envoix/unknown"), None);
}

#[test]
fn valid_parent_before_child_manifest_passes() {
    valid_manifest().validate(1_024).unwrap();
}

#[test]
fn encoded_size_limit_is_inclusive() {
    valid_manifest()
        .validate(MAX_MANIFEST_V1_ENCODED_BYTES)
        .unwrap();
    assert!(matches!(
        valid_manifest().validate(MAX_MANIFEST_V1_ENCODED_BYTES + 1),
        Err(ManifestValidationError::EncodedManifestTooLarge { .. })
    ));
}

#[test]
fn portable_path_rules_reject_unsafe_inputs() {
    let cases = [
        ("", ManifestPathViolation::Empty),
        ("/absolute", ManifestPathViolation::Absolute),
        (
            "trailing/",
            ManifestPathViolation::LeadingOrTrailingSeparator,
        ),
        ("two//parts", ManifestPathViolation::EmptyComponent),
        (".", ManifestPathViolation::CurrentDirectoryComponent),
        ("a/./b", ManifestPathViolation::CurrentDirectoryComponent),
        ("..", ManifestPathViolation::ParentDirectoryComponent),
        ("a/../b", ManifestPathViolation::ParentDirectoryComponent),
        ("a\\b", ManifestPathViolation::Backslash),
        ("a\0b", ManifestPathViolation::Null),
        ("a\nb", ManifestPathViolation::ControlCharacter),
    ];
    for (path, expected) in cases {
        assert_eq!(validate_manifest_relative_path(path), Err(expected));
    }
}

#[test]
fn portable_path_rules_measure_utf8_bytes_and_depth() {
    let maximum_component = "a".repeat(MAX_MANIFEST_V1_COMPONENT_BYTES);
    validate_manifest_relative_path(&maximum_component).unwrap();

    let maximum_depth = std::iter::repeat_n("a", MAX_MANIFEST_V1_PATH_DEPTH)
        .collect::<Vec<_>>()
        .join("/");
    validate_manifest_relative_path(&maximum_depth).unwrap();

    let long_component = "界".repeat(86);
    assert!(matches!(
        validate_manifest_relative_path(&long_component),
        Err(ManifestPathViolation::ComponentTooLong { .. })
    ));

    let deep_path = std::iter::repeat_n("a", MAX_MANIFEST_V1_PATH_DEPTH + 1)
        .collect::<Vec<_>>()
        .join("/");
    assert!(matches!(
        validate_manifest_relative_path(&deep_path),
        Err(ManifestPathViolation::TooDeep { .. })
    ));

    let long_path = (0..17)
        .map(|_| "a".repeat(MAX_MANIFEST_V1_COMPONENT_BYTES))
        .collect::<Vec<_>>()
        .join("/");
    assert!(matches!(
        validate_manifest_relative_path(&long_path),
        Err(ManifestPathViolation::PathTooLong { .. })
    ));
}

#[test]
fn empty_and_oversized_manifests_are_rejected_before_entry_walk() {
    let mut empty_id = valid_manifest();
    empty_id.manifest_id = ManifestId::new("  ");
    assert_eq!(
        empty_id.validate_structure(),
        Err(ManifestValidationError::EmptyManifestId)
    );

    let mut empty = valid_manifest();
    empty.entries.clear();
    assert_eq!(
        empty.validate_structure(),
        Err(ManifestValidationError::EmptyManifest)
    );

    let repeated = file(0, "item", 0);
    let mut oversized = valid_manifest();
    oversized.entries = vec![repeated; MAX_MANIFEST_V1_ENTRIES + 1];
    assert_eq!(
        oversized.validate_structure(),
        Err(ManifestValidationError::TooManyEntries {
            actual: MAX_MANIFEST_V1_ENTRIES + 1,
            maximum: MAX_MANIFEST_V1_ENTRIES,
        })
    );
}

#[test]
fn duplicate_missing_and_file_parents_are_rejected() {
    let mut duplicate = valid_manifest();
    duplicate.entries[2].relative_path = "Photos/photo.jpg".into();
    assert!(matches!(
        duplicate.validate_structure(),
        Err(ManifestValidationError::DuplicatePath { .. })
    ));

    let mut missing_parent = valid_manifest();
    missing_parent.entries.remove(0);
    missing_parent.entries[0].entry_id = 0;
    missing_parent.entries[1].entry_id = 1;
    assert!(matches!(
        missing_parent.validate_structure(),
        Err(ManifestValidationError::MissingParentDirectory { .. })
    ));

    let mut file_parent = valid_manifest();
    file_parent.entries[0] = file(0, "Photos", 1);
    assert!(matches!(
        file_parent.validate_structure(),
        Err(ManifestValidationError::ParentIsRegularFile { .. })
    ));
}

#[test]
fn entry_metadata_counts_and_total_are_verified() {
    let mut missing_hash = valid_manifest();
    missing_hash.entries[1].hash = None;
    assert_eq!(
        missing_hash.validate_structure(),
        Err(ManifestValidationError::FileHashMissing { entry_id: 1 })
    );

    let mut directory_metadata = valid_manifest();
    directory_metadata.entries[0].size = 1;
    assert_eq!(
        directory_metadata.validate_structure(),
        Err(ManifestValidationError::InvalidDirectoryMetadata { entry_id: 0 })
    );

    let mut bad_id = valid_manifest();
    bad_id.entries[1].entry_id = 7;
    assert_eq!(
        bad_id.validate_structure(),
        Err(ManifestValidationError::EntryIdMismatch {
            expected: 1,
            actual: 7,
        })
    );

    let mut bad_count = valid_manifest();
    bad_count.file_count = 1;
    assert!(matches!(
        bad_count.validate_structure(),
        Err(ManifestValidationError::CountMismatch {
            field: "file_count",
            ..
        })
    ));

    let mut bad_total = valid_manifest();
    bad_total.total_bytes = 7;
    assert_eq!(
        bad_total.validate_structure(),
        Err(ManifestValidationError::TotalBytesMismatch {
            declared: 7,
            actual: 8,
        })
    );
}

#[test]
fn aggregate_size_uses_checked_arithmetic() {
    let manifest = ManifestV1 {
        manifest_id: ManifestId::new("overflow"),
        entries: vec![file(0, "one", u64::MAX), file(1, "two", 1)],
        file_count: 2,
        directory_count: 0,
        root_count: 2,
        total_bytes: u64::MAX,
        hash_algorithm: ManifestHashAlgorithm::Blake3_256,
    };
    assert_eq!(
        manifest.validate_structure(),
        Err(ManifestValidationError::TotalBytesOverflow)
    );
}

#[test]
fn serialized_contract_uses_v1_names() {
    assert_eq!(
        serde_json::to_string(&TransferProtocol::ManifestV1).unwrap(),
        "\"manifest_v1\""
    );
    let encoded = serde_json::to_string(&valid_manifest()).unwrap();
    assert!(encoded.contains("\"regular_file\""));
    assert!(encoded.contains("\"blake3_256\""));
    assert_eq!(
        serde_json::to_string(&ManifestEntryResultStatus::SkippedIdentical).unwrap(),
        "\"skipped_identical\""
    );
}

#[tokio::test]
async fn manifest_frame_family_round_trips_with_frozen_type_ids() {
    let manifest = valid_manifest();
    let manifest_id = manifest.manifest_id.clone();
    let transfer_id = TransferId::new("entry-transfer-1");
    let frames = vec![
        (
            16,
            ManifestFrame::Hello(ManifestHelloV1 {
                protocol_version: MANIFEST_V1_PROTOCOL_VERSION,
                role: PeerRole::Sender,
            }),
        ),
        (
            17,
            ManifestFrame::Offer(ManifestOfferV1 {
                manifest: manifest.clone(),
                chunk_size: 64 * 1024,
                resume_requested: true,
            }),
        ),
        (
            18,
            ManifestFrame::Accept(ManifestAcceptV1 {
                manifest_id: manifest_id.clone(),
                entries: vec![
                    ManifestEntryDispositionV1 {
                        entry_id: 0,
                        disposition: ManifestEntryDispositionKind::CreateDirectory,
                        final_relative_path: "Photos".into(),
                    },
                    ManifestEntryDispositionV1 {
                        entry_id: 1,
                        disposition: ManifestEntryDispositionKind::Transfer,
                        final_relative_path: "Photos/photo (1).jpg".into(),
                    },
                    ManifestEntryDispositionV1 {
                        entry_id: 2,
                        disposition: ManifestEntryDispositionKind::SkipIdentical,
                        final_relative_path: "notes.txt".into(),
                    },
                ],
            }),
        ),
        (
            19,
            ManifestFrame::EntryStart(ManifestEntryStartV1 {
                manifest_id: manifest_id.clone(),
                entry_id: 1,
                transfer_id: transfer_id.clone(),
            }),
        ),
        (
            20,
            ManifestFrame::ResumeStatus(ManifestResumeStatusV1 {
                manifest_id: manifest_id.clone(),
                entry_id: 1,
                transfer_id: transfer_id.clone(),
                next_chunk_index: 2,
                bytes_received: 128,
                prefix_hash: [7; 32],
            }),
        ),
        (
            21,
            ManifestFrame::Chunk(ManifestChunkV1 {
                manifest_id: manifest_id.clone(),
                entry_id: 1,
                transfer_id: transfer_id.clone(),
                index: 2,
                offset: 128,
                bytes: b"manifest bytes".to_vec(),
            }),
        ),
        (
            22,
            ManifestFrame::EntryComplete(ManifestEntryCompleteV1 {
                manifest_id: manifest_id.clone(),
                entry_id: 1,
                transfer_id: transfer_id.clone(),
                file_hash: [2; 32],
            }),
        ),
        (
            23,
            ManifestFrame::EntryCompleteAck(ManifestEntryCompleteAckV1 {
                manifest_id: manifest_id.clone(),
                entry_id: 1,
                transfer_id,
            }),
        ),
        (
            24,
            ManifestFrame::Complete(ManifestCompleteV1 {
                manifest_id: manifest_id.clone(),
            }),
        ),
        (
            25,
            ManifestFrame::CompleteAck(ManifestCompleteAckV1 {
                manifest_id: manifest_id.clone(),
                entries: vec![
                    ManifestEntryResultV1 {
                        entry_id: 0,
                        status: ManifestEntryResultStatus::Completed,
                        offered_relative_path: "Photos".into(),
                        final_relative_path: Some("Photos".into()),
                        failure_code: None,
                    },
                    ManifestEntryResultV1 {
                        entry_id: 1,
                        status: ManifestEntryResultStatus::Renamed,
                        offered_relative_path: "Photos/photo.jpg".into(),
                        final_relative_path: Some("Photos/photo (1).jpg".into()),
                        failure_code: None,
                    },
                    ManifestEntryResultV1 {
                        entry_id: 2,
                        status: ManifestEntryResultStatus::SkippedIdentical,
                        offered_relative_path: "notes.txt".into(),
                        final_relative_path: Some("notes.txt".into()),
                        failure_code: None,
                    },
                ],
            }),
        ),
        (
            26,
            ManifestFrame::Error(ManifestErrorV1 {
                manifest_id: Some(manifest_id),
                entry_id: Some(1),
                code: "manifest.hash_mismatch".into(),
                message: "entry changed after preflight".into(),
            }),
        ),
    ];

    for (expected_type, frame) in frames {
        let mut encoded = Vec::new();
        write_manifest_frame(&mut encoded, &frame).await.unwrap();
        assert_eq!(encoded[6], expected_type);
        assert_eq!(
            read_manifest_frame(&mut encoded.as_slice()).await.unwrap(),
            frame
        );
    }
}

#[tokio::test]
async fn direct_manifest_chunk_writer_matches_regular_codec() {
    let manifest_id = ManifestId::new("manifest-1");
    let transfer_id = TransferId::new("entry-transfer-1");
    let expected = ManifestFrame::Chunk(ManifestChunkV1 {
        manifest_id: manifest_id.clone(),
        entry_id: 4,
        transfer_id: transfer_id.clone(),
        index: 7,
        offset: 1024,
        bytes: b"hello".to_vec(),
    });
    let mut regular = Vec::new();
    let mut direct = Vec::new();

    write_manifest_frame(&mut regular, &expected).await.unwrap();
    write_manifest_chunk_frame(
        &mut direct,
        &manifest_id,
        4,
        &transfer_id,
        7,
        1024,
        b"hello",
    )
    .await
    .unwrap();

    assert_eq!(direct, regular);
    assert_eq!(
        read_manifest_frame(&mut direct.as_slice()).await.unwrap(),
        expected
    );
}

#[tokio::test]
async fn manifest_codec_rejects_cross_family_and_unsafe_offers() {
    let mut single_file_hello = Vec::new();
    crate::write_frame(
        &mut single_file_hello,
        &crate::Frame::Hello(crate::Hello {
            protocol_version: 1,
            role: PeerRole::Sender,
        }),
    )
    .await
    .unwrap();
    assert!(matches!(
        read_manifest_frame(&mut single_file_hello.as_slice()).await,
        Err(CoreError::Protocol(_))
    ));

    let mut manifest_hello = Vec::new();
    write_manifest_frame(
        &mut manifest_hello,
        &ManifestFrame::Hello(ManifestHelloV1 {
            protocol_version: MANIFEST_V1_PROTOCOL_VERSION,
            role: PeerRole::Sender,
        }),
    )
    .await
    .unwrap();
    assert!(matches!(
        crate::read_frame(&mut manifest_hello.as_slice()).await,
        Err(CoreError::Protocol(_))
    ));

    let mut unsafe_manifest = valid_manifest();
    unsafe_manifest.entries[1].relative_path = "Photos/../escape".into();
    let error = write_manifest_frame(
        &mut Vec::new(),
        &ManifestFrame::Offer(ManifestOfferV1 {
            manifest: unsafe_manifest,
            chunk_size: 64 * 1024,
            resume_requested: true,
        }),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, CoreError::Protocol(_)));

    let invalid_result = ManifestFrame::CompleteAck(ManifestCompleteAckV1 {
        manifest_id: ManifestId::new("manifest-1"),
        entries: vec![ManifestEntryResultV1 {
            entry_id: 0,
            status: ManifestEntryResultStatus::Completed,
            offered_relative_path: "file.txt".into(),
            final_relative_path: None,
            failure_code: None,
        }],
    });
    assert!(matches!(
        write_manifest_frame(&mut Vec::new(), &invalid_result).await,
        Err(CoreError::Protocol(_))
    ));
}

#[tokio::test]
async fn decoded_offer_is_revalidated_before_use() {
    let frame = ManifestFrame::Offer(ManifestOfferV1 {
        manifest: valid_manifest(),
        chunk_size: 64 * 1024,
        resume_requested: true,
    });
    let mut encoded = Vec::new();
    write_manifest_frame(&mut encoded, &frame).await.unwrap();
    let path = b"Photos/photo.jpg";
    let position = encoded
        .windows(path.len())
        .position(|window| window == path)
        .expect("fixture path must be encoded verbatim");
    encoded[position] = b'/';

    assert!(matches!(
        read_manifest_frame(&mut encoded.as_slice()).await,
        Err(CoreError::Protocol(_))
    ));
}
