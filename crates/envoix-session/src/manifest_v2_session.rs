//! Authenticated Manifest v2 session orchestration.

use std::path::PathBuf;
use std::sync::Arc;

use envoix_error::{CoreError, TransferCause};
use envoix_protocol::manifest_v2::{ContentDigestV2, ManifestOfferV2, build_manifest_offer_v2};
use envoix_protocol::manifest_v2_frames::{
    ManifestV2Frame, ManifestV2FrameConnection, ResumeRequestV2,
    canonical_manifest_v2_frame_body_digest,
};
use envoix_transfer::{
    CanonicalTransferJob, DestinationPlanStoreV2, DestinationRequestV2, DestinationWritePlanV2,
    LocalDestinationProviderV2, ManifestV2DataPlane, ManifestV2DeliveryAuthority,
    ManifestV2ProgressPhase, ManifestV2ProgressSink, ManifestV2ResultGate,
    NoopManifestV2ResultGate, ReceiverDataPlaneLedgerV2, ReceiverDataPlaneStoreV2,
    ReceiverDataPlaneSummaryV2, ReceiverDeliveryRecordV2, ReceiverDeliveryStoreV2,
    SenderDataPlaneSummaryV2, SenderDeliveryRecordV2, SenderDeliveryStoreV2, SenderResumeIntentV2,
    SenderTransferPhaseV2, TransferJobError, sender_resume_intent,
};
use envoix_types::{TransferDirection, TransferId};

use crate::connection::IrohFrameConnection;
use crate::{
    BoundEndpoint, EventSink, PairingConfig, PeerDescriptor, SessionConfig, SessionError,
    TransferCancelToken, TransferProtocol, auth_bounded, authenticate_receiver,
    authenticate_sender, dial_peer_addr_for_protocol, interrupted_error, peer_addr_from_descriptor,
};

struct SessionManifestV2Progress {
    events: Arc<dyn EventSink>,
    transfer_id: TransferId,
    direction: TransferDirection,
}

impl SessionManifestV2Progress {
    fn new(
        events: Arc<dyn EventSink>,
        identity: envoix_protocol::manifest_v2_frames::JobGenerationV2,
        direction: TransferDirection,
    ) -> Self {
        Self {
            events,
            transfer_id: TransferId(format!(
                "{}-{}",
                identity
                    .job_id
                    .0
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
                identity.generation
            )),
            direction,
        }
    }
}

impl ManifestV2ProgressSink for SessionManifestV2Progress {
    fn on_progress(&self, completed_plaintext_bytes: u64, total_plaintext_bytes: u64) {
        self.events
            .on_event(envoix_transfer::TransferEvent::Progress {
                transfer_id: self.transfer_id.clone(),
                bytes_transferred: completed_plaintext_bytes,
                total_bytes: total_plaintext_bytes,
            });
    }

