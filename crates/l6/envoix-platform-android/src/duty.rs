//! The host↔service duty protocol: typed work orders out, typed reports back.
//!
//! This is a TRUSTED in-process lane between the Rust host and the Kotlin
//! service that executes platform capabilities. It is not the frontend
//! surface: frontends observe duties through the generated read contract and
//! never execute them. Values still cross a language boundary, so every field
//! Every frame on it is the GENERATED duty contract (`schema/duty.schema`).
//! These types are the domain side; `to_view`/`from_view` are the only crossing,
//! and the generated codec is the only encoder. That is deliberate: this lane
//! previously had a hand-written encoder here and a hand-written decoder in
//! Kotlin, and they disagreed about the shape of a notice for as long as both
//! existed.

use envoix_bindings::duty::{
    DutyAnswerView, DutyBody, DutyError, DutyFrame, DutyOrderView, DutyProvenanceView,
    DutyReportView, ForegroundWorkView, LockDirectiveView, LockWorkView, NoticeView,
    NotificationWorkView, OutcomeCodeView, PublicationWorkView, SourceAcquiredView,
    SourceFailedView, SourceFailureView, SourceReportView, SourceRetentionView,
    SourceSeekabilityView, WorkView, decode_duty_frame, encode_duty_frame,
};
use envoix_capabilities::{
    Duty, DutyKind, DutyProvenance, DutyReport, DutyResult, SourceAcquisitionFailure, SourceReport,
    SourceRetention, SourceSeekability,
};
use envoix_outcomes::OutcomeCode;
use envoix_types::{AttemptGen, OfferedName, RecordId, RequestId};

/// Longest permitted staged-artifact relative path, in UTF-8 bytes.
pub const MAX_STAGED_PATH_BYTES: usize = 512;
/// Longest permitted display name, in UTF-8 bytes. The name this lane carries
/// is the leaf a publication will land under, so the bound is L0's published
/// maximum for that type rather than a fourth statement of it.
pub const MAX_DISPLAY_NAME_BYTES: usize = OfferedName::MAX_BYTES;
/// Hard cap on one encoded order/report crossing the lane.
pub const MAX_LANE_FRAME_BYTES: usize = 4096;

/// Why an encoded work order or report was rejected by this module.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaneError {
    FrameTooLarge,
    Malformed,
    Bounds,
    KindMismatch,
}

/// Duty provenance in its lane encoding: hex identifiers, u32 generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WireProvenance {
    /// `RecordId` as 16 lowercase hex digits.
    pub card: WireHex16,
    pub generation: u32,
    /// `RequestId` as 32 lowercase hex digits.
    pub request: WireHex32,
}

impl WireProvenance {
    pub fn from_provenance(provenance: DutyProvenance) -> Self {
        Self {
            card: WireHex16::from_value(provenance.card.get()),
            generation: provenance.generation.get(),
            request: WireHex32::from_value(u128::from_be_bytes(provenance.request.to_bytes())),
        }
    }

    pub fn to_provenance(self) -> DutyProvenance {
        DutyProvenance {
            card: RecordId::new(self.card.value()),
            generation: AttemptGen::new(self.generation),
            request: RequestId::from_bytes(self.request.value().to_be_bytes()),
        }
    }

    /// The deterministic publication recovery key. It names the same pending
    /// MediaStore row across a crash, so replay reuses that row instead of
    /// inserting a duplicate. The Kotlin executor derives the identical string
    /// from the wire provenance it receives (`<card>-<gen:08x>-<request>`).
    fn to_view(self) -> DutyProvenanceView {
        DutyProvenanceView {
            card: String::from(self.card),
            generation: self.generation,
            request: String::from(self.request),
        }
    }

    /// The hex bounds are the contract's; only the parse back to integers can
    /// still fail, and a `hex16`/`hex32` the codec accepted always parses.
    fn from_view(view: &DutyProvenanceView) -> Result<Self, LaneError> {
        Ok(Self {
            card: WireHex16::try_from(view.card.clone()).map_err(|_| LaneError::Malformed)?,
            generation: view.generation,
            request: WireHex32::try_from(view.request.clone()).map_err(|_| LaneError::Malformed)?,
        })
    }

