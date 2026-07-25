//! The host↔service duty protocol: typed work orders out, typed reports back.
//!
//! This is a TRUSTED in-process lane between the Rust host and the Kotlin
//! service that executes platform capabilities. It is not the frontend
//! surface: frontends observe duties through the generated read contract and
//! never execute them. Values still cross a language boundary, so every field
//! is bounded, identifiers use the same fixed lowercase-hex convention as the
//! generated contracts (never JSON numbers wider than u32), and unknown fields
//! are rejected.

use envoix_capabilities::{Duty, DutyKind, DutyProvenance, DutyResult};
use envoix_outcomes::OutcomeCode;
use envoix_types::{AttemptGen, RecordId, RequestId};
use serde::{Deserialize, Serialize};

/// Longest permitted staged-artifact relative path, in UTF-8 bytes.
pub const MAX_STAGED_PATH_BYTES: usize = 512;
/// Longest permitted display name, in UTF-8 bytes (filesystem-leaf convention).
pub const MAX_DISPLAY_NAME_BYTES: usize = 255;
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
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
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
        #[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
        #[serde(try_from = "String", into = "String")]
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

/// The duty kinds the Kotlin `DutyExecutor` actually executes.
///
/// This is the ONE source of truth for the platform lane: [`platform_work`]
/// may only build work for these kinds, and the Kotlin `when` branches are
/// pinned against it by `kotlin_executor_handles_exactly_the_executed_kinds`.
/// Dispatching any other kind would strand the duty — the service drops what
/// it cannot execute, so no result is ever reported and the ledger never
/// admits one.
pub const EXECUTED_KINDS: [DutyKind; 5] = [
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
/// that cannot answer it. The receipt courier is the only duty the product
/// mints today; the F2 frontend flow supplies the picked source, the grant,
/// the staging root, the publication payload, and the share/courier targets.
pub fn platform_work(duty: Duty) -> Option<Work> {
    match duty.kind {
        DutyKind::Courier => Some(Work::Courier),
        DutyKind::SourceHandle
        | DutyKind::Grant
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
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum Work {
    /// Open the user-picked source. Real payload lands with the F2 pick flow.
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

/// A user-visible notice class. Free-form text never crosses the lane.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Notice {
    TransferComplete,
    TransferFailed,
    ActionNeeded,
}

/// One dispatched platform work item.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
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
        let bytes = serde_json::to_vec(self).map_err(|_| LaneError::Malformed)?;
        if bytes.len() > MAX_LANE_FRAME_BYTES {
            return Err(LaneError::FrameTooLarge);
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, LaneError> {
        if bytes.len() > MAX_LANE_FRAME_BYTES {
            return Err(LaneError::FrameTooLarge);
        }
        let order: Self = serde_json::from_slice(bytes).map_err(|_| LaneError::Malformed)?;
        if !order.work.within_bounds() {
            return Err(LaneError::Bounds);
        }
        Ok(order)
    }
}

/// The service's typed answer for one executed work order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkReport {
    pub provenance: WireProvenance,
    pub outcome: OutcomeCode,
}

impl WorkReport {
    pub fn new(provenance: DutyProvenance, outcome: OutcomeCode) -> Self {
        Self {
            provenance: WireProvenance::from_provenance(provenance),
            outcome,
        }
    }

    /// The untrusted adapter result; it must still pass the C6 ledger.
    pub fn to_result(self) -> DutyResult {
        DutyResult {
            provenance: self.provenance.to_provenance(),
            outcome: self.outcome,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, LaneError> {
        let bytes = serde_json::to_vec(self).map_err(|_| LaneError::Malformed)?;
        if bytes.len() > MAX_LANE_FRAME_BYTES {
            return Err(LaneError::FrameTooLarge);
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, LaneError> {
        if bytes.len() > MAX_LANE_FRAME_BYTES {
            return Err(LaneError::FrameTooLarge);
        }
        serde_json::from_slice(bytes).map_err(|_| LaneError::Malformed)
    }
}
