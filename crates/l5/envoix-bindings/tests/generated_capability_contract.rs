//! F2c proofs: the capability contract is a card-less seam every frontend can
//! implement unaided, its artifacts are drift-gated, and nothing about a camera
//! can be spelled on it.

use envoix_bindings::capability::{
    CAPABILITY_MAX_FRAME_BYTES, CAPABILITY_SCHEMA_ID, CapabilityBody, CapabilityError,
    CapabilityExchangeView, CapabilityFrame, CapabilityRequestView, CapabilityStepView,
    DeclinedReasonView, DeclinedView, ScannedTextView, decode_capability_frame,
    encode_capability_frame,
};
use envoix_bindings::{Decl, FieldTy, emit};
use envoix_types::Secret;

fn doc() -> envoix_bindings::SchemaDoc {
    envoix_bindings::parse_schema(envoix_bindings::capability_schema_text())
        .expect("capability schema parses")
}

fn artifacts(doc: &envoix_bindings::SchemaDoc) -> [(&'static str, String); 4] {
    [
        ("generated/rust/capability.rs", emit::rust::module(doc)),
        (
            "generated/dart/envoix_capability.dart",
            emit::dart::module(doc),
        ),
        (
            "generated/kotlin/EnvoixCapability.kt",
            emit::kotlin::module(doc),
        ),
        (
            "generated/swift/EnvoixCapability.swift",
            emit::swift::module(doc),
        ),
    ]
}

/// Every checked-in capability artifact is byte-identical to what the schema
/// emits. `ENVOIX_BINDINGS_REGEN=1` rewrites the artifacts instead.
#[test]
fn generated_artifacts_match_capability_schema() {
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
            "{path} drifted from schema/capability.schema; regenerate with ENVOIX_BINDINGS_REGEN=1"
        );
    }
}

fn frame(step: CapabilityStepView) -> CapabilityFrame {
    CapabilityFrame {
        body: CapabilityBody::Exchange(CapabilityExchangeView {
            capability: CapabilityRequestView::ScanInvite,
            step,
        }),
    }
}

fn declined(reason: DeclinedView) -> CapabilityFrame {
    frame(CapabilityStepView::Declined(DeclinedReasonView { reason }))
}

/// The round trip both peers depend on, for every step the contract has.
#[test]
fn every_exchange_step_round_trips() {
    let steps = [
        CapabilityStepView::Requested,
        CapabilityStepView::Provided(ScannedTextView {
            text: Secret::new("envoix://qr/1/abc".to_owned()),
        }),
        CapabilityStepView::Declined(DeclinedReasonView {
            reason: DeclinedView::Cancelled,
        }),
        CapabilityStepView::Declined(DeclinedReasonView {
            reason: DeclinedView::Refused,
        }),
        CapabilityStepView::Declined(DeclinedReasonView {
            reason: DeclinedView::Unsupported,
        }),
    ];
    for step in steps {
        let original = frame(step);
        let encoded = encode_capability_frame(&original).expect("encodes");
        let decoded = decode_capability_frame(&encoded).expect("decodes");
        assert_eq!(decoded, original);
    }
}

/// Declining is an ANSWER, not an error: all three reasons decode as ordinary
/// values, and a frontend can tell them apart. This is the property a desktop
/// or CLI adapter relies on — it answers `unsupported` and is a full peer.
#[test]
fn the_three_declines_are_three_distinct_answers() {
    let reasons = [
        DeclinedView::Cancelled,
        DeclinedView::Refused,
        DeclinedView::Unsupported,
    ];
    let encoded: Vec<String> = reasons
        .iter()
        .map(|reason| {
            String::from_utf8(encode_capability_frame(&declined(*reason)).expect("encodes"))
                .expect("utf8")
        })
        .collect();
    for (index, text) in encoded.iter().enumerate() {
        for (other, another) in encoded.iter().enumerate() {
            assert_eq!(
                index == other,
                text == another,
                "each decline must have its own spelling on the wire"
            );
        }
    }
    // And each survives the trip as itself rather than collapsing into a
    // generic failure.
    for reason in reasons {
        let decoded =
            decode_capability_frame(&encode_capability_frame(&declined(reason)).expect("encodes"))
                .expect("decodes");
        match decoded.body {
            CapabilityBody::Exchange(exchange) => match exchange.step {
                CapabilityStepView::Declined(view) => assert_eq!(view.reason, reason),
                _ => panic!("a decline must decode as a decline"),
            },
        }
    }
}

