use envoix_client::model::{
    RememberedAttemptOutcome, RememberedGenerationRole, remembered_generation_attempts,
};

#[test]
fn connector_attempts_overlap_the_responders_recovery_window() {
    assert_eq!(
        remembered_generation_attempts(9, None, RememberedGenerationRole::Connector).unwrap(),
        [9, 9]
    );
    assert_eq!(
        remembered_generation_attempts(9, Some(8), RememberedGenerationRole::Connector).unwrap(),
        [9, 9, 8]
    );
}

#[test]
fn responder_attempts_return_to_current_after_previous() {
    assert_eq!(
        remembered_generation_attempts(9, None, RememberedGenerationRole::Responder).unwrap(),
        [9]
    );
    assert_eq!(
        remembered_generation_attempts(9, Some(8), RememberedGenerationRole::Responder).unwrap(),
        [9, 8, 9]
    );
}

#[test]
fn invalid_previous_generations_fail_closed() {
    for role in [
        RememberedGenerationRole::Connector,
        RememberedGenerationRole::Responder,
    ] {
        for previous in [9, 10] {
            assert!(
                remembered_generation_attempts(9, Some(previous), role).is_err(),
                "{role:?} with previous generation {previous}"
            );
        }
    }
}

#[test]
fn only_pre_authentication_failure_may_try_another_generation() {
    for succeeded in [false, true] {
        for authenticated in [false, true] {
            for canceled in [false, true] {
                let outcome = RememberedAttemptOutcome {
                    succeeded,
                    authenticated,
                    canceled,
                };
                assert_eq!(
                    outcome.should_stop_fallback(),
                    succeeded || authenticated || canceled,
                    "succeeded={succeeded}, authenticated={authenticated}, canceled={canceled}"
                );
            }
        }
    }
}
