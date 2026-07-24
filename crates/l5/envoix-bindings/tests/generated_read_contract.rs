//! BN1 proofs: the drift gate, the full-surface round-trip, and containment.

use std::num::NonZeroUsize;

use envoix_bindings::read::{
    CapabilityActionView, CardUpdateKindView, CardView, DirectionView, DutyKindView, EpochGate,
    EvidenceValueView, GateDecision, LosslessKindView, OutcomeCodeView, OutcomeView,
    PauseOriginView, PausedView, PhaseView, ProductStateView, QuiescenceView, READ_MAX_FRAME_BYTES,
    READ_SCHEMA_ID, ReadBody, ReadError, ReadFrame, RecoveryView, RedactedIdKindView,
    RedactedIdView, RetirementIntentView, RetiringView, RetryabilityView, RunningView,
    SubscribeRejectionView, WorkerKindView, decode_read_frame, encode_read_frame,
};
use envoix_bindings::{
    FieldTy, build_manifest_frame, card_update_frame, closed_frame, emit, evidence_frame,
    lag_frame, parse_schema, read_schema_text, subscribe_rejected_frame,
};
use envoix_evidence::{
    BUILD_TRUST_MANIFEST, EvidenceRecord, EvidenceSink, EvidenceValue, RedactedId, SessionKey,
    TimelineStore, TrustRootFingerprintSlot,
};
use envoix_outcomes::{Outcome, OutcomeCode, Phase, Retryability, SafeDisplay};
use envoix_runtime::{
    CardUpdateKind, Duty, DutyKind, DutyProvenance, LosslessUpdateKind, SubscribeError,
    TransferRecord,
};
use envoix_types::{AttemptGen, RecordId, RequestId, TransferId};

fn artifacts(doc: &envoix_bindings::SchemaDoc) -> [(&'static str, String); 4] {
    [
        ("generated/rust/read.rs", emit::rust::module(doc)),
        ("generated/dart/envoix_read.dart", emit::dart::module(doc)),
        ("generated/kotlin/EnvoixRead.kt", emit::kotlin::module(doc)),
        ("generated/swift/EnvoixRead.swift", emit::swift::module(doc)),
    ]
}

/// Every checked-in artifact is byte-identical to what the schema emits.
/// `ENVOIX_BINDINGS_REGEN=1` rewrites the artifacts instead.
#[test]
fn generated_artifacts_match_schema() {
    let doc = parse_schema(read_schema_text()).expect("the checked-in schema parses");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    if std::env::var_os("ENVOIX_BINDINGS_REGEN").is_some() {
        for (path, content) in artifacts(&doc) {
            std::fs::write(root.join(path), content).expect("write generated artifact");
        }
        return;
    }
    for (path, content) in artifacts(&doc) {
        let on_disk = std::fs::read_to_string(root.join(path)).expect("read generated artifact");
        assert!(
            on_disk == content,
            "{path} drifted from schema/read.schema; regenerate with ENVOIX_BINDINGS_REGEN=1"
        );
    }

    // The generated tree holds exactly the four artifacts: a rogue extra file
    // would ship unreviewed to native consumers.
    fn walk(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read generated dir") {
            let path = entry.expect("generated dir entry").path();
            if path.is_dir() {
                walk(&path, files);
            } else {
                files.push(path);
            }
        }
    }
    let mut on_disk = Vec::new();
    walk(&root.join("generated"), &mut on_disk);
    let mut on_disk: Vec<String> = on_disk
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .expect("generated path under crate root")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    on_disk.sort();
    let mut expected: Vec<String> = artifacts(&doc)
        .iter()
        .map(|(path, _)| (*path).to_owned())
        .collect();
    expected.sort();
    assert_eq!(
        on_disk, expected,
        "generated/ must contain exactly the four artifacts"
    );
}

