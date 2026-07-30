//! Projection from live L4 values into generated read views.
//!
//! Bounds that the codec enforces are guaranteed here: safe-display text and
//! offered names are held to their owners' published maxima on char
//! boundaries, monotonic counters saturate into the u63 carrier, and
//! identifiers cross as fixed-length lowercase hex. Epoch parameters are plain
//! `u64` values taken
//! from `SubscriptionEpoch::get` / `CardUpdate::epoch` so hosts can project
//! without holding a live subscription type.

use envoix_evidence::{
    AbiSchemaManifest, BuildTrustManifest, DeploymentIdentity, DiagnosticsStatus, EvidenceValue,
    RedactedIdKind, SessionTimeline,
};
use envoix_outcomes::{Outcome, OutcomeCode, Phase, Recovery, Retryability, SafeDisplay};
use envoix_runtime::{
    AcceptedSourceOffer, CapabilityAction, CardUpdateKind, Duty, DutyKind, LosslessUpdateKind,
    MAX_ROOM_CODE_LENGTH, PairingChannel, PauseOrigin, ProductCommand, ProductState, QrMatrix,
    Quiescence, RetirementIntent, RoomParticipation, SelectionGate, SourceAcquisitionKey,
    SourceLifecycle, SourcePromptReason, SubscribeError, TransferContent, TransferRecord,
    WorkerKind,
};
use envoix_types::{Direction, OfferedName, RecordId, Secret};

use crate::capability::CAPABILITY_SCHEMA_ID;
use crate::command::COMMAND_SCHEMA_ID;
use crate::read::{
    AbiSchemaManifestView, AcceptedSourceOfferView, BuildManifestView, CapabilityActionView,
    CardActionView, CardUpdateKindView, CardUpdateView, CardView, ClosedView, CommandKindView,
    DegradedView, DeploymentManifestView, DiagnosticsStatusView, DirectionView, DutyFrameView,
    DutyKindView, DutyProvenanceView, DutyView, EvidenceProgressView, EvidenceTimelineView,
    EvidenceValueView, IdentityView, InviteView, LagView, LosslessKindView, OutcomeView,
    PausedView, PhaseView, PickSourceActionView, ProductStateView, ProtocolManifestView, QrView,
    QuiescenceView, READ_SCHEMA_ID, ReadBody, ReadFrame, RecoveryView, RedactedIdKindView,
    RedactedIdView, RetirementIntentView, RetiringView, RetryabilityView, RoomParticipationView,
    RunningView, SessionKeyView, SourceAcquisitionKeyView, SourceAwaitingSelectionView,
    SourceLifecycleView, SourceNotRequiredView, SourcePromptReasonView, SourceRePickRequiredView,
    SourceReadyView, SourceSelectableView, SourceSelectionGateView, SubscribeRejectedView,
    SubscribeRejectionView, TimelineEntryView, TransferContentView, WorkerKindView,
};

/// The codec bound on safe-display text and on offered names: L0 owns both
/// types and now publishes both maxima, so this contract derives them instead
/// of choosing what may cross to an observer on their behalf.
const MAX_DISPLAY_BYTES: usize = SafeDisplay::MAX_BYTES;
const MAX_NAME_BYTES: usize = OfferedName::MAX_BYTES;
/// The codec bound on evidence timeline entries per frame. Contract-local, and
/// about a frame rather than a store: a host configures its own ring capacity,
/// and `evidence_frame` keeps the newest entries with the sequence gaps left
/// visible.
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

