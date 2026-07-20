use super::*;
use envoix_session::TransferDirection::Send;

/// The snapshot JSON shape is the FFI contract: state/origin flatten to
/// top-level keys the frontend reads with optString.
#[test]
fn session_serializes_flat_state_and_origin() {
    let mut s = Session::new(Send);
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["state"], "connecting");
    assert!(v["origin"].is_null() || v.get("origin").is_none());

    s.state = State::Paused(PauseOrigin::Peer);
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["state"], "paused");
    assert_eq!(v["origin"], "peer");
    // Matches the existing public event stream shape (no rename_all on the
    // wire-adjacent TransferDirection); the app reads direction from its own
    // Spec, not the snapshot.
    assert_eq!(v["direction"], "Send");
}

/// Pins every `State` to the exact wire string the Android `Status` enum
/// maps (`Status.fromWire`, `Transfer.kt`). A rename here breaks this test
/// instead of silently freezing a card on the unmapped string. Keep in sync
/// with the Kotlin `every_wire_string_maps_to_a_status` test.
#[test]
fn every_state_serializes_to_its_wire_string() {
    let cases: &[(State, &str)] = &[
        (State::Preparing, "preparing"),
        (State::Waiting, "waiting"),
        (State::Connecting, "connecting"),
        (State::Verifying, "verifying"),
        (State::Transferring, "transferring"),
        (State::Confirming, "confirming"),
        (State::Paused(PauseOrigin::Local), "paused"),
        (State::Unconfirmed, "unconfirmed"),
        (State::Completed, "completed"),
        (State::Failed, "failed"),
        (State::Cancelled, "cancelled"),
    ];
    for (state, expected) in cases {
        let v = serde_json::to_value(state).unwrap();
        assert_eq!(v["state"].as_str(), Some(*expected), "{state:?}");
    }
}

#[test]
fn kind_labels_name_variants_and_fold_events() {
    assert_eq!(Input::Cancel.kind(), "Cancel");
    assert_eq!(
        Input::StageComplete { generation: 1 }.kind(),
        "StageComplete"
    );
    assert_eq!(Input::ReceiptPosted.kind(), "ReceiptPosted");
    // a core event folds to its AttemptEvent name — the input reads as the
    // fact it carries, not the generic "Event".
    let verified = Input::Event {
        attempt: 1,
        event: AttemptEvent::Verified,
    };
    assert_eq!(verified.kind(), "Verified");
    let progress = Input::Event {
        attempt: 1,
        event: AttemptEvent::Progress { bytes: 0 },
    };
    assert_eq!(progress.kind(), "Progress");
    assert_eq!(Effect::PostReceipt.kind(), "PostReceipt");
    assert_eq!(Effect::StartAttempt { resume: true }.kind(), "StartAttempt");
}