    pub fn recovery_key(&self) -> String {
        format!(
            "{}-{:08x}-{}",
            String::from(self.card),
            self.generation,
            String::from(self.request)
        )
    }
}

macro_rules! wire_hex {
    ($name:ident, $inner:ty, $digits:literal) => {
        /// A fixed-width lowercase-hex identifier in its lane encoding.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct $name($inner);

        impl $name {
            pub fn from_value(value: $inner) -> Self {
                Self(value)
            }

            pub fn value(self) -> $inner {
                self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;

            fn try_from(text: String) -> Result<Self, String> {
                if text.len() != $digits
                    || !text
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err(format!("expected {} lowercase hex digits", $digits));
                }
                <$inner>::from_str_radix(&text, 16)
                    .map(Self)
                    .map_err(|_| format!("expected {} lowercase hex digits", $digits))
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> String {
                format!(concat!("{:0", $digits, "x}"), value.0)
            }
        }
    };
}

wire_hex!(WireHex16, u64, 16);
wire_hex!(WireHex32, u128, 32);

/// The duty kinds this crate CLAIMS the platform executor performs.
///
/// It is a declaration, not a derivation, and nothing in production reads it —
/// routing lives in the Kotlin `DutyRouter`. So it can only be trusted as far
/// as something executable checks it, and the thing that does is
/// `the_shipped_router_replays_authority_encoded_orders`, which runs the real
/// router. Treat this constant as the intent; treat the replay as the evidence.
///
/// It must remain a superset of what [`platform_work`] can build: dispatching a
/// kind the executor drops would strand the duty, since no result is reported
/// and the ledger never admits one.
pub const EXECUTED_KINDS: [DutyKind; 6] = [
    DutyKind::SourceHandle,
    DutyKind::Publication,
    DutyKind::Courier,
    DutyKind::Foreground,
    DutyKind::Notification,
    DutyKind::Lock,
];

/// The platform work a committed duty dispatches to the service, or `None`
/// when the host cannot yet build its payload.
///
/// An undispatched duty stays outstanding and is re-delivered on the next
/// attachment — the honest state — instead of being handed to an executor
/// that cannot answer it. The receipt courier and the source handle are the
/// duties the product mints today; the F3 staging flow supplies the grant, the
/// staging root, the publication payload, and the share targets.
pub fn platform_work(duty: Duty) -> Option<Work> {
    match duty.kind {
        DutyKind::Courier => Some(Work::Courier),
        DutyKind::SourceHandle => Some(Work::SourceHandle),
        DutyKind::Grant
        | DutyKind::Staging
        | DutyKind::Publication
        | DutyKind::Foreground
        | DutyKind::Notification
        | DutyKind::Lock
        | DutyKind::OpenShare => None,
    }
}

/// One platform work item with its kind-specific payload.
///
/// The kind is derived from the variant, so a payload can never disagree with
/// the duty kind it serves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Work {
    /// Bind the document the user picked to the card this duty names, and hold
    /// a read grant on it. The provenance IS the payload: the platform already
    /// holds the picked source, and the card it belongs to is the one that
    /// asked. Nothing about the document crosses back — a duty report carries
    /// an outcome, never a handle.
    SourceHandle,
    /// Persist/refresh a content grant. Real payload lands with F2.
    Grant,
    /// Prepare a fresh staging root for the card. Real payload lands with F2.
    Staging,
    /// Publish the sealed artifact to shared storage. The staged copy must
    /// remain until the host settles the publication (never lose the last
    /// copy); execution must be idempotent per provenance — query before
    /// insert, never blind-insert (the legacy MediaStore UNIQUE lesson).
    Publication {
        /// Staged artifact path RELATIVE to the app's private storage root.
        staged: String,
        display_name: String,
        total_bytes: u64,
    },
    /// Carry the card's completion receipt (the product's `PostReceipt` duty).
    /// No platform carrier exists yet, so the service reports the honest
    /// failure rather than a fabricated delivery — F2/F3 wire the real one.
    Courier,
    /// Keep the host service foregrounded while transfers are active.
    Foreground { active_transfers: u32 },
    /// Surface a user-visible notice for the card.
    Notification { notice: Notice },
    /// Hold or release the transfer wake/wifi locks.
    Lock { hold: bool },
    /// Offer the completed artifact in the system share sheet. F2 payload.
    OpenShare,
}

