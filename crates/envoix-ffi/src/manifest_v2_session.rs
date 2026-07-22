//! Native Manifest-v2 session facade.
//!
//! The bridge exposes one canonical job/session path for every source shape.
//! Receiver metadata is returned before destination approval and payload.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use envoix_client::api::{
    DestinationDecisionV2, DestinationRequestV2, PairingConfig, PeerSource,
    PendingManifestV2Receive, SessionError, SessionEventSink, SessionTransferEvent,
    TransferCancelToken,
    parse_broker_addr, receive_manifest_v2_offer_enable_mdns,
    receive_manifest_v2_offer_via_room, receive_manifest_v2_offer_with_bound_peer,
    send_manifest_v2_enable_mdns, send_manifest_v2_manual, send_manifest_v2_to_endpoint_addr,
    send_manifest_v2_via_room,
};
use envoix_qr::{QrInvitePayload, generate_token};
use tokio::sync::Mutex;

use super::{
    EnvoixError, EnvoixRuntimeSettings, FfiFailureCategory, FfiFailureCode, FfiFailureOrigin,
    FfiFailurePhase, FfiRecoveryAction, FfiTransferDirection, FfiTransferFailure,
    FfiTransferJobV2, FfiTransferRequest, TransferObserver,
    build_client_for_request, op_err, peer_sources_for_request, transfer_options_for_request,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiManifestEntryKindV2 {
    File,
    Directory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiDestinationDecisionV2 {
    SaveDirectly,
    CopyAfterVerify,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiManifestOfferSummaryV2 {
    pub job_id: String,
    pub generation: u32,
    pub selection_revision: u64,
    pub root_count: u32,
    pub file_count: u32,
    pub directory_count: u32,
    pub total_plaintext_bytes: u64,
    pub exceptional_offer: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiManifestOfferEntryV2 {
    pub entry_id: u32,
    pub root_id: u32,
    pub parent_entry_id: Option<u32>,
    pub name: String,
    pub kind: FfiManifestEntryKindV2,
    pub plaintext_size: u64,
    pub digest_known: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiManifestOfferPageV2 {
    pub entries: Vec<FfiManifestOfferEntryV2>,
    pub next_offset: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiManifestV2Completion {
    pub job_id: String,
    pub entry_count: u32,
    pub total_plaintext_bytes: u64,
    pub delivery_proof_digest: Vec<u8>,
    /// Receiver-only final root paths. Sender completions keep this empty.
    pub saved_paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiDestinationRequestV2 {
    pub target_directory: String,
    pub copy_staging_directory: Option<String>,
    pub decision: FfiDestinationDecisionV2,
    pub target_allocatable_bytes: Option<u64>,
    pub staging_allocatable_bytes: Option<u64>,
    pub stable_object_identity: bool,
    pub exceptional_transfer_approved: bool,
}

#[derive(uniffi::Object)]
pub struct FfiManifestV2Cancellation {
    token: TransferCancelToken,
}

#[uniffi::export]
impl FfiManifestV2Cancellation {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            token: TransferCancelToken::new(),
        })
    }

    pub fn cancel(&self) {
        self.token.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

#[derive(uniffi::Object)]
pub struct FfiPendingManifestV2Receive {
    pending: Mutex<Option<PendingManifestV2Receive>>,
    summary: FfiManifestOfferSummaryV2,
    entries: Vec<FfiManifestOfferEntryV2>,
    state_directory: PathBuf,
    cancellation: Arc<FfiManifestV2Cancellation>,
}

#[uniffi::export]
impl FfiPendingManifestV2Receive {
    pub fn summary(&self) -> FfiManifestOfferSummaryV2 {
        self.summary.clone()
    }

    /// Bounded projection for native large-tree UIs.
    pub fn list_entries(&self, offset: u32, limit: u32) -> FfiManifestOfferPageV2 {
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(self.entries.len());
        let limit = usize::try_from(limit).unwrap_or(usize::MAX).clamp(1, 512);
        let end = start.saturating_add(limit).min(self.entries.len());
        FfiManifestOfferPageV2 {
            entries: self.entries[start..end].to_vec(),
            next_offset: (end < self.entries.len()).then(|| u32::try_from(end).unwrap_or(u32::MAX)),
        }
    }

    /// The only transition that permits payload: destination policy and known
    /// capacity are supplied and durably committed before Accept.
    pub async fn receive(
        &self,
        destination: FfiDestinationRequestV2,
        observer: Arc<dyn TransferObserver>,
    ) -> Result<FfiManifestV2Completion, EnvoixError> {
        let pending = self
            .pending
            .lock()
            .await
            .take()
            .ok_or_else(|| EnvoixError::Operation {
                reason: "this authenticated offer has already been continued".into(),
            })?;
        observer.on_started(
            format!("{} items", self.entries.len()),
            self.summary.total_plaintext_bytes,
        );
        let summary = match pending
            .receive(
                core_destination_request(destination)?,
                self.state_directory.clone(),
                &self.cancellation.token,
            )
            .await
        {
            Ok(summary) => summary,
            Err(error) => {
                report_v2_failure(
                    observer.as_ref(),
                    &error,
                    FfiTransferDirection::Receive,
                    FfiFailurePhase::Committing,
                );
                return Err(op_err(error));
            }
        };
        observer.on_status("received files saved; confirming delivery".into());
        observer.on_completed(self.summary.total_plaintext_bytes);
        let saved_paths = summary
            .destination_plan
            .root_plans
            .iter()
            .filter_map(|root| summary.destination_plan.target_path_for_root(root.root_id))
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        Ok(FfiManifestV2Completion {
            job_id: self.summary.job_id.clone(),
            entry_count: u32::try_from(summary.data_plane.entry_results.len()).unwrap_or(u32::MAX),
            total_plaintext_bytes: self.summary.total_plaintext_bytes,
            delivery_proof_digest: summary.delivery_proof_digest.0.to_vec(),
            saved_paths,
        })
    }

    pub fn cancel(&self) {
        self.cancellation.token.cancel();
    }
}

struct NativeSessionEvents {
    observer: Arc<dyn TransferObserver>,
}

impl SessionEventSink for NativeSessionEvents {
    fn on_event(&self, event: SessionTransferEvent) {
        match event {
            SessionTransferEvent::Diagnostic { message } => self.observer.on_status(message),
            SessionTransferEvent::Pairing { step } => {
                self.observer.on_status(format!("pairing: {step:?}"));
            }
            SessionTransferEvent::Connecting => self.observer.on_status("connecting".into()),
            SessionTransferEvent::Connected { path } => {
                self.observer.on_status(format!("connected via {path}"));
            }
            SessionTransferEvent::PathChanged { path } => {
                self.observer.on_status(format!("path changed: {path}"));
            }
            SessionTransferEvent::Started {
                file_name,
                total_bytes,
                ..
            } => self.observer.on_started(file_name, total_bytes),
            SessionTransferEvent::Progress {
                bytes_transferred,
                total_bytes,
                ..
            } => self.observer.on_progress(bytes_transferred, total_bytes),
            SessionTransferEvent::ManifestV2Phase { phase, .. } => self.observer.on_status(
                match phase {
                    envoix_client::api::ManifestV2ProgressPhase::Transferring => "transferring files",
                    envoix_client::api::ManifestV2ProgressPhase::Verifying => "verifying received content",
                    envoix_client::api::ManifestV2ProgressPhase::Saving => "saving verified files",
                    envoix_client::api::ManifestV2ProgressPhase::WaitingForReceiverSave => "waiting for receiver to save files",
                    envoix_client::api::ManifestV2ProgressPhase::Received => "received files saved; confirming delivery",
                }
                .into(),
            ),
            SessionTransferEvent::Confirming { .. } => self
                .observer
                .on_status("waiting for receiver to save files".into()),
            SessionTransferEvent::HashStarted { .. } => {
                self.observer.on_status("verifying".into());
            }
            SessionTransferEvent::HashCompleted { .. } => {
                self.observer.on_status("verified".into());
            }
            SessionTransferEvent::Completed {
                bytes_transferred,
                ..
            } => self.observer.on_completed(bytes_transferred),
            SessionTransferEvent::Failed { reason, .. } => self.observer.on_failed(reason),
        }
    }
}

#[uniffi::export]
pub async fn send_transfer_job_v2(
    job: Arc<FfiTransferJobV2>,
    settings: EnvoixRuntimeSettings,
    request: FfiTransferRequest,
    state_directory: String,
    cancellation: Arc<FfiManifestV2Cancellation>,
    observer: Arc<dyn TransferObserver>,
) -> Result<FfiManifestV2Completion, EnvoixError> {
    if request.direction != FfiTransferDirection::Send {
        return Err(EnvoixError::Operation {
            reason: "send_transfer_job_v2 requires a send request".into(),
        });
    }
    let state_directory = required_directory(state_directory, "state_directory")?;
    let job = job.clone_sealed_job().await?;
    let manifest = job
        .manifest()
        .expect("clone_sealed_job guarantees a manifest")
        .clone();
    observer.on_started(
        format!("{} items", manifest.entries.len()),
        manifest.totals.total_plaintext_bytes,
    );
    let attempts = peer_sources_for_request(&settings, &request)?;
    let mut last_error = None;
    for attempt in attempts {
        let options = transfer_options_for_request(
            &settings,
            &request,
            attempt.path_policy_override,
        )?;
        let client = build_client_for_request(&settings, &request)?;
        let config = client.session_config(&options);
        let events: Arc<dyn SessionEventSink> = Arc::new(NativeSessionEvents {
            observer: observer.clone(),
        });
        let result = send_attempt(
            &attempt.source,
            &job,
            state_directory.clone(),
            config,
            events,
            &cancellation.token,
            options.relay.as_deref(),
        )
        .await;
        match result {
            Ok(summary) => {
                observer.on_status("receiver saved files; delivery confirmed".into());
                observer.on_completed(manifest.totals.total_plaintext_bytes);
                return Ok(FfiManifestV2Completion {
                    job_id: encode_job_id(manifest.job_id),
                    entry_count: u32::try_from(summary.data_plane.entry_results.len())
                        .unwrap_or(u32::MAX),
                    total_plaintext_bytes: manifest.totals.total_plaintext_bytes,
                    delivery_proof_digest: summary.delivery_proof_digest.0.to_vec(),
                    saved_paths: Vec::new(),
                });
            }
            Err(error) if !cancellation.token.is_cancelled() => {
                observer.on_status(format!("route failed; trying next route: {error}"));
                last_error = Some(error);
            }
            Err(error) => {
                report_v2_failure(
                    observer.as_ref(),
                    &error,
                    FfiTransferDirection::Send,
                    FfiFailurePhase::Transferring,
                );
                return Err(op_err(error));
            }
        }
    }
    let error = last_error.unwrap_or_else(|| {
        SessionError::InvalidInput("no canonical send route is available".into())
    });
    report_v2_failure(
        observer.as_ref(),
        &error,
        FfiTransferDirection::Send,
        FfiFailurePhase::Connecting,
    );
    Err(op_err(error))
}

#[uniffi::export]
pub async fn receive_transfer_offer_v2(
    settings: EnvoixRuntimeSettings,
    request: FfiTransferRequest,
    state_directory: String,
    cancellation: Arc<FfiManifestV2Cancellation>,
    observer: Arc<dyn TransferObserver>,
) -> Result<Arc<FfiPendingManifestV2Receive>, EnvoixError> {
    if request.direction != FfiTransferDirection::Receive {
        return Err(EnvoixError::Operation {
            reason: "receive_transfer_offer_v2 requires a receive request".into(),
        });
    }
    let state_directory = required_directory(state_directory, "state_directory")?;
    let attempts = peer_sources_for_request(&settings, &request)?;
    let mut last_error = None;
    for attempt in attempts {
        let options = transfer_options_for_request(
            &settings,
            &request,
            attempt.path_policy_override,
        )?;
        let client = build_client_for_request(&settings, &request)?;
        let config = client.session_config(&options);
        let events: Arc<dyn SessionEventSink> = Arc::new(NativeSessionEvents {
            observer: observer.clone(),
        });
        match receive_offer_attempt(
            &attempt.source,
            config,
            events,
            observer.clone(),
            &cancellation.token,
            options.relay.as_deref(),
        )
        .await
        {
            Ok(pending) => {
                return Ok(Arc::new(project_pending_offer(
                    pending,
                    state_directory,
                    cancellation,
                )));
            }
            Err(error) if !cancellation.token.is_cancelled() => {
                observer.on_status(format!("route failed; trying next route: {error}"));
                last_error = Some(error);
            }
            Err(error) => {
                report_v2_failure(
                    observer.as_ref(),
                    &error,
                    FfiTransferDirection::Receive,
                    FfiFailurePhase::Connecting,
                );
                return Err(op_err(error));
            }
        }
    }
    let error = last_error.unwrap_or_else(|| {
        SessionError::InvalidInput("no canonical receive route is available".into())
    });
    report_v2_failure(
        observer.as_ref(),
        &error,
        FfiTransferDirection::Receive,
        FfiFailurePhase::Connecting,
    );
    Err(op_err(error))
}

async fn send_attempt(
    source: &PeerSource,
    job: &envoix_client::api::CanonicalTransferJob,
    state_directory: PathBuf,
    config: envoix_client::api::SessionConfig,
    events: Arc<dyn SessionEventSink>,
    cancel: &TransferCancelToken,
    relay: Option<&str>,
) -> Result<envoix_client::api::SenderManifestV2SessionSummary, SessionError>
{
    match source {
        PeerSource::Manual { peer, token } => {
            let pairing = PairingConfig::spake2_shared_token(token.clone())?;
            send_manifest_v2_manual(
                peer.clone(),
                job,
                state_directory,
                config,
                &pairing,
                events,
                cancel,
            )
            .await
        }
        PeerSource::Invite { invite } => {
            let payload = QrInvitePayload::decode(invite).map_err(op_err_core)?;
            payload.validate(now_unix_seconds()).map_err(op_err_core)?;
            let pairing = PairingConfig::spake2_shared_token(payload.token.clone())?;
            let endpoint = payload.endpoint_addr().map_err(op_err_core)?;
            send_manifest_v2_to_endpoint_addr(
                endpoint,
                job,
                state_directory,
                config,
                &pairing,
                events,
                cancel,
            )
            .await
        }
        PeerSource::Mdns { token: Some(token) } => {
            let pairing = PairingConfig::spake2_shared_token(token.clone())?;
            send_manifest_v2_enable_mdns(
                job.clone(),
                state_directory,
                config,
                &pairing,
                events,
                cancel.clone(),
            )
            .await
        }
        PeerSource::Room { code, broker } => {
            let broker = parse_broker_addr(broker, relay)?;
            send_manifest_v2_via_room(
                broker,
                code,
                job,
                state_directory,
                config,
                events,
                cancel,
            )
            .await
        }
        _ => Err(SessionError::InvalidInput(
            "selected route cannot dial a canonical receiver".into(),
        )),
    }
}

async fn receive_offer_attempt(
    source: &PeerSource,
    config: envoix_client::api::SessionConfig,
    events: Arc<dyn SessionEventSink>,
    observer: Arc<dyn TransferObserver>,
    cancel: &TransferCancelToken,
    relay: Option<&str>,
) -> Result<PendingManifestV2Receive, SessionError> {
    let listen = config
        .clone();
    match source {
        PeerSource::ShowManual { token } => {
            let token = token.clone().map(Ok).unwrap_or_else(generate_token).map_err(op_err_core)?;
            let pairing = PairingConfig::spake2_shared_token(token.clone())?;
            receive_manifest_v2_offer_with_bound_peer(
                listen_addrs(&listen),
                config,
                &pairing,
                events,
                move |peer, _| {
                    observer.on_invite_ready(peer.to_string());
                    observer.on_status(format!("receiver token: {token}"));
                },
                cancel,
            )
            .await
        }
        PeerSource::ShowInvite { token, ttl_secs } => {
            let token = token.clone().map(Ok).unwrap_or_else(generate_token).map_err(op_err_core)?;
            let pairing = PairingConfig::spake2_shared_token(token.clone())?;
            let expires_at = now_unix_seconds().saturating_add(*ttl_secs);
            receive_manifest_v2_offer_with_bound_peer(
                listen_addrs(&listen),
                config,
                &pairing,
                events,
                move |peer, relay_urls| {
                    observer.on_invite_ready(
                        QrInvitePayload {
                            version: envoix_qr::PAYLOAD_VERSION,
                            protocol_version: envoix_types::PROTOCOL_VERSION,
                            token,
                            peer,
                            relay_urls,
                            expires_at,
                            flags: 0,
                        }
                        .encode(),
                    );
                },
                cancel,
            )
            .await
        }
        PeerSource::Mdns { token } => {
            let token = token.clone().map(Ok).unwrap_or_else(generate_token).map_err(op_err_core)?;
            let pairing = PairingConfig::spake2_shared_token(token.clone())?;
            let expires_at = now_unix_seconds().saturating_add(300);
            receive_manifest_v2_offer_enable_mdns(
                listen_addrs(&listen),
                config,
                &pairing,
                events,
                move |peer, relay_urls| {
                    observer.on_invite_ready(
                        QrInvitePayload {
                            version: envoix_qr::PAYLOAD_VERSION,
                            protocol_version: envoix_types::PROTOCOL_VERSION,
                            token,
                            peer,
                            relay_urls,
                            expires_at,
                            flags: 0,
                        }
                        .encode(),
                    );
                },
                cancel,
            )
            .await
        }
        PeerSource::Room { code, broker } => {
            let broker = parse_broker_addr(broker, relay)?;
            receive_manifest_v2_offer_via_room(
                broker,
                code,
                listen_addrs(&listen),
                config,
                events,
                cancel,
            )
            .await
        }
        _ => Err(SessionError::InvalidInput(
            "selected route cannot listen for a canonical sender".into(),
        )),
    }
}

fn project_pending_offer(
    pending: PendingManifestV2Receive,
    state_directory: PathBuf,
    cancellation: Arc<FfiManifestV2Cancellation>,
) -> FfiPendingManifestV2Receive {
    let manifest = &pending.offer().manifest;
    let total = manifest.totals.total_plaintext_bytes;
    let summary = FfiManifestOfferSummaryV2 {
        job_id: encode_job_id(manifest.job_id),
        generation: manifest.generation,
        selection_revision: manifest.selection_revision,
        root_count: u32::try_from(manifest.roots.len()).unwrap_or(u32::MAX),
        file_count: manifest.totals.file_count,
        directory_count: manifest.totals.directory_count,
        total_plaintext_bytes: total,
        exceptional_offer: total > envoix_client::api::AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES,
    };
    let entries = manifest
        .entries
        .iter()
        .map(|entry| FfiManifestOfferEntryV2 {
            entry_id: entry.entry_id,
            root_id: entry.root_id,
            parent_entry_id: entry.parent_entry_id,
            name: entry.component.clone(),
            kind: match entry.kind {
                envoix_client::api::ManifestEntryKindV2::RegularFile => {
                    FfiManifestEntryKindV2::File
                }
                envoix_client::api::ManifestEntryKindV2::Directory => {
                    FfiManifestEntryKindV2::Directory
                }
            },
            plaintext_size: entry.plaintext_size,
            digest_known: matches!(
                entry.content_digest,
                envoix_client::api::EntryContentDigestV2::Known(_)
            ),
        })
        .collect();
    FfiPendingManifestV2Receive {
        pending: Mutex::new(Some(pending)),
        summary,
        entries,
        state_directory,
        cancellation,
    }
}

fn core_destination_request(
    request: FfiDestinationRequestV2,
) -> Result<DestinationRequestV2, EnvoixError> {
    Ok(DestinationRequestV2 {
        target_directory: required_directory(request.target_directory, "target_directory")?,
        copy_staging_directory: request
            .copy_staging_directory
            .filter(|path| !path.trim().is_empty())
            .map(PathBuf::from),
        decision: match request.decision {
            FfiDestinationDecisionV2::SaveDirectly => DestinationDecisionV2::UseDirectSave,
            FfiDestinationDecisionV2::CopyAfterVerify => {
                DestinationDecisionV2::ContinueWithCopyAfterVerify
            }
        },
        target_allocatable_bytes: request.target_allocatable_bytes,
        staging_allocatable_bytes: request.staging_allocatable_bytes,
        stable_object_identity: request.stable_object_identity,
        exceptional_transfer_approved: request.exceptional_transfer_approved,
    })
}

fn listen_addrs(config: &envoix_client::api::SessionConfig) -> envoix_client::BindAddrs {
    // SessionConfig does not own listen addresses; native options currently
    // freeze the canonical dual-stack ephemeral binding.
    let _ = config;
    envoix_client::BindAddrs::dual_stack(0)
}

fn required_directory(value: String, field: &str) -> Result<PathBuf, EnvoixError> {
    if value.trim().is_empty() {
        return Err(EnvoixError::Operation {
            reason: format!("{field} must not be empty"),
        });
    }
    Ok(PathBuf::from(value))
}

fn encode_job_id(job_id: envoix_client::api::JobIdV2) -> String {
    job_id.0.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn report_v2_failure(
    observer: &dyn TransferObserver,
    error: &SessionError,
    direction: FfiTransferDirection,
    fallback_phase: FfiFailurePhase,
) {
    let (code, category, phase, origin, retryable, recovery_action, message_key) = match error {
        SessionError::Cause { cause, .. } => manifest_v2_cause_projection(cause.code()),
        SessionError::Cancelled => (
            FfiFailureCode::UserCanceled,
            FfiFailureCategory::User,
            fallback_phase,
            FfiFailureOrigin::Local,
            false,
            FfiRecoveryAction::None,
            "transfer.user_canceled",
        ),
        SessionError::Transport(_) | SessionError::Discovery(_) => (
            FfiFailureCode::NetworkLost,
            FfiFailureCategory::Network,
            fallback_phase,
            FfiFailureOrigin::Unknown,
            true,
            FfiRecoveryAction::Resume,
            "transfer.network_lost",
        ),
        SessionError::Crypto(_) => (
            FfiFailureCode::AuthenticationFailed,
            FfiFailureCategory::Authentication,
            FfiFailurePhase::Authenticating,
            FfiFailureOrigin::Unknown,
            true,
            FfiRecoveryAction::RePair,
            "transfer.authentication_failed",
        ),
        SessionError::Protocol(_) => (
            FfiFailureCode::ProtocolOrIntegrityFailure,
            FfiFailureCategory::Integrity,
            FfiFailurePhase::Verifying,
            FfiFailureOrigin::Unknown,
            false,
            FfiRecoveryAction::None,
            "transfer.protocol_or_integrity_failure",
        ),
        SessionError::Io(_) | SessionError::Storage(_) => (
            if direction == FfiTransferDirection::Receive {
                FfiFailureCode::ReceiverSaveFailed
            } else {
                FfiFailureCode::SenderSourceUnavailable
            },
            FfiFailureCategory::Storage,
            fallback_phase,
            FfiFailureOrigin::Local,
            true,
            FfiRecoveryAction::Retry,
            if direction == FfiTransferDirection::Receive {
                "transfer.receiver_save_failed"
            } else {
                "transfer.sender_source_unavailable"
            },
        ),
        SessionError::InvalidInput(_) => (
            FfiFailureCode::UnsupportedFeature,
            FfiFailureCategory::Unsupported,
            fallback_phase,
            FfiFailureOrigin::Local,
            false,
            FfiRecoveryAction::None,
            "transfer.unsupported_feature",
        ),
        SessionError::Transfer(_) => (
            FfiFailureCode::InternalError,
            FfiFailureCategory::Internal,
            fallback_phase,
            FfiFailureOrigin::Unknown,
            true,
            FfiRecoveryAction::Retry,
            "transfer.internal_error",
        ),
    };
    observer.on_transfer_failed(FfiTransferFailure {
        code,
        category,
        phase,
        origin,
        direction,
        transfer_id: String::new(),
        attempt_id: String::new(),
        retryable,
        recovery_action,
        user_message_key: message_key.into(),
        diagnostic_message: error.to_string(),
    });
    observer.on_failed(error.to_string());
}

#[allow(clippy::type_complexity)]
fn manifest_v2_cause_projection(
    cause: &str,
) -> (
    FfiFailureCode,
    FfiFailureCategory,
    FfiFailurePhase,
    FfiFailureOrigin,
    bool,
    FfiRecoveryAction,
    &'static str,
) {
    let local = FfiFailureOrigin::Local;
    match cause {
        "sender_source_unavailable" => (
            FfiFailureCode::SenderSourceUnavailable,
            FfiFailureCategory::Storage,
            FfiFailurePhase::Transferring,
            local,
            true,
            FfiRecoveryAction::Retry,
            "transfer.sender_source_unavailable",
        ),
        "sender_permission_lost" => (
            FfiFailureCode::SenderPermissionLost,
            FfiFailureCategory::Permission,
            FfiFailurePhase::Transferring,
            local,
            true,
            FfiRecoveryAction::OpenSettings,
            "transfer.sender_permission_lost",
        ),
        "sender_source_changed" => (
            FfiFailureCode::SenderSourceChanged,
            FfiFailureCategory::Integrity,
            FfiFailurePhase::Verifying,
            local,
            true,
            FfiRecoveryAction::Retry,
            "transfer.sender_source_changed",
        ),
        "sender_item_removed" => (
            FfiFailureCode::SenderItemRemoved,
            FfiFailureCategory::User,
            FfiFailurePhase::Transferring,
            local,
            false,
            FfiRecoveryAction::None,
            "transfer.sender_item_removed",
        ),
        "sender_canceled" => (
            FfiFailureCode::SenderCanceled,
            FfiFailureCategory::User,
            FfiFailurePhase::Transferring,
            local,
            false,
            FfiRecoveryAction::None,
            "transfer.sender_canceled",
        ),
        "receiver_space_insufficient" => (
            FfiFailureCode::ReceiverSpaceInsufficient,
            FfiFailureCategory::Storage,
            FfiFailurePhase::Negotiating,
            local,
            true,
            FfiRecoveryAction::ChooseFolder,
            "transfer.receiver_space_insufficient",
        ),
        "receiver_destination_decision_required" => (
            FfiFailureCode::ReceiverDestinationDecisionRequired,
            FfiFailureCategory::Storage,
            FfiFailurePhase::Negotiating,
            local,
            true,
            FfiRecoveryAction::ChooseFolder,
            "transfer.receiver_destination_decision_required",
        ),
        "receiver_destination_unavailable" => (
            FfiFailureCode::ReceiverDestinationUnavailable,
            FfiFailureCategory::Storage,
            FfiFailurePhase::Committing,
            local,
            true,
            FfiRecoveryAction::ChooseFolder,
            "transfer.receiver_destination_unavailable",
        ),
        "receiver_save_failed" => (
            FfiFailureCode::ReceiverSaveFailed,
            FfiFailureCategory::Storage,
            FfiFailurePhase::Committing,
            local,
            true,
            FfiRecoveryAction::Resume,
            "transfer.receiver_save_failed",
        ),
        "receiver_reused_object_lost" => (
            FfiFailureCode::ReceiverReusedObjectLost,
            FfiFailureCategory::Storage,
            FfiFailurePhase::Committing,
            local,
            true,
            FfiRecoveryAction::Resume,
            "transfer.receiver_reused_object_lost",
        ),
        "receiver_finalization_outcome_unknown" => (
            FfiFailureCode::ReceiverFinalizationOutcomeUnknown,
            FfiFailureCategory::Storage,
            FfiFailurePhase::Committing,
            local,
            true,
            FfiRecoveryAction::Resume,
            "transfer.receiver_finalization_outcome_unknown",
        ),
        _ => (
            FfiFailureCode::ProtocolOrIntegrityFailure,
            FfiFailureCategory::Integrity,
            FfiFailurePhase::Verifying,
            FfiFailureOrigin::Unknown,
            false,
            FfiRecoveryAction::None,
            "transfer.protocol_or_integrity_failure",
        ),
    }
}

fn op_err_core(error: impl std::fmt::Display) -> SessionError {
    SessionError::InvalidInput(error.to_string())
}
