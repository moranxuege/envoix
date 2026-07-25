//! Projection from live L4 values into generated read views.
//!
//! Bounds that the codec enforces are guaranteed here: safe-display text is
//! truncated to 160 bytes and offered names to 255 bytes on char boundaries,
//! monotonic counters saturate into the u63 carrier, and identifiers cross as
//! fixed-length lowercase hex. Epoch parameters are plain `u64` values taken
//! from `SubscriptionEpoch::get` / `CardUpdate::epoch` so hosts can project
//! without holding a live subscription type.

use envoix_evidence::{
    AbiSchemaManifest, BuildTrustManifest, DiagnosticsStatus, EvidenceValue, RedactedIdKind,
    SessionTimeline, TrustRootFingerprintSlot,
};
use envoix_outcomes::{Outcome, OutcomeCode, Phase, Recovery, Retryability};
use envoix_runtime::{
    CapabilityAction, CardUpdateKind, Duty, DutyKind, LosslessUpdateKind, PauseOrigin,
    ProductState, Quiescence, RetirementIntent, SubscribeError, TransferRecord, WorkerKind,
};
use envoix_types::{Direction, RecordId};

use crate::command::COMMAND_SCHEMA_ID;
use crate::read::{
    AbiSchemaManifestView, BuildManifestView, CapabilityActionView, CardUpdateKindView,
    CardUpdateView, CardView, ClosedView, DegradedView, DiagnosticsStatusView, DirectionView,
    DutyFrameView, DutyKindView, DutyProvenanceView, DutyView, EvidenceProgressView,
    EvidenceTimelineView, EvidenceValueView, IdentityView, LagView, LosslessKindView, OutcomeView,
    PausedView, PhaseView, ProductStateView, ProtocolManifestView, QuiescenceView, READ_SCHEMA_ID,
    ReadBody, ReadFrame, RecoveryView, RedactedIdKindView, RedactedIdView, RetirementIntentView,
    RetiringView, RetryabilityView, RunningView, SessionKeyView, SubscribeRejectedView,
    SubscribeRejectionView, TimelineEntryView, TrustRootSha256View, TrustRootView, WorkerKindView,
};

/// The codec bound on safe-display text, mirrored from evidence.
const MAX_DISPLAY_BYTES: usize = 160;
/// The codec bound on offered names (the filesystem leaf convention).
const MAX_NAME_BYTES: usize = 255;
/// The codec bound on evidence timeline entries per frame.
const MAX_TIMELINE_ENTRIES: usize = 1024;

const U63_MAX: u64 = 9_223_372_036_854_775_807;

/// One card-stream update as a read frame.
pub fn card_update_frame(epoch: u64, card: RecordId, kind: &CardUpdateKind) -> ReadFrame {
    let kind = match kind {
        CardUpdateKind::Snapshot(record) => CardUpdateKindView::Snapshot(card_view(record)),
        CardUpdateKind::Progress(record) => CardUpdateKindView::Progress(card_view(record)),
        CardUpdateKind::State(record) => CardUpdateKindView::State(card_view(record)),
        CardUpdateKind::Terminal(record) => CardUpdateKindView::Terminal(card_view(record)),
        CardUpdateKind::CapabilityDuty { duty, action } => {
            CardUpdateKindView::CapabilityDuty(DutyFrameView {
                duty: duty_view(*duty),
                action: action_view(*action),
            })
        }
    };
    frame(ReadBody::CardUpdate(CardUpdateView {
        epoch: u63(epoch),
        card: hex16(card.get()),
        kind,
    }))
}

/// The typed lag signal that closed an epoch.
pub fn lag_frame(epoch: u64, card: RecordId, missed: LosslessUpdateKind) -> ReadFrame {
    frame(ReadBody::Lag(LagView {
        epoch: u63(epoch),
        card: hex16(card.get()),
        missed: match missed {
            LosslessUpdateKind::Terminal => LosslessKindView::Terminal,
            LosslessUpdateKind::CapabilityDuty => LosslessKindView::CapabilityDuty,
        },
    }))
}

