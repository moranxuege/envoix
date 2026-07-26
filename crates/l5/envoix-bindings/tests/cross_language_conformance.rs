//! BN3b proofs: the per-schema direction policy is a build gate, every native
//! decoder helper has an encoder half that checks the same bound, and the Rust
//! reference codec exports the vectors a native harness replays.
//!
//! The vector bundle lands in `CARGO_TARGET_TMPDIR` because only a real
//! Dart/Kotlin/Swift toolchain can consume it; this crate's own gate is that
//! every vector round-trips through the reference codec.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use envoix_bindings::command::{
    AcceptanceView, CardCreatedView, CommandAcceptanceView, CommandBody, CommandCompletionView,
    CommandError, CommandFrame, CommandView, CompletionView, CreateIntentView, CreateOutcomeView,
    CreateRefusalView, CreateResultView, CreateView, DispositionView, FrontendIntentView,
    JoinInviteView, PauseCauseView, PausedStateView, RejectionView, SendSourceView, SubmitView,
    decode_command_frame, encode_command_frame,
};
use envoix_bindings::read::{
    AbiSchemaManifestView, BuildManifestView, CardUpdateKindView, CardUpdateView, CardView,
    ClosedView, CommandKindView, DegradedView, DiagnosticsStatusView, DirectionView,
    EvidenceProgressView, EvidenceTimelineView, EvidenceValueView, IdentityView, InviteView,
    LagView, LosslessKindView, OutcomeCodeView, OutcomeView, PauseOriginView, PausedView,
    PhaseView, ProductStateView, ProtocolManifestView, QuiescenceView, ReadBody, ReadError,
    ReadFrame, RecoveryView, RedactedIdKindView, RedactedIdView, RetirementIntentView,
    RetiringView, RetryabilityView, RunningView, SessionKeyView, SubscribeRejectedView,
    SubscribeRejectionView, TimelineEntryView, TrustRootSha256View, TrustRootView, WorkerKindView,
    decode_read_frame, encode_read_frame,
};
use envoix_bindings::{
    Decl, Direction, SchemaDoc, command_schema_text, emit, parse_schema, read_schema_text,
};
use envoix_runtime::{MAX_INVITE_LINK_LENGTH, MAX_ROOM_CODE_LENGTH};

/// A schema that exercises every scalar, wrapper, and declaration kind in the
/// grammar, in the direction that needs encoders. The two shipped contracts
/// together use only part of the vocabulary, so this is what keeps the encode
/// half of the generator honest — and what a native harness compiles to prove
/// the emitted helpers are real code.
const PROBE_SCHEMA: &str = r#"
id = "envoix/binding/probe/1"
root = "ProbeFrame"
direction = "bidirectional"

# Between the 528 UTF-16 units and the 578 UTF-8 bytes of a fully-populated
# probe body whose text fields hold 2-, 3-, and 4-byte characters: the frame cap
# is reachable with every field in bounds, and reachable ONLY when it is counted
# in bytes, so the harnesses pin the accounting unit instead of assuming it.
[limits]
max_frame_bytes = 544

[[decl]]
kind = "enum"
name = "ProbeTone"
variants = ["calm", "loud"]

[[decl]]
kind = "struct"
name = "ProbeLeaf"
fields = [{ name = "tone", type = "ProbeTone" }]

[[decl]]
kind = "union"
name = "ProbeChoice"
variants = [{ name = "nothing" }, { name = "leaf", payload = "ProbeLeaf" }]

[[decl]]
kind = "struct"
name = "ProbeScalars"
fields = [
  { name = "small", type = "u16" },
  { name = "medium", type = "u32" },
  { name = "large", type = "u63" },
  { name = "short_id", type = "hex16" },
  { name = "long_id", type = "hex32" },
  { name = "digest", type = "hex64" },
  { name = "blobby", type = "hexv(8)" },
  { name = "text", type = "str(45)" },
  { name = "label", type = "ascii(8)" },
  { name = "maybe", type = "option(ProbeTone)" },
  { name = "maybe_text", type = "option(str(45))" },
  { name = "leaves", type = "list(ProbeLeaf, 4)" },
  { name = "choice", type = "ProbeChoice" },
]

[[decl]]
kind = "union"
name = "ProbeBody"
variants = [
  { name = "scalars", payload = "ProbeScalars", originator = "frontend" },
  { name = "idle" },
]

[[decl]]
kind = "struct"
name = "ProbeFrame"
fields = [
  { name = "schema", type = "ascii(64)" },
  { name = "body", type = "ProbeBody" },
]
"#;

/// The declarations each contract's natives may encode. Written out rather than
/// recomputed from the emitter, so the emitter's reachability walk and this
/// statement of intent have to agree.
fn originable_decls(id: &str) -> &'static [&'static str] {
    match id {
        "envoix/binding/command/3" => &[
            "CommandView",
            "SubmitView",
            "CreateIntentView",
            "CreateView",
            "FrontendIntentView",
            "JoinInviteView",
            "SendSourceView",
        ],
        "envoix/binding/probe/1" => &["ProbeTone", "ProbeLeaf", "ProbeChoice", "ProbeScalars"],
        _ => &[],
    }
}

