//! Provider-level transport selection.
//!
//! A provider is the mechanism that creates the authenticated byte channel
//! (`iroh`, Wi-Fi Aware, or a future wired channel). This is deliberately one
//! level above [`PathPolicy`](super::PathPolicy): direct/relay is an iroh path
//! decision and must not be used to represent a different provider.

use std::fmt;

/// Provider priority used by automatic selection.
///
/// A candidate is present only when it can reach the selected peer. Within
/// that peer-specific set, a physical cable wins over Wi-Fi Aware, which wins
/// over the general-purpose iroh path.
const DEFAULT_PROVIDER_ORDER: [TransportProvider; 3] = [
    TransportProvider::Wired,
    TransportProvider::WifiAware,
    TransportProvider::Iroh,
];

/// Provider adapters compiled into the current client.
///
/// Platform capability probes are necessary inputs for Wi-Fi Aware, but they
/// do not make the provider selectable before its connection adapter exists.
pub(crate) const BUILT_IN_TRANSPORT_CANDIDATES: [TransportCandidate; 3] = [
    TransportCandidate::new(
        TransportProvider::Wired,
        TransportAvailability::ImplementationPending,
    ),
    TransportCandidate::new(
        TransportProvider::WifiAware,
        TransportAvailability::ImplementationPending,
    ),
    TransportCandidate::new(TransportProvider::Iroh, TransportAvailability::Ready),
];

/// A mechanism capable of establishing an Envoix data channel.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum TransportProvider {
    /// Raw wired transport, such as a future USB bulk/accessory channel.
    Wired,
    /// Apple/Android Wi-Fi Aware (NAN) data path.
    WifiAware,
    /// Existing iroh QUIC transport, including its direct and relay paths.
    Iroh,
}

impl TransportProvider {
    /// Stable diagnostic/configuration name.
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Wired => "wired",
            Self::WifiAware => "wifi_aware",
            Self::Iroh => "iroh",
        }
    }
}

impl fmt::Display for TransportProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.wire_name())
    }
}

/// Structured readiness of one provider adapter.
///
/// `Ready` means both the adapter and a peer-specific candidate are usable;
/// global hardware capability alone is not enough.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum TransportAvailability {
    Ready,
    UnsupportedOs,
    UnsupportedHardware,
    EntitlementMissing,
    PermissionRequired,
    PermissionDenied,
    Disabled,
    TemporarilyUnavailable,
    PairingRequired,
    /// The provider is part of the architecture but has no registered adapter
    /// in this build, so it must never be selected.
    ImplementationPending,
}

impl TransportAvailability {
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// Stable diagnostic/configuration name.
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::UnsupportedOs => "unsupported_os",
            Self::UnsupportedHardware => "unsupported_hardware",
            Self::EntitlementMissing => "entitlement_missing",
            Self::PermissionRequired => "permission_required",
            Self::PermissionDenied => "permission_denied",
            Self::Disabled => "disabled",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::PairingRequired => "pairing_required",
            Self::ImplementationPending => "implementation_pending",
        }
    }
}

impl fmt::Display for TransportAvailability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.wire_name())
    }
}

/// One peer-specific provider candidate considered by the selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct TransportCandidate {
    pub provider: TransportProvider,
    pub availability: TransportAvailability,
}

impl TransportCandidate {
    pub const fn new(provider: TransportProvider, availability: TransportAvailability) -> Self {
        Self {
            provider,
            availability,
        }
    }
}

/// Caller policy for provider selection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum TransportPreference {
    /// Use the first ready provider in the default priority order.
    #[default]
    Automatic,
    /// Try this provider first, then fall back to another ready provider.
    Prefer(TransportProvider),
    /// Use only this provider; return a structured error when it is not ready.
    Require(TransportProvider),
}

/// Why the selected provider won.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum TransportSelectionReason {
    Automatic,
    Preferred,
    Required,
    Fallback {
        preferred: TransportProvider,
        preferred_availability: Option<TransportAvailability>,
    },
}

impl TransportSelectionReason {
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Preferred => "preferred",
            Self::Required => "required",
            Self::Fallback { .. } => "fallback",
        }
    }
}

impl fmt::Display for TransportSelectionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.wire_name())
    }
}

/// A deterministic provider decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct TransportSelection {
    pub provider: TransportProvider,
    pub reason: TransportSelectionReason,
}

/// Structured selector failure suitable for setup diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TransportSelectionError {
    DuplicateProvider {
        provider: TransportProvider,
    },
    RequiredProviderUnavailable {
        provider: TransportProvider,
        availability: Option<TransportAvailability>,
    },
    NoReadyProvider {
        candidates: Vec<TransportCandidate>,
    },
}