/// The runtime closed this subscription (shutdown or card removal).
pub fn closed_frame(epoch: u64, card: RecordId) -> ReadFrame {
    frame(ReadBody::Closed(ClosedView {
        epoch: u63(epoch),
        card: hex16(card.get()),
    }))
}

/// A rejected attach, so the reason crosses as typed data.
pub fn subscribe_rejected_frame(card: RecordId, error: SubscribeError) -> ReadFrame {
    frame(ReadBody::SubscribeRejected(SubscribeRejectedView {
        card: hex16(card.get()),
        reason: match error {
            SubscribeError::UnknownCard => SubscribeRejectionView::UnknownCard,
            SubscribeError::RuntimeStopped => SubscribeRejectionView::RuntimeStopped,
            SubscribeError::EpochExhausted => SubscribeRejectionView::EpochExhausted,
        },
    }))
}

/// One bounded session timeline. If the store holds more entries than the
/// codec bound, the newest are kept; sequence gaps stay visible.
pub fn evidence_frame(timeline: &SessionTimeline) -> ReadFrame {
    let session = timeline.session();
    let entries = timeline.entries();
    let skip = entries.len().saturating_sub(MAX_TIMELINE_ENTRIES);
    let entries = entries[skip..]
        .iter()
        .map(|entry| TimelineEntryView {
            sequence: u63(entry.sequence()),
            value: evidence_value_view(entry.value()),
        })
        .collect();
    frame(ReadBody::Evidence(EvidenceTimelineView {
        session: SessionKeyView {
            card: hex16(session.card.get()),
            generation: session.generation.get(),
        },
        status: match timeline.diagnostics() {
            DiagnosticsStatus::Complete => DiagnosticsStatusView::Complete,
            DiagnosticsStatus::DiagnosticsDegraded(degraded) => {
                DiagnosticsStatusView::Degraded(DegradedView {
                    dropped_events: u63(degraded.dropped_events()),
                })
            }
        },
        entries,
    }))
}

/// The static build/trust manifest as a read frame — the COMPLETE build
/// identity.
///
/// The L4 manifest cannot name the generated binding contracts (L5 depends on
/// L4, never the reverse), so this projection composes them: the L4
/// identities plus the read and command schema ids taken straight from the
/// generated modules, where they cannot drift from what the codecs speak. The
/// L4 half is destructured rather than field-accessed, so a new identity added
/// to `AbiSchemaManifest` fails to compile until it is projected here.
pub fn build_manifest_frame(manifest: &BuildTrustManifest) -> ReadFrame {
    let AbiSchemaManifest {
        evidence_rust_abi_id,
        evidence_timeline_schema_id,
        mailbox_receipt_schema_id,
        operation_envelope_schema_id,
    } = manifest.abi_schema;
    frame(ReadBody::BuildManifest(BuildManifestView {
        package_version: manifest.package_version.to_owned(),
        protocol: ProtocolManifestView {
            set_id: manifest.protocol.set_id.to_owned(),
            data_alpn: hex_bytes(manifest.protocol.data_alpn),
            data_magic: hex_bytes(manifest.protocol.data_magic),
            data_wire_version: manifest.protocol.data_wire_version,
        },
        abi_schema: AbiSchemaManifestView {
            read_binding_schema_id: READ_SCHEMA_ID.to_owned(),
            command_binding_schema_id: COMMAND_SCHEMA_ID.to_owned(),
            evidence_rust_abi_id: evidence_rust_abi_id.to_owned(),
            evidence_timeline_schema_id: evidence_timeline_schema_id.to_owned(),
            mailbox_receipt_schema_id: mailbox_receipt_schema_id.to_owned(),
            operation_envelope_schema_id: operation_envelope_schema_id.to_owned(),
        },
        trust_root: match manifest.trust_root {
            TrustRootFingerprintSlot::Unprovisioned => TrustRootView::Unprovisioned,
            TrustRootFingerprintSlot::Sha256(fingerprint) => {
                TrustRootView::Sha256(TrustRootSha256View {
                    fingerprint: hex_bytes(&fingerprint),
                })
            }
        },
    }))
}