fn doc(text: &str) -> SchemaDoc {
    parse_schema(text).expect("schema parses")
}

fn natives(doc: &SchemaDoc) -> [(&'static str, String); 3] {
    [
        ("dart", emit::dart::module(doc)),
        ("kotlin", emit::kotlin::module(doc)),
        ("swift", emit::swift::module(doc)),
    ]
}

/// The capability-parity gate. Direction is a property of the contract, so the
/// entry points each artifact carries are too: a native artifact that lost its
/// encoder — or grew one on an observe-only contract — fails here rather than
/// stranding a frontend that can read a verdict but never issue a command.
#[test]
fn artifacts_expose_exactly_their_direction_entry_points() {
    // The Rust reference codec encodes and decodes every contract; naming the
    // four entry points as function values proves that at compile time.
    let _: fn(&[u8]) -> Result<ReadFrame, ReadError> = decode_read_frame;
    let _: fn(&ReadFrame) -> Result<Vec<u8>, ReadError> = encode_read_frame;
    let _: fn(&[u8]) -> Result<CommandFrame, CommandError> = decode_command_frame;
    let _: fn(&CommandFrame) -> Result<Vec<u8>, CommandError> = encode_command_frame;

    for text in [read_schema_text(), command_schema_text(), PROBE_SCHEMA] {
        let doc = doc(text);
        let root = &doc.root;
        let payload = doc.frontend_body().map(|body| body.payload);
        for (language, source) in natives(&doc) {
            // The forbidden needles are the weakest form of "encodes", not the
            // entry point's own name: a planted `_encodeCardView` in a read
            // artifact is exactly as much of a fabrication tool as a public
            // `encodeReadFrame` would be.
            let (decoder, encoder, forbidden): (_, _, Vec<String>) = match language {
                "dart" => (
                    format!("{root} decode{root}(String text)"),
                    payload.map(|payload| format!("String encode{root}({payload} body)")),
                    vec!["_encode".to_owned(), format!("encode{root}(")],
                ),
                "kotlin" => (
                    format!("fun decode(text: String): {root}"),
                    payload.map(|payload| format!("fun encode(body: {payload}): String")),
                    vec!["fun encode".to_owned()],
                ),
                _ => (
                    format!("public static func decode(_ data: Data) throws -> {root}"),
                    payload.map(|payload| {
                        format!("public static func encode(_ body: {payload}) throws -> Data")
                    }),
                    vec!["func encode".to_owned()],
                ),
            };
            assert!(
                source.contains(&decoder),
                "{language} artifact for {} lacks `{decoder}`",
                doc.id
            );
            match encoder {
                Some(encoder) => assert!(
                    source.contains(&encoder),
                    "{language} artifact for {} lacks `{encoder}`",
                    doc.id
                ),
                None => {
                    for needle in forbidden {
                        assert!(
                            !source.contains(&needle),
                            "{language} artifact for {} contains `{needle}` on an \
                             observe-only contract",
                            doc.id
                        );
                    }
                }
            }
        }
    }
}

/// Direction is per union arm, so encoders are too: a native gets one for every
/// declaration reachable from the body its frontends may originate, and for no
/// other. `acceptance` and `completion` are host observations — a frontend that
/// cannot build one has no way to forge one, which is the same standard the
/// envelope is held to.
#[test]
fn natives_encode_exactly_the_originable_declarations() {
    for text in [read_schema_text(), command_schema_text(), PROBE_SCHEMA] {
        let doc = doc(text);
        let originable = originable_decls(&doc.id);
        assert_eq!(
            originable.is_empty(),
            doc.direction == Direction::HostToFrontend,
            "{} originable set",
            doc.id
        );
        for (language, source) in natives(&doc) {
            let prefix = if language == "dart" { "_" } else { "" };
            for decl in &doc.decls {
                let name = decl.name();
                assert!(
                    source.contains(&format!("{prefix}decode{name}(")),
                    "{language} artifact for {} lacks a decoder for {name}",
                    doc.id
                );
                assert_eq!(
                    source.contains(&format!("{prefix}encode{name}(")),
                    originable.contains(&name),
                    "{language} artifact for {}: encoder for {name}",
                    doc.id
                );
            }
        }
    }
}

/// Encoder honesty, made unrepresentable rather than tested: every scalar
/// encode helper *is* its decode twin, delegating to it with the same bound and
/// carrying no predicate of its own, so the two halves cannot check different
/// things. The list cap is the one aggregate predicate written twice (factoring
/// it would change the byte-frozen read artifacts); the native harnesses pin it
/// behaviourally instead.
#[test]
fn every_native_encode_helper_is_its_decode_predicate() {
    let doc = doc(PROBE_SCHEMA);
    assert_eq!(doc.direction, Direction::Bidirectional);
    let stems = [
        "integer",
        "hexFixed",
        "hexVariable",
        "utf8Bounded",
        "asciiBounded",
    ];
    for (language, source) in natives(&doc) {
        let prefix = if language == "dart" { "_" } else { "" };
        for stem in stems {
            let decoder = format!("{prefix}{stem}(");
            let encoder = format!("{prefix}encode{}{}(", stem[..1].to_uppercase(), &stem[1..]);
            assert!(
                source.contains(&decoder),
                "{language} probe lacks {decoder}"
            );
            let start = source
                .find(&encoder)
                .unwrap_or_else(|| panic!("{language} probe lacks {encoder}"));
            let block = &source[start..];
            let block = &block[..block.find("\n\n").expect("the helper block ends")];
            // Past the signature, so Swift's `throws` is not mistaken for one.
            let (_, body) = block.split_once('\n').expect("a helper signature line");
            assert!(
                body.contains(&decoder),
                "{language} {encoder} does not delegate to {decoder}"
            );
            assert!(
                !body.contains("throw"),
                "{language} {encoder} carries a predicate of its own"
            );
        }
        let list = match language {
            "dart" => ("_list<", "_encodeList<"),
            "kotlin" => ("fun <T> decodeList(", "fun <T> encodeList("),
            _ => ("func decodeList<T>(", "func encodeList<T>("),
        };
        assert!(source.contains(list.0) && source.contains(list.1));
    }
}

/// Constructibility as a build gate, standing in for the `swiftc` this
/// workspace cannot run. A Swift `public struct`'s memberwise initializer is
/// `internal`, so a type the encoder accepts would be public and still
/// unbuildable from the app module that calls it — an artifact that compiles
/// and no consumer can use, which is the same class of gap as the missing
/// encoders themselves. The originable structs get one and no others do: a
/// frontend that cannot build an acceptance cannot fabricate one.
///
/// Dart constructors and Kotlin data-class constructors are public by default,
/// so those two artifacts satisfy this trivially; pinned rather than assumed,
/// since a visibility modifier is one word away.
#[test]
fn originable_structs_are_constructible_outside_the_module() {
    for text in [command_schema_text(), PROBE_SCHEMA] {
        let doc = doc(text);
        assert_eq!(doc.direction, Direction::Bidirectional);
        let originable = originable_decls(&doc.id);
        let swift = emit::swift::module(&doc);
        let mut swept = 0;
        for block in swift.split("\npublic struct ").skip(1) {
            let (head, body) = block.split_once('\n').expect("a struct head");
            // The codec's typed failure is read by consumers, never built by
            // them; the encode direction does not change that.
            if head.contains(": Error,") {
                continue;
            }
            let name = head.split(':').next().expect("a struct name").trim();
            let body = &body[..body.find("\n}\n").expect("the struct body ends")];
            let properties: Vec<&str> = body
                .lines()
                .filter_map(|line| line.trim().strip_prefix("public let "))
                .collect();
            let parameters = body
                .lines()
                .find_map(|line| line.trim().strip_prefix("public init("))
                .and_then(|line| line.strip_suffix(") {"));
            if !originable.contains(&name) {
                assert!(
                    parameters.is_none(),
                    "public struct {name} is not originable but is constructible"
                );
                swept += 1;
                continue;
            }
            let parameters = parameters
                .unwrap_or_else(|| panic!("originable public struct {name} has no public init"));
            // A struct whose members are all envelope fields emits `init()`;
            // splitting an empty list yields one empty name, not none.
            let parameters: Vec<&str> = if parameters.is_empty() {
                Vec::new()
            } else {
                parameters.split(", ").collect()
            };
            assert_eq!(
                parameters, properties,
                "public struct {name} init does not take its stored properties"
            );
            swept += 1;
        }
        let structs = doc
            .decls
            .iter()
            .filter(|decl| matches!(decl, Decl::Struct(_)))
            .count();
        assert_eq!(swept, structs, "{} swift structs swept", doc.id);

        let dart = emit::dart::module(&doc);
        let kotlin = emit::kotlin::module(&doc);
        for decl in &doc.decls {
            let Decl::Struct(decl) = decl else { continue };
            let name = &decl.name;
            assert!(
                dart.contains(&format!("\nfinal class {name} {{\n  const {name}({{")),
                "{name} has no public Dart constructor"
            );
            assert!(
                kotlin.contains(&format!("\ndata class {name}(")),
                "{name} has no public Kotlin constructor"
            );
        }
    }
}

/// Dart and Swift emit frames byte-identical to the reference codec's, which
/// rests on one property: object keys cross in the order serde_json serializes
/// them (sorted), not in schema field order. Pinned here because the proof that
/// the bytes match — a native harness — cannot run in this crate's gates.
#[test]
fn dart_and_swift_emit_object_keys_in_the_reference_order() {
    for text in [command_schema_text(), PROBE_SCHEMA] {
        let doc = doc(text);
        let originable = originable_decls(&doc.id);
        let dart = emit::dart::module(&doc);
        // Keys of the object being built are the lines indented one level
        // inside the map literal; a nested arm sits one level deeper.
        let sorted_keys = |head: &str, expected: usize| {
            let start = dart
                .find(head)
                .unwrap_or_else(|| panic!("{head} is emitted"));
            let body = &dart[start..];
            let body = &body[..body.find("\n}\n").expect("the encoder body ends")];
            let keys: Vec<&str> = body
                .lines()
                .filter_map(|line| line.strip_prefix("    '"))
                .filter_map(|line| line.split('\'').next())
                .collect();
            assert_eq!(keys.len(), expected, "{head} key count");
            let mut sorted = keys.clone();
            sorted.sort_unstable();
            assert_eq!(keys, sorted, "{head} keys are out of reference order");
        };
        for decl in &doc.decls {
            let Decl::Struct(decl) = decl else { continue };
            if !originable.contains(&decl.name.as_str()) {
                continue;
            }
            sorted_keys(
                &format!("Map<String, Object?> _encode{}(", decl.name),
                decl.fields.len(),
            );
        }
        // The entry point writes the root object itself: the envelope and the
        // body arm, and nothing else, in the same sorted order.
        let arm = doc.frontend_body().expect("a bidirectional contract");
        sorted_keys(&format!("String encode{}(", doc.root), 2);
        assert!(
            dart.contains(&format!(
                "      'kind': '{}',\n      'value': _encode{}(body),\n",
                arm.variant, arm.payload
            )),
            "{} body arm is not stamped in sorted order",
            doc.id
        );
        // A Swift dictionary has no order of its own; the serializer supplies
        // it, and the parser has already rejected any key set whose collation
        // and ASCII orders could differ.
        assert!(emit::swift::module(&doc).contains("options: [.sortedKeys]"));
    }
}

fn submit(card: &str, epoch: u64, command_id: &str, command: CommandView) -> CommandFrame {
    CommandFrame {
        body: CommandBody::Intent(FrontendIntentView::Command(SubmitView {
            card: card.to_owned(),
            epoch,
            command_id: command_id.to_owned(),
            command,
        })),
    }
}

fn create(request_id: &str, intent: CreateIntentView) -> CommandFrame {
    CommandFrame {
        body: CommandBody::Intent(FrontendIntentView::Create(CreateView {
            intent,
            request_id: request_id.to_owned(),
        })),
    }
}

fn create_result(outcome: CreateOutcomeView) -> CommandFrame {
    CommandFrame {
        body: CommandBody::CreateResult(CreateResultView {
            outcome,
            request_id: "11111111111111111111111111111111".to_owned(),
        }),
    }
}

fn acceptance(acceptance: AcceptanceView) -> CommandFrame {
    CommandFrame {
        body: CommandBody::Acceptance(CommandAcceptanceView {
            command_id: "11111111111111111111111111111111".to_owned(),
            acceptance,
        }),
    }
}

fn completion(completion: CompletionView) -> CommandFrame {
    CommandFrame {
        body: CommandBody::Completion(CommandCompletionView {
            command_id: "22222222222222222222222222222222".to_owned(),
            completion,
        }),
    }
}

/// Every disposition shape, named for the harness. Pinned against the schema's
/// union arity so a new arm cannot slip through unswept.
fn dispositions() -> Vec<(&'static str, DispositionView)> {
    let paused = |origin| DispositionView::Paused(PausedStateView { origin });
    vec![
        ("preparing", DispositionView::Preparing),
        ("waiting", DispositionView::Waiting),
        ("connecting", DispositionView::Connecting),
        ("verifying", DispositionView::Verifying),
        ("transferring", DispositionView::Transferring),
        ("confirming", DispositionView::Confirming),
        ("paused_local", paused(PauseCauseView::Local)),
        ("paused_peer", paused(PauseCauseView::Peer)),
        ("paused_lost", paused(PauseCauseView::Lost)),
        ("unconfirmed", DispositionView::Unconfirmed),
        ("completed", DispositionView::Completed),
        ("failed", DispositionView::Failed),
        ("cancelled", DispositionView::Cancelled),
    ]
}

/// The frontend→host direction plus the answers it must be able to read back:
/// every command, every numeric and identifier edge, every acceptance,
/// rejection, disposition, and completion arm.
fn command_vectors() -> Vec<(String, CommandFrame)> {
    let card = "00000000000000ab";
    let id = "000102030405060708090a0b0c0d0e0f";
    let commands = [
        ("pause", CommandView::Pause),
        ("cancel", CommandView::Cancel),
        ("resume", CommandView::Resume),
        ("remove", CommandView::Remove),
        ("re_pick_source", CommandView::RePickSource),
    ];
    assert_eq!(commands.len(), CommandView::ALL.len(), "commands swept");
    let rejections = [
        ("unknown_card", RejectionView::UnknownCard),
        ("stale_epoch", RejectionView::StaleEpoch),
        ("superseded", RejectionView::Superseded),
        ("at_capacity", RejectionView::AtCapacity),
        ("runtime_stopped", RejectionView::RuntimeStopped),
        ("interrupted", RejectionView::Interrupted),
        ("internal", RejectionView::Internal),
    ];
    assert_eq!(
        rejections.len(),
        RejectionView::ALL.len(),
        "rejections swept"
    );

    let mut vectors = Vec::new();
    for (name, command) in commands {
        vectors.push((format!("submit_{name}"), submit(card, 7, id, command)));
    }
    // Numeric edges on the u63 carrier: zero, one, the float-safe limit every
    // JavaScript-family runtime tops out at, and the largest accepted value.
    for (name, epoch) in [
        ("zero", 0),
        ("one", 1),
        ("two_pow_53", 1u64 << 53),
        ("u63_max", u64::MAX >> 1),
    ] {
        vectors.push((
            format!("submit_epoch_{name}"),
            submit(card, epoch, id, CommandView::Pause),
        ));
    }
    // Identifier edges: both hex fields at their all-zero and all-f extremes.
    vectors.push((
        "submit_ids_min".to_owned(),
        submit("0000000000000000", 1, &"0".repeat(32), CommandView::Cancel),
    ));
    vectors.push((
        "submit_ids_max".to_owned(),
        submit("ffffffffffffffff", 1, &"f".repeat(32), CommandView::Cancel),
    ));

    // The create intent, at the edges a native encoder is most likely to get
    // wrong: an empty name and a zero total, a name at its byte bound, and
    // invite text that is deliberately not an invite — carried verbatim,
    // because nothing on this path may interpret it.
    vectors.push((
        "create_send_narrowest".to_owned(),
        create(
            id,
            CreateIntentView::Send(SendSourceView {
                display_name: String::new(),
                total: 0,
            }),
        ),
    ));
    vectors.push((
        "create_send_widest".to_owned(),
        create(
            &"f".repeat(32),
            CreateIntentView::Send(SendSourceView {
                // 255 bytes exactly, in three-byte characters plus one ASCII.
                display_name: format!("{}x", "世".repeat(84)),
                total: u64::MAX >> 1,
            }),
        ),
    ));
    for (name, invite) in [
        ("empty", String::new()),
        (
            "canonical",
            "envoix://invite/v3/eyJ2ZXJzaW9uIjozfQ".to_owned(),
        ),
        ("bidirectional", "\u{202e}envoix://invite".to_owned()),
        ("at_bound", "e".repeat(16_384)),
    ] {
        vectors.push((
            format!("create_join_{name}"),
            create(id, CreateIntentView::Join(JoinInviteView { invite })),
        ));
    }
    for (index, refusal) in CreateRefusalView::ALL.into_iter().enumerate() {
        vectors.push((
            format!("create_refused_{index}"),
            create_result(CreateOutcomeView::Refused(refusal)),
        ));
    }
    vectors.push((
        "create_created".to_owned(),
        create_result(CreateOutcomeView::Created(CardCreatedView {
            card: card.to_owned(),
        })),
    ));

    vectors.push((
        "acceptance_accepted".to_owned(),
        acceptance(AcceptanceView::Accepted),
    ));
    for (name, disposition) in dispositions() {
        vectors.push((
            format!("acceptance_duplicate_{name}"),
            acceptance(AcceptanceView::Duplicate(disposition)),
        ));
    }
    // A conflict names the command that owns the reused identity, so the whole
    // command vocabulary has to cross as a conflict payload.
    for (name, command) in commands {
        vectors.push((
            format!("acceptance_conflict_{name}"),
            acceptance(AcceptanceView::Conflict(command)),
        ));
    }
    for (name, rejection) in rejections {
        vectors.push((
            format!("acceptance_rejected_{name}"),
            acceptance(AcceptanceView::Rejected(rejection)),
        ));
    }
    for (name, disposition) in dispositions() {
        vectors.push((
            format!("completion_committed_{name}"),
            completion(CompletionView::Committed(disposition)),
        ));
    }
    vectors.push((
        "completion_commit_failed_paused_lost".to_owned(),
        completion(CompletionView::CommitFailed(DispositionView::Paused(
            PausedStateView {
                origin: PauseCauseView::Lost,
            },
        ))),
    ));
    vectors.push((
        "completion_interrupted".to_owned(),
        completion(CompletionView::Interrupted),
    ));
    vectors.push((
        "completion_internal".to_owned(),
        completion(CompletionView::Internal),
    ));
    vectors
}

fn card_update(epoch: u64, kind: CardUpdateKindView) -> ReadFrame {
    ReadFrame {
        body: ReadBody::CardUpdate(CardUpdateView {
            epoch,
            card: "00000000000000ab".to_owned(),
            kind,
        }),
    }
}

/// The host→frontend direction, at the edges a native decoder is most likely
/// to get wrong: u32/u16 maxima, integers past 2^53, text at exactly its byte
/// bound, multi-byte and bidirectional text, an absent optional, and lists at
/// both ends of their cap.
fn read_vectors() -> Vec<(String, ReadFrame)> {
    let identity = IdentityView {
        card: "00000000000000ab".to_owned(),
        transfer: "000102030405060708090a0b0c0d0e0f".to_owned(),
        artifact: "101112131415161718191a1b1c1d1e1f".to_owned(),
    };
    // 160 bytes exactly: the str(160) bound, reached with two-byte characters.
    let display = "é".repeat(80);
    let widest = CardView {
        identity: identity.clone(),
        direction: DirectionView::Receive,
        // Emoji with a skin-tone modifier, RTL text, a combining mark, and a
        // flag sequence: every UTF-16 shape a native string can hold.
        offered_name: "👍🏽 مرحبا e\u{301} 🇺🇳.pdf".to_owned(),
        total: u64::MAX >> 1,
        state: ProductStateView::Paused(PausedView {
            origin: PauseOriginView::Lost,
        }),
        quiescence: QuiescenceView::Retiring(RetiringView {
            worker: WorkerKindView::Staging,
            intent: RetirementIntentView::Finalize,
        }),
        generation: u32::MAX,
        phase: PhaseView::Confirming,
        bytes: 1u64 << 53,
        bytes_resumed: (1u64 << 53) + 1,
        outcome: Some(OutcomeView {
            code: OutcomeCodeView::PeerLost,
            phase: PhaseView::Transferring,
            retry: RetryabilityView::NeedsUser,
            recovery: Some(RecoveryView::ReconnectPeer),
            display,
        }),
        // The list at its cap, which is also every command the contract has.
        allowed_actions: vec![
            CommandKindView::Pause,
            CommandKindView::Cancel,
            CommandKindView::Resume,
            CommandKindView::Remove,
            CommandKindView::RePickSource,
        ],
        invite: Some(InviteView {
            code: "0".repeat(MAX_ROOM_CODE_LENGTH),
            // The link bound exactly, reached in ASCII — and it is the invite
            // grammar's own emit maximum, so this vector is a real invite's
            // worst case rather than a number the contract chose.
            link: Some(format!(
                "envoix://invite/v3/{}",
                "A".repeat(MAX_INVITE_LINK_LENGTH - "envoix://invite/v3/".len())
            )),
        }),
    };
    let narrowest = CardView {
        identity,
        direction: DirectionView::Send,
        offered_name: String::new(),
        total: 0,
        state: ProductStateView::Preparing,
        quiescence: QuiescenceView::Running(RunningView {
            worker: WorkerKindView::Attempt,
        }),
        generation: 0,
        phase: PhaseView::Preparing,
        bytes: 0,
        bytes_resumed: 0,
        outcome: None,
        // A card the authority will admit nothing for: the empty list, which is
        // a legality fact and not an absence.
        allowed_actions: Vec::new(),
        // A card with no channel at all — the absent optional.
        invite: None,
    };
    let entry = |sequence, value| TimelineEntryView { sequence, value };
    let session = SessionKeyView {
        card: "00000000000000ab".to_owned(),
        generation: u32::MAX,
    };

    // A channel whose link the contract cannot carry: the code still crosses.
    let linkless = CardView {
        invite: Some(InviteView {
            code: "000000-amber-brass".to_owned(),
            link: None,
        }),
        ..widest.clone()
    };

    vec![
        (
            "read_card_update_linkless_invite".to_owned(),
            card_update(1, CardUpdateKindView::State(linkless)),
        ),
        (
            "read_card_update_widest".to_owned(),
            card_update(u64::MAX >> 1, CardUpdateKindView::Snapshot(widest)),
        ),
        (
            "read_card_update_narrowest".to_owned(),
            card_update(0, CardUpdateKindView::Progress(narrowest)),
        ),
        (
            "read_lag".to_owned(),
            ReadFrame {
                body: ReadBody::Lag(LagView {
                    epoch: 2,
                    card: "00000000000000ab".to_owned(),
                    missed: LosslessKindView::CapabilityDuty,
                }),
            },
        ),
        (
            "read_closed".to_owned(),
            ReadFrame {
                body: ReadBody::Closed(ClosedView {
                    epoch: 3,
                    card: "00000000000000ab".to_owned(),
                }),
            },
        ),
        (
            "read_subscribe_rejected".to_owned(),
            ReadFrame {
                body: ReadBody::SubscribeRejected(SubscribeRejectedView {
                    card: "00000000000000ab".to_owned(),
                    reason: SubscribeRejectionView::EpochExhausted,
                }),
            },
        ),
        (
            "read_evidence_empty".to_owned(),
            ReadFrame {
                body: ReadBody::Evidence(EvidenceTimelineView {
                    session,
                    status: DiagnosticsStatusView::Complete,
                    entries: Vec::new(),
                }),
            },
        ),
        (
            "read_evidence_entries".to_owned(),
            ReadFrame {
                body: ReadBody::Evidence(EvidenceTimelineView {
                    session: SessionKeyView {
                        card: "00000000000000ab".to_owned(),
                        generation: 7,
                    },
                    status: DiagnosticsStatusView::Degraded(DegradedView { dropped_events: 9 }),
                    entries: vec![
                        entry(0, EvidenceValueView::Phase(PhaseView::Restoring)),
                        entry(
                            1,
                            EvidenceValueView::Progress(EvidenceProgressView {
                                transferred: 1u64 << 53,
                                total: u64::MAX >> 1,
                            }),
                        ),
                        entry(
                            u64::MAX >> 1,
                            EvidenceValueView::Identifier(RedactedIdView {
                                kind: RedactedIdKindView::Artifact,
                            }),
                        ),
                    ],
                }),
            },
        ),
        (
            "read_build_manifest".to_owned(),
            ReadFrame {
                body: ReadBody::BuildManifest(BuildManifestView {
                    package_version: "0.2.0".to_owned(),
                    protocol: ProtocolManifestView {
                        set_id: "envoix/protocol/probe".to_owned(),
                        data_alpn: "656e766f69782f64617461".to_owned(),
                        data_magic: "cafebabe".to_owned(),
                        data_wire_version: u16::MAX,
                    },
                    abi_schema: AbiSchemaManifestView {
                        read_binding_schema_id: "envoix/binding/read/2".to_owned(),
                        command_binding_schema_id: "envoix/binding/command/2".to_owned(),
                        evidence_rust_abi_id: "envoix/evidence/abi/1".to_owned(),
                        evidence_timeline_schema_id: "envoix/evidence/timeline/1".to_owned(),
                        mailbox_receipt_schema_id: "envoix/mailbox/receipt/1".to_owned(),
                        operation_envelope_schema_id: "envoix/operation/envelope/1".to_owned(),
                    },
                    trust_root: TrustRootView::Sha256(TrustRootSha256View {
                        fingerprint: "ab".repeat(32),
                    }),
                }),
            },
        ),
    ]
}

/// Builds the bundle a native harness replays: every vector, whether a frontend
/// may originate it, and the probe artifacts. Each vector is round-tripped
/// through the reference codec on the way out, so the bundle is known-good
/// before any native sees it.
fn write_bundle(directory: &Path) {
    let mut command = Vec::new();
    for (name, frame) in command_vectors() {
        let bytes = encode_command_frame(&frame).expect("vector encodes");
        assert_eq!(
            decode_command_frame(&bytes).as_ref(),
            Ok(&frame),
            "{name} round-trips"
        );
        let text = String::from_utf8(bytes).expect("utf8 frame");
        let originable = matches!(frame.body, CommandBody::Intent(_));
        command.push(serde_json::json!({
            "name": name,
            "frame": text,
            "originable": originable,
        }));
    }
    let mut read = Vec::new();
    for (name, frame) in read_vectors() {
        let bytes = encode_read_frame(&frame).expect("vector encodes");
        assert_eq!(
            decode_read_frame(&bytes).as_ref(),
            Ok(&frame),
            "{name} round-trips"
        );
        let text = String::from_utf8(bytes).expect("utf8 frame");
        read.push(serde_json::json!({"name": name, "frame": text}));
    }

    fs::create_dir_all(directory).expect("create the bundle directory");
    let bundle = serde_json::json!({"command": command, "read": read});
    fs::write(
        directory.join("vectors.json"),
        serde_json::to_vec_pretty(&bundle).expect("bundle serializes"),
    )
    .expect("write the vector bundle");
    let probe = doc(PROBE_SCHEMA);
    fs::write(directory.join("probe.schema"), PROBE_SCHEMA).expect("write the probe schema");
    for (name, source) in [
        ("envoix_probe.dart", emit::dart::module(&probe)),
        ("EnvoixProbe.kt", emit::kotlin::module(&probe)),
        ("EnvoixProbe.swift", emit::swift::module(&probe)),
    ] {
        fs::write(directory.join(name), source).expect("write the probe artifact");
    }
}

/// Every exported vector round-trips through the reference codec, and the
/// bundle a native harness replays is written beside the test binaries. The
/// harness proves the other half: that a native artifact produces and consumes
/// exactly these bytes.
#[test]
fn conformance_vectors_round_trip_and_export() {
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("conformance");
    write_bundle(&directory);
    println!("conformance bundle: {}", directory.display());
}

// ---- the behavioural half: the emitted code, compiled and executed ----

/// Set this to run the crate's gates without the native replay. It is named in
/// the failure message on purpose: a gate that skips itself when a toolchain is
/// missing is how "encoder honesty as a build gate" came to mean a grep for
/// function names.
const SKIP_NATIVE: &str = "ENVOIX_BINDINGS_SKIP_NATIVE";

fn from_env(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|path| path.exists())
}

fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME is set"))
}

/// The newest jar in `directory` whose name starts with `prefix`.
fn jar(directory: &Path, prefix: &str) -> Option<PathBuf> {
    let mut jars: Vec<PathBuf> = fs::read_dir(directory)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".jar"))
        })
        .collect();
    jars.sort();
    jars.pop()
}

/// The Gradle distribution ships `kotlin-compiler-embeddable`, which is the
/// only Kotlin compiler on this machine.
fn kotlin_lib(directory: &Path, depth: usize) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let mut nested: Vec<PathBuf> = fs::read_dir(directory)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    nested.sort();
    for path in &nested {
        if path.file_name().is_some_and(|name| name == "lib")
            && jar(path, "kotlin-compiler-embeddable").is_some()
        {
            return Some(path.clone());
        }
    }
    nested.iter().find_map(|path| kotlin_lib(path, depth - 1))
}

fn require(found: Option<PathBuf>, what: &str, key: &str) -> PathBuf {
    found.unwrap_or_else(|| {
        panic!(
            "{what} was not found. Install it, point {key} at it, or set {SKIP_NATIVE}=1 to run \
             this crate's gates without the behavioural replay — which leaves the native \
             encoders unproven."
        )
    })
}

fn run(label: &str, command: &mut Command) -> String {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{label} did not start: {error}"));
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "{label} failed ({}):\n{stdout}\n{stderr}",
        output.status
    );
    assert!(
        !stderr.contains("warning:"),
        "{label} is not clean:\n{stderr}"
    );
    stdout
}

