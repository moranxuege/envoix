//! BN1 proofs: the drift gate, the full-surface round-trip, and containment.

use std::num::NonZeroUsize;

use envoix_bindings::capability::CAPABILITY_SCHEMA_ID;
use envoix_bindings::command::COMMAND_SCHEMA_ID;
use envoix_bindings::read::{
    AbiSchemaManifestView, CapabilityActionView, CardUpdateKindView, CardView, DirectionView,
    DutyKindView, EpochGate, EvidenceValueView, GateDecision, LosslessKindView, OutcomeCodeView,
    OutcomeView, PauseOriginView, PausedView, PhaseView, ProductStateView, QuiescenceView,
    READ_MAX_FRAME_BYTES, READ_SCHEMA_ID, ReadBody, ReadError, ReadFrame, RecoveryView,
    RedactedIdKindView, RedactedIdView, RetirementIntentView, RetiringView, RetryabilityView,
    RunningView, SubscribeRejectionView, WorkerKindView, decode_read_frame, encode_read_frame,
};
use envoix_bindings::{
    FieldTy, build_manifest_frame, card_update_frame, closed_frame, command_schema_text, emit,
    evidence_frame, lag_frame, parse_schema, read_schema_text, subscribe_rejected_frame,
};
use envoix_evidence::{
    BUILD_TRUST_MANIFEST, EvidenceRecord, EvidenceSink, EvidenceValue, RedactedId, SessionKey,
    TimelineStore,
};
use envoix_outcomes::{Outcome, OutcomeCode, Phase, Retryability, SafeDisplay};
use envoix_runtime::{
    CardUpdateKind, Duty, DutyKind, DutyProvenance, LosslessUpdateKind, MAX_BROKER_LENGTH,
    MAX_INVITE_LINK_LENGTH, MAX_RELAY_LENGTH, MAX_ROOM_CODE_LENGTH, SubscribeError, TransferRecord,
};
use envoix_types::{AttemptGen, OfferedName, RecordId, RequestId, TransferId};

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

    // The generated tree holds exactly the artifacts of the four schemas: a
    // rogue extra file would ship unreviewed to native consumers.
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
    let command_artifacts = [
        "generated/rust/command.rs",
        "generated/dart/envoix_command.dart",
        "generated/kotlin/EnvoixCommand.kt",
        "generated/swift/EnvoixCommand.swift",
        "generated/rust/capability.rs",
        "generated/dart/envoix_capability.dart",
        "generated/kotlin/EnvoixCapability.kt",
        "generated/swift/EnvoixCapability.swift",
        "generated/rust/duty.rs",
        "generated/dart/envoix_duty.dart",
        "generated/kotlin/EnvoixDuty.kt",
        "generated/swift/EnvoixDuty.swift",
    ];
    let mut expected: Vec<String> = artifacts(&doc)
        .iter()
        .map(|(path, _)| (*path).to_owned())
        .chain(command_artifacts.iter().map(|path| (*path).to_owned()))
        .collect();
    expected.sort();
    assert_eq!(
        on_disk, expected,
        "generated/ must contain exactly the read, command, capability and duty artifacts"
    );
}

