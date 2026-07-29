//! BN3 proofs: the generated command contract mirrors BN2's live vocabulary
//! exhaustively, its artifacts are drift-gated, and the Rust decoder carries
//! the full hostile-input containment burden (the direction reverses BN1).

use envoix_bindings::bridge::{
    CreateIntent, FrontendIntent, SubmitDecodeError, acceptance_frame, command_view,
    completion_frame, create_result_frame, decode_intent, live_command,
};
use envoix_bindings::command::{
    AcceptanceView, COMMAND_MAX_FRAME_BYTES, COMMAND_SCHEMA_ID, CardCreatedView,
    CommandAcceptanceView, CommandBody, CommandError, CommandFrame, CommandView, CreateIntentView,
    CreateOutcomeView, CreateRefusalView, CreateView, FrontendIntentView, JoinInviteView,
    LocalDirectionView, MintRoomView, NEWEST_ATTACHMENT_COMMANDS, RETRY_HORIZON_COMPLETIONS,
    RejectionView, SUPERSESSION_INERT_PRE_ACCEPTANCE_ONLY, SubmitView, decode_command_frame,
    encode_command_frame,
};
use envoix_bindings::{Decl, FieldTy, emit};
use envoix_runtime::{
    CommandCompletion, CommandLedger, CommandRejected, CommandVerdict, MAX_INVITE_INPUT_LENGTH,
    PauseOrigin, ProductState,
};
use envoix_types::{CommandId, OfferedName, RecordId, Secret};

fn doc() -> envoix_bindings::SchemaDoc {
    envoix_bindings::parse_schema(envoix_bindings::command_schema_text())
        .expect("command schema parses")
}