    fn on_phase(&self, phase: ManifestV2ProgressPhase) {
        self.events
            .on_event(envoix_transfer::TransferEvent::ManifestV2Phase {
                transfer_id: self.transfer_id.clone(),
                direction: self.direction,
                phase,
            });
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SenderManifestV2SessionSummary {
    pub data_plane: SenderDataPlaneSummaryV2,
    pub delivery_proof_digest: ContentDigestV2,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiverManifestV2SessionSummary {
    pub data_plane: ReceiverDataPlaneSummaryV2,
    pub delivery_proof_digest: ContentDigestV2,
    pub destination_plan: DestinationWritePlanV2,
}

/// An authenticated offer whose metadata is available for bounded receiver UI
/// inspection before any Accept or payload effect.
pub struct PendingManifestV2Receive {
    bound_endpoint: BoundEndpoint,
    connection: IrohFrameConnection,
    offer: ManifestOfferV2,
    resume_request: Option<ResumeRequestV2>,
    events: Arc<dyn EventSink>,
}

impl std::fmt::Debug for PendingManifestV2Receive {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingManifestV2Receive")
            .field("offer", &self.offer)
            .finish_non_exhaustive()
    }
}

impl PendingManifestV2Receive {
    pub fn offer(&self) -> &ManifestOfferV2 {
        &self.offer
    }

    /// Persists destination and capability state before Accept and does not
    /// return until the final destination save has produced a delivery proof.
    pub async fn receive(
        self,
        destination: DestinationRequestV2,
        state_directory: PathBuf,
        cancel: &TransferCancelToken,
    ) -> Result<ReceiverManifestV2SessionSummary, SessionError> {
        self.receive_with_result_gate(
            destination,
            state_directory,
            &NoopManifestV2ResultGate,
            cancel,
        )
        .await
    }

    /// Variant for destinations whose final save is owned by the platform.
    /// The result gate must complete that save before this session can report
    /// receiver results or produce delivery proof.
    pub async fn receive_with_result_gate(
        mut self,
        destination: DestinationRequestV2,
        state_directory: PathBuf,
        result_gate: &dyn ManifestV2ResultGate,
        cancel: &TransferCancelToken,
    ) -> Result<ReceiverManifestV2SessionSummary, SessionError> {
        let result = tokio::select! {
            result = receive_after_offer(
                &self.offer,
                self.resume_request.as_ref(),
                destination,
                state_directory,
                &mut self.connection,
                self.events.clone(),
                result_gate,
            ) => result,
            () = cancel.cancelled() => Err(interrupted_error(cancel)),
        };
        match &result {
            Ok(_) => self.connection.await_peer_close().await,
            Err(_) => {
                let _ = ManifestV2FrameConnection::close(&mut self.connection).await;
            }
        }
        self.bound_endpoint.local_endpoint.close().await;
        result
    }
}

pub async fn send_manifest_v2_manual(
    peer: PeerDescriptor,
    job: &CanonicalTransferJob,
    state_directory: PathBuf,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Arc<dyn EventSink>,
    cancel: &TransferCancelToken,
) -> Result<SenderManifestV2SessionSummary, SessionError> {
    let peer_addr = peer_addr_from_descriptor(&peer)?;
    send_manifest_v2_to_endpoint_addr(
        peer_addr,
        job,
        state_directory,
        config,
        pairing,
        events,
        cancel,
    )
    .await
}

pub async fn send_manifest_v2_to_endpoint_addr(
    peer_addr: iroh::EndpointAddr,
    job: &CanonicalTransferJob,
    state_directory: PathBuf,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Arc<dyn EventSink>,
    cancel: &TransferCancelToken,
) -> Result<SenderManifestV2SessionSummary, SessionError> {
    let manifest = job.manifest().ok_or_else(|| {
        CoreError::InvalidInput("transfer job must be sealed before dialing".into())
    })?;
    let offer = build_manifest_offer_v2(manifest.clone())
        .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    let store = SenderDeliveryStoreV2::new(state_directory.join("sender-delivery"));
    let identity = envoix_protocol::manifest_v2_frames::JobGenerationV2 {
        job_id: manifest.job_id,
        generation: manifest.generation,
    };
    let mut record = match store.load(identity).await.map_err(session_delivery_error)? {
        Some(record) => record,
        None => SenderDeliveryRecordV2::new(&offer),
    };
    record
        .validate_offer(&offer)
        .map_err(session_delivery_error)?;
    store.save(&record).await.map_err(session_delivery_error)?;
    events.on_event(envoix_transfer::TransferEvent::Connecting);
    let local_endpoint = super::build_dial_endpoint(
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await?;
    let mut connection = match dial_peer_addr_for_protocol(
        local_endpoint.clone(),
        peer_addr,
        TransferProtocol::ManifestV2,
    )
    .await
    {
        Ok(connection) => connection,
        Err(error) => {
            local_endpoint.close().await;
            return Err(error);
        }
    };
    connection.watch_path(events.clone());
    if let Err(error) = auth_bounded(authenticate_sender(&mut connection, pairing), cancel).await {
        let _ = ManifestV2FrameConnection::close(&mut connection).await;
        local_endpoint.close().await;
        return Err(error);
    }
    let result = tokio::select! {
        result = async {
            let data_plane = match record.phase() {
                SenderTransferPhaseV2::Offering | SenderTransferPhaseV2::Transferring => {
                    let progress = SessionManifestV2Progress::new(
                        events.clone(),
                        identity,
                        TransferDirection::Send,
                    );
                    ManifestV2DataPlane::send(
                        job,
                        &mut record,
                        &store,
                        &mut connection,
                        &progress,
                    )
                        .await
                        .map_err(session_sender_data_error)?
                }
                SenderTransferPhaseV2::WaitingForReceiverSave
                | SenderTransferPhaseV2::Delivered => {
                    ManifestV2DataPlane::establish_sender_reconnect(
                        job,
                        &record,
                        &mut connection,
                    )
                    .await
                    .map_err(session_sender_data_error)?;
                    record.completed_data_summary().ok_or_else(|| {
                        CoreError::Protocol(
                            "sender delivery phase has no durable result summary".into(),
                        )
                    })?
                }
                SenderTransferPhaseV2::Failed | SenderTransferPhaseV2::Canceled => {
                    return Err(CoreError::Transfer(
                        "terminal failed or canceled job cannot reconnect".into(),
                    ));
                }
            };
            events.on_event(envoix_transfer::TransferEvent::Diagnostic {
                message: "waiting for receiver to save files".into(),
            });
            let proof = ManifestV2DeliveryAuthority::sender_confirm_delivery(
                &mut record,
                &store,
                &mut connection,
            )
            .await
            .map_err(session_delivery_error)?;
            let delivery_proof_digest = canonical_manifest_v2_frame_body_digest(
                &ManifestV2Frame::DeliveryProof(proof),
            )
            .map_err(|error| CoreError::Protocol(error.to_string()))?;
            Ok(SenderManifestV2SessionSummary {
                data_plane,
                delivery_proof_digest,
            })
        } => result,
        () = cancel.cancelled() => Err(interrupted_error(cancel)),
    };
    let _ = ManifestV2FrameConnection::close(&mut connection).await;
    local_endpoint.close().await;
    result
}

pub async fn receive_manifest_v2_offer(
    bound_endpoint: BoundEndpoint,
    pairing: &PairingConfig,
    events: Arc<dyn EventSink>,
    cancel: &TransferCancelToken,
) -> Result<PendingManifestV2Receive, SessionError> {
    let mut authentication_failures = 0_u32;
    let mut connection = loop {
        let mut connection = tokio::select! {
            result = bound_endpoint.accept_with_events(events.as_ref()) => result?,
            () = cancel.cancelled() => return Err(interrupted_error(cancel)),
        };
        if connection.protocol() != TransferProtocol::ManifestV2 {
            let _ = ManifestV2FrameConnection::close(&mut connection).await;
            return Err(CoreError::Protocol(
                "canonical receive endpoint negotiated a non-Manifest-v2 protocol".into(),
            ));
        }
        connection.watch_path(events.clone());
        match auth_bounded(authenticate_receiver(&mut connection, pairing), cancel).await {
            Ok(()) => break connection,
            Err(error) if cancel.is_cancelled() => {
                let _ = ManifestV2FrameConnection::close(&mut connection).await;
                return Err(error);
            }
            Err(error) => {
                let _ = ManifestV2FrameConnection::close(&mut connection).await;
                authentication_failures += 1;
                events.on_event(envoix_transfer::TransferEvent::Diagnostic {
                    message: format!(
                        "rejected an unauthenticated peer ({authentication_failures}/{})",
                        super::MAX_AUTH_FAILURES
                    ),
                });
                if authentication_failures >= super::MAX_AUTH_FAILURES {
                    return Err(CoreError::Crypto("too many failed pairing attempts".into()));
                }
            }
        }
    };
    let (offer, resume_request) = match tokio::select! {
        result = connection.recv_manifest_v2_frame() => result.map_err(|error| CoreError::Protocol(error.to_string()))?,
        () = cancel.cancelled() => return Err(interrupted_error(cancel)),
    } {
        ManifestV2Frame::Offer(offer) => (offer, None),
        ManifestV2Frame::ResumeRequest(request) => (request.offer.clone(), Some(request)),
        _ => {
            return Err(CoreError::Protocol(
                "expected Manifest v2 Offer or ResumeRequest after authentication".into(),
            ));
        }
    };
    offer
        .manifest
        .validate()
        .map_err(|error| CoreError::Protocol(error.to_string()))?;
    let canonical_offer = build_manifest_offer_v2(offer.manifest.clone())
        .map_err(|error| CoreError::Protocol(error.to_string()))?;
    if canonical_offer.structural_digest != offer.structural_digest {
        return Err(CoreError::Protocol(
            "Manifest v2 structural digest does not match its canonical manifest".into(),
        ));
    }
    Ok(PendingManifestV2Receive {
        bound_endpoint,
        connection,
        offer,
        resume_request,
        events,
    })
}

async fn receive_after_offer(
    offer: &ManifestOfferV2,
    resume_request: Option<&ResumeRequestV2>,
    destination: DestinationRequestV2,
    state_directory: PathBuf,
    connection: &mut IrohFrameConnection,
    events: Arc<dyn EventSink>,
    result_gate: &dyn ManifestV2ResultGate,
) -> Result<ReceiverManifestV2SessionSummary, SessionError> {
    let identity = envoix_protocol::manifest_v2_frames::JobGenerationV2 {
        job_id: offer.manifest.job_id,
        generation: offer.manifest.generation,
    };
    let initial_request = resume_request.is_none();
    if resume_request.is_some_and(|request| request.identity != identity) {
        return Err(CoreError::Protocol(
            "ResumeRequest identity does not match its Offer".into(),
        ));
    }
    let plan_store = DestinationPlanStoreV2::new(state_directory.join("destination-plans"));
    let plan = match plan_store
        .load(offer.manifest.job_id, offer.manifest.generation)
        .await
        .map_err(session_destination_error)?
    {
        Some(plan) => {
            plan.validate_resume_request(offer, &destination)
                .await
                .map_err(session_destination_error)?;
            plan
        }
        None if initial_request => {
            let plan = DestinationWritePlanV2::create(offer, destination)
                .await
                .map_err(session_destination_error)?;
            plan_store
                .save(&plan)
                .await
                .map_err(session_destination_error)?;
            plan
        }
        None => {
            return Err(CoreError::Cause {
                cause: TransferCause::ReceiverDestinationUnavailable,
                detail: "receiver destination plan needed for resume is missing".into(),
            });
        }
    };
    let data_store = ReceiverDataPlaneStoreV2::new(state_directory.join("receiver-data"));
    let mut ledger = match data_store
        .load(identity)
        .await
        .map_err(session_receiver_data_error)?
    {
        Some(ledger) => {
            ledger
                .validate(&offer.manifest)
                .map_err(session_receiver_data_error)?;
            ledger
        }
        None if initial_request => {
            let accept = plan
                .create_initial_accept(offer)
                .map_err(session_destination_error)?;
            let ledger = ReceiverDataPlaneLedgerV2::new(offer, accept)
                .map_err(session_receiver_data_error)?;
            data_store
                .save(&ledger)
                .await
                .map_err(session_receiver_data_error)?;
            ledger
        }
        None => {
            return Err(CoreError::Cause {
                cause: TransferCause::ReceiverDestinationUnavailable,
                detail: "receiver data ledger needed for resume is missing".into(),
            });
        }
    };
    let accept = ledger.accept().clone();
    let mut resume_intent = None;
    let mut resumed_destination_provider = None;
    if let Some(resume_request) = resume_request {
        ledger
            .validate_resume_request(resume_request)
            .map_err(session_receiver_data_error)?;
        resume_intent = Some(
            sender_resume_intent(
                offer.structural_digest,
                ledger.accept_body_digest(),
                resume_request,
            )
            .map_err(session_receiver_data_error)?,
        );
        if resume_intent == Some(SenderResumeIntentV2::ContinueData) {
            let mut provider =
                LocalDestinationProviderV2::new(plan.clone(), offer.manifest.clone())
                    .await
                    .map_err(session_destination_error)?;
            provider
                .reconcile_resume(&mut ledger, &data_store)
                .await
                .map_err(session_receiver_data_error)?;
            resumed_destination_provider = Some(provider);
        }
        let mut status = ledger.resume_status();
        status.challenge_nonce = resume_request.challenge_nonce;
        status.challenge_mac = ManifestV2DeliveryAuthority::answer_resume_challenge(
            identity,
            resume_request.challenge_nonce,
            accept.proof_capability,
        );
        connection
            .send_manifest_v2_frame(ManifestV2Frame::ResumeStatus(status))
            .await
            .map_err(|error| CoreError::Protocol(error.to_string()))?;
    } else {
        if ledger.requires_authenticated_resume() {
            return Err(CoreError::Crypto(
                "started Manifest v2 job requires an authenticated ResumeRequest".into(),
            ));
        }
        connection
            .send_manifest_v2_frame(ManifestV2Frame::Accept(accept.clone()))
            .await
            .map_err(|error| CoreError::Protocol(error.to_string()))?;
    }
    let delivery_store = ReceiverDeliveryStoreV2::new(state_directory.join("receiver-delivery"));
    let existing_delivery_record = delivery_store
        .load(identity)
        .await
        .map_err(session_delivery_error)?;
    if resume_intent == Some(SenderResumeIntentV2::AwaitDelivery) {
        let data_plane = ledger.completed_summary().ok_or_else(|| {
            CoreError::Protocol("delivery record exists without a complete data ledger".into())
        })?;
        let mut delivery_record = match existing_delivery_record.clone() {
            Some(record) => record,
            None => ReceiverDeliveryRecordV2::new(offer, &accept, &data_plane)
                .map_err(session_delivery_error)?,
        };
        let proof = ManifestV2DeliveryAuthority::receiver_send_proof(
            &mut delivery_record,
            &delivery_store,
            connection,
        )
        .await
        .map_err(session_delivery_error)?;
        let delivery_proof_digest =
            canonical_manifest_v2_frame_body_digest(&ManifestV2Frame::DeliveryProof(proof))
                .map_err(|error| CoreError::Protocol(error.to_string()))?;
        return Ok(ReceiverManifestV2SessionSummary {
            data_plane,
            delivery_proof_digest,
            destination_plan: plan,
        });
    }
    let mut destination_provider = match resumed_destination_provider {
        Some(provider) => provider,
        None => LocalDestinationProviderV2::new(plan.clone(), offer.manifest.clone())
            .await
            .map_err(session_destination_error)?,
    };
    let progress = SessionManifestV2Progress::new(events, identity, TransferDirection::Receive);
    let data_plane = ManifestV2DataPlane::receive(
        offer,
        &mut ledger,
        &data_store,
        &mut destination_provider,
        connection,
        &progress,
        result_gate,
    )
    .await
    .map_err(session_receiver_data_error)?;
    plan_store
        .save(destination_provider.plan())
        .await
        .map_err(session_destination_error)?;
    let mut delivery_record = match existing_delivery_record {
        Some(record) => record,
        None => ReceiverDeliveryRecordV2::new(offer, &accept, &data_plane)
            .map_err(session_delivery_error)?,
    };
    let proof = ManifestV2DeliveryAuthority::receiver_send_proof(
        &mut delivery_record,
        &delivery_store,
        connection,
    )
    .await
    .map_err(session_delivery_error)?;
    let delivery_proof_digest =
        canonical_manifest_v2_frame_body_digest(&ManifestV2Frame::DeliveryProof(proof))
            .map_err(|error| CoreError::Protocol(error.to_string()))?;
    Ok(ReceiverManifestV2SessionSummary {
        data_plane,
        delivery_proof_digest,
        destination_plan: destination_provider.plan().clone(),
    })
}

fn session_sender_data_error(error: envoix_transfer::ManifestV2DataError) -> SessionError {
    use envoix_transfer::ManifestV2DataError as Error;
    match error {
        Error::Core(error) => error,
        Error::Job(error) => session_sender_job_error(error),
        Error::Io(error) => session_sender_io_error(error),
        Error::Delivery(error) => session_delivery_error(error),
        Error::Destination(error) => session_destination_error(error),
        Error::Internal(detail) => CoreError::Transfer(detail),
        other => transfer_cause(TransferCause::ProtocolOrIntegrityFailure, other),
    }
}

fn session_receiver_data_error(error: envoix_transfer::ManifestV2DataError) -> SessionError {
    use envoix_transfer::ManifestV2DataError as Error;
    match error {
        Error::Core(error) => error,
        Error::Io(error) => session_receiver_io_error(error),
        Error::Destination(error) => session_destination_error(error),
        Error::DestinationContract(detail) => CoreError::Cause {
            cause: TransferCause::ReceiverSaveFailed,
            detail,
        },
        Error::Delivery(error) => session_delivery_error(error),
        Error::Job(error) => session_sender_job_error(error),
        Error::Internal(detail) => CoreError::Transfer(detail),
        other => transfer_cause(TransferCause::ProtocolOrIntegrityFailure, other),
    }
}

fn session_sender_job_error(error: TransferJobError) -> SessionError {
    match error {
        TransferJobError::SourceChanged => {
            transfer_cause(TransferCause::SenderSourceChanged, error)
        }
        TransferJobError::Canceled => transfer_cause(TransferCause::SenderCanceled, error),
        TransferJobError::Io(error) => session_sender_io_error(error),
        other => transfer_cause(TransferCause::SenderSourceUnavailable, other),
    }
}

fn session_sender_io_error(error: std::io::Error) -> SessionError {
    let cause = match error.kind() {
        std::io::ErrorKind::PermissionDenied => TransferCause::SenderPermissionLost,
        std::io::ErrorKind::NotFound => TransferCause::SenderSourceUnavailable,
        _ => return CoreError::Io(error.to_string()),
    };
    transfer_cause(cause, error)
}

fn session_receiver_io_error(error: std::io::Error) -> SessionError {
    let cause = match error.kind() {
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound => {
            TransferCause::ReceiverDestinationUnavailable
        }
        _ => TransferCause::ReceiverSaveFailed,
    };
    transfer_cause(cause, error)
}

fn session_delivery_error(error: envoix_transfer::DeliveryAuthorityErrorV2) -> SessionError {
    use envoix_transfer::DeliveryAuthorityErrorV2 as Error;
    match error {
        Error::Io(error) => CoreError::Storage(error.to_string()),
        Error::Transport(detail) => CoreError::Transport(detail),
        other => transfer_cause(TransferCause::ProtocolOrIntegrityFailure, other),
    }
}

fn session_destination_error(error: envoix_transfer::DestinationPlanErrorV2) -> SessionError {
    use envoix_transfer::DestinationPlanErrorV2 as Error;
    let cause = match &error {
        Error::CopyDecisionRequired
        | Error::MissingCopyStaging
        | Error::ExceptionalTransferApprovalRequired => {
            TransferCause::ReceiverDestinationDecisionRequired
        }
        Error::InsufficientSpace { .. } => TransferCause::ReceiverSpaceInsufficient,
        Error::ReusedObjectLost => TransferCause::ReceiverReusedObjectLost,
        Error::UnsupportedProvider | Error::UnknownCapacity => {
            TransferCause::ReceiverDestinationUnavailable
        }
        Error::Io(io_error)
            if matches!(
                io_error.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
            ) =>
        {
            TransferCause::ReceiverDestinationUnavailable
        }
        Error::InvalidEntryState => TransferCause::ProtocolOrIntegrityFailure,
        Error::SpaceOverflow
        | Error::NameExhausted
        | Error::ReservationLost
        | Error::LateCollision
        | Error::DestinationContended
        | Error::Io(_) => TransferCause::ReceiverSaveFailed,
    };
    transfer_cause(cause, error)
}

fn transfer_cause(cause: TransferCause, error: impl std::fmt::Display) -> SessionError {
    CoreError::Cause {
        cause,
        detail: error.to_string(),
    }
}