/// Fabricates a durable-authority record through its serde contract; the
/// binding never constructs records in production, only projects them.
fn record(
    state: serde_json::Value,
    quiescence: serde_json::Value,
    outcome: serde_json::Value,
) -> TransferRecord {
    serde_json::from_value(serde_json::json!({
        "identity": {
            "card": 18_446_744_073_709_551_615_u64,
            "transfer": "000102030405060708090a0b0c0d0e0f",
            "artifact": "101112131415161718191a1b1c1d1e1f",
        },
        "direction": "receive",
        "offered_name": "quarterly-report.pdf",
        "total": 4096,
        "state": state,
        "quiescence": quiescence,
        "generation": 7,
        "phase": "transferring",
        "bytes": 2048,
        "bytes_resumed": 1024,
        "outcome": outcome,
        "facts": {
            "source_ready": true,
            "complete_sent": false,
            "proof_delivered": false,
            "receipt_mismatch": false,
            "remove_requested": false,
        },
        "source_recoverable": true,
        "receipt_request": "202122232425262728292a2b2c2d2e2f",
    }))
    .expect("fabricate an authority record")
}

fn card() -> RecordId {
    RecordId::new(u64::MAX)
}

fn full_surface_frames() -> Vec<ReadFrame> {
    let running = record(
        serde_json::json!({"state": "transferring"}),
        serde_json::json!({"status": "running", "worker": "attempt"}),
        serde_json::Value::Null,
    );
    let paused = record(
        serde_json::json!({"state": "paused", "origin": "lost"}),
        serde_json::json!({"status": "quiescent"}),
        serde_json::json!({
            "code": "peer_lost",
            "phase": "transferring",
            "retry": "retryable",
            "recovery": "reconnect_peer",
            "display": "connection to the peer was lost",
        }),
    );
    let retiring = record(
        serde_json::json!({"state": "confirming"}),
        serde_json::json!({"status": "retiring", "worker": "staging", "intent": "pause"}),
        serde_json::Value::Null,
    );
    let terminal = record(
        serde_json::json!({"state": "completed"}),
        serde_json::json!({"status": "quiescent"}),
        serde_json::json!({
            "code": "completed",
            "phase": "publishing",
            "retry": "terminal",
            "recovery": null,
            "display": "transfer complete",
        }),
    );
    let duty = Duty {
        provenance: DutyProvenance {
            card: card(),
            generation: AttemptGen::new(7),
            request: RequestId::from_bytes([0x42; 16]),
        },
        kind: DutyKind::Publication,
    };

    let store = TimelineStore::new(
        NonZeroUsize::new(4).expect("nonzero"),
        NonZeroUsize::new(8).expect("nonzero"),
    );
    let session = SessionKey {
        card: card(),
        generation: AttemptGen::new(7),
    };
    let outcome = Outcome::new(
        OutcomeCode::PeerLost,
        Phase::Transferring,
        Retryability::Retryable,
        SafeDisplay::new("peer went away"),
    );
    let values = [
        EvidenceValue::phase(Phase::Transferring),
        EvidenceValue::progress(envoix_evidence::EvidenceProgress::new(
            envoix_types::ByteCount::new(1024),
            envoix_types::ByteCount::new(4096),
        )),
        EvidenceValue::outcome(&outcome),
        EvidenceValue::identifier(RedactedId::transfer(TransferId::from_bytes([1; 16]))),
        EvidenceValue::phase(Phase::Confirming),
        EvidenceValue::phase(Phase::Publishing),
    ];
    for value in values {
        store
            .record(EvidenceRecord::new(session, value))
            .expect("timeline retains");
    }
    let timeline = store.snapshot(session).expect("session retained");

    let mut provisioned = BUILD_TRUST_MANIFEST;
    provisioned.trust_root = TrustRootFingerprintSlot::Sha256([0xab; 32]);

    vec![
        card_update_frame(1, card(), &CardUpdateKind::Snapshot(running.clone())),
        card_update_frame(1, card(), &CardUpdateKind::Progress(running)),
        card_update_frame(1, card(), &CardUpdateKind::State(paused)),
        card_update_frame(2, card(), &CardUpdateKind::State(retiring)),
        card_update_frame(2, card(), &CardUpdateKind::Terminal(terminal)),
        card_update_frame(
            2,
            card(),
            &CardUpdateKind::CapabilityDuty {
                duty,
                action: envoix_runtime::CapabilityAction::PostReceipt,
            },
        ),
        lag_frame(2, card(), LosslessUpdateKind::CapabilityDuty),
        closed_frame(3, card()),
        subscribe_rejected_frame(card(), SubscribeError::UnknownCard),
        evidence_frame(&timeline),
        build_manifest_frame(&BUILD_TRUST_MANIFEST),
        build_manifest_frame(&provisioned),
    ]
}

