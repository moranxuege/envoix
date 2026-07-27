//! Native Manifest-v2 session facade.
//!
//! The bridge exposes one canonical job/session path for every source shape.
//! Receiver metadata is returned before destination approval and payload.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use envoix_client::api::{
    AuthenticationHandler, AuthenticationOutcome, DestinationDecisionV2, DestinationRequestV2,
    EventSink, InvitationConsumption, PairingConfig, PeerSource, PendingManifestV2Receive,
    RendezvousCause, SessionError, TransferCancelToken, TransferEvent, acquire_invitation,
    acquire_remembered_credential, acquire_shared_token, parse_broker_addr,
    receive_manifest_v2_offer_enable_mdns, receive_manifest_v2_offer_via_remembered,
    receive_manifest_v2_offer_via_room_with_authentication,
    receive_manifest_v2_offer_with_bound_peer, send_manifest_v2_enable_mdns,
    send_manifest_v2_manual, send_manifest_v2_via_remembered,
    send_manifest_v2_via_room_with_authentication,
};
use envoix_types::{DataPath, PairingStep};
use tokio::sync::Mutex;
use tokio::task::JoinSet;

use super::{
    EnvoixError, EnvoixRuntimeSettings, FfiConnectionPathEvent, FfiConnectionPathEventKind,
    FfiDataPathKind, FfiFailureCategory, FfiFailureCode, FfiFailureOrigin, FfiFailurePhase,
    FfiManifestV2Phase, FfiRecoveryAction, FfiTransferDirection, FfiTransferFailure,
    FfiTransferJobV2, FfiTransferRequest, TransferObserver, build_client_for_request,
    on_ffi_runtime, op_err, peer_sources_for_request, spawn_on_ffi_runtime,
    transfer_options_for_request,
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
        on_ffi_runtime(async {
            let destination = core_destination_request(destination)?;
            let pending =
                self.pending
                    .lock()
                    .await
                    .take()
                    .ok_or_else(|| EnvoixError::Operation {
                        reason: "this authenticated offer has already been continued".into(),
                    })?;
            observer.on_started(
                u32::try_from(self.entries.len()).unwrap_or(u32::MAX),
                self.summary.total_plaintext_bytes,
            );
            let summary = match pending
                .receive(
                    destination,
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
            observer.on_phase(FfiManifestV2Phase::Delivered);
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
                entry_count: u32::try_from(summary.data_plane.entry_results.len())
                    .unwrap_or(u32::MAX),
                total_plaintext_bytes: self.summary.total_plaintext_bytes,
                delivery_proof_digest: summary.delivery_proof_digest.0.to_vec(),
                saved_paths,
            })
        })
        .await
    }

    pub fn cancel(&self) {
        self.cancellation.token.cancel();
    }
}

struct NativeSessionEvents {
    observer: Arc<dyn TransferObserver>,
}

fn project_connection_path(
    path: &DataPath,
    event_kind: FfiConnectionPathEventKind,
) -> FfiConnectionPathEvent {
    let path_kind = match path {
        DataPath::Direct { .. } => FfiDataPathKind::Direct,
        DataPath::Relay { .. } => FfiDataPathKind::Relay,
        DataPath::Other { .. } => FfiDataPathKind::Other,
    };
    FfiConnectionPathEvent {
        path_kind,
        event_kind,
    }
}

struct NativeAuthentication {
    observer: Arc<dyn TransferObserver>,
    remember_consent: bool,
    rotation: Option<(Vec<u8>, u64)>,
    invitation_consumption: Option<InvitationConsumption>,
    authenticated: AtomicBool,
    persisted: AtomicBool,
}

struct SendAttemptContext<'a> {
    job: &'a envoix_client::api::CanonicalTransferJob,
    state_directory: PathBuf,
    config: envoix_client::api::SessionConfig,
    events: Arc<dyn EventSink>,
    cancel: &'a TransferCancelToken,
    relay: Option<&'a str>,
    observer: Arc<dyn TransferObserver>,
    remember_consent: bool,
}

impl NativeAuthentication {
    fn invitation(
        observer: Arc<dyn TransferObserver>,
        remember_consent: bool,
        invitation_consumption: InvitationConsumption,
    ) -> Self {
        Self {
            observer,
            remember_consent,
            rotation: None,
            invitation_consumption: Some(invitation_consumption),
            authenticated: AtomicBool::new(false),
            persisted: AtomicBool::new(false),
        }
    }