fn frame(body: ReadBody) -> ReadFrame {
    ReadFrame { body }
}

fn card_view(record: &TransferRecord) -> CardView {
    CardView {
        identity: IdentityView {
            card: hex16(record.identity.card.get()),
            transfer: record.identity.transfer.to_string(),
            artifact: record.identity.artifact.to_string(),
        },
        direction: match record.direction {
            Direction::Send => DirectionView::Send,
            Direction::Receive => DirectionView::Receive,
        },
        offered_name: truncate_utf8(record.offered_name.as_str(), MAX_NAME_BYTES),
        total: u63(record.total.get()),
        state: state_view(record.state),
        quiescence: quiescence_view(record.quiescence),
        generation: record.generation.get(),
        phase: phase_view(record.phase),
        bytes: u63(record.bytes.get()),
        bytes_resumed: u63(record.bytes_resumed.get()),
        outcome: record.outcome.as_ref().map(outcome_view),
    }
}

fn state_view(state: ProductState) -> ProductStateView {
    match state {
        ProductState::Preparing => ProductStateView::Preparing,
        ProductState::Waiting => ProductStateView::Waiting,
        ProductState::Connecting => ProductStateView::Connecting,
        ProductState::Verifying => ProductStateView::Verifying,
        ProductState::Transferring => ProductStateView::Transferring,
        ProductState::Confirming => ProductStateView::Confirming,
        ProductState::Paused(origin) => ProductStateView::Paused(PausedView {
            origin: match origin {
                PauseOrigin::Local => crate::read::PauseOriginView::Local,
                PauseOrigin::Peer => crate::read::PauseOriginView::Peer,
                PauseOrigin::Lost => crate::read::PauseOriginView::Lost,
            },
        }),
        ProductState::Unconfirmed => ProductStateView::Unconfirmed,
        ProductState::Completed => ProductStateView::Completed,
        ProductState::Failed => ProductStateView::Failed,
        ProductState::Cancelled => ProductStateView::Cancelled,
    }
}

fn quiescence_view(quiescence: Quiescence) -> QuiescenceView {
    match quiescence {
        Quiescence::Running { worker } => QuiescenceView::Running(RunningView {
            worker: worker_view(worker),
        }),
        Quiescence::Retiring { worker, intent } => QuiescenceView::Retiring(RetiringView {
            worker: worker_view(worker),
            intent: match intent {
                RetirementIntent::Pause => RetirementIntentView::Pause,
                RetirementIntent::Cancel => RetirementIntentView::Cancel,
                RetirementIntent::Finalize => RetirementIntentView::Finalize,
            },
        }),
        Quiescence::Quiescent => QuiescenceView::Quiescent,
    }
}

fn worker_view(worker: WorkerKind) -> WorkerKindView {
    match worker {
        WorkerKind::Attempt => WorkerKindView::Attempt,
        WorkerKind::Staging => WorkerKindView::Staging,
    }
}

fn outcome_view(outcome: &Outcome) -> OutcomeView {
    OutcomeView {
        code: code_view(outcome.code),
        phase: phase_view(outcome.phase),
        retry: retry_view(outcome.retry),
        recovery: outcome.recovery.map(recovery_view),
        display: truncate_utf8(outcome.display.as_str(), MAX_DISPLAY_BYTES),
    }
}

fn code_view(code: OutcomeCode) -> crate::read::OutcomeCodeView {
    use crate::read::OutcomeCodeView as View;
    match code {
        OutcomeCode::Completed => View::Completed,
        OutcomeCode::Cancelled => View::Cancelled,
        OutcomeCode::Paused => View::Paused,
        OutcomeCode::PeerLost => View::PeerLost,
        OutcomeCode::Timeout => View::Timeout,
        OutcomeCode::Unauthenticated => View::Unauthenticated,
        OutcomeCode::VersionMismatch => View::VersionMismatch,
        OutcomeCode::StorageFault => View::StorageFault,
        OutcomeCode::PublishFailed => View::PublishFailed,
        OutcomeCode::SourceUnreadable => View::SourceUnreadable,
        OutcomeCode::NetworkUnreachable => View::NetworkUnreachable,
        OutcomeCode::Internal => View::Internal,
    }
}