fn artifacts(doc: &envoix_bindings::SchemaDoc) -> [(&'static str, String); 4] {
    [
        ("generated/rust/command.rs", emit::rust::module(doc)),
        (
            "generated/dart/envoix_command.dart",
            emit::dart::module(doc),
        ),
        (
            "generated/kotlin/EnvoixCommand.kt",
            emit::kotlin::module(doc),
        ),
        (
            "generated/swift/EnvoixCommand.swift",
            emit::swift::module(doc),
        ),
    ]
}

/// Every checked-in command artifact is byte-identical to what the schema
/// emits. `ENVOIX_BINDINGS_REGEN=1` rewrites the artifacts instead.
#[test]
fn generated_artifacts_match_command_schema() {
    let doc = doc();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    if std::env::var_os("ENVOIX_BINDINGS_REGEN").is_some() {
        for (path, content) in artifacts(&doc) {
            std::fs::write(root.join(path), content).expect("write generated artifact");
        }
        return;
    }
    for (path, content) in artifacts(&doc) {
        let on_disk = std::fs::read_to_string(root.join(path)).expect("read generated artifact");
        assert_eq!(
            on_disk, content,
            "{path} drifted from schema/command.schema; regenerate with ENVOIX_BINDINGS_REGEN=1"
        );
    }
}

fn submit_frame(command: CommandView) -> CommandFrame {
    CommandFrame {
        body: CommandBody::Intent(FrontendIntentView::Command(SubmitView {
            card: "00000000000000ab".to_owned(),
            epoch: 7,
            command_id: "000102030405060708090a0b0c0d0e0f".to_owned(),
            command,
        })),
    }
}

fn create_frame(intent: CreateIntentView) -> CommandFrame {
    CommandFrame {
        body: CommandBody::Intent(FrontendIntentView::Create(CreateView {
            intent,
            request_id: "000102030405060708090a0b0c0d0e0f".to_owned(),
        })),
    }
}

fn command_id() -> CommandId {
    CommandId::from_bytes([0x11; 16])
}

fn dispositions() -> [ProductState; 13] {
    [
        ProductState::Preparing,
        ProductState::Waiting,
        ProductState::Connecting,
        ProductState::Verifying,
        ProductState::Transferring,
        ProductState::Confirming,
        ProductState::Paused(PauseOrigin::Local),
        ProductState::Paused(PauseOrigin::Peer),
        ProductState::Paused(PauseOrigin::Lost),
        ProductState::Unconfirmed,
        ProductState::Completed,
        ProductState::Failed,
        ProductState::Cancelled,
    ]
}

fn roundtrip(frame: &CommandFrame) {
    let bytes = encode_command_frame(frame).expect("frame encodes");
    assert_eq!(&decode_command_frame(&bytes).expect("frame decodes"), frame);
}

/// The invariant test of this step: every live command, rejection,
/// disposition, and completion appears in the schema and round-trips; the
/// bridge's wildcard-free matches turn any vocabulary drift into a compile
/// error, and the schema's variant counts are pinned against the live counts.
#[test]
fn generated_command_schema_exhaustiveness() {
    // The BN2 carry-forwards are generated contract, not prose — proven at
    // compile time, with the retry horizon pinned to the live ledger.
    const {
        assert!(NEWEST_ATTACHMENT_COMMANDS);
        assert!(SUPERSESSION_INERT_PRE_ACCEPTANCE_ONLY);
        assert!(RETRY_HORIZON_COMPLETIONS as usize == CommandLedger::RETENTION);
    }
    assert_eq!(COMMAND_SCHEMA_ID, "envoix/binding/command/5");

    // The invite field carries text the grammar has not seen yet, so its bound
    // is the one place `MAX_INVITE_INPUT_LENGTH` — the parser's permissive
    // INTAKE limit — legitimately shapes a contract. It must stay STRICTLY
    // wider: an over-long paste has to reach the authority and come back as
    // `invite_too_long`, and a bound at or below the intake limit would have
    // the encoder refuse it first, with no words for the user.
    let invite_bound = doc()
        .decls
        .iter()
        .find_map(|decl| match decl {
            Decl::Struct(decl) if decl.name == "JoinInviteView" => decl
                .fields
                .iter()
                .find(|field| field.name == "invite")
                .map(|field| field.ty.clone()),
            _ => None,
        })
        .expect("the join intent carries invite text");
    let FieldTy::Str { max_bytes } = invite_bound else {
        panic!("invite text crosses as a bounded string");
    };
    assert!(
        max_bytes as usize > MAX_INVITE_INPUT_LENGTH,
        "the carried bound ({max_bytes}) must exceed the grammar's intake limit \
         ({MAX_INVITE_INPUT_LENGTH}), or the encoder refuses before the authority can"
    );

    // Every command variant, swept from the generated ALL array: view -> live
    // -> view is identity, and a full submit frame round-trips into the typed
    // bridge value a host feeds to submit_command.
    for view in CommandView::ALL {
        assert_eq!(command_view(live_command(view)), view);
        let bytes = encode_command_frame(&submit_frame(view)).expect("submit encodes");
        let FrontendIntent::Command(spec) = decode_intent(&bytes).expect("submit decodes") else {
            panic!("a submit body decodes as a command intent");
        };
        assert_eq!(spec.command, live_command(view));
        assert_eq!(spec.card, RecordId::new(0xab));
        assert_eq!(spec.epoch, 7);
        assert_eq!(
            spec.command_id,
            CommandId::from_bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])
        );
    }

    // Every live rejection crosses and round-trips. A new live variant breaks
    // the bridge match; a new schema variant breaks the count pin below.
    let rejections = [
        CommandRejected::UnknownCard,
        CommandRejected::StaleEpoch,
        CommandRejected::Superseded,
        CommandRejected::AtCapacity,
        CommandRejected::RuntimeStopped,
        CommandRejected::Interrupted,
        CommandRejected::Internal,
    ];
    assert_eq!(rejections.len(), RejectionView::ALL.len());
    for rejected in rejections {
        roundtrip(&acceptance_frame(command_id(), &Err(rejected)));
    }

    // Every disposition crosses in a duplicate acceptance.
    for state in dispositions() {
        roundtrip(&acceptance_frame(
            command_id(),
            &Ok(CommandVerdict::Duplicate { state }),
        ));
    }

    // A conflict names the command that owns the reused identity, so every
    // command has to cross as a conflict payload too.
    for view in CommandView::ALL {
        roundtrip(&acceptance_frame(
            command_id(),
            &Ok(CommandVerdict::Conflict {
                applied: live_command(view),
            }),
        ));
    }

    // The accepted arm at wire level. (`CommandVerdict::Accepted` carries the
    // host-only ticket, which only a live runtime can mint; the bridge arm
    // itself is covered by the exhaustive match.)
    roundtrip(&CommandFrame {
        body: CommandBody::Acceptance(CommandAcceptanceView {
            command_id: "11111111111111111111111111111111".to_owned(),
            acceptance: AcceptanceView::Accepted,
        }),
    });

    // Both create intents round-trip into the typed bridge value the host
    // plans from, and every refusal the authority can answer crosses back.
    // Invite text is carried, never inspected: what goes in comes out.
    let bytes = encode_command_frame(&create_frame(CreateIntentView::MintRoom(MintRoomView {
        local_direction: LocalDirectionView::Send,
    })))
    .expect("send intent encodes");
    let FrontendIntent::Create(spec) = decode_intent(&bytes).expect("send intent decodes") else {
        panic!("a create body decodes as a create intent");
    };
    assert_eq!(
        spec.request_id,
        CommandId::from_bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])
    );
    assert_eq!(
        spec.intent,
        CreateIntent::MintRoom {
            local_direction: envoix_types::Direction::Send
        }
    );
    let hostile = "envoix://invite/v3/\u{202e}not-really'; DROP--";
    let bytes = encode_command_frame(&create_frame(CreateIntentView::JoinRoom(JoinInviteView {
        invite: Secret::new(hostile.to_owned()),
    })))
    .expect("join intent encodes");
    let FrontendIntent::Create(spec) = decode_intent(&bytes).expect("join intent decodes") else {
        panic!("a create body decodes as a create intent");
    };
    assert_eq!(
        spec.intent,
        CreateIntent::JoinRoom {
            invite: hostile.to_owned(),
        },
        "invite text crosses verbatim; nothing on this path interprets it"
    );
    for refusal in CreateRefusalView::ALL {
        roundtrip(&create_result_frame(
            command_id(),
            CreateOutcomeView::Refused(refusal),
        ));
    }
    roundtrip(&create_result_frame(
        command_id(),
        CreateOutcomeView::Created(CardCreatedView {
            card: "00000000000000ab".to_owned(),
        }),
    ));

    // Every live completion crosses and round-trips.
    let completions = [
        CommandCompletion::Committed {
            state: ProductState::Transferring,
        },
        CommandCompletion::CommitFailed {
            state: ProductState::Paused(PauseOrigin::Lost),
        },
        CommandCompletion::Interrupted,
        CommandCompletion::Internal,
    ];
    for completion in completions {
        roundtrip(&completion_frame(command_id(), completion));
    }

    // Schema variant counts pinned against the live vocabulary, so a
    // schema-side addition without live meaning fails loudly too.
    let doc = doc();
    let union_len = |name: &str| match doc.find(name) {
        Some(Decl::Union(decl)) => decl.variants.len(),
        _ => panic!("union {name} expected"),
    };
    assert_eq!(union_len("CommandBody"), 4);
    // command, create, source_offer — the third arrived with acquisition.
    assert_eq!(union_len("FrontendIntentView"), 3);
    assert_eq!(union_len("CreateIntentView"), 2);
    assert_eq!(union_len("CreateOutcomeView"), 2);
    assert_eq!(CreateRefusalView::ALL.len(), 9);
    assert_eq!(union_len("AcceptanceView"), 4);
    assert_eq!(union_len("CompletionView"), completions.len());
    assert_eq!(union_len("DispositionView"), 11);
    assert_eq!(RejectionView::ALL.len(), 7);
    assert_eq!(CommandView::ALL.len(), 5);
}

