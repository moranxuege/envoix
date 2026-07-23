use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

mod args;

use args::{Cli, Command, SaveModeArg, SourceIssueActionArg, TransferPlan};
use clap::Parser;
use envoix_client::api::{
    self, CanonicalTransferJob, DestinationDecisionV2, DestinationRequestV2, EventSink,
    PairingConfig, PeerSource, PendingManifestV2Receive, SourceDecision, SourceSelectionState,
    TransferEvent, TransferJobStore,
};
use envoix_client::{IdentityConfig, SPAKE2_EXPERIMENTAL_WARNING, TransferCancelToken};
use envoix_qr::{QrInvitePayload, generate_token, render_terminal_qr};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing(verbosity: u8) {
    use tracing_subscriber::{EnvFilter, fmt};
    let default_filter = match verbosity {
        0 => "envoix=info,warn",
        1 => "envoix=debug,warn",
        _ => "envoix=trace,iroh=debug,warn",
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .with_target(false)
        .init();
}

async fn run(cli: Cli) -> CliResult<()> {
    let json = cli.json;
    match cli.command {
        Command::Send(args) => send(args.into_plan()?, json).await,
        Command::Receive(args) => receive(args.into_plan()?, json).await,
    }
}

async fn send(plan: TransferPlan, json: bool) -> CliResult<()> {
    if let Some(note) = &plan.note {
        eprintln!("{note}");
    }
    let client = api_client(plan.config.as_deref(), plan.identity.clone())?;
    let config = client.session_config(&plan.options);
    let state_directory = sender_state_directory()?;
    let store = TransferJobStore::new(state_directory.join("jobs"));
    let mut job = CanonicalTransferJob::new(plan.compression)?;
    let mut sources = vec![plan.path];
    sources.extend(plan.additional_paths);
    for source in sources {
        job.add_local_path(source).await?;
        store.save(&job).await?;
    }
    job.prepare_all().await?;
    store.save(&job).await?;
    if job.lifecycle() == api::JobLifecycle::NeedsSourceDecision {
        let affected_roots = job
            .source_selections()
            .into_iter()
            .filter(|selection| selection.state == SourceSelectionState::NeedsDecision)
            .map(|selection| selection.root_item_id)
            .collect::<Vec<_>>();
        match plan.source_issue_action {
            SourceIssueActionArg::Fail => {}
            SourceIssueActionArg::ApprovePartial => {
                for root in affected_roots {
                    store
                        .apply_source_decision(&mut job, root, SourceDecision::ApprovePartial)
                        .await?;
                }
            }
            SourceIssueActionArg::RemoveRoot => {
                for root in affected_roots {
                    store
                        .apply_source_decision(&mut job, root, SourceDecision::RemoveSelection)
                        .await?;
                }
            }
        }
    }
    if job.lifecycle() != api::JobLifecycle::ReadyToSend {
        return Err(format!(
            "source preparation needs a user decision before Send; reauthorize the source or choose --source-issue-action approve-partial|remove-root: {:?}",
            job.source_selections()
        )
        .into());
    }
    job.seal_for_send()?;
    store.save(&job).await?;

    let manifest = job.manifest().expect("sealed job has manifest");
    eprintln!(
        "sending {} files and {} directories ({} bytes)",
        manifest.totals.file_count,
        manifest.totals.directory_count,
        manifest.totals.total_plaintext_bytes
    );
    let events: Arc<dyn EventSink> = Arc::new(CliEvents { json });
    let cancel = TransferCancelToken::new();
    let operation = send_job(
        &plan.source,
        &job,
        state_directory,
        config,
        events,
        &cancel,
        plan.options.relay.as_deref(),
    );
    tokio::pin!(operation);
    let summary = tokio::select! {
        result = &mut operation => result?,
        signal = tokio::signal::ctrl_c() => {
            signal?;
            cancel.cancel();
            operation.await?
        }
    };
    eprintln!(
        "receiver saved and confirmed {} entries ({} bytes)",
        summary.data_plane.entry_results.len(),
        manifest.totals.total_plaintext_bytes
    );
    Ok(())
}

async fn receive(plan: TransferPlan, json: bool) -> CliResult<()> {
    if let Some(note) = &plan.note {
        eprintln!("{note}");
    }
    tokio::fs::create_dir_all(&plan.path).await?;
    let target_directory = tokio::fs::canonicalize(&plan.path).await?;
    let available = api::local_allocatable_bytes(&target_directory)?;
    let state_directory = target_directory.join(".envoix-state-v2");
    let client = api_client(plan.config.as_deref(), plan.identity.clone())?;
    let listen_addrs = plan
        .options
        .listen_addrs
        .clone()
        .unwrap_or_else(|| envoix_client::BindAddrs::dual_stack(0));
    let config = client.session_config(&plan.options);
    let events: Arc<dyn EventSink> = Arc::new(CliEvents { json });
    let cancel = TransferCancelToken::new();
    let operation = receive_offer(
        &plan.source,
        listen_addrs,
        config,
        events,
        &cancel,
        plan.options.relay.as_deref(),
    );
    tokio::pin!(operation);
    let pending = tokio::select! {
        result = &mut operation => result?,
        signal = tokio::signal::ctrl_c() => {
            signal?;
            cancel.cancel();
            operation.await?
        }
    };
    print_offer(&pending);
    let total = pending.offer().manifest.totals.total_plaintext_bytes;
    let exceptional = total > api::AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES || total > available / 2;
    if exceptional && !plan.approve_large_transfer {
        return Err(format!(
            "offer requires explicit approval before payload ({} bytes offered, {} bytes allocatable); rerun with --approve-large-transfer",
            total, available
        )
        .into());
    }
    let (decision, copy_staging_directory, staging_available) = match plan.save_mode {
        SaveModeArg::Direct => (DestinationDecisionV2::UseDirectSave, None, None),
        SaveModeArg::CopyAfterVerify => {
            let staging = target_directory.join(".envoix-copy-staging-v2");
            tokio::fs::create_dir_all(&staging).await?;
            (
                DestinationDecisionV2::ContinueWithCopyAfterVerify,
                Some(staging),
                Some(available),
            )
        }
    };
    let summary = pending
        .receive(
            DestinationRequestV2 {
                target_directory,
                copy_staging_directory,
                decision,
                target_allocatable_bytes: Some(available),
                staging_allocatable_bytes: staging_available,
                stable_object_identity: true,
                exceptional_transfer_approved: plan.approve_large_transfer,
            },
            state_directory,
            &cancel,
        )
        .await?;
    eprintln!(
        "saved and confirmed {} entries ({} bytes)",
        summary.data_plane.entry_results.len(),
        total
    );
    Ok(())
}

async fn send_job(
    source: &PeerSource,
    job: &CanonicalTransferJob,
    state_directory: PathBuf,
    config: api::SessionConfig,
    events: Arc<dyn EventSink>,
    cancel: &TransferCancelToken,
    relay: Option<&str>,
) -> Result<api::SenderManifestV2SessionSummary, api::SessionError> {
    match source {
        PeerSource::Manual { peer, token } => {
            let pairing = PairingConfig::spake2_shared_token(token.clone())?;
            api::send_manifest_v2_manual(
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
            let payload = QrInvitePayload::decode(invite)
                .map_err(|error| api::SessionError::InvalidInput(error.to_string()))?;
            payload
                .validate(now_unix_seconds())
                .map_err(|error| api::SessionError::InvalidInput(error.to_string()))?;
            let pairing = PairingConfig::spake2_shared_token(payload.token.clone())?;
            let endpoint = payload
                .endpoint_addr()
                .map_err(|error| api::SessionError::InvalidInput(error.to_string()))?;
            api::send_manifest_v2_to_endpoint_addr(
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
            api::send_manifest_v2_enable_mdns(
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
            let broker = api::parse_broker_addr(broker, relay)?;
            api::send_manifest_v2_via_room(
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
        _ => Err(api::SessionError::InvalidInput(
            "this route cannot dial a receiver".into(),
        )),
    }
}

async fn receive_offer(
    source: &PeerSource,
    listen_addrs: envoix_client::BindAddrs,
    config: api::SessionConfig,
    events: Arc<dyn EventSink>,
    cancel: &TransferCancelToken,
    relay: Option<&str>,
) -> Result<PendingManifestV2Receive, api::SessionError> {
    match source {
        PeerSource::ShowManual { token } => {
            let token = token
                .clone()
                .map(Ok)
                .unwrap_or_else(generate_token)
                .map_err(|error| api::SessionError::InvalidInput(error.to_string()))?;
            let pairing = PairingConfig::spake2_shared_token(token.clone())?;
            api::receive_manifest_v2_offer_with_bound_peer(
                listen_addrs.clone(),
                config,
                &pairing,
                events,
                move |peer, _| eprintln!("receiver: {peer}\ntoken: {token}"),
                cancel,
            )
            .await
        }
        PeerSource::ShowInvite { token, ttl_secs } => {
            let token = token
                .clone()
                .map(Ok)
                .unwrap_or_else(generate_token)
                .map_err(|error| api::SessionError::InvalidInput(error.to_string()))?;
            let pairing = PairingConfig::spake2_shared_token(token.clone())?;
            let expires_at = now_unix_seconds().saturating_add(*ttl_secs);
            api::receive_manifest_v2_offer_with_bound_peer(
                listen_addrs.clone(),
                config,
                &pairing,
                events,
                move |peer, relay_urls| print_invite(peer, relay_urls, token, expires_at),
                cancel,
            )
            .await
        }
        PeerSource::Mdns { token } => {
            let token = token
                .clone()
                .map(Ok)
                .unwrap_or_else(generate_token)
                .map_err(|error| api::SessionError::InvalidInput(error.to_string()))?;
            let pairing = PairingConfig::spake2_shared_token(token.clone())?;
            let expires_at = now_unix_seconds().saturating_add(300);
            api::receive_manifest_v2_offer_enable_mdns(
                listen_addrs.clone(),
                config,
                &pairing,
                events,
                move |peer, relay_urls| print_invite(peer, relay_urls, token, expires_at),
                cancel,
            )
            .await
        }
        PeerSource::Room { code, broker } => {
            let broker = api::parse_broker_addr(broker, relay)?;
            api::receive_manifest_v2_offer_via_room(
                broker,
                code,
                listen_addrs,
                config,
                events,
                cancel,
            )
            .await
        }
        _ => Err(api::SessionError::InvalidInput(
            "this route cannot listen for a sender".into(),
        )),
    }
}

fn print_offer(pending: &PendingManifestV2Receive) {
    let manifest = &pending.offer().manifest;
    eprintln!(
        "authenticated offer: {} roots, {} files, {} directories, {} bytes",
        manifest.roots.len(),
        manifest.totals.file_count,
        manifest.totals.directory_count,
        manifest.totals.total_plaintext_bytes
    );
    for root in &manifest.roots {
        eprintln!("  {}", root.requested_name);
    }
}

fn print_invite(
    peer: envoix_client::PeerDescriptor,
    relay_urls: Vec<String>,
    token: String,
    expires_at: u64,
) {
    let invite = QrInvitePayload {
        version: envoix_qr::PAYLOAD_VERSION,
        protocol_version: envoix_client::PROTOCOL_VERSION,
        token,
        peer,
        relay_urls,
        expires_at,
        flags: 0,
    }
    .encode();
    eprintln!("invite: {invite}");
    if let Some(qr) = render_terminal_qr(&invite) {
        eprintln!("{qr}");
    }
}

fn sender_state_directory() -> io::Result<PathBuf> {
    Ok(std::env::current_dir()?.join(".envoix-state-v2"))
}

fn api_client(config_path: Option<&Path>, identity: IdentityConfig) -> CliResult<api::Client> {
    eprintln!("{SPAKE2_EXPERIMENTAL_WARNING}");
    let mut client = api::Client::from_runtime_sources(config_path)?;
    client.identity = identity;
    Ok(client)
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

struct CliEvents {
    json: bool,
}

impl EventSink for CliEvents {
    fn on_event(&self, event: TransferEvent) {
        if self.json {
            let (kind, detail) = match event {
                TransferEvent::Diagnostic { message } => ("diagnostic", message),
                TransferEvent::Pairing { step } => ("pairing", format!("{step:?}")),
                TransferEvent::Connecting => ("connecting", String::new()),
                TransferEvent::Connected { path } => ("connected", path.to_string()),
                TransferEvent::PathChanged { path } => ("path_changed", path.to_string()),
                TransferEvent::Progress {
                    bytes_transferred,
                    total_bytes,
                    ..
                } => ("progress", format!("{bytes_transferred}/{total_bytes}")),
                TransferEvent::ManifestV2Phase { phase, .. } => {
                    ("manifest_v2_phase", format!("{phase:?}"))
                }
            };
            println!("{}", serde_json::json!({ "kind": kind, "detail": detail }));
        } else {
            match event {
                TransferEvent::Diagnostic { message } => eprintln!("{message}"),
                TransferEvent::Pairing { step } => eprintln!("pairing: {step:?}"),
                TransferEvent::Connecting => eprintln!("connecting…"),
                TransferEvent::Connected { path } => eprintln!("connected via {path}"),
                TransferEvent::PathChanged { path } => eprintln!("path changed: {path}"),
                TransferEvent::Progress {
                    bytes_transferred,
                    total_bytes,
                    ..
                } => eprintln!("{bytes_transferred}/{total_bytes}"),
                TransferEvent::ManifestV2Phase { phase, .. } => {
                    eprintln!("manifest v2: {phase:?}")
                }
            }
        }
    }
}
