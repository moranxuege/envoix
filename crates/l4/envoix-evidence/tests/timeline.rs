use std::num::NonZeroUsize;

use envoix_attempt_api::AttemptStamp;
use envoix_evidence::{
    BUILD_TRUST_MANIFEST, DiagnosticsStatus, EvidenceProgress, EvidenceRecord, EvidenceSink,
    EvidenceValue, MAX_SAFE_DISPLAY_BYTES, RedactedId, TimelineStore, TrustRootFingerprintSlot,
};
use envoix_outcomes::{Outcome, OutcomeCode, Phase, Retryability, SafeDisplay};
use envoix_types::{AttemptGen, ByteCount, RecordId, TransferId};

fn session(card: u64, generation: u32) -> AttemptStamp {
    AttemptStamp {
        card: RecordId::new(card),
        generation: AttemptGen::new(generation),
    }
}

#[test]
fn no_free_form_sensitive_error_in_timeline() {
    let store = TimelineStore::new(NonZeroUsize::new(4).unwrap(), NonZeroUsize::MIN);
    let key = session(7, 2);
    let private_transfer = TransferId::from_bytes([
        0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0xbe, 1, 2, 3, 4, 5, 6, 7, 8,
    ]);
    let known_sensitive = private_transfer.to_string();

    store
        .record(EvidenceRecord::new(
            key,
            EvidenceValue::identifier(RedactedId::transfer(private_transfer)),
        ))
        .unwrap();
    let long_safe_display = "x".repeat(MAX_SAFE_DISPLAY_BYTES + 80);
    let outcome = Outcome::new(
        OutcomeCode::PeerLost,
        Phase::Transferring,
        Retryability::Retryable,
        SafeDisplay::new(long_safe_display),
    );
    store
        .record(EvidenceRecord::new(key, EvidenceValue::outcome(&outcome)))
        .unwrap();

    let timeline = store.snapshot(key).unwrap();
    let encoded = serde_json::to_string(&timeline).unwrap();
    assert!(!encoded.contains(&known_sensitive));
    let EvidenceValue::Outcome(outcome) = timeline.entries()[1].value() else {
        panic!("the second value is the typed outcome");
    };
    assert!(outcome.display().as_str().len() <= MAX_SAFE_DISPLAY_BYTES);
}

#[test]
fn bounded_timeline_marks_diagnostics_degraded_and_evicts_sessions() {
    let store = TimelineStore::new(NonZeroUsize::new(2).unwrap(), NonZeroUsize::new(2).unwrap());
    let first = session(1, 1);
    for phase in [
        Phase::Preparing,
        Phase::Pairing,
        Phase::Authenticating,
        Phase::Transferring,
    ] {
        store
            .record(EvidenceRecord::new(first, EvidenceValue::phase(phase)))
            .unwrap();
    }

    let timeline = store.snapshot(first).unwrap();
    assert_eq!(timeline.entries().len(), 2);
    assert_eq!(timeline.entries()[0].sequence(), 3);
    assert_eq!(timeline.entries()[1].sequence(), 4);
    let DiagnosticsStatus::DiagnosticsDegraded(degraded) = timeline.diagnostics() else {
        panic!("overflow must leave a typed diagnostics_degraded marker");
    };
    assert_eq!(degraded.dropped_events(), 2);

    let second = session(2, 1);
    let third = session(3, 1);
    for key in [second, third] {
        store
            .record(EvidenceRecord::new(
                key,
                EvidenceValue::progress(EvidenceProgress::new(
                    ByteCount::new(1),
                    ByteCount::new(10),
                )),
            ))
            .unwrap();
    }
    assert_eq!(store.session_count(), 2);
    assert!(store.snapshot(first).is_none());
    assert!(store.snapshot(second).is_some());
    assert!(store.snapshot(third).is_some());

    store.evict_card(RecordId::new(2)).unwrap();
    assert!(store.snapshot(second).is_none());
    assert_eq!(store.session_count(), 1);
}

#[test]
fn static_build_trust_manifest_is_typed_and_descriptive() {
    assert_eq!(
        BUILD_TRUST_MANIFEST.package_version,
        env!("CARGO_PKG_VERSION")
    );
    assert!(!BUILD_TRUST_MANIFEST.protocol.data_alpn.is_empty());
    assert!(!BUILD_TRUST_MANIFEST.protocol.data_magic.is_empty());
    assert!(!BUILD_TRUST_MANIFEST.protocol.set_id.is_empty());
    assert!(
        !BUILD_TRUST_MANIFEST
            .abi_schema
            .evidence_timeline_schema_id
            .is_empty()
    );
    assert_eq!(
        BUILD_TRUST_MANIFEST.trust_root,
        TrustRootFingerprintSlot::Unprovisioned
    );
}