/// The command must carry every provider name Android can report so L0, the
/// authority that owns the portable byte limit, can answer with `name_too_long`
/// instead of the frontend encoder failing first. A provider leaf is at most
/// 255 UTF-16 units and each may occupy four UTF-8 bytes.
#[test]
fn the_picked_name_bound_reaches_the_authority_for_every_android_leaf() {
    let doc = doc();
    // The bound moved with the field: a document's name now crosses on the
    // SOURCE OFFER rather than at create, because a card exists before a
    // document is chosen. The invariant is unchanged — the authority, not the
    // frontend's encoder, must be the thing that says a name is too long.
    let Some(Decl::Struct(decl)) = doc.find("SourceOfferView") else {
        panic!("SourceOfferView expected");
    };
    let field = decl
        .fields
        .iter()
        .find(|field| field.name == "display_name")
        .expect("SourceOfferView declares a display_name");
    assert!(
        matches!(field.ty, FieldTy::Str { max_bytes }
            if max_bytes as usize == OfferedName::MAX_BYTES * 4),
        "display_name must carry Android's UTF-16 leaf maximum, found {:?}",
        field.ty
    );
}

fn tamper(base: &[u8], mutate: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut value: serde_json::Value = serde_json::from_slice(base).expect("valid base frame");
    mutate(&mut value);
    serde_json::to_vec(&value).expect("re-serialize tampered frame")
}