/// Fabricates a durable-authority record through its serde contract; the
/// binding never constructs records in production, only projects them.
///
/// The card's published channel, exercised at the widest the grammar admits:
/// an invite the authority holds REACHES the frontend, whole, however long the
/// grammar let it be. F2b shipped the opposite — a 1 KiB bound restated from
/// nowhere meant a legal invite over that length was published as absent, so
/// the sender saw no link, could not share it, and nothing errored anywhere.
///
/// The bound is the grammar's own now, so the only absence left is a channel
/// that no longer spells an invite at all.
#[test]
fn an_invite_reaches_the_frontend_however_long_the_grammar_let_it_be() {
    // Built the way this suite builds every other durable value: from the
    // record's own encoding. L5 sees the channel through L4's facade and has no
    // dependency on the invite grammar, which is the point — the projection
    // calls the channel's own encoder rather than re-implementing one.
    let channel = |broker: &str, relay: &str| -> envoix_runtime::PairingChannel {
        serde_json::from_value(serde_json::json!({
            "code": "000123-amber-brass",
            "broker": broker,
            "relay": relay,
            "role": "send",
        }))
        .expect("a well-formed channel")
    };
    let published = |broker: &str, relay: &str| {
        let mut held = record(
            serde_json::json!({"state": "waiting"}),
            serde_json::json!({"status": "quiescent"}),
            serde_json::Value::Null,
        );
        held.pairing = Some(Box::new(channel(broker, relay)));
        let frame = card_update_frame(1, card(), &CardUpdateKind::Snapshot(held));
        let ReadBody::CardUpdate(update) = frame.body else {
            panic!("card update expected");
        };
        let CardUpdateKindView::Snapshot(view) = update.kind else {
            panic!("snapshot expected");
        };
        view.invite.expect("a channel publishes an invite")
    };

    let short = published("broker.example", "relay.example");
    let link = short.link.expect("a short channel publishes its link");
    assert!(
        link.expose().starts_with("envoix://"),
        "the link is the grammar's own encoding, not a fragment"
    );
    assert_eq!(short.code.expose(), "000123-amber-brass");
    assert_eq!(
        short.code_fingerprint,
        blake3::hash(short.code.expose().as_bytes()).to_hex()[..16],
        "instrumentation gets a stable digest prefix, never the SPAKE2 password"
    );

    // The widest endpoints the grammar admits — an invite far past the bound
    // F2b restated, and one the frontend must still be handed whole.
    let widest = published(&"b".repeat(1024), &"r".repeat(2048));
    let link = widest.link.expect("a maximal channel publishes its link");
    assert!(
        link.expose().len() > 1024 && link.expose().len() <= MAX_INVITE_LINK_LENGTH,
        "a maximal invite is {} bytes and must cross whole",
        link.expose().len()
    );
    assert_eq!(
        widest.code.expose(),
        "000123-amber-brass",
        "the code crosses beside it"
    );

    // And it survives the codec: the schema's bound admits what the grammar
    // emits, so the frame carrying it encodes and decodes byte for byte.
    let mut held = record(
        serde_json::json!({"state": "waiting"}),
        serde_json::json!({"status": "quiescent"}),
        serde_json::Value::Null,
    );
    held.pairing = Some(Box::new(channel(&"b".repeat(1024), &"r".repeat(2048))));
    let frame = card_update_frame(1, card(), &CardUpdateKind::Snapshot(held));
    let bytes = encode_read_frame(&frame).expect("a maximal invite encodes");
    let decoded = decode_read_frame(&bytes).expect("a maximal invite decodes");
    let ReadBody::CardUpdate(update) = decoded.body else {
        panic!("card update expected");
    };
    let CardUpdateKindView::Snapshot(view) = update.kind else {
        panic!("snapshot expected");
    };
    assert_eq!(
        view.invite.expect("the invite survives the codec").link,
        Some(link)
    );

    // The one absence left: fields that no longer spell an invite.
    let unspellable = published("broker.example", " relay.example ");
    assert_eq!(
        unspellable.link, None,
        "a channel the grammar cannot re-read has no link to publish"
    );
}

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
        // A receiver needs no source, and record v5 makes that explicit rather
        // than leaving it to be inferred from the direction.
        "source": { "not_required": { "peer_content": null } },
        "participation": "minted",
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

    // The manifest projection self-describes both generated contracts.
    let ReadBody::BuildManifest(manifest_view) = &frames[10].body else {
        panic!("build manifest frame expected");
    };
    assert_eq!(
        manifest_view.abi_schema.read_binding_schema_id,
        READ_SCHEMA_ID
    );
    assert_eq!(
        manifest_view.abi_schema.command_binding_schema_id,
        COMMAND_SCHEMA_ID
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
        value["schema"] = serde_json::json!("envoix/binding/read/9");
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

    // The schema grammar itself cannot express bulk bytes or OS handles: the
    // scalar vocabulary is closed and every string/list is bounded.
    //
    // A string's bound is one of two things and never a third. Either it is
    // this contract's own containment choice about text it MINTS — schema ids,
    // a package version, a protocol set id — and 1 KiB is more of that than any
    // observer renders; or the value belongs to another layer, and then the
    // bound must EQUAL that layer's published maximum. The second kind is where
    // F2b shipped a silent failure: a 1 KiB link bound restated a grammar whose
    // encoder emits up to 5481 bytes, so every longer invite the authority held
    // was published as absent. Equality is what makes that unrepresentable
    // rather than gated.
    //
    // The FALLBACK is the rule, not a detail of it. A field nobody classified
    // is not contract-local, it is UNEXAMINED — and "unexamined" defaulting to
    // "ours" is exactly how `display` and `offered_name` sat here restating two
    // L0 facts whose owner had never published them. So an unclassified text
    // bound stops the build and asks which of the two it is.
    const CONTRACT_LOCAL_TEXT_BYTES: u32 = 1024;
    enum TextBound {
        /// Another layer's value: this bound must be that layer's maximum.
        Derived(usize),
        /// Text this contract mints, so the bound is this contract's call.
        ContractLocal,
    }
    let classified: [(&str, &str, TextBound); 17] = [
        (
            "OutcomeView",
            "display",
            TextBound::Derived(SafeDisplay::MAX_BYTES),
        ),
        (
            "InviteView",
            "code",
            TextBound::Derived(MAX_ROOM_CODE_LENGTH),
        ),
        (
            "InviteView",
            "link",
            TextBound::Derived(MAX_INVITE_LINK_LENGTH),
        ),
        (
            "CardView",
            "offered_name",
            TextBound::Derived(OfferedName::MAX_BYTES),
        ),
        ("ProtocolManifestView", "set_id", TextBound::ContractLocal),
        (
            "AbiSchemaManifestView",
            "read_binding_schema_id",
            TextBound::ContractLocal,
        ),
        (
            "AbiSchemaManifestView",
            "command_binding_schema_id",
            TextBound::ContractLocal,
        ),
        (
            "AbiSchemaManifestView",
            "capability_binding_schema_id",
            TextBound::ContractLocal,
        ),
        (
            "AbiSchemaManifestView",
            "evidence_rust_abi_id",
            TextBound::ContractLocal,
        ),
        (
            "AbiSchemaManifestView",
            "evidence_timeline_schema_id",
            TextBound::ContractLocal,
        ),
        (
            "AbiSchemaManifestView",
            "mailbox_receipt_schema_id",
            TextBound::ContractLocal,
        ),
        (
            "AbiSchemaManifestView",
            "operation_envelope_schema_id",
            TextBound::ContractLocal,
        ),
        (
            "BuildManifestView",
            "package_version",
            TextBound::ContractLocal,
        ),
        (
            "DeploymentManifestView",
            "environment",
            TextBound::ContractLocal,
        ),
        // These two ARE the broker and relay of every invite this build mints,
        // so they are sized by the grammar that owns invites rather than by a
        // number this contract chose.
        (
            "DeploymentManifestView",
            "rendezvous_endpoint",
            TextBound::Derived(MAX_BROKER_LENGTH),
        ),
        (
            "DeploymentManifestView",
            "relay_url",
            TextBound::Derived(MAX_RELAY_LENGTH),
        ),
        ("ReadFrame", "schema", TextBound::ContractLocal),
    ];
    let doc = parse_schema(read_schema_text()).expect("schema parses");
    /// Returns how many text bounds this type carries, so the caller can prove
    /// the classification covers every one of them and nothing else.
    fn assert_contained(ty: &FieldTy, bound: Option<&TextBound>, context: &str) -> usize {
        match ty {
            FieldTy::U16 | FieldTy::U32 | FieldTy::U63 => 0,
            FieldTy::Hex16 | FieldTy::Hex32 | FieldTy::Hex64 => 0,
            FieldTy::HexVar { max_chars } => {
                assert!(*max_chars > 0);
                0
            }
            FieldTy::Str { max_bytes } | FieldTy::Ascii { max_bytes } => {
                match bound {
                    Some(TextBound::Derived(published)) => assert_eq!(
                        *max_bytes as usize, *published,
                        "{context} carries another layer's value, so its bound must be that \
                         layer's published maximum"
                    ),
                    Some(TextBound::ContractLocal) => assert!(
                        *max_bytes > 0 && *max_bytes <= CONTRACT_LOCAL_TEXT_BYTES,
                        "{context}: a contract-local bound above {CONTRACT_LOCAL_TEXT_BYTES} \
                         bytes is a number this contract invented about somebody else's data"
                    ),
                    None => panic!(
                        "{context} carries a text bound nobody classified: say whether it is \
                         text this contract mints or another layer's published maximum"
                    ),
                }
                1
            }
            FieldTy::Named(_) => 0,
            FieldTy::Option(inner) => assert_contained(inner, bound, context),
            FieldTy::List { element, max_len } => {
                assert!(*max_len > 0);
                assert_contained(element, bound, context)
            }
        }
    }
    let mut judged = 0;
    for decl in &doc.decls {
        if let envoix_bindings::Decl::Struct(decl) = decl {
            for field in &decl.fields {
                let bound = classified
                    .iter()
                    .find(|(owner, name, _)| *owner == decl.name && *name == field.name)
                    .map(|(_, _, bound)| bound);
                judged +=
                    assert_contained(&field.ty, bound, &format!("{}.{}", decl.name, field.name));
            }
        }
    }
    assert_eq!(
        judged,
        classified.len(),
        "every classification must name a field that carries a text bound"
    );
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

/// Duplicate JSON object keys are rejected outright, never resolved
/// last-wins — the read decoder shares the strict parse with the hostile
/// command boundary (one algorithm, both schemas).
#[test]
fn duplicate_json_keys_are_rejected() {
    let base = encode_read_frame(&full_surface_frames().remove(0)).expect("base frame encodes");
    let text = String::from_utf8(base).expect("utf8 frame");
    let dup_schema = text.replacen(
        "\"schema\":",
        "\"schema\":\"envoix/binding/read/2\",\"schema\":",
        1,
    );
    assert_ne!(dup_schema, text, "schema key found");
    assert_eq!(
        decode_read_frame(dup_schema.as_bytes()),
        Err(ReadError::MalformedJson)
    );
}

#[test]
fn schema_parser_rejects_unbounded_or_malformed_grammar() {
    let minimal = |body: &str| {
        format!(
            "id = \"envoix/binding/read/2\"\nroot = \"ReadFrame\"\ndirection = \"host_to_frontend\"\n\n[limits]\nmax_frame_bytes = 1024\n\n{body}"
        )
    };
    let valid = minimal(
        "[[decl]]\nkind = \"union\"\nname = \"ReadBody\"\nvariants = [{ name = \"closed\" }]\n\n\
         [[decl]]\nkind = \"struct\"\nname = \"ReadFrame\"\nfields = [\n\
         \x20 { name = \"schema\", type = \"ascii(64)\" },\n\
         \x20 { name = \"body\", type = \"ReadBody\" },\n]\n",
    );
    assert!(parse_schema(&valid).is_ok());

    // Every contract states who originates its frames: the emitters read the
    // direction to decide which entry points an artifact carries, so an
    // unstated or unknown one is a parse error, never a silent decode-only
    // default.
    assert!(
        parse_schema(&valid.replace("direction = \"host_to_frontend\"\n", "")).is_err(),
        "an unstated direction must be rejected"
    );
    assert!(
        parse_schema(&valid.replace("host_to_frontend", "outbound")).is_err(),
        "an unknown direction must be rejected"
    );
    // Origination is per union arm. A bidirectional contract names exactly one
    // frontend-originated body, and that arm's payload is the only type the
    // native artifacts can encode — so a forged observation has no encoder to
    // call rather than a runtime check saying it must not.
    let payload_decl = "[[decl]]\nkind = \"struct\"\nname = \"OpenView\"\n\
                        fields = [{ name = \"card\", type = \"hex16\" }]\n\n";
    let bidirectional = |arms: &str| {
        minimal(&format!(
            "{payload_decl}[[decl]]\nkind = \"union\"\nname = \"ReadBody\"\nvariants = [{arms}]\n\n\
             [[decl]]\nkind = \"struct\"\nname = \"ReadFrame\"\nfields = [\n\
             \x20 {{ name = \"schema\", type = \"ascii(64)\" }},\n\
             \x20 {{ name = \"body\", type = \"ReadBody\" }},\n]\n"
        ))
        .replace("host_to_frontend", "bidirectional")
    };
    let originated = bidirectional(
        "{ name = \"open\", payload = \"OpenView\", originator = \"frontend\" }, { name = \"closed\" }",
    );
    let doc = parse_schema(&originated).expect("an originated contract parses");
    assert_eq!(doc.direction, envoix_bindings::Direction::Bidirectional);
    let body = doc.frontend_body().expect("the originated body resolves");
    assert_eq!(
        (body.field, body.variant, body.payload),
        ("body", "open", "OpenView")
    );
    for (label, arms) in [
        ("no originated arm", "{ name = \"closed\" }"),
        (
            "two originated arms",
            "{ name = \"open\", payload = \"OpenView\", originator = \"frontend\" }, \
             { name = \"again\", payload = \"OpenView\", originator = \"frontend\" }",
        ),
        (
            "an originated unit arm",
            "{ name = \"open\", originator = \"frontend\" }",
        ),
        (
            "an unknown originator",
            "{ name = \"open\", payload = \"OpenView\", originator = \"host\" }",
        ),
    ] {
        assert!(
            parse_schema(&bidirectional(arms)).is_err(),
            "{label} must be rejected"
        );
    }
    assert!(
        parse_schema(&originated.replace("bidirectional", "host_to_frontend")).is_err(),
        "an observe-only contract must originate nothing"
    );
    // The originated payload is the frame's whole body, so the encoder's
    // argument type IS the arm it may originate.
    assert!(
        parse_schema(&originated.replace(
            "  { name = \"body\", type = \"ReadBody\" },\n",
            "  { name = \"body\", type = \"ReadBody\" },\n  { name = \"extra\", type = \"hex16\" },\n"
        ))
        .is_err(),
        "a second body field must be rejected"
    );

    // Foundation's `.sortedKeys` is a collation, not a byte sort, so an
    // encode-direction schema whose key set could order differently there is
    // rejected rather than shipped under a false byte-identity claim.
    let collide =
        |keys: &str| originated.replace("fields = [{ name = \"card\", type = \"hex16\" }]", keys);
    for keys in [
        "fields = [{ name = \"a0b\", type = \"hex16\" }, { name = \"a_b\", type = \"hex16\" }]",
        "fields = [{ name = \"a2\", type = \"hex16\" }, { name = \"a10\", type = \"hex16\" }]",
    ] {
        assert!(
            parse_schema(&collide(keys)).is_err(),
            "collation-unstable keys must be rejected on an encode-direction contract"
        );
        // The claim exists only where a native encodes, so the same key set is
        // legal on an observe-only contract.
        let observe_only = collide(keys)
            .replace(", originator = \"frontend\"", "")
            .replace("bidirectional", "host_to_frontend");
        assert!(
            parse_schema(&observe_only).is_ok(),
            "the collation rule is scoped to contracts a native encodes"
        );
    }

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

    // Names whose emitted forms match a naming-scaffold token would be
    // silently renamed in some languages but not others.
    for scaffold_member in ["read_schema_id", "read_max_frame_bytes"] {
        let body = format!(
            "[[decl]]\nkind = \"union\"\nname = \"ReadBody\"\nvariants = [{{ name = \"closed\" }}]\n\n\
             [[decl]]\nkind = \"struct\"\nname = \"ReadFrame\"\nfields = [\n\
             \x20 {{ name = \"schema\", type = \"ascii(64)\" }},\n\
             \x20 {{ name = \"{scaffold_member}\", type = \"ascii(8)\" }},\n\
             \x20 {{ name = \"body\", type = \"ReadBody\" }},\n]\n"
        );
        assert!(
            parse_schema(&minimal(&body)).is_err(),
            "scaffold member {scaffold_member} must be rejected"
        );
    }
    let scaffold_decl = minimal(
        "[[decl]]\nkind = \"union\"\nname = \"ReadError\"\nvariants = [{ name = \"closed\" }]\n\n\
         [[decl]]\nkind = \"struct\"\nname = \"ReadFrame\"\nfields = [\n\
         \x20 { name = \"schema\", type = \"ascii(64)\" },\n\
         \x20 { name = \"body\", type = \"ReadError\" },\n]\n",
    );
    assert!(
        parse_schema(&scaffold_decl).is_err(),
        "scaffold decl name must be rejected"
    );

    // Malformed schema ids are parse errors, never a silent artifact stem.
    for bad_id in [
        "evil/binding/read/1",
        "envoix/binding/Read/1",
        "envoix/binding//1",
        "envoix/binding/read/x",
        "envoix/binding/read",
        "envoix/binding/read/2/extra",
    ] {
        let bad = format!(
            "id = \"{bad_id}\"\nroot = \"ReadFrame\"\ndirection = \"host_to_frontend\"\n\n[limits]\nmax_frame_bytes = 1024\n\n\
             [[decl]]\nkind = \"union\"\nname = \"ReadBody\"\nvariants = [{{ name = \"closed\" }}]\n\n\
             [[decl]]\nkind = \"struct\"\nname = \"ReadFrame\"\nfields = [\n\
             \x20 {{ name = \"schema\", type = \"ascii(64)\" }},\n\
             \x20 {{ name = \"body\", type = \"ReadBody\" }},\n]\n"
        );
        assert!(parse_schema(&bad).is_err(), "id {bad_id} must be rejected");
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
        // Taken from the schema rather than written out: the artifact is
        // generated from that id, and a literal here only rots at the next bump.
        assert!(content.contains(&doc.id), "{path}");
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

/// F2a: the read contract's `allowed_actions` is an OFFER, and the command
/// contract is what the frontend may then send. A command the read side offers
/// but the command side cannot express is an affordance that encodes to
/// nothing; a command the command side accepts but the read side never offers
/// is one a compliant frontend can never reach. Neither is representable while
/// the two vocabularies are equal, and they live in two schema files precisely
/// because the generator has no cross-schema reference — so the equality is
/// asserted here, from the schema sources themselves, in both directions.
#[test]
fn the_read_contract_publishes_every_command_a_frontend_can_send() {
    let variants = |text: &str, name: &str| -> Vec<String> {
        let doc = parse_schema(text).expect("schema parses");
        doc.decls
            .iter()
            .find_map(|decl| match decl {
                envoix_bindings::Decl::Enum(decl) if decl.name == name => {
                    Some(decl.variants.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name} is an enum of this schema"))
    };
    let offered = variants(read_schema_text(), "CommandKindView");
    let sendable = variants(command_schema_text(), "CommandView");
    assert_eq!(
        offered, sendable,
        "the read contract's offer and the command contract's vocabulary drifted"
    );
    // Vacuity: an equality between two empty lists proves nothing, and the
    // bound on `CardView.allowed_actions` has to admit the whole set.
    assert_eq!(offered.len(), 5, "{offered:?}");
    let card = parse_schema(read_schema_text())
        .expect("schema parses")
        .decls
        .iter()
        .find_map(|decl| match decl {
            envoix_bindings::Decl::Struct(decl) if decl.name == "CardView" => Some(decl.clone()),
            _ => None,
        })
        .expect("CardView is a struct of the read schema");
    let actions = card
        .fields
        .iter()
        .find(|field| field.name == "allowed_actions")
        .expect("CardView publishes allowed_actions");
    assert_eq!(
        actions.ty,
        FieldTy::List {
            element: Box::new(FieldTy::Named("CommandKindView".to_owned())),
            max_len: 5,
        }
    );
}

/// Manifest coherence: the projected manifest names EVERY identity this build
/// speaks — the four L4 ids plus all three generated binding contracts — and
/// none of them crosses empty. The view is destructured, so an identity added to the
/// contract fails to compile here until it is accounted for; `project.rs`
/// destructures the L4 manifest for the same reason in the other direction.
#[test]
fn projected_manifest_names_every_identity() {
    let frame = build_manifest_frame(&BUILD_TRUST_MANIFEST);
    let ReadBody::BuildManifest(view) = &frame.body else {
        panic!("build manifest frame expected");
    };
    let AbiSchemaManifestView {
        read_binding_schema_id,
        command_binding_schema_id,
        capability_binding_schema_id,
        evidence_rust_abi_id,
        evidence_timeline_schema_id,
        mailbox_receipt_schema_id,
        operation_envelope_schema_id,
    } = &view.abi_schema;
    let compiled = BUILD_TRUST_MANIFEST.abi_schema;
    for (name, projected, source) in [
        (
            "read_binding_schema_id",
            read_binding_schema_id,
            READ_SCHEMA_ID,
        ),
        (
            "command_binding_schema_id",
            command_binding_schema_id,
            COMMAND_SCHEMA_ID,
        ),
        (
            "capability_binding_schema_id",
            capability_binding_schema_id,
            CAPABILITY_SCHEMA_ID,
        ),
        (
            "evidence_rust_abi_id",
            evidence_rust_abi_id,
            compiled.evidence_rust_abi_id,
        ),
        (
            "evidence_timeline_schema_id",
            evidence_timeline_schema_id,
            compiled.evidence_timeline_schema_id,
        ),
        (
            "mailbox_receipt_schema_id",
            mailbox_receipt_schema_id,
            compiled.mailbox_receipt_schema_id,
        ),
        (
            "operation_envelope_schema_id",
            operation_envelope_schema_id,
            compiled.operation_envelope_schema_id,
        ),
    ] {
        assert!(!source.is_empty(), "{name} has no compiled value");
        assert_eq!(projected, source, "{name} disagrees with its source");
    }
    assert_eq!(view.package_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(view.protocol.set_id, BUILD_TRUST_MANIFEST.protocol.set_id);
}