/// The scanned text is carried at the join intent's own bound, so text a
/// scanner can read is text the create-join call can carry: an over-long scan
/// is refused by the AUTHORITY rather than by whichever encoder saw it first.
#[test]
fn scanned_text_is_bounded_at_the_join_intents_own_limit() {
    fn text_bound(schema: &str, decl_name: &str, field_name: &str) -> u32 {
        let doc = envoix_bindings::parse_schema(schema).expect("schema parses");
        let Some(Decl::Struct(decl)) = doc.find(decl_name) else {
            panic!("{decl_name} is not a struct");
        };
        let field = decl
            .fields
            .iter()
            .find(|field| field.name == field_name)
            .expect("field exists");
        match field.ty {
            FieldTy::Str { max_bytes } => max_bytes,
            _ => panic!("{decl_name}.{field_name} is not bounded text"),
        }
    }
    // The claim the schema comment makes, enforced: text a scanner can carry is
    // text the create-join intent can carry. Were these to drift, a scan could
    // succeed and then be unsendable — refused by an encoder instead of by the
    // authority that owns the grammar.
    let scanned = text_bound(
        envoix_bindings::capability_schema_text(),
        "ScannedTextView",
        "text",
    );
    let joinable = text_bound(
        envoix_bindings::command_schema_text(),
        "JoinInviteView",
        "invite",
    );
    assert_eq!(
        scanned, joinable,
        "a scan must be bounded exactly where the join intent is"
    );

    let widest = "a".repeat(scanned as usize);
    let accepted = frame(CapabilityStepView::Provided(ScannedTextView {
        text: Secret::new(widest.clone()),
    }));
    encode_capability_frame(&accepted).expect("the widest carriable scan encodes");

    let overlong = frame(CapabilityStepView::Provided(ScannedTextView {
        text: Secret::new(format!("{widest}a")),
    }));
    assert!(
        matches!(
            encode_capability_frame(&overlong),
            Err(CapabilityError::Bound { .. })
        ),
        "one byte past the bound is a typed refusal, not a truncation"
    );
}

/// The scanned invite is sealed exactly as the published link is: it IS the
/// pairing password, so it must not be spellable by an accidental log line.
#[test]
fn a_scanned_invite_is_redacted_by_its_own_type() {
    let secret = Secret::new("envoix://qr/1/the-password".to_owned());
    let rendered = format!("{secret:?}");
    assert!(
        !rendered.contains("the-password"),
        "a scanned invite must not render its text"
    );
    assert!(rendered.contains("redacted"));
}

/// Nothing about a camera can be spelled on this contract. The schema's scalar
/// vocabulary has no bytes/blob and no handle/path/URI type, which is what
/// keeps a platform's implementation out of the contract that names its
/// capability — and is why a SwiftUI peer owes nothing to the Android one.
#[test]
fn the_capability_contract_can_carry_no_frame_handle_or_blob() {
    fn carries_only_bounded_scalars(ty: &FieldTy) -> bool {
        match ty {
            FieldTy::Str { .. }
            | FieldTy::Ascii { .. }
            | FieldTy::U16
            | FieldTy::U32
            | FieldTy::U63
            | FieldTy::Hex16
            | FieldTy::Hex32
            | FieldTy::Hex64
            | FieldTy::HexVar { .. }
            | FieldTy::Named(_) => true,
            FieldTy::Option(inner) => carries_only_bounded_scalars(inner),
            FieldTy::List { element, .. } => carries_only_bounded_scalars(element),
        }
    }
    let doc = doc();
    for decl in &doc.decls {
        let Decl::Struct(decl) = decl else { continue };
        for field in &decl.fields {
            assert!(
                carries_only_bounded_scalars(&field.ty),
                "{}.{} escapes the bounded vocabulary",
                decl.name,
                field.name
            );
        }
    }
    // The words a platform implementation is made of never appear.
    let text = envoix_bindings::capability_schema_text().to_lowercase();
    for forbidden in [
        "cameracharacteristics",
        "avcapture",
        "zxing",
        "surfacetexture",
        "resolution",
        "megapixel",
    ] {
        assert!(
            !text.contains(forbidden),
            "the shared contract must not name a platform mechanism: {forbidden}"
        );
    }
}