    fn rotation(
        observer: Arc<dyn TransferObserver>,
        opaque_credential: Vec<u8>,
        next_generation: u64,
    ) -> Self {
        Self {
            observer,
            remember_consent: false,
            rotation: Some((opaque_credential, next_generation)),
            invitation_consumption: None,
            authenticated: AtomicBool::new(false),
            persisted: AtomicBool::new(false),
        }
    }

    fn authenticated(&self) -> bool {
        self.authenticated.load(Ordering::Acquire)
    }

    fn persisted(&self) -> bool {
        self.persisted.load(Ordering::Acquire)
    }
}

impl AuthenticationHandler for NativeAuthentication {
    fn remember_consent(&self) -> bool {
        self.remember_consent
    }

    fn on_authenticated(&self, outcome: AuthenticationOutcome) -> Result<(), SessionError> {
        self.authenticated.store(true, Ordering::Release);
        if let Some(consumption) = &self.invitation_consumption {
            consumption.consume();
        }
        let credential = if let Some(secret) = outcome.remember_secret {
            Some((secret.into_credential().to_opaque(), 0))
        } else {
            self.rotation.clone()
        };
        let Some((opaque, generation)) = credential else {
            return Ok(());
        };
        if !self.observer.on_remembered_credential(opaque, generation) {
            return Err(SessionError::Storage(
                "protected remembered credential could not be persisted".into(),
            ));
        }
        self.persisted.store(true, Ordering::Release);
        Ok(())
    }
}

fn should_stop_remembered_fallback<T>(
    result: &Result<T, SessionError>,
    authentication: &NativeAuthentication,
    cancel: &TransferCancelToken,
) -> bool {
    result.is_ok() || authentication.authenticated() || cancel.is_cancelled()
}

impl EventSink for NativeSessionEvents {
    fn on_event(&self, event: TransferEvent) {
        match event {
            TransferEvent::Diagnostic { message } => self.observer.on_diagnostic(message),
            TransferEvent::Pairing { step } => {
                if let Some(phase) = pairing_phase(step) {
                    self.observer.on_phase(phase);
                }
                self.observer.on_diagnostic(format!("pairing: {step:?}"));
            }
            TransferEvent::Connecting => {
                self.observer.on_phase(FfiManifestV2Phase::Connecting);
            }
            TransferEvent::Connected { path } => {
                let event = project_connection_path(&path, FfiConnectionPathEventKind::Selected);
                self.observer.on_connection_path(event);
                self.observer
                    .on_diagnostic(format!("connected via {:?}", event.path_kind));
            }
            TransferEvent::PathChanged { path } => {
                let event = project_connection_path(&path, FfiConnectionPathEventKind::Changed);
                self.observer.on_connection_path(event);
                self.observer
                    .on_diagnostic(format!("path changed: {:?}", event.path_kind));
            }
            TransferEvent::Progress {
                bytes_transferred,
                total_bytes,
                ..
            } => self.observer.on_progress(bytes_transferred, total_bytes),
            TransferEvent::ManifestV2Phase { phase, .. } => self.observer.on_phase(match phase {
                envoix_client::api::ManifestV2ProgressPhase::Transferring => {
                    FfiManifestV2Phase::Transferring
                }
                envoix_client::api::ManifestV2ProgressPhase::Verifying => {
                    FfiManifestV2Phase::Verifying
                }
                envoix_client::api::ManifestV2ProgressPhase::Saving => FfiManifestV2Phase::Saving,
                envoix_client::api::ManifestV2ProgressPhase::WaitingForReceiverSave => {
                    FfiManifestV2Phase::WaitingForReceiverSave
                }
                envoix_client::api::ManifestV2ProgressPhase::FinalizingDelivery => {
                    FfiManifestV2Phase::FinalizingDelivery
                }
            }),
        }
    }
}