fn tamper(base: &[u8], mutate: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut value: serde_json::Value = serde_json::from_slice(base).expect("valid base frame");
    mutate(&mut value);
    serde_json::to_vec(&value).expect("re-serialize tampered frame")
}

#[test]
fn generated_read_schema_roundtrip_and_containment() {
    // Round-trip: every read-surface frame survives encode -> decode intact.
    let frames = full_surface_frames();
    for frame in &frames {
        let bytes = encode_read_frame(frame).expect("projected frames encode");
        let decoded = decode_read_frame(&bytes).expect("encoded frames decode");
        assert_eq!(&decoded, frame);
    }

    // A snapshot whose record carries an outcome, as the tamper base.
    let base = encode_read_frame(&frames[4]).expect("terminal frame encodes");

    // The encoder stamps the schema envelope itself: an in-memory frame cannot
    // carry a wrong id, and every encoded frame identifies the contract.
    let stamped: serde_json::Value = serde_json::from_slice(&base).expect("frame json");
    assert_eq!(stamped["schema"], serde_json::json!(READ_SCHEMA_ID));

    // The manifest projection self-describes this read contract.
    let ReadBody::BuildManifest(manifest_view) = &frames[10].body else {
        panic!("build manifest frame expected");
    };
    assert_eq!(
        manifest_view.abi_schema.read_binding_schema_id,
        READ_SCHEMA_ID
    );

    // Every leaf enum variant round-trips, driven by the generated `ALL`
    // arrays so future schema variants are swept automatically.
    let roundtrips = |frame: &ReadFrame| {
        let bytes = encode_read_frame(frame).expect("sweep frame encodes");
        assert_eq!(
            &decode_read_frame(&bytes).expect("sweep frame decodes"),
            frame
        );
    };
    let with_card_view = |frame: &ReadFrame, mutate: &dyn Fn(&mut CardView)| {
        let mut frame = frame.clone();
        let ReadBody::CardUpdate(update) = &mut frame.body else {
            panic!("card update expected");
        };
        match &mut update.kind {
            CardUpdateKindView::Snapshot(view)
            | CardUpdateKindView::Progress(view)
            | CardUpdateKindView::State(view)
            | CardUpdateKindView::Terminal(view) => mutate(view),
            CardUpdateKindView::CapabilityDuty(_) => panic!("card view expected"),
        }
        frame
    };
    for direction in DirectionView::ALL {
        roundtrips(&with_card_view(&frames[4], &|view| {
            view.direction = direction
        }));
    }
    for phase in PhaseView::ALL {
        roundtrips(&with_card_view(&frames[4], &|view| view.phase = phase));
    }
    for code in OutcomeCodeView::ALL {
        roundtrips(&with_card_view(&frames[4], &|view| {
            view.outcome.as_mut().expect("outcome kept").code = code;
        }));
    }
    for retry in RetryabilityView::ALL {
        roundtrips(&with_card_view(&frames[4], &|view| {
            view.outcome.as_mut().expect("outcome kept").retry = retry;
        }));
    }
    for recovery in RecoveryView::ALL {
        roundtrips(&with_card_view(&frames[4], &|view| {
            view.outcome.as_mut().expect("outcome kept").recovery = Some(recovery);
        }));
    }
    for origin in PauseOriginView::ALL {
        roundtrips(&with_card_view(&frames[4], &|view| {
            view.state = ProductStateView::Paused(PausedView { origin });
        }));
    }
    for worker in WorkerKindView::ALL {
        roundtrips(&with_card_view(&frames[4], &|view| {
            view.quiescence = QuiescenceView::Running(RunningView { worker });
        }));
    }
    for intent in RetirementIntentView::ALL {
        roundtrips(&with_card_view(&frames[4], &|view| {
            view.quiescence = QuiescenceView::Retiring(RetiringView {
                worker: WorkerKindView::Attempt,
                intent,
            });
        }));
    }
    for kind in DutyKindView::ALL {
        for action in CapabilityActionView::ALL {
            let mut frame = frames[5].clone();
            let ReadBody::CardUpdate(update) = &mut frame.body else {
                panic!("duty update expected");
            };
            let CardUpdateKindView::CapabilityDuty(duty_frame) = &mut update.kind else {
                panic!("capability duty expected");
            };
            duty_frame.duty.kind = kind;
            duty_frame.action = action;
            roundtrips(&frame);
        }
    }
    for missed in LosslessKindView::ALL {
        let mut frame = frames[6].clone();
        let ReadBody::Lag(lag) = &mut frame.body else {
            panic!("lag frame expected");
        };
        lag.missed = missed;
        roundtrips(&frame);
    }
    for reason in SubscribeRejectionView::ALL {
        let mut frame = frames[8].clone();
        let ReadBody::SubscribeRejected(rejected) = &mut frame.body else {
            panic!("subscribe rejection expected");
        };
        rejected.reason = reason;
        roundtrips(&frame);
    }
    for kind in RedactedIdKindView::ALL {
        let mut frame = frames[9].clone();
        let ReadBody::Evidence(timeline) = &mut frame.body else {
            panic!("evidence frame expected");
        };
        timeline.entries[0].value = EvidenceValueView::Identifier(RedactedIdView { kind });
        roundtrips(&frame);
    }

    // Unknown or missing schema versions fail explicitly.
    let future = tamper(&base, |value| {
        value["schema"] = serde_json::json!("envoix/binding/read/2");
    });
    assert_eq!(decode_read_frame(&future), Err(ReadError::UnknownSchema));
    let missing = tamper(&base, |value| {
        value
            .as_object_mut()
            .expect("frame object")
            .remove("schema");
    });
    assert_eq!(
        decode_read_frame(&missing),
        Err(ReadError::Shape {
            context: "ReadFrame.schema"
        })
    );

    // Unknown union kinds and unknown fields fail explicitly.
    let unknown_kind = tamper(&base, |value| {
        value["body"]["kind"] = serde_json::json!("telemetry");
    });
    assert!(matches!(
        decode_read_frame(&unknown_kind),
        Err(ReadError::UnknownVariant { .. })
    ));
    let unknown_field = tamper(&base, |value| {
        value["body"]["value"]["kind"]["value"]["path"] = serde_json::json!("/etc/passwd");
    });
    assert!(matches!(
        decode_read_frame(&unknown_field),
        Err(ReadError::UnknownField { .. })
    ));

    // Numeric ranges are enforced: u63 overflow, u32 overflow, negatives.
    let overflow = tamper(&base, |value| {
        value["body"]["value"]["kind"]["value"]["total"] = serde_json::json!(u64::MAX);
    });
    assert!(matches!(
        decode_read_frame(&overflow),
        Err(ReadError::Range { .. })
    ));
    let generation_overflow = tamper(&base, |value| {
        value["body"]["value"]["kind"]["value"]["generation"] =
            serde_json::json!(4_294_967_296_u64);
    });
    assert!(matches!(
        decode_read_frame(&generation_overflow),
        Err(ReadError::Range { .. })
    ));
    let negative = tamper(&base, |value| {
        value["body"]["value"]["epoch"] = serde_json::json!(-5);
    });
    assert!(matches!(
        decode_read_frame(&negative),
        Err(ReadError::Shape { .. })
    ));

    // Identifier hex is fixed-length lowercase.
    let bad_hex = tamper(&base, |value| {
        value["body"]["value"]["card"] = serde_json::json!("FFFF");
    });
    assert!(matches!(
        decode_read_frame(&bad_hex),
        Err(ReadError::Bound { .. })
    ));

    // String bounds hold.
    let oversized_display = tamper(&base, |value| {
        value["body"]["value"]["kind"]["value"]["outcome"]["display"] =
            serde_json::json!("x".repeat(200));
    });
    assert!(matches!(
        decode_read_frame(&oversized_display),
        Err(ReadError::Bound { .. })
    ));

    // Unit union variants reject payloads.
    let unit_payload = tamper(&base, |value| {
        value["body"]["value"]["kind"]["value"]["quiescence"] =
            serde_json::json!({"kind": "quiescent", "value": {}});
    });
    assert!(matches!(
        decode_read_frame(&unit_payload),
        Err(ReadError::Shape { .. })
    ));

    // List bounds hold: 1025 timeline entries exceed the schema bound.
    let evidence = encode_read_frame(&frames[9]).expect("evidence frame encodes");
    let oversized_list = tamper(&evidence, |value| {
        let entry = serde_json::json!({
            "sequence": 1,
            "value": {"kind": "phase", "value": "pairing"},
        });
        value["body"]["value"]["entries"] = serde_json::Value::Array(vec![entry; 1025]);
    });
    assert!(matches!(
        decode_read_frame(&oversized_list),
        Err(ReadError::Bound { .. })
    ));

    // Malformed, truncated, and oversized inputs fail without panicking.
    assert_eq!(
        decode_read_frame(&base[..base.len() / 2]),
        Err(ReadError::MalformedJson)
    );
    assert_eq!(
        decode_read_frame(b"not json"),
        Err(ReadError::MalformedJson)
    );
    assert_eq!(
        decode_read_frame(&vec![b' '; READ_MAX_FRAME_BYTES + 1]),
        Err(ReadError::FrameTooLarge)
    );

    // Deterministic fuzz-ish sweep: byte mutations and truncations of valid
    // frames never panic, whatever they decode to.
    let mut seed: u64 = 0x0123_4567_89ab_cdef;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for base_frame in [&base, &evidence] {
        for _ in 0..2000 {
            let mut mutated = base_frame.clone();
            let index = (next() as usize) % mutated.len();
            mutated[index] ^= (next() as u8) | 1;
            let _ = decode_read_frame(&mutated);
            let cut = (next() as usize) % mutated.len();
            let _ = decode_read_frame(&mutated[..cut]);
        }
    }

    // The encoder enforces the same bounds: a hand-built frame that bypasses
    // the projection cannot smuggle an over-bound value out.
    let mut oversized = frames[4].clone();
    if let ReadBody::CardUpdate(update) = &mut oversized.body
        && let CardUpdateKindView::Terminal(card_view) = &mut update.kind
    {
        card_view.outcome = Some(OutcomeView {
            code: OutcomeCodeView::Internal,
            phase: PhaseView::Publishing,
            retry: RetryabilityView::Terminal,
            recovery: None,
            display: "x".repeat(200),
        });
    }
    assert!(matches!(
        encode_read_frame(&oversized),
        Err(ReadError::Bound { .. })
    ));

    // Projection truncation: over-bound display and offered names land on
    // char boundaries within the codec bounds.
    let long_display = Outcome::new(
        OutcomeCode::Internal,
        Phase::Publishing,
        Retryability::Terminal,
        SafeDisplay::new("é".repeat(120)),
    );
    let truncated = record(
        serde_json::json!({"state": "failed"}),
        serde_json::json!({"status": "quiescent"}),
        serde_json::to_value(&long_display).expect("outcome serializes"),
    );
    let frame = card_update_frame(1, card(), &CardUpdateKind::Terminal(truncated));
    let bytes = encode_read_frame(&frame).expect("truncated projection encodes");
    let decoded = decode_read_frame(&bytes).expect("truncated projection decodes");
    if let ReadBody::CardUpdate(update) = &decoded.body {
        if let CardUpdateKindView::Terminal(card_view) = &update.kind {
            let display = &card_view.outcome.as_ref().expect("outcome kept").display;
            assert!(display.len() <= 160);
            assert!(display.chars().all(|c| c == 'é'));
        } else {
            panic!("terminal update expected");
        }
    } else {
        panic!("card update expected");
    }

    // The schema grammar itself cannot express bulk bytes or OS handles:
    // the scalar vocabulary is closed and every string/list is bounded.
    let doc = parse_schema(read_schema_text()).expect("schema parses");
    fn assert_contained(ty: &FieldTy) {
        match ty {
            FieldTy::U16 | FieldTy::U32 | FieldTy::U63 => {}
            FieldTy::Hex16 | FieldTy::Hex32 | FieldTy::Hex64 => {}
            FieldTy::HexVar { max_chars } => assert!(*max_chars > 0),
            FieldTy::Str { max_bytes } | FieldTy::Ascii { max_bytes } => {
                assert!(*max_bytes > 0 && *max_bytes <= 1024);
            }
            FieldTy::Named(_) => {}
            FieldTy::Option(inner) => assert_contained(inner),
            FieldTy::List { element, max_len } => {
                assert!(*max_len > 0);
                assert_contained(element);
            }
        }
    }
    for decl in &doc.decls {
        if let envoix_bindings::Decl::Struct(decl) = decl {
            for field in &decl.fields {
                assert_contained(&field.ty);
            }
        }
    }
}