impl fmt::Display for TransportSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateProvider { provider } => {
                write!(
                    formatter,
                    "duplicate transport provider candidate: {provider}"
                )
            }
            Self::RequiredProviderUnavailable {
                provider,
                availability: Some(availability),
            } => write!(
                formatter,
                "required transport provider {provider} is not ready: {availability}"
            ),
            Self::RequiredProviderUnavailable {
                provider,
                availability: None,
            } => write!(
                formatter,
                "required transport provider {provider} is not registered"
            ),
            Self::NoReadyProvider { candidates } => {
                formatter.write_str("no transport provider is ready")?;
                if !candidates.is_empty() {
                    formatter.write_str(": ")?;
                    for (index, candidate) in candidates.iter().enumerate() {
                        if index > 0 {
                            formatter.write_str(", ")?;
                        }
                        write!(
                            formatter,
                            "{}={}",
                            candidate.provider, candidate.availability
                        )?;
                    }
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for TransportSelectionError {}

/// Stateless, deterministic provider selector.
pub struct TransportSelector;

impl TransportSelector {
    pub fn select(
        preference: TransportPreference,
        candidates: &[TransportCandidate],
    ) -> Result<TransportSelection, TransportSelectionError> {
        validate_unique_candidates(candidates)?;

        match preference {
            TransportPreference::Automatic => first_ready(candidates)
                .map(|provider| TransportSelection {
                    provider,
                    reason: TransportSelectionReason::Automatic,
                })
                .ok_or_else(|| TransportSelectionError::NoReadyProvider {
                    candidates: candidates.to_vec(),
                }),
            TransportPreference::Prefer(preferred) => {
                let preferred_availability = availability_of(preferred, candidates);
                if preferred_availability.is_some_and(TransportAvailability::is_ready) {
                    return Ok(TransportSelection {
                        provider: preferred,
                        reason: TransportSelectionReason::Preferred,
                    });
                }
                first_ready(candidates)
                    .map(|provider| TransportSelection {
                        provider,
                        reason: TransportSelectionReason::Fallback {
                            preferred,
                            preferred_availability,
                        },
                    })
                    .ok_or_else(|| TransportSelectionError::NoReadyProvider {
                        candidates: candidates.to_vec(),
                    })
            }
            TransportPreference::Require(provider) => {
                let availability = availability_of(provider, candidates);
                if availability.is_some_and(TransportAvailability::is_ready) {
                    Ok(TransportSelection {
                        provider,
                        reason: TransportSelectionReason::Required,
                    })
                } else {
                    Err(TransportSelectionError::RequiredProviderUnavailable {
                        provider,
                        availability,
                    })
                }
            }
        }
    }
}

fn validate_unique_candidates(
    candidates: &[TransportCandidate],
) -> Result<(), TransportSelectionError> {
    for (index, candidate) in candidates.iter().enumerate() {
        if candidates[..index]
            .iter()
            .any(|seen| seen.provider == candidate.provider)
        {
            return Err(TransportSelectionError::DuplicateProvider {
                provider: candidate.provider,
            });
        }
    }
    Ok(())
}

fn availability_of(
    provider: TransportProvider,
    candidates: &[TransportCandidate],
) -> Option<TransportAvailability> {
    candidates
        .iter()
        .find(|candidate| candidate.provider == provider)
        .map(|candidate| candidate.availability)
}

fn first_ready(candidates: &[TransportCandidate]) -> Option<TransportProvider> {
    DEFAULT_PROVIDER_ORDER.iter().copied().find(|provider| {
        availability_of(*provider, candidates).is_some_and(TransportAvailability::is_ready)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_selection_uses_stable_priority_not_input_order() {
        let candidates = [
            ready(TransportProvider::Iroh),
            ready(TransportProvider::Wired),
            ready(TransportProvider::WifiAware),
        ];

        assert_eq!(
            TransportSelector::select(TransportPreference::Automatic, &candidates).unwrap(),
            TransportSelection {
                provider: TransportProvider::Wired,
                reason: TransportSelectionReason::Automatic,
            }
        );
    }

    #[test]
    fn automatic_selection_prefers_wifi_aware_over_iroh() {
        let candidates = [
            ready(TransportProvider::Iroh),
            ready(TransportProvider::WifiAware),
            TransportCandidate::new(
                TransportProvider::Wired,
                TransportAvailability::TemporarilyUnavailable,
            ),
        ];

        assert_eq!(
            TransportSelector::select(TransportPreference::Automatic, &candidates)
                .unwrap()
                .provider,
            TransportProvider::WifiAware
        );
    }

    #[test]
    fn preferred_ready_provider_overrides_automatic_priority() {
        let candidates = [
            ready(TransportProvider::Wired),
            ready(TransportProvider::Iroh),
        ];

        assert_eq!(
            TransportSelector::select(
                TransportPreference::Prefer(TransportProvider::Iroh),
                &candidates,
            )
            .unwrap(),
            TransportSelection {
                provider: TransportProvider::Iroh,
                reason: TransportSelectionReason::Preferred,
            }
        );
    }

    #[test]
    fn preferred_provider_falls_back_with_structured_reason() {
        let candidates = [
            TransportCandidate::new(
                TransportProvider::WifiAware,
                TransportAvailability::PairingRequired,
            ),
            ready(TransportProvider::Iroh),
        ];

        assert_eq!(
            TransportSelector::select(
                TransportPreference::Prefer(TransportProvider::WifiAware),
                &candidates,
            )
            .unwrap(),
            TransportSelection {
                provider: TransportProvider::Iroh,
                reason: TransportSelectionReason::Fallback {
                    preferred: TransportProvider::WifiAware,
                    preferred_availability: Some(TransportAvailability::PairingRequired),
                },
            }
        );
    }

    #[test]
    fn required_provider_never_falls_back() {
        let candidates = [
            TransportCandidate::new(
                TransportProvider::WifiAware,
                TransportAvailability::TemporarilyUnavailable,
            ),
            ready(TransportProvider::Iroh),
        ];

        assert_eq!(
            TransportSelector::select(
                TransportPreference::Require(TransportProvider::WifiAware),
                &candidates,
            ),
            Err(TransportSelectionError::RequiredProviderUnavailable {
                provider: TransportProvider::WifiAware,
                availability: Some(TransportAvailability::TemporarilyUnavailable),
            })
        );
    }

    #[test]
    fn required_ready_provider_retains_required_reason() {
        assert_eq!(
            TransportSelector::select(
                TransportPreference::Require(TransportProvider::Iroh),
                &[ready(TransportProvider::Iroh)],
            )
            .unwrap(),
            TransportSelection {
                provider: TransportProvider::Iroh,
                reason: TransportSelectionReason::Required,
            }
        );
    }

    #[test]
    fn unregistered_required_provider_is_distinct_from_unavailable() {
        assert_eq!(
            TransportSelector::select(
                TransportPreference::Require(TransportProvider::Wired),
                &[ready(TransportProvider::Iroh)],
            ),
            Err(TransportSelectionError::RequiredProviderUnavailable {
                provider: TransportProvider::Wired,
                availability: None,
            })
        );
    }

    #[test]
    fn no_ready_provider_preserves_diagnostics() {
        let candidates = [TransportCandidate::new(
            TransportProvider::WifiAware,
            TransportAvailability::PermissionDenied,
        )];

        assert_eq!(
            TransportSelector::select(TransportPreference::Automatic, &candidates),
            Err(TransportSelectionError::NoReadyProvider {
                candidates: candidates.to_vec(),
            })
        );
    }

    #[test]
    fn duplicate_provider_candidates_are_rejected() {
        let candidates = [
            ready(TransportProvider::Iroh),
            TransportCandidate::new(
                TransportProvider::Iroh,
                TransportAvailability::TemporarilyUnavailable,
            ),
        ];

        assert_eq!(
            TransportSelector::select(TransportPreference::Automatic, &candidates),
            Err(TransportSelectionError::DuplicateProvider {
                provider: TransportProvider::Iroh,
            })
        );
    }

    #[test]
    fn current_build_selects_iroh_without_advertising_pending_providers() {
        assert_eq!(
            TransportSelector::select(
                TransportPreference::Automatic,
                &BUILT_IN_TRANSPORT_CANDIDATES,
            )
            .unwrap()
            .provider,
            TransportProvider::Iroh
        );
    }

    #[test]
    fn wire_names_match_platform_capability_contract() {
        assert_eq!(TransportProvider::WifiAware.wire_name(), "wifi_aware");
        assert_eq!(
            TransportAvailability::EntitlementMissing.wire_name(),
            "entitlement_missing"
        );
        assert_eq!(
            TransportAvailability::ImplementationPending.wire_name(),
            "implementation_pending"
        );
    }

    const fn ready(provider: TransportProvider) -> TransportCandidate {
        TransportCandidate::new(provider, TransportAvailability::Ready)
    }
}