fn phase_view(phase: Phase) -> PhaseView {
    match phase {
        Phase::Preparing => PhaseView::Preparing,
        Phase::Pairing => PhaseView::Pairing,
        Phase::Authenticating => PhaseView::Authenticating,
        Phase::Transferring => PhaseView::Transferring,
        Phase::Confirming => PhaseView::Confirming,
        Phase::Publishing => PhaseView::Publishing,
        Phase::Restoring => PhaseView::Restoring,
    }
}

fn retry_view(retry: Retryability) -> RetryabilityView {
    match retry {
        Retryability::Retryable => RetryabilityView::Retryable,
        Retryability::Terminal => RetryabilityView::Terminal,
        Retryability::NeedsUser => RetryabilityView::NeedsUser,
    }
}

fn recovery_view(recovery: Recovery) -> RecoveryView {
    match recovery {
        Recovery::RePickSource => RecoveryView::RePickSource,
        Recovery::RetryLater => RecoveryView::RetryLater,
        Recovery::ReconnectPeer => RecoveryView::ReconnectPeer,
    }
}

fn duty_view(duty: Duty) -> DutyView {
    DutyView {
        provenance: DutyProvenanceView {
            card: hex16(duty.provenance.card.get()),
            generation: duty.provenance.generation.get(),
            request: duty.provenance.request.to_string(),
        },
        kind: match duty.kind {
            DutyKind::SourceHandle => DutyKindView::SourceHandle,
            DutyKind::Grant => DutyKindView::Grant,
            DutyKind::Staging => DutyKindView::Staging,
            DutyKind::Publication => DutyKindView::Publication,
            DutyKind::Courier => DutyKindView::Courier,
            DutyKind::Foreground => DutyKindView::Foreground,
            DutyKind::Notification => DutyKindView::Notification,
            DutyKind::Lock => DutyKindView::Lock,
            DutyKind::OpenShare => DutyKindView::OpenShare,
        },
    }
}

fn action_view(action: CapabilityAction) -> CapabilityActionView {
    match action {
        CapabilityAction::PostReceipt => CapabilityActionView::PostReceipt,
    }
}

fn evidence_value_view(value: &EvidenceValue) -> EvidenceValueView {
    match value {
        EvidenceValue::Phase(phase) => EvidenceValueView::Phase(phase_view(*phase)),
        EvidenceValue::Progress(progress) => EvidenceValueView::Progress(EvidenceProgressView {
            transferred: u63(progress.transferred().get()),
            total: u63(progress.total().get()),
        }),
        EvidenceValue::Outcome(outcome) => EvidenceValueView::Outcome(OutcomeView {
            code: code_view(outcome.code()),
            phase: phase_view(outcome.phase()),
            retry: retry_view(outcome.retryability()),
            recovery: outcome.recovery().map(recovery_view),
            display: truncate_utf8(outcome.display().as_str(), MAX_DISPLAY_BYTES),
        }),
        EvidenceValue::Identifier(identifier) => EvidenceValueView::Identifier(RedactedIdView {
            kind: match identifier.kind() {
                RedactedIdKind::Record => RedactedIdKindView::Record,
                RedactedIdKind::Transfer => RedactedIdKindView::Transfer,
                RedactedIdKind::Artifact => RedactedIdKindView::Artifact,
                RedactedIdKind::Request => RedactedIdKindView::Request,
            },
        }),
    }
}

fn u63(value: u64) -> u64 {
    value.min(U63_MAX)
}

fn hex16(value: u64) -> String {
    format!("{value:016x}")
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