impl Work {
    pub const fn kind(&self) -> DutyKind {
        match self {
            Self::SourceHandle => DutyKind::SourceHandle,
            Self::Grant => DutyKind::Grant,
            Self::Staging => DutyKind::Staging,
            Self::Publication { .. } => DutyKind::Publication,
            Self::Courier => DutyKind::Courier,
            Self::Foreground { .. } => DutyKind::Foreground,
            Self::Notification { .. } => DutyKind::Notification,
            Self::Lock { .. } => DutyKind::Lock,
            Self::OpenShare => DutyKind::OpenShare,
        }
    }

    /// This lane's work as the contract spells it.
    ///
    /// Exhaustive both ways, so a new `Work` variant cannot reach the wire
    /// without a matching arm and a schema version to carry it — which is the
    /// whole difference from the hand-written encoder this replaced.
    fn to_view(&self) -> WorkView {
        match self {
            Self::SourceHandle => WorkView::SourceHandle,
            Self::Grant => WorkView::Grant,
            Self::Staging => WorkView::Staging,
            Self::Courier => WorkView::Courier,
            Self::OpenShare => WorkView::OpenShare,
            Self::Publication {
                staged,
                display_name,
                total_bytes,
            } => WorkView::Publication(PublicationWorkView {
                staged: staged.clone(),
                display_name: display_name.clone(),
                // The contract's horizon is u63 and its encoder range-checks
                // this, so a legacy value above 2^63-1 is refused there rather
                // than bounded twice in two places that could disagree.
                total_bytes: *total_bytes,
            }),
            Self::Foreground { active_transfers } => WorkView::Foreground(ForegroundWorkView {
                active_transfers: *active_transfers,
            }),
            Self::Notification { notice } => WorkView::Notification(NotificationWorkView {
                notice: match notice {
                    Notice::TransferComplete => NoticeView::TransferComplete,
                    Notice::TransferFailed => NoticeView::TransferFailed,
                    Notice::ActionNeeded => NoticeView::ActionNeeded,
                },
            }),
            Self::Lock { hold } => WorkView::Lock(LockWorkView {
                // The boolean stops here. Past this point the lane carries a
                // named directive, which has no value to silently default to.
                directive: if *hold {
                    LockDirectiveView::Hold
                } else {
                    LockDirectiveView::Release
                },
            }),
        }
    }

    fn from_view(view: WorkView) -> Self {
        match view {
            WorkView::SourceHandle => Self::SourceHandle,
            WorkView::Grant => Self::Grant,
            WorkView::Staging => Self::Staging,
            WorkView::Courier => Self::Courier,
            WorkView::OpenShare => Self::OpenShare,
            WorkView::Publication(payload) => Self::Publication {
                staged: payload.staged,
                display_name: payload.display_name,
                total_bytes: payload.total_bytes,
            },
            WorkView::Foreground(payload) => Self::Foreground {
                active_transfers: payload.active_transfers,
            },
            WorkView::Notification(payload) => Self::Notification {
                notice: match payload.notice {
                    NoticeView::TransferComplete => Notice::TransferComplete,
                    NoticeView::TransferFailed => Notice::TransferFailed,
                    NoticeView::ActionNeeded => Notice::ActionNeeded,
                },
            },
            WorkView::Lock(payload) => Self::Lock {
                hold: matches!(payload.directive, LockDirectiveView::Hold),
            },
        }
    }

    fn within_bounds(&self) -> bool {
        match self {
            Self::Publication {
                staged,
                display_name,
                total_bytes: _,
            } => {
                !staged.is_empty()
                    && staged.len() <= MAX_STAGED_PATH_BYTES
                    && !staged.starts_with('/')
                    && !staged.split('/').any(|part| part == "..")
                    && !display_name.is_empty()
                    && display_name.len() <= MAX_DISPLAY_NAME_BYTES
                    && !display_name.contains('/')
            }
            _ => true,
        }
    }
}

/// The contract's failure, in this lane's vocabulary. The generated codec owns
/// every shape, range and bound question; this only renames the answer.
fn lane_error(error: DutyError) -> LaneError {
    match error {
        DutyError::FrameTooLarge => LaneError::FrameTooLarge,
        DutyError::Range { .. } | DutyError::Bound { .. } => LaneError::Bounds,
        _ => LaneError::Malformed,
    }
}

