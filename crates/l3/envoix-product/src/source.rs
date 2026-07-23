use crate::SourceDecision;

/// Selects the authoritative send source. An app-owned artifact always wins;
/// a platform source is consulted only when no complete artifact exists.
pub const fn resolve_source(
    owned_artifact_ready: bool,
    platform_source_recoverable: Option<bool>,
) -> SourceDecision {
    if owned_artifact_ready {
        SourceDecision::Ready
    } else if let Some(recoverable) = platform_source_recoverable {
        SourceDecision::Stage { recoverable }
    } else {
        SourceDecision::NeedsRepick
    }
}
