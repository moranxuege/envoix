use envoix_outcomes::OutcomeCode;
use envoix_types::{AttemptGen, RecordId, RequestId};

use crate::{
    Admission, Duty, DutyKind, DutyLedger, DutyProvenance, DutyReport, DutyResult,
    GenerationUpdate, Registration, SourceAcquisitionKey,
};

fn provenance(card: u64, generation: u32, request: u128) -> DutyProvenance {
    DutyProvenance {
        card: RecordId::new(card),
        generation: AttemptGen::new(generation),
        request: RequestId::from_bytes(request.to_be_bytes()),
    }
}

fn duty(card: u64, generation: u32, request: u128, kind: DutyKind) -> Duty {
    Duty {
        provenance: provenance(card, generation, request),
        kind,
    }
}

fn result(duty: Duty, outcome: OutcomeCode) -> DutyResult {
    DutyResult {
        provenance: duty.provenance,
        report: DutyReport::Outcome(outcome),
    }
}

#[test]
fn stale_or_duplicate_duty_rejected() {
    let mut ledger = DutyLedger::new();
    let card = RecordId::new(7);
    assert_eq!(
        ledger.advance_generation(card, AttemptGen::new(1)),
        GenerationUpdate::Initialized
    );

    let publish = duty(7, 1, 1, DutyKind::Publication);
    let notify = duty(7, 1, 2, DutyKind::Notification);
    assert_eq!(ledger.register(publish), Registration::Registered);
    assert_eq!(ledger.register(notify), Registration::Registered);

    let publish_result = result(publish, OutcomeCode::Completed);
    let Admission::Fresh(admitted) = ledger.admit(publish_result.clone()) else {
        panic!("first matching result must be fresh");
    };
    assert_eq!(admitted.duty(), publish);
    assert_eq!(admitted.outcome(), Some(OutcomeCode::Completed));
    assert_eq!(ledger.admit(publish_result), Admission::Duplicate);

    let Admission::Fresh(admitted) = ledger.admit(result(notify, OutcomeCode::Completed)) else {
        panic!("independent request must remain fresh");
    };
    assert_eq!(admitted.duty(), notify);
    assert_eq!(admitted.outcome(), Some(OutcomeCode::Completed));

    let pending = duty(7, 1, 3, DutyKind::Courier);
    assert_eq!(ledger.register(pending), Registration::Registered);
    assert_eq!(
        ledger.advance_generation(card, AttemptGen::new(2)),
        GenerationUpdate::Advanced
    );
    assert_eq!(ledger.outstanding_len(), 0);
    assert_eq!(
        ledger.admit(result(pending, OutcomeCode::Completed)),
        Admission::Stale
    );

    let never_registered = DutyResult {
        provenance: provenance(7, 1, 99),
        report: DutyReport::Outcome(OutcomeCode::Completed),
    };
    assert_eq!(ledger.admit(never_registered), Admission::Stale);

    let unknown_current = DutyResult {
        provenance: provenance(7, 2, 100),
        report: DutyReport::Outcome(OutcomeCode::Completed),
    };
    assert_eq!(ledger.admit(unknown_current), Admission::Unknown);
}

#[test]
fn registration_requires_the_authoritative_current_generation() {
    let mut ledger = DutyLedger::new();
    let current = duty(4, 3, 1, DutyKind::Staging);

    assert_eq!(ledger.register(current), Registration::NoCurrentGeneration);
    assert_eq!(
        ledger.advance_generation(RecordId::new(4), AttemptGen::new(3)),
        GenerationUpdate::Initialized
    );
    assert_eq!(
        ledger.current_generation(RecordId::new(4)),
        Some(AttemptGen::new(3))
    );
    assert_eq!(
        ledger.register(duty(4, 2, 2, DutyKind::Grant)),
        Registration::StaleGeneration
    );
    assert_eq!(
        ledger.register(duty(4, 4, 3, DutyKind::Grant)),
        Registration::FutureGeneration
    );
    assert_eq!(ledger.register(current), Registration::Registered);
    assert_eq!(ledger.register(current), Registration::AlreadyOutstanding);

    // ADMITTED is not DONE. An answer in flight leaves the duty pending, so
    // nothing dispatches the work a second time while its result is being
    // applied — and it is not discharged, because nothing has yet said the
    // result was acted on.
    assert!(matches!(
        ledger.admit(result(current, OutcomeCode::Completed)),
        Admission::Fresh(_)
    ));
    assert_eq!(ledger.register(current), Registration::AlreadyOutstanding);
    assert_eq!(
        ledger.admit(result(current, OutcomeCode::Completed)),
        Admission::Duplicate,
        "a second answer was admitted while the first was in flight"
    );
    ledger.finalize(current.provenance);
    assert_eq!(ledger.register(current), Registration::AlreadyDischarged);
    assert_eq!(
        ledger.advance_generation(RecordId::new(4), AttemptGen::new(2)),
        GenerationUpdate::RejectedRegression
    );
}

