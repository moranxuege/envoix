use envoix_client::model::{RememberedGenerationRole, remembered_generation_attempts};

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