#[test]
fn epoch_gate_enforces_reattach_contract() {
    let frames = full_surface_frames();
    let snapshot = &frames[0];
    let progress = &frames[1];
    let lag = &frames[6];
    let manifest = &frames[10];

    let mut gate = EpochGate::attach(1);
    assert_eq!(gate.admit(progress), GateDecision::ContractBreach);
    assert_eq!(gate.admit(snapshot), GateDecision::Deliver);
    assert_eq!(gate.admit(progress), GateDecision::Deliver);
    assert_eq!(gate.admit(snapshot), GateDecision::ContractBreach);
    assert_eq!(gate.admit(manifest), GateDecision::Deliver);

    // Frames from another epoch are stale, including its lag.
    let mut stale_gate = EpochGate::attach(9);
    assert_eq!(stale_gate.admit(snapshot), GateDecision::DropStale);
    assert_eq!(stale_gate.admit(lag), GateDecision::DropStale);

    // A lag for the current epoch delivers once and kills the epoch.
    let mut lagged = EpochGate::attach(2);
    assert_eq!(lagged.admit(lag), GateDecision::Deliver);
    assert_eq!(lagged.admit(lag), GateDecision::DropStale);

    // A close for the current epoch does the same.
    let mut closed_gate = EpochGate::attach(3);
    assert_eq!(closed_gate.admit(&frames[7]), GateDecision::Deliver);
    assert_eq!(closed_gate.admit(&frames[7]), GateDecision::DropStale);
}

