//! Local CLI/desktop capability adapter.
//!
//! A local frontend has no camera service. It still answers the generated
//! capability conversation: declining `scan_invite` as `unsupported` lets the
//! frontend keep paste available without turning a missing camera into a
//! transport failure.

#![forbid(unsafe_code)]

pub mod identifiers;

use envoix_bindings::capability::{
    CapabilityBody, CapabilityError, CapabilityExchangeView, CapabilityFrame, DeclinedReasonView,
    DeclinedView, PickSourceExchangeView, PickSourceFailureReasonView, PickSourceFailureView,
    PickSourceStepView, ScanInviteExchangeView, ScanInviteStepView, decode_capability_frame,
    encode_capability_frame,
};

/// A malformed or directionally-invalid request at the local adapter boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityAdapterError {
    Contract(CapabilityError),
    NotARequest,
}

impl std::fmt::Display for CapabilityAdapterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Contract(error) => write!(formatter, "capability contract error: {error:?}"),
            Self::NotARequest => formatter.write_str("the capability frame is not a request"),
        }
    }
}

impl std::error::Error for CapabilityAdapterError {}

/// Answers one generated capability request with honest local-platform truth.
///
/// Answered PER CAPABILITY, not blanket-declined. This build has no camera and
/// no document picker, and those are two different sentences: a scanner this
/// platform has nothing to say yes with is `unsupported`, while a picker that is
/// simply not built here is `picker_unavailable` — an answer in the pick's own
/// vocabulary, which exists so a frontend does not have to read "you cancelled"
/// as "there is nothing to cancel with".
///
/// Neither is a statement that a desktop CANNOT do these things. A desktop has a
/// filesystem and often a camera; when either is implemented it replaces one
/// match arm here, and nothing in this file has to be argued with first.
pub fn answer_capability(bytes: &[u8]) -> Result<Vec<u8>, CapabilityAdapterError> {
    let CapabilityBody::Exchange(exchange) = decode_capability_frame(bytes)
        .map_err(CapabilityAdapterError::Contract)?
        .body;
    let answered = match exchange {
        CapabilityExchangeView::ScanInvite(scan) => {
            if scan.step != ScanInviteStepView::Requested {
                return Err(CapabilityAdapterError::NotARequest);
            }
            CapabilityExchangeView::ScanInvite(ScanInviteExchangeView {
                step: ScanInviteStepView::Declined(DeclinedReasonView {
                    reason: DeclinedView::Unsupported,
                }),
            })
        }
        CapabilityExchangeView::PickSource(pick) => {
            if pick.step != PickSourceStepView::Requested {
                return Err(CapabilityAdapterError::NotARequest);
            }
            // The acquisition is echoed back unchanged. Even a refusal names
            // which ask it refused, so a frontend waiting on two cards can tell
            // them apart.
            CapabilityExchangeView::PickSource(PickSourceExchangeView {
                acquisition: pick.acquisition,
                step: PickSourceStepView::Failed(PickSourceFailureReasonView {
                    reason: PickSourceFailureView::PickerUnavailable,
                }),
            })
        }
    };
    encode_capability_frame(&CapabilityFrame {
        body: CapabilityBody::Exchange(answered),
    })
    .map_err(CapabilityAdapterError::Contract)
}

#[cfg(test)]
mod tests {
    use envoix_bindings::capability::{
        CapabilityBody, CapabilityExchangeView, CapabilityFrame, DeclinedReasonView, DeclinedView,
        PickSourceExchangeView, PickSourceFailureReasonView, PickSourceFailureView,
        PickSourceStepView, ScanInviteExchangeView, ScanInviteStepView, SourceAcquisitionKeyView,
        decode_capability_frame, encode_capability_frame,
    };

    use super::{CapabilityAdapterError, answer_capability};

    fn acquisition() -> SourceAcquisitionKeyView {
        SourceAcquisitionKeyView {
            card: "00000000000000ab".to_owned(),
            generation: 7,
            request: "0123456789abcdef0123456789abcdef".to_owned(),
        }
    }

    fn answered(request: &CapabilityExchangeView) -> CapabilityExchangeView {
        let encoded = encode_capability_frame(&CapabilityFrame {
            body: CapabilityBody::Exchange(request.clone()),
        })
        .expect("the generated request encodes");
        let CapabilityBody::Exchange(answer) = decode_capability_frame(
            &answer_capability(&encoded).expect("a request shape is answered"),
        )
        .expect("the local answer uses the generated codec")
        .body;
        answer
    }

    /// Every capability gets its OWN answer, in its own vocabulary. A blanket
    /// decline would say "you cancelled" for a picker that was never built.
    #[test]
    fn every_published_local_capability_gets_an_honest_answer() {
        assert_eq!(
            answered(&CapabilityExchangeView::ScanInvite(
                ScanInviteExchangeView {
                    step: ScanInviteStepView::Requested,
                }
            )),
            CapabilityExchangeView::ScanInvite(ScanInviteExchangeView {
                step: ScanInviteStepView::Declined(DeclinedReasonView {
                    reason: DeclinedView::Unsupported,
                }),
            })
        );
        assert_eq!(
            answered(&CapabilityExchangeView::PickSource(
                PickSourceExchangeView {
                    acquisition: acquisition(),
                    step: PickSourceStepView::Requested,
                }
            )),
            CapabilityExchangeView::PickSource(PickSourceExchangeView {
                // Echoed unchanged, so even a refusal says which ask it refused.
                acquisition: acquisition(),
                step: PickSourceStepView::Failed(PickSourceFailureReasonView {
                    reason: PickSourceFailureView::PickerUnavailable,
                }),
            })
        );
    }

    #[test]
    fn an_answer_cannot_be_presented_as_a_request() {
        let answer = encode_capability_frame(&CapabilityFrame {
            body: CapabilityBody::Exchange(CapabilityExchangeView::ScanInvite(
                ScanInviteExchangeView {
                    step: ScanInviteStepView::Declined(DeclinedReasonView {
                        reason: DeclinedView::Unsupported,
                    }),
                },
            )),
        })
        .expect("the generated answer encodes");
        assert_eq!(
            answer_capability(&answer),
            Err(CapabilityAdapterError::NotARequest)
        );
    }
}