fn pairing_phase(step: PairingStep) -> Option<FfiManifestV2Phase> {
    match step {
        PairingStep::Joining => Some(FfiManifestV2Phase::WaitingForPeer),
        PairingStep::Matched | PairingStep::Exchanged => Some(FfiManifestV2Phase::Pairing),
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
    spawn_on_ffi_runtime(async move {
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
            u32::try_from(manifest.entries.len()).unwrap_or(u32::MAX),
            manifest.totals.total_plaintext_bytes,
        );
        let attempts = peer_sources_for_request(&settings, &request)?;
        let mut last_error = None;
        for attempt in attempts {
            let options =
                transfer_options_for_request(&settings, &request, attempt.path_policy_override)?;
            let client = build_client_for_request(&settings, &request)?;
            let config = client.session_config(&options);
            let events: Arc<dyn EventSink> = Arc::new(NativeSessionEvents {
                observer: observer.clone(),
            });
            let result = send_attempt(
                &attempt.source,
                SendAttemptContext {
                    job: &job,
                    state_directory: state_directory.clone(),
                    config,
                    events,
                    cancel: &cancellation.token,
                    relay: options.relay.as_deref(),
                    observer: observer.clone(),
                    remember_consent: request.remember_consent,
                },
            )
            .await;
            match result {
                Ok(summary) => {
                    observer.on_phase(FfiManifestV2Phase::Delivered);
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
                Err(error)
                    if !cancellation.token.is_cancelled()
                        && !matches!(error, SessionError::InvitationConsumed(_)) =>
                {
                    observer.on_diagnostic(format!("route failed; trying next route: {error}"));
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
    })
    .await
}

#[uniffi::export]
pub async fn receive_transfer_offer_v2(
    settings: EnvoixRuntimeSettings,
    request: FfiTransferRequest,
    state_directory: String,
    cancellation: Arc<FfiManifestV2Cancellation>,
    observer: Arc<dyn TransferObserver>,
) -> Result<Arc<FfiPendingManifestV2Receive>, EnvoixError> {
    spawn_on_ffi_runtime(async move {
        if request.direction != FfiTransferDirection::Receive {
            return Err(EnvoixError::Operation {
                reason: "receive_transfer_offer_v2 requires a receive request".into(),
            });
        }
        let state_directory = required_directory(state_directory, "state_directory")?;
        let attempts = peer_sources_for_request(&settings, &request)?;
        receive_offer_from_attempts(
            settings,
            request,
            attempts,
            state_directory,
            cancellation,
            observer,
        )
        .await
    })
    .await
}

async fn receive_offer_from_attempts(
    settings: EnvoixRuntimeSettings,
    request: FfiTransferRequest,
    attempts: Vec<super::RouteAttempt>,
    state_directory: PathBuf,
    cancellation: Arc<FfiManifestV2Cancellation>,
    observer: Arc<dyn TransferObserver>,
) -> Result<Arc<FfiPendingManifestV2Receive>, EnvoixError> {
    if attempts.len() == 1 {
        let attempt = attempts
            .into_iter()
            .next()
            .ok_or_else(|| EnvoixError::Operation {
                reason: "no canonical receive route is available".into(),
            })?;
        let pending = receive_one_offer_attempt(
            &settings,
            &request,
            attempt,
            observer.clone(),
            &cancellation.token,
        )
        .await
        .map_err(|error| {
            report_v2_failure(
                observer.as_ref(),
                &error,
                FfiTransferDirection::Receive,
                FfiFailurePhase::Connecting,
            );
            op_err(error)
        })?;
        return Ok(Arc::new(project_pending_offer(
            pending,
            state_directory,
            cancellation,
        )));
    }

    let mut routes = JoinSet::new();
    let mut route_cancellations = Vec::with_capacity(attempts.len());
    for (index, attempt) in attempts.into_iter().enumerate() {
        let route_cancellation = TransferCancelToken::new();
        route_cancellations.push(route_cancellation.clone());
        let settings = settings.clone();
        let request = request.clone();
        let observer = observer.clone();
        routes.spawn(async move {
            let result = receive_one_offer_attempt(
                &settings,
                &request,
                attempt,
                observer,
                &route_cancellation,
            )
            .await;
            (index, result)
        });
    }

    let mut last_error = None;
    loop {
        tokio::select! {
            joined = routes.join_next() => match joined {
                Some(Ok((winner, Ok(pending)))) => {
                    for (index, token) in route_cancellations.iter().enumerate() {
                        if index != winner {
                            token.cancel();
                        }
                    }
                    while routes.join_next().await.is_some() {}
                    return Ok(Arc::new(project_pending_offer(
                        pending,
                        state_directory,
                        cancellation,
                    )));
                }
                Some(Ok((_, Err(error @ SessionError::InvitationConsumed(_))))) => {
                    route_cancellations.iter().for_each(TransferCancelToken::cancel);
                    while routes.join_next().await.is_some() {}
                    report_v2_failure(
                        observer.as_ref(),
                        &error,
                        FfiTransferDirection::Receive,
                        FfiFailurePhase::Authenticating,
                    );
                    return Err(op_err(error));
                }
                Some(Ok((_, Err(error)))) => {
                    observer.on_diagnostic(format!("receive route failed; keeping other routes active: {error}"));
                    last_error = Some(error);
                    if routes.is_empty() {
                        break;
                    }
                }
                Some(Err(error)) => {
                    last_error = Some(SessionError::Transfer(format!("receive route task failed: {error}")));
                    if routes.is_empty() {
                        break;
                    }
                }
                None => break,
            },
            () = cancellation.token.cancelled() => {
                route_cancellations.iter().for_each(TransferCancelToken::cancel);
                while routes.join_next().await.is_some() {}
                let error = SessionError::Cancelled;
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

async fn receive_one_offer_attempt(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
    attempt: super::RouteAttempt,
    observer: Arc<dyn TransferObserver>,
    cancel: &TransferCancelToken,
) -> Result<PendingManifestV2Receive, SessionError> {
    let options = transfer_options_for_request(settings, request, attempt.path_policy_override)
        .map_err(|error| SessionError::InvalidInput(error.to_string()))?;
    let client = build_client_for_request(settings, request)
        .map_err(|error| SessionError::InvalidInput(error.to_string()))?;
    let config = client.session_config(&options);
    let events: Arc<dyn EventSink> = Arc::new(NativeSessionEvents {
        observer: observer.clone(),
    });
    receive_offer_attempt(
        &attempt.source,
        config,
        events,
        observer,
        cancel,
        options.relay.as_deref(),
        request.remember_consent,
    )
    .await
}

async fn send_attempt(
    source: &PeerSource,
    context: SendAttemptContext<'_>,
) -> Result<envoix_client::api::SenderManifestV2SessionSummary, SessionError> {
    let SendAttemptContext {
        job,
        state_directory,
        config,
        events,
        cancel,
        relay,
        observer,
        remember_consent,
    } = context;
    match source {
        PeerSource::Manual { peer, token_ref } => {
            let token = acquire_shared_token(token_ref).map_err(op_err_core)?;
            let pairing = PairingConfig::spake2_shared_token(token)?;
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
        PeerSource::Invitation {
            secret_ref, broker, ..
        } => {
            let lease = acquire_invitation(secret_ref).map_err(op_err_core)?;
            let broker = parse_broker_addr(broker, relay)?;
            let authentication =
                NativeAuthentication::invitation(observer, remember_consent, lease.consumption());
            let result = send_manifest_v2_via_room_with_authentication(
                broker,
                lease.bootstrap().clone(),
                job,
                state_directory,
                config,
                events,
                cancel,
                &authentication,
            )
            .await;
            if result.is_ok() || authentication.authenticated() {
                lease.consume();
            }
            result
        }
        PeerSource::Remembered {
            credential_ref,
            generation,
            previous_generation,
            broker,
        } => {
            let credential = acquire_remembered_credential(credential_ref).map_err(op_err_core)?;
            let broker_addr = parse_broker_addr(broker, relay)?;
            // Keep joining the receiver's fallback window before trying our
            // own previous generation.
            let mut generations = vec![*generation, *generation];
            if let Some(previous) = previous_generation {
                generations.push(*previous);
            }
            let mut last_error = None;
            for generation in generations {
                let next_generation = generation.checked_add(1).ok_or_else(|| {
                    SessionError::InvalidInput(
                        "remembered credential generation is exhausted".into(),
                    )
                })?;
                let authentication = NativeAuthentication::rotation(
                    observer.clone(),
                    credential.to_opaque(),
                    next_generation,
                );
                let result = send_manifest_v2_via_remembered(
                    broker_addr.clone(),
                    broker.clone(),
                    credential.derive_session(generation),
                    job,
                    state_directory.clone(),
                    config.clone(),
                    events.clone(),
                    cancel,
                    &authentication,
                )
                .await;
                if should_stop_remembered_fallback(&result, &authentication, cancel) {
                    return result;
                }
                last_error = result.err();
            }
            Err(last_error.unwrap_or_else(|| {
                SessionError::InvalidInput("remembered credential has no usable generation".into())
            }))
        }
        PeerSource::Mdns {
            token_ref: Some(token_ref),
        } => {
            let token = acquire_shared_token(token_ref).map_err(op_err_core)?;
            let pairing = PairingConfig::spake2_shared_token(token)?;
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
        _ => Err(SessionError::InvalidInput(
            "selected route cannot dial a canonical receiver".into(),
        )),
    }
}

async fn receive_offer_attempt(
    source: &PeerSource,
    config: envoix_client::api::SessionConfig,
    events: Arc<dyn EventSink>,
    observer: Arc<dyn TransferObserver>,
    cancel: &TransferCancelToken,
    relay: Option<&str>,
    remember_consent: bool,
) -> Result<PendingManifestV2Receive, SessionError> {
    let listen = config.clone();
    match source {
        PeerSource::ShowManual { token_ref } => {
            let token_ref = token_ref.as_ref().ok_or_else(|| {
                SessionError::InvalidInput(
                    "manual receiver display requires a caller-owned pairing token".into(),
                )
            })?;
            let token = acquire_shared_token(token_ref).map_err(op_err_core)?;
            let pairing = PairingConfig::spake2_shared_token(token)?;
            receive_manifest_v2_offer_with_bound_peer(
                listen_addrs(&listen),
                config,
                &pairing,
                events,
                move |peer, _| {
                    observer.on_invite_ready(peer.to_string());
                },
                cancel,
            )
            .await
        }
        PeerSource::Mdns { token_ref } => {
            let token = token_ref
                .as_ref()
                .map(|token_ref| acquire_shared_token(token_ref).map_err(op_err_core))
                .unwrap_or_else(generate_token)?;
            let pairing = PairingConfig::spake2_shared_token(token.clone())?;
            receive_manifest_v2_offer_enable_mdns(
                listen_addrs(&listen),
                config,
                &pairing,
                events,
                move |peer, _relay_urls| {
                    observer.on_invite_ready(peer.to_string());
                },
                cancel,
            )
            .await
        }
        PeerSource::Invitation {
            secret_ref, broker, ..
        } => {
            let lease = acquire_invitation(secret_ref).map_err(op_err_core)?;
            let broker = parse_broker_addr(broker, relay)?;
            let authentication =
                NativeAuthentication::invitation(observer, remember_consent, lease.consumption());
            let result = receive_manifest_v2_offer_via_room_with_authentication(
                broker,
                lease.bootstrap().clone(),
                listen_addrs(&listen),
                config,
                events,
                cancel,
                &authentication,
            )
            .await;
            if result.is_ok() || authentication.authenticated() {
                lease.consume();
            }
            result
        }
        PeerSource::Remembered {
            credential_ref,
            generation,
            previous_generation,
            broker,
        } => {
            let credential = acquire_remembered_credential(credential_ref).map_err(op_err_core)?;
            let broker_addr = parse_broker_addr(broker, relay)?;
            // Offset the sender's current/current/previous schedule so either
            // side of a one-generation crash can rendezvous.
            let mut generations = vec![*generation];
            if let Some(previous) = previous_generation {
                generations.push(*previous);
                generations.push(*generation);
            }
            let last_index = generations.len() - 1;
            let mut last_error = None;
            for (index, generation) in generations.into_iter().enumerate() {
                let next_generation = generation.checked_add(1).ok_or_else(|| {
                    SessionError::InvalidInput(
                        "remembered credential generation is exhausted".into(),
                    )
                })?;
                let authentication = NativeAuthentication::rotation(
                    observer.clone(),
                    credential.to_opaque(),
                    next_generation,
                );
                let receive = receive_manifest_v2_offer_via_remembered(
                    broker_addr.clone(),
                    broker.clone(),
                    credential.derive_session(generation),
                    listen_addrs(&listen),
                    config.clone(),
                    events.clone(),
                    cancel,
                    &authentication,
                );
                let result = if index < last_index {
                    match tokio::time::timeout(std::time::Duration::from_secs(35), receive).await {
                        Ok(result) => result,
                        Err(_) => Err(SessionError::Transport(
                            "current remembered generation did not find the peer".into(),
                        )),
                    }
                } else {
                    receive.await
                };
                if should_stop_remembered_fallback(&result, &authentication, cancel) {
                    return result;
                }
                last_error = result.err();
            }
            Err(last_error.unwrap_or_else(|| {
                SessionError::InvalidInput("remembered credential has no usable generation".into())
            }))
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
        preplanned_root_names: None,
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

fn generate_token() -> Result<String, SessionError> {
    let mut bytes = [0_u8; 18];
    getrandom::fill(&mut bytes)
        .map_err(|error| SessionError::Crypto(format!("token entropy unavailable: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn report_v2_failure(
    observer: &dyn TransferObserver,
    error: &SessionError,
    direction: FfiTransferDirection,
    fallback_phase: FfiFailurePhase,
) {
    let (projected_error, invitation_consumed) = match error {
        SessionError::InvitationConsumed(source) => (source.as_ref(), true),
        error => (error, false),
    };
    let (code, category, phase, origin, retryable, mut recovery_action, message_key) =
        match projected_error {
            SessionError::Cause { cause, .. } => manifest_v2_cause_projection(cause.code()),
            SessionError::Rendezvous { cause, .. } => rendezvous_cause_projection(*cause),
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
                io_failure_recovery(direction),
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
            SessionError::InvitationConsumed(_) => {
                unreachable!("consumed invitation was unwrapped")
            }
        };
    if invitation_consumed && retryable {
        recovery_action = FfiRecoveryAction::RePair;
    }
    observer.on_transfer_failed(FfiTransferFailure {
        code,
        category,
        phase,
        origin,
        direction,
        retryable,
        recovery_action,
        user_message_key: message_key.into(),
        diagnostic_message: error.to_string(),
    });
}

#[allow(clippy::type_complexity)]
fn rendezvous_cause_projection(
    cause: RendezvousCause,
) -> (
    FfiFailureCode,
    FfiFailureCategory,
    FfiFailurePhase,
    FfiFailureOrigin,
    bool,
    FfiRecoveryAction,
    &'static str,
) {
    let (code, retryable, recovery, key) = match cause {
        RendezvousCause::RoomNotFound => (
            FfiFailureCode::RoomNotFound,
            true,
            FfiRecoveryAction::Retry,
            "transfer.room_not_found",
        ),
        RendezvousCause::RoomExpired => (
            FfiFailureCode::RoomExpired,
            true,
            FfiRecoveryAction::RePair,
            "transfer.room_expired",
        ),
        RendezvousCause::RoomFull => (
            FfiFailureCode::RoomFull,
            true,
            FfiRecoveryAction::Retry,
            "transfer.room_full",
        ),
        RendezvousCause::RoomRateLimited => (
            FfiFailureCode::RoomRateLimited,
            true,
            FfiRecoveryAction::Retry,
            "transfer.room_rate_limited",
        ),
        RendezvousCause::RoomUnderAttack => (
            FfiFailureCode::RoomUnderAttack,
            true,
            FfiRecoveryAction::RePair,
            "transfer.room_under_attack",
        ),
        RendezvousCause::EndpointRateLimited => (
            FfiFailureCode::EndpointRateLimited,
            true,
            FfiRecoveryAction::Retry,
            "transfer.endpoint_rate_limited",
        ),
        RendezvousCause::IpRateLimited => (
            FfiFailureCode::IpRateLimited,
            true,
            FfiRecoveryAction::Retry,
            "transfer.ip_rate_limited",
        ),
        RendezvousCause::ServerBusy => (
            FfiFailureCode::ServerBusy,
            true,
            FfiRecoveryAction::Retry,
            "transfer.server_busy",
        ),
        RendezvousCause::MalformedJoin => (
            FfiFailureCode::MalformedJoin,
            false,
            FfiRecoveryAction::None,
            "transfer.malformed_join",
        ),
        RendezvousCause::UnsupportedVersion => (
            FfiFailureCode::UnsupportedRendezvousVersion,
            false,
            FfiRecoveryAction::None,
            "transfer.unsupported_rendezvous_version",
        ),
    };
    (
        code,
        if matches!(
            cause,
            RendezvousCause::MalformedJoin | RendezvousCause::UnsupportedVersion
        ) {
            FfiFailureCategory::Unsupported
        } else {
            FfiFailureCategory::Network
        },
        FfiFailurePhase::Pairing,
        FfiFailureOrigin::Unknown,
        retryable,
        recovery,
        key,
    )
}

fn io_failure_recovery(direction: FfiTransferDirection) -> FfiRecoveryAction {
    if direction == FfiTransferDirection::Receive {
        FfiRecoveryAction::Resume
    } else {
        FfiRecoveryAction::Retry
    }
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

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Mutex as StdMutex;

    use envoix_error::TransferCause;

    use super::*;

    #[derive(Default)]
    struct RecordingObserver {
        failure: StdMutex<Option<FfiTransferFailure>>,
        path_events: StdMutex<Vec<FfiConnectionPathEvent>>,
        diagnostics: StdMutex<Vec<String>>,
    }

    impl TransferObserver for RecordingObserver {
        fn on_invite_ready(&self, _invite: String) {}

        fn on_started(&self, _item_count: u32, _total_bytes: u64) {}

        fn on_phase(&self, _phase: FfiManifestV2Phase) {}

        fn on_progress(&self, _transferred: u64, _total: u64) {}

        fn on_completed(&self, _bytes: u64) {}

        fn on_transfer_failed(&self, failure: FfiTransferFailure) {
            *self.failure.lock().expect("failure mutex") = Some(failure);
        }

        fn on_connection_path(&self, event: FfiConnectionPathEvent) {
            self.path_events.lock().unwrap().push(event);
        }

        fn on_diagnostic(&self, message: String) {
            self.diagnostics.lock().unwrap().push(message);
        }

        fn on_remembered_credential(&self, _opaque_credential: Vec<u8>, _generation: u64) -> bool {
            false
        }
    }

    #[test]
    fn room_joining_reports_native_receiver_readiness() {
        assert_eq!(
            pairing_phase(PairingStep::Joining),
            Some(FfiManifestV2Phase::WaitingForPeer)
        );
        assert_eq!(
            pairing_phase(PairingStep::Matched),
            Some(FfiManifestV2Phase::Pairing)
        );
        assert_eq!(
            pairing_phase(PairingStep::Exchanged),
            Some(FfiManifestV2Phase::Pairing)
        );
    }

    #[test]
    fn generic_io_recovery_matches_platform_resume_semantics() {
        assert_eq!(
            io_failure_recovery(FfiTransferDirection::Receive),
            FfiRecoveryAction::Resume
        );
        assert_eq!(
            io_failure_recovery(FfiTransferDirection::Send),
            FfiRecoveryAction::Retry
        );
    }

    #[test]
    fn rendezvous_causes_project_without_parsing_diagnostics() {
        let rate_limited = rendezvous_cause_projection(RendezvousCause::RoomRateLimited);
        assert_eq!(rate_limited.0, FfiFailureCode::RoomRateLimited);
        assert!(rate_limited.4);
        assert_eq!(rate_limited.5, FfiRecoveryAction::Retry);

        let exhausted = rendezvous_cause_projection(RendezvousCause::RoomUnderAttack);
        assert_eq!(exhausted.0, FfiFailureCode::RoomUnderAttack);
        assert_eq!(exhausted.5, FfiRecoveryAction::RePair);
    }

    #[test]
    fn consumed_invitation_failure_requires_repair() {
        let observer = RecordingObserver::default();
        report_v2_failure(
            &observer,
            &SessionError::InvitationConsumed(Box::new(SessionError::Transport(
                "connection lost".into(),
            ))),
            FfiTransferDirection::Send,
            FfiFailurePhase::Transferring,
        );
        let failure = observer
            .failure
            .lock()
            .expect("failure mutex")
            .clone()
            .expect("reported failure");

        assert_eq!(failure.code, FfiFailureCode::NetworkLost);
        assert_eq!(failure.recovery_action, FfiRecoveryAction::RePair);
        assert!(
            failure
                .diagnostic_message
                .contains("one-time invitation was consumed after authentication")
        );
    }

    #[test]
    fn consumed_invitation_preserves_receiver_save_failure() {
        let observer = RecordingObserver::default();
        report_v2_failure(
            &observer,
            &SessionError::InvitationConsumed(Box::new(SessionError::Cause {
                cause: TransferCause::ReceiverSaveFailed,
                detail: "destination contended".into(),
            })),
            FfiTransferDirection::Send,
            FfiFailurePhase::Transferring,
        );
        let failure = observer
            .failure
            .lock()
            .expect("failure mutex")
            .clone()
            .expect("reported failure");

        assert_eq!(failure.code, FfiFailureCode::ReceiverSaveFailed);
        assert_eq!(failure.category, FfiFailureCategory::Storage);
        assert_eq!(failure.phase, FfiFailurePhase::Committing);
        assert_eq!(failure.recovery_action, FfiRecoveryAction::RePair);
    }

    #[test]
    fn consumed_invitation_does_not_make_integrity_failure_retryable() {
        let observer = RecordingObserver::default();
        report_v2_failure(
            &observer,
            &SessionError::InvitationConsumed(Box::new(SessionError::Protocol(
                "bad digest".into(),
            ))),
            FfiTransferDirection::Send,
            FfiFailurePhase::Verifying,
        );
        let failure = observer
            .failure
            .lock()
            .expect("failure mutex")
            .clone()
            .expect("reported failure");

        assert_eq!(failure.code, FfiFailureCode::ProtocolOrIntegrityFailure);
        assert!(!failure.retryable);
        assert_eq!(failure.recovery_action, FfiRecoveryAction::None);
    }

    #[test]
    fn authentication_milestone_precedes_credential_persistence() {
        let authentication =
            NativeAuthentication::rotation(Arc::new(RecordingObserver::default()), vec![1], 1);

        let result = authentication.on_authenticated(AuthenticationOutcome {
            remember_secret: None,
        });

        assert!(matches!(&result, Err(SessionError::Storage(_))));
        assert!(authentication.authenticated());
        assert!(!authentication.persisted());
        assert!(should_stop_remembered_fallback(
            &result,
            &authentication,
            &TransferCancelToken::new(),
        ));
    }

    #[test]
    fn connection_path_projection_classifies_without_endpoint_details() {
        let direct = DataPath::Direct {
            addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 42)), 4242),
        };
        let relay = DataPath::Relay {
            url: "https://sensitive-relay.example".into(),
        };
        let other = DataPath::Other {
            description: "sensitive transport details".into(),
        };

        let projections = [
            project_connection_path(&direct, FfiConnectionPathEventKind::Selected),
            project_connection_path(&relay, FfiConnectionPathEventKind::Changed),
            project_connection_path(&other, FfiConnectionPathEventKind::Changed),
        ];

        assert_eq!(projections[0].path_kind, FfiDataPathKind::Direct);
        assert_eq!(
            projections[0].event_kind,
            FfiConnectionPathEventKind::Selected
        );
        assert_eq!(projections[1].path_kind, FfiDataPathKind::Relay);
        assert_eq!(projections[2].path_kind, FfiDataPathKind::Other);
        let rendered = format!("{projections:?}");
        assert!(!rendered.contains("198.51.100.42"));
        assert!(!rendered.contains("sensitive-relay.example"));
        assert!(!rendered.contains("sensitive transport details"));
    }

    #[test]
    fn native_events_forward_selected_and_changed_paths() {
        let observer = Arc::new(RecordingObserver::default());
        let sink = NativeSessionEvents {
            observer: observer.clone(),
        };

        sink.on_event(TransferEvent::Connected {
            path: DataPath::Relay {
                url: "https://relay.example".into(),
            },
        });
        sink.on_event(TransferEvent::PathChanged {
            path: DataPath::Direct {
                addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 443),
            },
        });

        assert_eq!(
            *observer.path_events.lock().unwrap(),
            vec![
                FfiConnectionPathEvent {
                    path_kind: FfiDataPathKind::Relay,
                    event_kind: FfiConnectionPathEventKind::Selected,
                },
                FfiConnectionPathEvent {
                    path_kind: FfiDataPathKind::Direct,
                    event_kind: FfiConnectionPathEventKind::Changed,
                },
            ]
        );
        let diagnostics = observer.diagnostics.lock().unwrap().join("\n");
        assert!(diagnostics.contains("Relay"));
        assert!(diagnostics.contains("Direct"));
        assert!(!diagnostics.contains("relay.example"));
        assert!(!diagnostics.contains("127.0.0.1"));
    }
}
