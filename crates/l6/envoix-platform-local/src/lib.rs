//! Local CLI/desktop capability adapter.
//!
//! A local frontend has no camera service. It still answers the generated
//! capability conversation: declining `scan_invite` as `unsupported` lets the
//! frontend keep paste available without turning a missing camera into a
//! transport failure.

#![forbid(unsafe_code)]

pub mod identifiers;

use envoix_bindings::capability::{
    CapabilityBody, CapabilityError, CapabilityExchangeView, CapabilityFrame, CapabilityStepView,
    DeclinedReasonView, DeclinedView, decode_capability_frame, encode_capability_frame,
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
/// The only capability in version 1 is invite scanning. A desktop CLI has no
/// camera adapter, so it declines that request as `unsupported`. The reply
/// remains a successful generated-contract exchange rather than an error.
pub fn answer_capability(bytes: &[u8]) -> Result<Vec<u8>, CapabilityAdapterError> {
    let CapabilityBody::Exchange(exchange) = decode_capability_frame(bytes)
        .map_err(CapabilityAdapterError::Contract)?
        .body;
    if exchange.step != CapabilityStepView::Requested {
        return Err(CapabilityAdapterError::NotARequest);
    }
    encode_capability_frame(&CapabilityFrame {
        body: CapabilityBody::Exchange(CapabilityExchangeView {
            capability: exchange.capability,
            step: CapabilityStepView::Declined(DeclinedReasonView {
                reason: DeclinedView::Unsupported,
            }),
        }),
    })
    .map_err(CapabilityAdapterError::Contract)
}

#[cfg(test)]
mod tests {
    use envoix_bindings::capability::{
        CapabilityBody, CapabilityExchangeView, CapabilityFrame, CapabilityRequestView,
        CapabilityStepView, DeclinedReasonView, DeclinedView, decode_capability_frame,
        encode_capability_frame,
    };

    use super::{CapabilityAdapterError, answer_capability};

    #[test]
    fn every_published_local_capability_gets_an_honest_answer() {
        assert_eq!(
            CapabilityRequestView::ALL,
            [CapabilityRequestView::ScanInvite],
            "a new capability needs an explicit local-platform answer"
        );
        for capability in CapabilityRequestView::ALL {
            let request = encode_capability_frame(&CapabilityFrame {
                body: CapabilityBody::Exchange(CapabilityExchangeView {
                    capability,
                    step: CapabilityStepView::Requested,
                }),
            })
            .expect("the generated request encodes");
            let answer = decode_capability_frame(
                &answer_capability(&request).expect("a supported request shape is answered"),
            )
            .expect("the local answer uses the generated codec");
            assert_eq!(
                answer.body,
                CapabilityBody::Exchange(CapabilityExchangeView {
                    capability,
                    step: CapabilityStepView::Declined(DeclinedReasonView {
                        reason: DeclinedView::Unsupported,
                    }),
                })
            );
        }
    }

    #[test]
    fn an_answer_cannot_be_presented_as_a_request() {
        let answer = encode_capability_frame(&CapabilityFrame {
            body: CapabilityBody::Exchange(CapabilityExchangeView {
                capability: CapabilityRequestView::ScanInvite,
                step: CapabilityStepView::Declined(DeclinedReasonView {
                    reason: DeclinedView::Unsupported,
                }),
            }),
        })
        .expect("the generated answer encodes");
        assert_eq!(
            answer_capability(&answer),
            Err(CapabilityAdapterError::NotARequest)
        );
    }
}