/// The static build manifest as a read frame — the COMPLETE build identity.
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
    let DeploymentIdentity {
        environment,
        rendezvous_endpoint,
        relay_url,
    } = manifest.deployment;
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
            capability_binding_schema_id: CAPABILITY_SCHEMA_ID.to_owned(),
            evidence_rust_abi_id: evidence_rust_abi_id.to_owned(),
            evidence_timeline_schema_id: evidence_timeline_schema_id.to_owned(),
            mailbox_receipt_schema_id: mailbox_receipt_schema_id.to_owned(),
            operation_envelope_schema_id: operation_envelope_schema_id.to_owned(),
        },
        deployment: DeploymentManifestView {
            environment: environment.to_string(),
            rendezvous_endpoint: rendezvous_endpoint.to_string(),
            relay_url: relay_url.to_string(),
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
        participation: match record.participation {
            RoomParticipation::Minted => RoomParticipationView::Minted,
            RoomParticipation::Joined => RoomParticipationView::Joined,
        },
        direction: match record.direction {
            Direction::Send => DirectionView::Send,
            Direction::Receive => DirectionView::Receive,
        },
        // The lifecycle itself, replacing the top-level name and total that
        // used to be projected beside it. Those two fields could only ever be
        // published as an invented empty string and a zero for a card that had
        // not been given a document, and a frontend could not tell that zero
        // from a real empty file. Here the difference is a variant.
        source: source_lifecycle_view(record),
        state: state_view(record.state),
        quiescence: quiescence_view(record.quiescence),
        generation: record.generation.get(),
        phase: phase_view(record.phase),
        bytes: u63(record.bytes.get()),
        bytes_resumed: u63(record.bytes_resumed.get()),
        outcome: record.outcome.as_ref().map(outcome_view),
        // Legality is the reducer's, not the observer's: this publishes
        // `allowed_commands` verbatim so a frontend renders the authority's
        // answer instead of re-deriving one from the state beside it (R0).
        // `pick_source` joins it from the same source of truth — the lifecycle
        // — rather than being something a frontend infers from direction or
        // state.
        allowed_actions: card_actions(record),
        // The card's frozen channel, rendered by the invite grammar that owns
        // it — and ONLY for a card that minted its room.
        //
        // A joined card holds the channel it adopted, so publishing on channel
        // presence alone made a joiner republish the secret it was given. An
        // invite names a ONE-peer rendezvous: a third party acting on the
        // republished one races the two already pairing. A card with no
        // channel, or one whose stored fields no longer spell a valid invite,
        // still publishes nothing rather than a partial one.
        invite: record
            .participation
            .publishes_the_invite()
            .then(|| record.pairing.as_deref().and_then(invite_view))
            .flatten(),
    }
}

fn invite_view(pairing: &PairingChannel) -> Option<InviteView> {
    let code = truncate_utf8(pairing.code(), MAX_ROOM_CODE_LENGTH);
    let digest = blake3::hash(code.as_bytes()).to_hex();
    Some(InviteView {
        code: Secret::new(code),
        code_fingerprint: digest[..16].to_owned(),
        // Nothing measures the link here. The schema's bound IS the grammar's
        // published emit maximum, so a link this contract cannot carry is not a
        // thing the encoder can produce; were the two ever to disagree, the
        // read codec refuses the whole frame as `Bound` rather than quietly
        // dropping the field. Absence therefore means one thing only: the
        // stored fields no longer spell an invite the grammar can encode.
        link: pairing.shareable().map(Secret::new),
        // Absent when the grammar has no square for these fields — either they
        // no longer spell an invite, or they spell one past the QR frontier.
        // Both are answers a frontend draws; neither is a blank code.
        qr: pairing.qr().as_ref().map(qr_view),
    })
}

/// Packs a QR's modules row-major, one bit each, MSB first, as lowercase hex.
///
/// The schema's bound is version 40's worst case, so a square the grammar can
/// produce always fits and is never truncated. Nothing measures it here: were
/// the two ever to disagree, the read codec refuses the whole frame rather than
/// quietly publishing half a code.
fn qr_view(matrix: &QrMatrix) -> QrView {
    let modules = matrix.modules();
    let mut packed = vec![0u8; modules.len().div_ceil(8)];
    for (index, dark) in modules.iter().enumerate() {
        if *dark {
            packed[index / 8] |= 0x80 >> (index % 8);
        }
    }
    QrView {
        width: u16::try_from(matrix.width()).unwrap_or(u16::MAX),
        modules: Secret::new(packed.iter().fold(
            String::with_capacity(packed.len() * 2),
            |mut hex, byte| {
                use std::fmt::Write as _;
                let _ = write!(hex, "{byte:02x}");
                hex
            },
        )),
    }
}