/// Both peers here are NATIVE — a frontend asks and its adapter answers — so
/// each needs an encoder AND a decoder for every declaration. The generator
/// gives native artifacts encoders for one marked arm's reachable payload, so
/// this pins that the arm chosen reaches everything: an adapter that could
/// decode a request but not encode an answer would be a contract nobody can
/// implement, and the failure would be silent in the emitter rather than loud
/// here.
///
/// Asserted against the EMITTED ARTIFACTS rather than the generator's internal
/// reachability walk, because what a SwiftUI engineer receives is the file.
#[test]
fn every_native_artifact_can_both_ask_and_answer() {
    let doc = doc();
    let spoken = [
        "CapabilityExchangeView",
        "CapabilityRequestView",
        "CapabilityStepView",
        "ScannedTextView",
        "DeclinedReasonView",
        "DeclinedView",
    ];
    let natives = [
        ("dart", emit::dart::module(&doc), "_encode", "_decode"),
        ("kotlin", emit::kotlin::module(&doc), "encode", "decode"),
        ("swift", emit::swift::module(&doc), "encode", "decode"),
    ];
    for (language, source, encode, decode) in natives {
        for name in spoken {
            assert!(
                source.contains(&format!("{encode}{name}")),
                "{language} cannot encode {name}, so that peer could not speak it"
            );
            assert!(
                source.contains(&format!("{decode}{name}")),
                "{language} cannot decode {name}, so that peer could not hear it"
            );
        }
    }
}

/// The envelope refuses the absurd, and the refusal is typed rather than a
/// panic — the same containment burden every other contract carries.
#[test]
fn hostile_bytes_are_typed_refusals() {
    for hostile in [
        b"".as_slice(),
        b"{",
        b"null",
        b"{\"schema\":\"envoix/binding/capability/1\"}",
        b"{\"schema\":\"envoix/binding/capability/2\",\"body\":{\"kind\":\"exchange\"}}",
        b"{\"schema\":\"envoix/binding/capability/1\",\"body\":{\"kind\":\"nope\"}}",
    ] {
        assert!(
            decode_capability_frame(hostile).is_err(),
            "hostile input must be refused, not believed"
        );
    }
    assert_eq!(CAPABILITY_SCHEMA_ID, "envoix/binding/capability/1");
    // The envelope must be able to carry the widest thing the contract permits,
    // or the bound below it would be unreachable and a legal scan would be
    // refused by the frame cap instead of admitted.
    let widest = frame(CapabilityStepView::Provided(ScannedTextView {
        text: Secret::new("a".repeat(16_384)),
    }));
    assert!(
        encode_capability_frame(&widest)
            .expect("the widest legal exchange encodes")
            .len()
            <= CAPABILITY_MAX_FRAME_BYTES,
        "the frame cap must admit every exchange the field bounds allow"
    );
}

/// An unknown capability or decline reason is a decode failure rather than a
/// silently-dropped field, so a newer adapter cannot make an older frontend
/// believe something it has no name for.
#[test]
fn unknown_vocabulary_is_refused() {
    let good = String::from_utf8(
        encode_capability_frame(&declined(DeclinedView::Cancelled)).expect("encodes"),
    )
    .expect("utf8");
    let unknown_reason = good.replace("cancelled", "exploded");
    assert!(decode_capability_frame(unknown_reason.as_bytes()).is_err());
    let unknown_capability = good.replace("scan_invite", "scan_face");
    assert!(decode_capability_frame(unknown_capability.as_bytes()).is_err());
}