/// Hostile bytes arrive AT the Rust boundary in this direction: the command
/// decoder rejects everything typed and never panics.
#[test]
fn command_frames_reject_hostile_input() {
    let base = encode_command_frame(&submit_frame(CommandView::Pause)).expect("base encodes");

    // The encoder stamps the envelope itself.
    let stamped: serde_json::Value = serde_json::from_slice(&base).expect("frame json");
    assert_eq!(stamped["schema"], serde_json::json!(COMMAND_SCHEMA_ID));

    let wrong_schema = tamper(&base, |value| {
        value["schema"] = serde_json::json!("evil/schema/9");
    });
    assert_eq!(
        decode_command_frame(&wrong_schema),
        Err(CommandError::UnknownSchema)
    );

    let future_version = tamper(&base, |value| {
        value["schema"] = serde_json::json!("envoix/binding/command/6");
    });
    assert_eq!(
        decode_command_frame(&future_version),
        Err(CommandError::UnknownSchema)
    );

    let missing_schema = tamper(&base, |value| {
        value.as_object_mut().expect("object").remove("schema");
    });
    assert!(matches!(
        decode_command_frame(&missing_schema),
        Err(CommandError::Shape { .. })
    ));

    let unknown_field = tamper(&base, |value| {
        value["smuggled"] = serde_json::json!("payload");
    });
    assert!(matches!(
        decode_command_frame(&unknown_field),
        Err(CommandError::UnknownField { .. })
    ));

    let unknown_body = tamper(&base, |value| {
        value["body"]["kind"] = serde_json::json!("shell");
    });
    assert!(matches!(
        decode_command_frame(&unknown_body),
        Err(CommandError::UnknownVariant { .. })
    ));

    let unknown_command = tamper(&base, |value| {
        value["body"]["value"]["value"]["command"] = serde_json::json!("format_disk");
    });
    assert!(matches!(
        decode_command_frame(&unknown_command),
        Err(CommandError::UnknownVariant { .. })
    ));

    let uppercase_hex = tamper(&base, |value| {
        value["body"]["value"]["value"]["card"] = serde_json::json!("00000000000000AB");
    });
    assert!(matches!(
        decode_command_frame(&uppercase_hex),
        Err(CommandError::Bound { .. })
    ));

    let short_id = tamper(&base, |value| {
        value["body"]["value"]["value"]["command_id"] = serde_json::json!("0102");
    });
    assert!(matches!(
        decode_command_frame(&short_id),
        Err(CommandError::Bound { .. })
    ));

    let negative_epoch = tamper(&base, |value| {
        value["body"]["value"]["value"]["epoch"] = serde_json::json!(-1);
    });
    assert!(matches!(
        decode_command_frame(&negative_epoch),
        Err(CommandError::Shape { .. })
    ));

    let oversized_epoch = tamper(&base, |value| {
        value["body"]["value"]["value"]["epoch"] = serde_json::json!(9_223_372_036_854_775_808_u64);
    });
    assert!(matches!(
        decode_command_frame(&oversized_epoch),
        Err(CommandError::Range { .. })
    ));

    let float_epoch = tamper(&base, |value| {
        value["body"]["value"]["value"]["epoch"] = serde_json::json!(1.5);
    });
    assert!(matches!(
        decode_command_frame(&float_epoch),
        Err(CommandError::Shape { .. })
    ));

    assert_eq!(
        decode_command_frame(&base[..base.len() / 2]),
        Err(CommandError::MalformedJson)
    );
    assert_eq!(
        decode_command_frame(b"not json"),
        Err(CommandError::MalformedJson)
    );
    assert_eq!(
        decode_command_frame(&vec![b' '; COMMAND_MAX_FRAME_BYTES + 1]),
        Err(CommandError::FrameTooLarge)
    );

    // A unit variant with a payload and a payload variant with null are both
    // shape violations, matching the read suite's probes.
    let accepted = encode_command_frame(&CommandFrame {
        body: CommandBody::Acceptance(CommandAcceptanceView {
            command_id: "11111111111111111111111111111111".to_owned(),
            acceptance: AcceptanceView::Accepted,
        }),
    })
    .expect("accepted encodes");
    let unit_with_payload = tamper(&accepted, |value| {
        value["body"]["value"]["acceptance"] = serde_json::json!({"kind": "accepted", "value": 1});
    });
    assert!(matches!(
        decode_command_frame(&unit_with_payload),
        Err(CommandError::Shape { .. })
    ));

    let null_payload = tamper(&base, |value| {
        value["body"]["value"] = serde_json::Value::Null;
    });
    assert!(matches!(
        decode_command_frame(&null_payload),
        Err(CommandError::Shape { .. })
    ));

    // Result bodies are host->frontend only: a well-formed acceptance frame
    // arriving AS an intent is a typed contract violation, not a request.
    let acceptance = encode_command_frame(&acceptance_frame(
        command_id(),
        &Err(CommandRejected::Internal),
    ))
    .expect("acceptance encodes");
    assert_eq!(
        decode_intent(&acceptance),
        Err(SubmitDecodeError::NotAnIntent)
    );
    let result = encode_command_frame(&create_result_frame(
        command_id(),
        CreateOutcomeView::Refused(CreateRefusalView::Internal),
    ))
    .expect("create result encodes");
    assert_eq!(decode_intent(&result), Err(SubmitDecodeError::NotAnIntent));

    // Deterministic fuzz-ish sweep: byte mutations and truncations never
    // panic, whatever they decode to.
    let mut seed: u64 = 0xfeed_face_cafe_beef;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for base_frame in [&base, &acceptance] {
        for _ in 0..2000 {
            let mut mutated = (*base_frame).clone();
            let index = (next() as usize) % mutated.len();
            mutated[index] ^= (next() as u8) | 1;
            let _ = decode_command_frame(&mutated);
            let cut = (next() as usize) % mutated.len();
            let _ = decode_command_frame(&mutated[..cut]);
        }
    }
}