/// The duty's answer, in the contract's words. Total over both vocabularies, so
/// a new answer kind cannot be dropped on the way out.
fn answer_to_view(answer: DutyReport) -> DutyAnswerView {
    match answer {
        DutyReport::Outcome(outcome) => DutyAnswerView::Outcome(outcome_to_view(outcome)),
        DutyReport::Source(SourceReport::Acquired {
            retention,
            seekability,
        }) => DutyAnswerView::Source(SourceReportView::Acquired(SourceAcquiredView {
            retention: match retention {
                SourceRetention::Process => SourceRetentionView::Process,
                SourceRetention::Persisted => SourceRetentionView::Persisted,
            },
            seekability: match seekability {
                SourceSeekability::Seekable => SourceSeekabilityView::Seekable,
                SourceSeekability::SequentialOnly => SourceSeekabilityView::SequentialOnly,
            },
        })),
        DutyReport::Source(SourceReport::Failed(reason)) => {
            DutyAnswerView::Source(SourceReportView::Failed(SourceFailedView {
                reason: match reason {
                    SourceAcquisitionFailure::Unreadable => SourceFailureView::Unreadable,
                    SourceAcquisitionFailure::PermissionLost => SourceFailureView::PermissionLost,
                    SourceAcquisitionFailure::StorageFault => SourceFailureView::StorageFault,
                    SourceAcquisitionFailure::Internal => SourceFailureView::Internal,
                },
            }))
        }
    }
}

fn answer_from_view(view: DutyAnswerView) -> DutyReport {
    match view {
        DutyAnswerView::Outcome(outcome) => DutyReport::Outcome(outcome_from_view(outcome)),
        DutyAnswerView::Source(SourceReportView::Acquired(acquired)) => {
            DutyReport::Source(SourceReport::Acquired {
                retention: match acquired.retention {
                    SourceRetentionView::Process => SourceRetention::Process,
                    SourceRetentionView::Persisted => SourceRetention::Persisted,
                },
                seekability: match acquired.seekability {
                    SourceSeekabilityView::Seekable => SourceSeekability::Seekable,
                    SourceSeekabilityView::SequentialOnly => SourceSeekability::SequentialOnly,
                },
            })
        }
        DutyAnswerView::Source(SourceReportView::Failed(failed)) => {
            DutyReport::Source(SourceReport::Failed(match failed.reason {
                SourceFailureView::Unreadable => SourceAcquisitionFailure::Unreadable,
                SourceFailureView::PermissionLost => SourceAcquisitionFailure::PermissionLost,
                SourceFailureView::StorageFault => SourceAcquisitionFailure::StorageFault,
                SourceFailureView::Internal => SourceAcquisitionFailure::Internal,
            }))
        }
    }
}

fn outcome_to_view(outcome: OutcomeCode) -> OutcomeCodeView {
    match outcome {
        OutcomeCode::Completed => OutcomeCodeView::Completed,
        OutcomeCode::Cancelled => OutcomeCodeView::Cancelled,
        OutcomeCode::Paused => OutcomeCodeView::Paused,
        OutcomeCode::PeerLost => OutcomeCodeView::PeerLost,
        OutcomeCode::Timeout => OutcomeCodeView::Timeout,
        OutcomeCode::Unauthenticated => OutcomeCodeView::Unauthenticated,
        OutcomeCode::VersionMismatch => OutcomeCodeView::VersionMismatch,
        OutcomeCode::StorageFault => OutcomeCodeView::StorageFault,
        OutcomeCode::PublishFailed => OutcomeCodeView::PublishFailed,
        OutcomeCode::SourceUnreadable => OutcomeCodeView::SourceUnreadable,
        OutcomeCode::NetworkUnreachable => OutcomeCodeView::NetworkUnreachable,
        OutcomeCode::Internal => OutcomeCodeView::Internal,
    }
}

