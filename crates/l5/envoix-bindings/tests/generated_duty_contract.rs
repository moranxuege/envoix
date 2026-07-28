//! The duty contract: the authority's orders and the platform executor's
//! reports are one drift-gated vocabulary, not two hand-written codecs that
//! independently decide a JSON shape.

use envoix_bindings::duty::{
    DUTY_MAX_FRAME_BYTES, DutyBody, DutyFrame, DutyOrderView, DutyProvenanceView, DutyReportView,
    LockDirectiveView, LockWorkView, NoticeView, NotificationWorkView, OutcomeCodeView, WorkView,
    decode_duty_frame, encode_duty_frame,
};
use envoix_bindings::{Decl, FieldTy, emit};

fn doc() -> envoix_bindings::SchemaDoc {
    envoix_bindings::parse_schema(envoix_bindings::duty_schema_text()).expect("duty schema parses")
}

fn artifacts(doc: &envoix_bindings::SchemaDoc) -> [(&'static str, String); 4] {
    [
        ("generated/rust/duty.rs", emit::rust::module(doc)),
        ("generated/dart/envoix_duty.dart", emit::dart::module(doc)),
        ("generated/kotlin/EnvoixDuty.kt", emit::kotlin::module(doc)),
        ("generated/swift/EnvoixDuty.swift", emit::swift::module(doc)),
    ]
}

/// Every checked-in duty artifact is byte-identical to what the schema emits.
#[test]
fn generated_artifacts_match_duty_schema() {
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
            "{path} drifted from schema/duty.schema; regenerate with ENVOIX_BINDINGS_REGEN=1"
        );
    }
}

fn provenance() -> DutyProvenanceView {
    DutyProvenanceView {
        card: "00112233445566aa".to_owned(),
        generation: 7,
        request: "efefefefefefefefefefefefefefefef".to_owned(),
    }
}

fn order(work: WorkView) -> DutyFrame {
    DutyFrame {
        body: DutyBody::Order(DutyOrderView {
            provenance: provenance(),
            work,
        }),
    }
}

/// THE defect this contract exists for. Rust encoded `notice` as a string while
/// Kotlin asked `optJSONObject("notice")` and got null, so every notification
/// silently became "Envoix needs your attention". The shape is now the
/// contract's to state, and it states a string.
#[test]
fn a_notice_crosses_as_its_own_value_not_an_object() {
    let encoded = encode_duty_frame(&order(WorkView::Notification(NotificationWorkView {
        notice: NoticeView::TransferComplete,
    })))
    .expect("a notification order encodes");
    let encoded = String::from_utf8(encoded).expect("utf-8");
    assert!(
        encoded.contains(r#""notice":"transfer_complete""#),
        "the notice must be the value at its key, not an object: {encoded}"
    );
    // And the inverse spelling is not merely different, it is refused.
    let as_object = encoded.replace(
        r#""notice":"transfer_complete""#,
        r#""notice":{"kind":"transfer_complete"}"#,
    );
    assert!(
        decode_duty_frame(as_object.as_bytes()).is_err(),
        "an object-shaped notice must be refused, not coerced"
    );
}

/// `Lock { hold: bool }` was the one shape a permissive reader defaults: a
/// missing or mistyped `hold` read as `false`, which is "release the lock" on a
/// live transfer. Two named directives have no default to fall into.
#[test]
fn a_lock_directive_has_no_default_to_fall_into() {
    for directive in [LockDirectiveView::Hold, LockDirectiveView::Release] {
        let frame = order(WorkView::Lock(LockWorkView { directive }));
        let encoded = encode_duty_frame(&frame).expect("a lock order encodes");
        let decoded = decode_duty_frame(&encoded).expect("a lock order decodes");
        assert_eq!(decoded, frame);
    }
    let encoded = encode_duty_frame(&order(WorkView::Lock(LockWorkView {
        directive: LockDirectiveView::Hold,
    })))
    .expect("a lock order encodes");
    let encoded = String::from_utf8(encoded).expect("utf-8");
    // The old boolean spelling, and anything else, is a refusal rather than a
    // silent release.
    for hostile in [r#""directive":false"#, r#""directive":"hold_forever""#] {
        let mutated = encoded.replace(r#""directive":"hold""#, hostile);
        assert!(
            decode_duty_frame(mutated.as_bytes()).is_err(),
            "a lock directive must not accept {hostile}"
        );
    }
}

/// Every arm of today's `Work` round-trips, including the three unit
/// placeholders. A vocabulary that could not carry a shape would push the
/// authority back to a second, unversioned encoding for it.
#[test]
fn every_work_arm_round_trips() {
    let arms = [
        WorkView::SourceHandle,
        WorkView::Grant,
        WorkView::Staging,
        WorkView::Courier,
        WorkView::OpenShare,
        WorkView::Notification(NotificationWorkView {
            notice: NoticeView::ActionNeeded,
        }),
        WorkView::Lock(LockWorkView {
            directive: LockDirectiveView::Release,
        }),
    ];
    for work in arms {
        let frame = order(work);
        let encoded = encode_duty_frame(&frame).expect("an order encodes");
        assert_eq!(
            decode_duty_frame(&encoded).expect("an order decodes"),
            frame
        );
    }
}

/// A report carries an outcome and its provenance, and that is the whole of it.
/// Decoding one is not admitting it — the C6 ledger owns that, and this only
/// proves the claim is well formed.
#[test]
fn a_report_carries_an_outcome_and_nothing_else() {
    for outcome in [
        OutcomeCodeView::Completed,
        OutcomeCodeView::SourceUnreadable,
        OutcomeCodeView::Internal,
    ] {
        let frame = DutyFrame {
            body: DutyBody::Report(DutyReportView {
                provenance: provenance(),
                outcome,
            }),
        };
        let encoded = encode_duty_frame(&frame).expect("a report encodes");
        assert_eq!(
            decode_duty_frame(&encoded).expect("a report decodes"),
            frame
        );
    }
}

/// Containment: nothing on this lane can spell a file, a handle or a path.
/// A duty report answering with bytes is the shape this rules out.
#[test]
fn the_duty_vocabulary_cannot_spell_a_handle() {
    let doc = doc();
    for decl in &doc.decls {
        let Decl::Struct(decl) = decl else { continue };
        for field in &decl.fields {
            let bounded = matches!(
                &field.ty,
                FieldTy::Str { .. }
                    | FieldTy::Ascii { .. }
                    | FieldTy::Hex16
                    | FieldTy::Hex32
                    | FieldTy::U16
                    | FieldTy::U32
                    | FieldTy::U63
                    | FieldTy::Named(_)
            );
            assert!(
                bounded,
                "{}.{} is not a bounded scalar the duty lane admits",
                decl.name, field.name
            );
        }
    }
    assert_eq!(DUTY_MAX_FRAME_BYTES, 4096);
}