/// A result that did not reach product state leaves the duty OUTSTANDING.
///
/// One-phase admission discharged immediately, so a delivery that failed was
/// recorded as done: the platform was told its work had landed, and the same
/// answer re-reported was refused as a duplicate of something nothing had acted
/// on. Only a restart cleared it, and only because this ledger is process memory.
#[test]
fn an_abandoned_result_can_be_reported_again() {
    let mut ledger = DutyLedger::new();
    let duty = duty(9, 1, 1, DutyKind::Lock);
    assert_eq!(
        ledger.advance_generation(RecordId::new(9), AttemptGen::new(1)),
        GenerationUpdate::Initialized
    );
    assert_eq!(ledger.register(duty), Registration::Registered);
    assert!(matches!(
        ledger.admit(result(duty, OutcomeCode::Completed)),
        Admission::Fresh(_)
    ));

    ledger.abandon(duty.provenance);

    assert!(
        matches!(
            ledger.admit(result(duty, OutcomeCode::Completed)),
            Admission::Fresh(_)
        ),
        "an abandoned result could not be reported again"
    );
    // And finalizing THAT one is what ends it.
    ledger.finalize(duty.provenance);
    assert_eq!(
        ledger.admit(result(duty, OutcomeCode::Completed)),
        Admission::Duplicate
    );
}

#[test]
fn generation_advance_is_scoped_to_one_card() {
    let mut ledger = DutyLedger::new();
    let first = duty(1, 1, 1, DutyKind::Lock);
    let second = duty(2, 1, 2, DutyKind::Foreground);

    ledger.advance_generation(RecordId::new(1), AttemptGen::new(1));
    ledger.advance_generation(RecordId::new(2), AttemptGen::new(1));
    assert_eq!(ledger.register(first), Registration::Registered);
    assert_eq!(ledger.register(second), Registration::Registered);

    ledger.advance_generation(RecordId::new(1), AttemptGen::new(2));
    assert_eq!(ledger.outstanding_len(), 1);
    assert_eq!(
        ledger.admit(result(first, OutcomeCode::Completed)),
        Admission::Stale
    );
    assert!(matches!(
        ledger.admit(result(second, OutcomeCode::Completed)),
        Admission::Fresh(_)
    ));
}

#[test]
fn mismatched_or_future_provenance_is_unknown() {
    let mut ledger = DutyLedger::new();
    let registered = duty(8, 2, 5, DutyKind::OpenShare);
    ledger.advance_generation(RecordId::new(8), AttemptGen::new(2));
    ledger.advance_generation(RecordId::new(9), AttemptGen::new(2));
    assert_eq!(ledger.register(registered), Registration::Registered);

    let wrong_card = DutyResult {
        provenance: provenance(9, 2, 5),
        report: DutyReport::Outcome(OutcomeCode::Completed),
    };
    let future = DutyResult {
        provenance: provenance(8, 3, 5),
        report: DutyReport::Outcome(OutcomeCode::Completed),
    };
    assert_eq!(ledger.admit(wrong_card), Admission::Unknown);
    assert_eq!(ledger.admit(future), Admission::Unknown);
    assert_eq!(ledger.outstanding_len(), 1);
}

#[test]
fn durable_duty_round_trips_for_every_capability_domain() {
    assert_eq!(DutyKind::ALL.len(), 9);

    for (index, kind) in DutyKind::ALL.into_iter().enumerate() {
        let duty = duty(12, 6, index as u128 + 1, kind);
        let encoded = serde_json::to_vec(&duty).expect("duty should serialize");
        let decoded: Duty = serde_json::from_slice(&encoded).expect("duty should deserialize");
        assert_eq!(decoded, duty);
    }
}

// ---- the identity of one source acquisition ----

fn key_provenance(card: u64, generation: u32, request: u8) -> DutyProvenance {
    DutyProvenance {
        card: RecordId::new(card),
        generation: AttemptGen::new(generation),
        request: RequestId::from_bytes([request; 16]),
    }
}

/// The whole key, or it is a different acquisition.
///
/// The Tier 0 review found a picked document held under no identity at all, so
/// whichever card asked first consumed it. Keying on the card alone would keep
/// that bug with extra steps: a re-pick advances the generation, and a late
/// answer to the superseded request must not bind a document to an attempt that
/// has moved on.
#[test]
fn a_source_key_differs_when_any_of_its_three_parts_does() {
    let key = SourceAcquisitionKey::of(key_provenance(7, 2, 0xab));
    assert!(key.is(&SourceAcquisitionKey::of(key_provenance(7, 2, 0xab))));

    for other in [
        key_provenance(8, 2, 0xab), // another card
        key_provenance(7, 3, 0xab), // the same card, after a re-pick
        key_provenance(7, 2, 0xac), // the same attempt, a different request
    ] {
        let other = SourceAcquisitionKey::of(other);
        assert!(
            !key.is(&other),
            "{other:?} is not the acquisition {key:?} names"
        );
    }
}

/// The key round-trips to the provenance the duty wire carries, so promoting it
/// to a name costs the lane nothing.
#[test]
fn a_source_key_is_the_duty_provenance_it_came_from() {
    let provenance = key_provenance(0x5150, 9, 0x11);
    let key = SourceAcquisitionKey::of(provenance);
    assert_eq!(key.provenance(), provenance);
    assert_eq!(key.card(), provenance.card);
    assert_eq!(key.generation(), provenance.generation);
    assert_eq!(key.request(), provenance.request);
}