fn outcome_from_view(view: OutcomeCodeView) -> OutcomeCode {
    match view {
        OutcomeCodeView::Completed => OutcomeCode::Completed,
        OutcomeCodeView::Cancelled => OutcomeCode::Cancelled,
        OutcomeCodeView::Paused => OutcomeCode::Paused,
        OutcomeCodeView::PeerLost => OutcomeCode::PeerLost,
        OutcomeCodeView::Timeout => OutcomeCode::Timeout,
        OutcomeCodeView::Unauthenticated => OutcomeCode::Unauthenticated,
        OutcomeCodeView::VersionMismatch => OutcomeCode::VersionMismatch,
        OutcomeCodeView::StorageFault => OutcomeCode::StorageFault,
        OutcomeCodeView::PublishFailed => OutcomeCode::PublishFailed,
        OutcomeCodeView::SourceUnreadable => OutcomeCode::SourceUnreadable,
        OutcomeCodeView::NetworkUnreachable => OutcomeCode::NetworkUnreachable,
        OutcomeCodeView::Internal => OutcomeCode::Internal,
    }
}

/// A user-visible notice class. Free-form text never crosses the lane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Notice {
    TransferComplete,
    TransferFailed,
    ActionNeeded,
}

/// One dispatched platform work item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkOrder {
    pub provenance: WireProvenance,
    pub work: Work,
}

impl WorkOrder {
    /// Builds the order for `duty`, refusing a payload of the wrong kind.
    pub fn for_duty(duty: Duty, work: Work) -> Result<Self, LaneError> {
        if work.kind() != duty.kind {
            return Err(LaneError::KindMismatch);
        }
        if !work.within_bounds() {
            return Err(LaneError::Bounds);
        }
        Ok(Self {
            provenance: WireProvenance::from_provenance(duty.provenance),
            work,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, LaneError> {
        let frame = DutyFrame {
            body: DutyBody::Order(DutyOrderView {
                provenance: self.provenance.to_view(),
                work: self.work.to_view(),
            }),
        };
        encode_duty_frame(&frame).map_err(lane_error)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, LaneError> {
        let DutyBody::Order(order) = decode_duty_frame(bytes).map_err(lane_error)?.body else {
            // A report is a well-formed frame that is not an order. Saying
            // "malformed" would blame the encoder for a routing mistake.
            return Err(LaneError::KindMismatch);
        };
        let work = Work::from_view(order.work);
        // The contract bounds every field; these are the invariants above it
        // that the schema language deliberately does not spell (a relative
        // staged name, a leaf with no separator).
        if !work.within_bounds() {
            return Err(LaneError::Bounds);
        }
        Ok(Self {
            provenance: WireProvenance::from_view(&order.provenance)?,
            work,
        })
    }
}

/// The service's typed answer for one executed work order.
///
/// An outcome for every kind but the source handle, which answers retention and
/// seekability instead. Those are not an outcome code's to carry: `completed`
/// says the platform did something, not whether its hold survives a restart or
/// whether the source can be re-read from an offset — and those two facts are
/// what decide whether a send streams or copies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkReport {
    pub provenance: WireProvenance,
    pub answer: DutyReport,
}

impl WorkReport {
    pub fn new(provenance: DutyProvenance, outcome: OutcomeCode) -> Self {
        Self {
            provenance: WireProvenance::from_provenance(provenance),
            answer: DutyReport::Outcome(outcome),
        }
    }

    /// A source acquisition's answer, which no outcome code can spell.
    pub fn source(provenance: DutyProvenance, report: SourceReport) -> Self {
        Self {
            provenance: WireProvenance::from_provenance(provenance),
            answer: DutyReport::Source(report),
        }
    }

    /// The untrusted adapter result; it must still pass the C6 ledger, which
    /// refuses an answer in the wrong vocabulary for the duty's kind.
    pub fn to_result(self) -> DutyResult {
        DutyResult {
            provenance: self.provenance.to_provenance(),
            report: self.answer,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, LaneError> {
        let frame = DutyFrame {
            body: DutyBody::Report(DutyReportView {
                provenance: self.provenance.to_view(),
                answer: answer_to_view(self.answer),
            }),
        };
        encode_duty_frame(&frame).map_err(lane_error)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, LaneError> {
        let DutyBody::Report(report) = decode_duty_frame(bytes).map_err(lane_error)?.body else {
            return Err(LaneError::KindMismatch);
        };
        Ok(Self {
            provenance: WireProvenance::from_view(&report.provenance)?,
            answer: answer_from_view(report.answer),
        })
    }
}
