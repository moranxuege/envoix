pub const PRIVATE_STORAGE_ROOT: &str = "state-envoix/";
pub const OPERATIONS_DIRECTORY: &str = "operations";
pub const ARTIFACTS_DIRECTORY: &str = "artifacts";
pub const EVIDENCE_DIRECTORY: &str = "evidence";
pub const KEYS_DIRECTORY: &str = "keys";

const ACTIONS: &[&str] = &[
    "start",
    "cancel",
    "pause",
    "resume",
    "remove",
    "restore-all",
    "reverify",
    // Debug-only instrumentation (the debug source set's E2eBridge). The names
    // are reserved here so nothing else can claim them.
    "e2e-create",
    "e2e-probe",
];

/// Android actions are derived from the variant application ID owned by Gradle.
pub fn internal_action_identifiers(application_id: &str) -> Vec<String> {
    ACTIONS
        .iter()
        .map(|action| format!("{application_id}.action.{action}"))
        .collect()
}

/// The platform channel the Flutter frontend lane rides on, derived from the
/// gradle-owned namespace so the two cannot drift. It names a slot in one
/// process's message-channel namespace, which a plugin could otherwise claim.
pub fn frontend_lane_channel(namespace: &str) -> String {
    format!("{namespace}/frontend-lane")
}