fn suite(label: &str, command: &mut Command) {
    let stdout = run(label, command);
    print!("{stdout}");
    assert!(
        stdout.contains("RESULT: all checks passed"),
        "{label} did not report a clean result:\n{stdout}"
    );
}

fn copy(from: &Path, into: &Path) {
    let name = from.file_name().expect("a file name");
    fs::copy(from, into.join(name))
        .unwrap_or_else(|error| panic!("copy {} -> {}: {error}", from.display(), into.display()));
}

/// The behavioural half of encoder honesty. No text gate can tell an encoder
/// that emits `'Pause'` from one that emits `'pause'`, or a deleted frame cap
/// from an enforced one — only running the emitted code against the reference
/// bytes can. Both harnesses therefore run here, inside `cargo test`, and a
/// missing toolchain fails loudly rather than passing quietly.
#[test]
fn native_harnesses_replay_the_conformance_vectors() {
    if std::env::var_os(SKIP_NATIVE).is_some() {
        eprintln!("{SKIP_NATIVE} is set: the Dart and Kotlin replay did NOT run");
        return;
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let generated = manifest.join("generated");
    let suites = manifest.join("tests/native");
    let work = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native");
    let dart_artifacts = work.join("generated");
    let kotlin_sources = work.join("kotlin");
    for directory in [&dart_artifacts, &kotlin_sources] {
        let _ = fs::remove_dir_all(directory);
        fs::create_dir_all(directory).expect("create the harness directory");
    }
    write_bundle(&work);

    for name in ["envoix_read.dart", "envoix_command.dart"] {
        copy(&generated.join("dart").join(name), &dart_artifacts);
    }
    copy(&work.join("envoix_probe.dart"), &dart_artifacts);
    copy(&suites.join("conformance_test.dart"), &work);
    for name in ["EnvoixRead.kt", "EnvoixCommand.kt"] {
        copy(&generated.join("kotlin").join(name), &kotlin_sources);
    }
    copy(&work.join("EnvoixProbe.kt"), &kotlin_sources);
    copy(&suites.join("KotlinConformance.kt"), &kotlin_sources);

    let dart = require(
        from_env("ENVOIX_DART")
            .or_else(|| on_path("dart"))
            .or_else(|| Some(home().join("development/flutter/bin/dart")).filter(|p| p.is_file())),
        "the Dart SDK",
        "ENVOIX_DART",
    );
    let script = work.join("conformance_test.dart");
    run(
        "dart analyze",
        Command::new(&dart)
            .arg("analyze")
            .arg(&dart_artifacts)
            .arg(&script),
    );
    suite(
        "the Dart conformance suite",
        Command::new(&dart)
            .arg("run")
            .arg(&script)
            .arg(work.join("vectors.json"))
            .arg(&dart_artifacts),
    );

    let java = require(
        from_env("ENVOIX_JAVA").or_else(|| on_path("java")),
        "a JVM",
        "ENVOIX_JAVA",
    );
    let lib = require(
        from_env("ENVOIX_KOTLIN_LIB")
            .or_else(|| kotlin_lib(&home().join(".gradle/wrapper/dists"), 5)),
        "the Kotlin compiler (kotlin-compiler-embeddable, shipped with Gradle)",
        "ENVOIX_KOTLIN_LIB",
    );
    let stdlib = jar(&lib, "kotlin-stdlib").expect("kotlin-stdlib beside the compiler");
    let org_json = require(
        from_env("ENVOIX_ORG_JSON_JAR").or_else(|| {
            Some(home().join(".cache/envoix-bindings/org-json.jar")).filter(|jar| jar.is_file())
        }),
        "the org.json reference jar (Android bundles it; the JVM does not)",
        "ENVOIX_ORG_JSON_JAR",
    );
    let classes = work.join("kotlin-classes");
    let _ = fs::remove_dir_all(&classes);
    let classpath = format!("{}:{}", stdlib.display(), org_json.display());
    run(
        "the Kotlin compiler",
        Command::new(&java)
            .arg("-cp")
            .arg(lib.join("*"))
            .arg("org.jetbrains.kotlin.cli.jvm.K2JVMCompiler")
            .arg("-no-stdlib")
            .arg("-classpath")
            .arg(&classpath)
            .arg("-d")
            .arg(&classes)
            .arg(kotlin_sources.join("EnvoixRead.kt"))
            .arg(kotlin_sources.join("EnvoixCommand.kt"))
            .arg(kotlin_sources.join("EnvoixProbe.kt"))
            .arg(kotlin_sources.join("KotlinConformance.kt")),
    );
    suite(
        "the Kotlin conformance suite",
        Command::new(&java)
            .arg("-cp")
            .arg(format!("{}:{classpath}", classes.display()))
            .arg("com.envoix.bindings.KotlinConformanceKt")
            .arg(work.join("vectors.json")),
    );
}