#[test]
fn schema_parser_rejects_unbounded_or_malformed_grammar() {
    let minimal = |body: &str| {
        format!(
            "id = \"envoix/binding/read/1\"\nroot = \"ReadFrame\"\n\n[limits]\nmax_frame_bytes = 1024\n\n{body}"
        )
    };
    let valid = minimal(
        "[[decl]]\nkind = \"union\"\nname = \"ReadBody\"\nvariants = [{ name = \"closed\" }]\n\n\
         [[decl]]\nkind = \"struct\"\nname = \"ReadFrame\"\nfields = [\n\
         \x20 { name = \"schema\", type = \"ascii(64)\" },\n\
         \x20 { name = \"body\", type = \"ReadBody\" },\n]\n",
    );
    assert!(parse_schema(&valid).is_ok());

    // Unbounded or unknown types cannot be declared.
    for bad_type in ["str", "bytes", "blob", "handle", "uri", "list(ReadBody)"] {
        let body = format!(
            "[[decl]]\nkind = \"union\"\nname = \"ReadBody\"\nvariants = [{{ name = \"closed\" }}]\n\n\
             [[decl]]\nkind = \"struct\"\nname = \"ReadFrame\"\nfields = [\n\
             \x20 {{ name = \"schema\", type = \"ascii(64)\" }},\n\
             \x20 {{ name = \"body\", type = \"{bad_type}\" }},\n]\n"
        );
        assert!(
            parse_schema(&minimal(&body)).is_err(),
            "type {bad_type} must be rejected"
        );
    }

    // Member names every target language cannot represent are rejected:
    // trailing underscores (the Dart reserved-word rename), consecutive
    // underscores (camel-case ambiguity), and Rust's un-escapable keywords.
    for bad_member in ["in_", "a__b", "self", "crate", "super"] {
        let body = format!(
            "[[decl]]\nkind = \"union\"\nname = \"ReadBody\"\nvariants = [{{ name = \"closed\" }}]\n\n\
             [[decl]]\nkind = \"struct\"\nname = \"ReadFrame\"\nfields = [\n\
             \x20 {{ name = \"schema\", type = \"ascii(64)\" }},\n\
             \x20 {{ name = \"{bad_member}\", type = \"ReadBody\" }},\n]\n"
        );
        assert!(
            parse_schema(&minimal(&body)).is_err(),
            "member name {bad_member} must be rejected"
        );
    }

    // The root struct must lead with the ascii schema envelope field.
    let schema_not_first = minimal(
        "[[decl]]\nkind = \"union\"\nname = \"ReadBody\"\nvariants = [{ name = \"closed\" }]\n\n\
         [[decl]]\nkind = \"struct\"\nname = \"ReadFrame\"\nfields = [\n\
         \x20 { name = \"body\", type = \"ReadBody\" },\n\
         \x20 { name = \"schema\", type = \"ascii(64)\" },\n]\n",
    );
    assert!(parse_schema(&schema_not_first).is_err());

    // Forward references, duplicates, and a missing id are rejected.
    assert!(parse_schema("root = \"R\"\n[limits]\nmax_frame_bytes = 1\n").is_err());
    let forward = minimal(
        "[[decl]]\nkind = \"struct\"\nname = \"ReadFrame\"\nfields = [\n\
         \x20 { name = \"schema\", type = \"ascii(64)\" },\n\
         \x20 { name = \"body\", type = \"ReadBody\" },\n]\n\n\
         [[decl]]\nkind = \"union\"\nname = \"ReadBody\"\nvariants = [{ name = \"closed\" }]\n",
    );
    assert!(
        parse_schema(&forward).is_err(),
        "forward references rejected"
    );
    assert!(parse_schema("not toml [").is_err());
}

/// The native artifacts are real generated code, not stubs, and carry the
/// schema id so a mismatched consumer fails loudly.
#[test]
fn native_artifacts_carry_schema_and_types() {
    let doc = parse_schema(read_schema_text()).expect("schema parses");
    for (path, content) in artifacts(&doc) {
        assert!(content.contains("envoix/binding/read/1"), "{path}");
        assert!(!content.contains("TODO"), "{path}");
    }
    let dart = emit::dart::module(&doc);
    assert!(dart.contains("sealed class ReadBody"));
    assert!(dart.contains("ReadFrame decodeReadFrame(String text)"));
    let kotlin = emit::kotlin::module(&doc);
    assert!(kotlin.contains("sealed interface ReadBody"));
    assert!(kotlin.contains("fun decode(text: String): ReadFrame"));
    let swift = emit::swift::module(&doc);
    assert!(swift.contains("public enum ReadBody"));
    assert!(swift.contains("public static func decode(_ data: Data) throws -> ReadFrame"));
}