/// Duplicate JSON object keys are rejected outright, never resolved
/// last-wins: a first-wins upstream parser would see a different command than
/// the one Rust applies (the smuggling shape the adversarial round probed —
/// `{"command":"pause","command":"cancel"}` used to decode as Cancel).
#[test]
fn duplicate_json_keys_are_rejected() {
    let base = encode_command_frame(&submit_frame(CommandView::Pause)).expect("base encodes");
    let text = String::from_utf8(base).expect("utf8 frame");

    let dup_command = text.replacen(
        "\"command\":\"pause\"",
        "\"command\":\"pause\",\"command\":\"cancel\"",
        1,
    );
    assert_ne!(dup_command, text, "command key found");
    assert_eq!(
        decode_command_frame(dup_command.as_bytes()),
        Err(CommandError::MalformedJson)
    );

    let dup_schema = text.replacen(
        "\"schema\":",
        &format!("\"schema\":\"{COMMAND_SCHEMA_ID}\",\"schema\":"),
        1,
    );
    assert_ne!(dup_schema, text, "schema key found");
    assert_eq!(
        decode_command_frame(dup_schema.as_bytes()),
        Err(CommandError::MalformedJson)
    );

    let dup_nested = text.replacen("\"epoch\":", "\"epoch\":1,\"epoch\":", 1);
    assert_ne!(dup_nested, text, "epoch key found");
    assert_eq!(
        decode_command_frame(dup_nested.as_bytes()),
        Err(CommandError::MalformedJson)
    );
}

/// The native command artifacts are real generated code carrying the schema
/// id, the frozen rules, and the reserved-word escapes BN1 taught the
/// emitters.
#[test]
fn native_command_artifacts_carry_schema_and_rules() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (path, _) in artifacts(&doc()) {
        let content = std::fs::read_to_string(root.join(path)).expect("read artifact");
        // Taken from the schema rather than written out: the artifact is
        // generated from that id, and a literal here only rots at the next bump.
        assert!(content.contains(&doc().id), "{path}");
    }
    let dart = std::fs::read_to_string(root.join("generated/dart/envoix_command.dart"))
        .expect("dart artifact");
    assert!(dart.contains("const int retryHorizonCompletions = 256;"));
    let kotlin = std::fs::read_to_string(root.join("generated/kotlin/EnvoixCommand.kt"))
        .expect("kotlin artifact");
    assert!(kotlin.contains("const val RETRY_HORIZON_COMPLETIONS: Int = 256"));
    let swift = std::fs::read_to_string(root.join("generated/swift/EnvoixCommand.swift"))
        .expect("swift artifact");
    // `static` because every declaration now lives inside the contract's own
    // Swift namespace enum; the constant itself is unchanged.
    assert!(swift.contains("public static let retryHorizonCompletions = 256"));
    // RejectionView/CompletionView carry an `internal` variant: the Swift
    // keyword escape from BN1's adversarial round must hold here too.
    assert!(swift.contains("case `internal` = \"internal\""));
}
