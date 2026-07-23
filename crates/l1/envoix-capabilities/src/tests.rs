use envoix_outcomes::OutcomeCode;
use envoix_types::{AttemptGen, RecordId, RequestId};

use crate::{
    Admission, Duty, DutyKind, DutyLedger, DutyProvenance, DutyResult, GenerationUpdate,
    Registration,
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
        outcome,
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
    let Admission::Fresh(admitted) = ledger.admit(publish_result) else {
        panic!("first matching result must be fresh");
    };
    assert_eq!(admitted.duty(), publish);
    assert_eq!(admitted.outcome(), OutcomeCode::Completed);
    assert_eq!(ledger.admit(publish_result), Admission::Duplicate);

    let Admission::Fresh(admitted) = ledger.admit(result(notify, OutcomeCode::Completed)) else {
        panic!("independent request must remain fresh");
    };
    assert_eq!(admitted.duty(), notify);
    assert_eq!(admitted.outcome(), OutcomeCode::Completed);

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
        outcome: OutcomeCode::Completed,
    };
    assert_eq!(ledger.admit(never_registered), Admission::Stale);

    let unknown_current = DutyResult {
        provenance: provenance(7, 2, 100),
        outcome: OutcomeCode::Completed,
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

    assert!(matches!(
        ledger.admit(result(current, OutcomeCode::Completed)),
        Admission::Fresh(_)
    ));
    assert_eq!(ledger.register(current), Registration::AlreadyDischarged);
    assert_eq!(
        ledger.advance_generation(RecordId::new(4), AttemptGen::new(2)),
        GenerationUpdate::RejectedRegression
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
        outcome: OutcomeCode::Completed,
    };
    let future = DutyResult {
        provenance: provenance(8, 3, 5),
        outcome: OutcomeCode::Completed,
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