/// Where this card's send source is, as the frontend renders it.
///
/// Total over the lifecycle by construction, so a new source state cannot be
/// published as a plausible-looking old one. Every payload carries only facts
/// that state actually holds: `acquiring` and `staging` have the provider's
/// normalized name and its advisory size and nothing more, because nothing more
/// is true of them yet.
fn source_lifecycle_view(record: &TransferRecord) -> SourceLifecycleView {
    match &record.source {
        SourceLifecycle::NotRequired { peer_content, .. } => {
            SourceLifecycleView::NotRequired(SourceNotRequiredView {
                peer_content: peer_content.as_ref().map(transfer_content_view),
            })
        }
        SourceLifecycle::AwaitingSelection(gate) => {
            SourceLifecycleView::AwaitingSelection(SourceAwaitingSelectionView {
                selection: match gate {
                    // The key is published HERE and nowhere else derived: it is
                    // the same `current_acquisition()` the authority will check
                    // an offer against, so a frontend answering what it was told
                    // cannot answer the wrong acquisition.
                    SelectionGate::Selectable { reason, .. } => {
                        SourceSelectionGateView::Selectable(SourceSelectableView {
                            acquisition: acquisition_view(record.current_acquisition()),
                            reason: prompt_reason_view(*reason),
                        })
                    }
                    // No key: this gate accepts no offer at all, and publishing
                    // one would invite a frontend to answer with it.
                    SelectionGate::RePickRequired {
                        reason,
                        previous_offer,
                        ..
                    } => SourceSelectionGateView::RePickRequired(SourceRePickRequiredView {
                        reason: prompt_reason_view(*reason),
                        previous_offer: accepted_offer_view(previous_offer),
                    }),
                },
            })
        }
        SourceLifecycle::Acquiring(offer) => {
            SourceLifecycleView::Acquiring(accepted_offer_view(offer))
        }
        SourceLifecycle::Staging { offer, .. } => {
            SourceLifecycleView::Staging(accepted_offer_view(offer))
        }
        SourceLifecycle::Ready { offer, content, .. } => {
            SourceLifecycleView::Ready(SourceReadyView {
                offer: accepted_offer_view(offer),
                content: transfer_content_view(content.content()),
            })
        }
    }
}

fn acquisition_view(key: SourceAcquisitionKey) -> SourceAcquisitionKeyView {
    SourceAcquisitionKeyView {
        card: hex16(key.card().get()),
        generation: key.generation().get(),
        request: key.request().to_string(),
    }
}

fn accepted_offer_view(offer: &AcceptedSourceOffer) -> AcceptedSourceOfferView {
    AcceptedSourceOfferView {
        acquisition: acquisition_view(*offer.key()),
        display_name: truncate_utf8(offer.display_name().as_str(), MAX_NAME_BYTES),
        reported_size: offer.reported_size().map(|size| u63(size.get())),
    }
}

fn transfer_content_view(content: &TransferContent) -> TransferContentView {
    TransferContentView {
        offered_name: truncate_utf8(content.name().as_str(), MAX_NAME_BYTES),
        total: u63(content.total().get()),
    }
}

const fn prompt_reason_view(reason: SourcePromptReason) -> SourcePromptReasonView {
    match reason {
        SourcePromptReason::Initial => SourcePromptReasonView::Initial,
        SourcePromptReason::Unreadable => SourcePromptReasonView::Unreadable,
        SourcePromptReason::PermissionLost => SourcePromptReasonView::PermissionLost,
        SourcePromptReason::StorageFault => SourcePromptReasonView::StorageFault,
        SourcePromptReason::StagingFailed => SourcePromptReasonView::StagingFailed,
        SourcePromptReason::Internal => SourcePromptReasonView::Internal,
    }
}

/// Everything a frontend may do to this card, command or not.
///
/// `pick_source` comes first because it is the one constructive thing a card
/// waiting for a document can do, and the published order is the order a
/// frontend draws. It appears for exactly one lifecycle state — a gate that can
/// still accept an offer — so a card that is acquiring, staging, ready, or that
/// has lost its source and needs `re_pick_source` instead never offers it.
fn card_actions(record: &TransferRecord) -> Vec<CardActionView> {
    let pick = match &record.source {
        SourceLifecycle::AwaitingSelection(gate) if gate.accepts_an_offer() => {
            Some(CardActionView::PickSource(PickSourceActionView {
                acquisition: acquisition_view(record.current_acquisition()),
            }))
        }
        SourceLifecycle::NotRequired { .. }
        | SourceLifecycle::AwaitingSelection(_)
        | SourceLifecycle::Acquiring(_)
        | SourceLifecycle::Staging { .. }
        | SourceLifecycle::Ready { .. } => None,
    };
    pick.into_iter()
        .chain(
            record
                .allowed_commands()
                .into_iter()
                .map(|command| CardActionView::Command(command_kind_view(command))),
        )
        .collect()
}

const fn command_kind_view(command: ProductCommand) -> CommandKindView {
    match command {
        ProductCommand::Pause => CommandKindView::Pause,
        ProductCommand::Cancel => CommandKindView::Cancel,
        ProductCommand::Resume => CommandKindView::Resume,
        ProductCommand::Remove => CommandKindView::Remove,
        ProductCommand::RePickSource => CommandKindView::RePickSource,
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
        CapabilityAction::AcquireSource => CapabilityActionView::AcquireSource,
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
