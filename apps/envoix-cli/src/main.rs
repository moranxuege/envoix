use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
mod agent_service;
mod args;

use args::{
    AgentCommand, Cli, Command, DevicesCommand, InboxCommand, SaveModeArg, SourceIssueActionArg,
    TransferPlan,
};
use clap::Parser;
use envoix_client::api::{
    self, CanonicalTransferJob, DestinationDecisionV2, DestinationRequestV2, EventSink,
    InvitationConsumption, PairingConfig, PeerSource, PendingManifestV2Receive, SourceDecision,
    SourceSelectionState, TransferEvent, TransferJobStore, acquire_invitation,
};
use envoix_client::product::{
    AgentEventCursor, AgentRequest, AgentRequestEnvelope, AgentResponse, AgentResponseEnvelope,
    MAX_AGENT_REQUEST_BYTES, MAX_AGENT_RESPONSE_BYTES, default_agent_control_endpoint,
};
use envoix_client::{IdentityConfig, SPAKE2_EXPERIMENTAL_WARNING, TransferCancelToken};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

struct OneTimeInvitationAuthentication {
    consumption: InvitationConsumption,
}

impl api::AuthenticationHandler for OneTimeInvitationAuthentication {
    fn on_authenticated(
        &self,
        _outcome: api::AuthenticationOutcome,
    ) -> Result<(), api::SessionError> {
        self.consumption.consume();
        Ok(())
    }
}

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
    let agent_endpoint = cli.agent_endpoint;
    match cli.command {
        Command::Send(args) => send(args.into_plan()?, json).await,
        Command::Receive(args) => receive(args.into_plan()?, json).await,
        Command::Agent(args) => match args.command {
            AgentCommand::Status => show_agent_status(
                call_agent(agent_endpoint, AgentRequest::Status).await?,
                json,
            ),
            AgentCommand::Snapshot { inbox_limit } => show_agent_snapshot(
                call_agent(agent_endpoint, AgentRequest::Snapshot { inbox_limit }).await?,
                json,
            ),
            AgentCommand::Events {
                instance_id,
                after,
                limit,
            } => show_agent_events(
                call_agent(
                    agent_endpoint,
                    AgentRequest::Events {
                        after: AgentEventCursor {
                            instance_id,
                            sequence: after,
                        },
                        limit,
                    },
                )
                .await?,
                json,
            ),
            AgentCommand::Install {
                inbox,
                device_name,
                agent_binary,
            } => install_agent(inbox, device_name, agent_binary, json),
            AgentCommand::Start => manage_agent_service("started", agent_service::start, json),
            AgentCommand::Stop => manage_agent_service("stopped", agent_service::stop, json),
            AgentCommand::Pair { name } => show_pairing(
                call_agent(agent_endpoint, AgentRequest::Pair { label: name }).await?,
                json,
            ),
        },
        Command::Devices(args) => match args.command {
            DevicesCommand::List => show_devices(
                call_agent(agent_endpoint, AgentRequest::ListDevices).await?,
                json,
            ),
            DevicesCommand::Forget { device, yes: _ } => show_revoked_device(
                call_agent(agent_endpoint, AgentRequest::RevokeDevice { device }).await?,
                json,
            ),
        },
        Command::Inbox(args) => match args.command {
            InboxCommand::List { limit } => show_inbox(
                call_agent(agent_endpoint, AgentRequest::ListInbox { limit }).await?,
                json,
            ),
            InboxCommand::Latest => show_latest_inbox(
                call_agent(agent_endpoint, AgentRequest::LatestInbox).await?,
                json,
            ),
        },
    }
}

#[cfg(unix)]
async fn call_agent(socket: Option<PathBuf>, request: AgentRequest) -> CliResult<AgentResponse> {
    use tokio::net::UnixStream;

    let socket = socket
        .map(Ok)
        .unwrap_or_else(default_agent_control_endpoint)?;
    let stream = UnixStream::connect(&socket).await.map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "cannot connect to Envoix Agent at {}: {error}; run `envoix agent start` or start envoix-agent in a foreground shell",
                socket.display()
            ),
        )
    })?;
    call_agent_stream(stream, request).await
}

#[cfg(windows)]
async fn call_agent(endpoint: Option<PathBuf>, request: AgentRequest) -> CliResult<AgentResponse> {
    use std::time::Duration;

    use tokio::net::windows::named_pipe::ClientOptions;

    let endpoint = endpoint
        .map(Ok)
        .unwrap_or_else(default_agent_control_endpoint)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let stream = loop {
        match ClientOptions::new().open(&endpoint) {
            Ok(stream) => break stream,
            Err(error)
                if error.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "cannot connect to Envoix Agent at {}: {error}; start envoix-agent for this user",
                        endpoint.display()
                    ),
                )
                .into());
            }
        }
    };
    call_agent_stream(stream, request).await
}

#[cfg(any(unix, windows))]
async fn call_agent_stream<S>(stream: S, request: AgentRequest) -> CliResult<AgentResponse>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    let (read, mut write) = tokio::io::split(stream);
    let request_id = agent_request_id()?;
    let envelope = AgentRequestEnvelope::new(request_id.clone(), request)?;
    let mut bytes = serde_json::to_vec(&envelope)?;
    if bytes.len() as u64 > MAX_AGENT_REQUEST_BYTES {
        return Err("Agent request exceeds the control message limit".into());
    }
    bytes.push(b'\n');
    write.write_all(&bytes).await?;
    write.shutdown().await?;
    let mut response_bytes = Vec::new();
    let mut limited = BufReader::new(read).take(MAX_AGENT_RESPONSE_BYTES + 1);
    limited.read_until(b'\n', &mut response_bytes).await?;
    if response_bytes.is_empty() {
        return Err("Envoix Agent closed the control connection without a response".into());
    }
    if response_bytes.len() as u64 > MAX_AGENT_RESPONSE_BYTES {
        return Err("Agent response exceeds the control message limit".into());
    }
    let response: AgentResponseEnvelope = serde_json::from_slice(&response_bytes)?;
    response.validate_for(&request_id)?;
    Ok(response.response)
}

fn agent_request_id() -> CliResult<String> {
    let mut random = [0_u8; 12];
    getrandom::fill(&mut random)
        .map_err(|error| io::Error::other(format!("request ID entropy unavailable: {error}")))?;
    Ok(format!("cli_{}", URL_SAFE_NO_PAD.encode(random)))
}

fn install_agent(
    inbox: Option<PathBuf>,
    device_name: String,
    agent_binary: Option<PathBuf>,
    json: bool,
) -> CliResult<()> {
    let installed = agent_service::install(agent_service::InstallOptions {
        inbox,
        device_name,
        agent_binary,
    })?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "kind": "agent_service_installed",
                "agent_binary": installed.agent_binary,
                "cli_binary": installed.cli_binary,
                "settings_file": installed.settings_file,
                "unit_file": installed.unit_file,
            })
        );
    } else {
        println!("Agent installed and started.");
        println!("agent: {}", installed.agent_binary.display());
        println!("cli: {}", installed.cli_binary.display());
        println!("settings: {}", installed.settings_file.display());
        println!("service: {}", installed.unit_file.display());
    }
    Ok(())
}

fn manage_agent_service(
    completed: &str,
    operation: impl FnOnce() -> io::Result<()>,
    json: bool,
) -> CliResult<()> {
    operation()?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "kind": "agent_service_changed",
                "state": completed,
            })
        );
    } else {
        println!("Agent service {completed}.");
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
async fn call_agent(
    _endpoint: Option<PathBuf>,
    _request: AgentRequest,
) -> CliResult<AgentResponse> {
    Err("the local Envoix Agent control transport is unsupported on this platform".into())
}

fn agent_error(response: AgentResponse) -> CliResult<AgentResponse> {
    match response {
        AgentResponse::Error { code, message } => Err(format!("Agent {code}: {message}").into()),
        response => Ok(response),
    }
}

fn show_agent_status(response: AgentResponse, json: bool) -> CliResult<()> {
    let response = agent_error(response)?;
    if json {
        println!("{}", serde_json::to_string(&response)?);
        return Ok(());
    }
    let AgentResponse::Status { status } = response else {
        return Err("Agent returned an unexpected response".into());
    };
    println!("agent: running (pid {})", status.pid);
    println!("device: {}", status.device_name);
    println!("inbox: {}", status.inbox_directory);
    println!(
        "devices: {} paired, {} listening, {} pairing",
        status.paired_devices, status.active_receivers, status.active_pairings
    );
    println!("broker: {}", status.broker);
    println!("relay: {}", status.relay.as_deref().unwrap_or("disabled"));
    Ok(())
}

fn show_agent_snapshot(response: AgentResponse, json: bool) -> CliResult<()> {
    let response = agent_error(response)?;
    if json {
        println!("{}", serde_json::to_string(&response)?);
        return Ok(());
    }
    let AgentResponse::Snapshot { snapshot } = response else {
        return Err("Agent returned an unexpected response".into());
    };
    println!("engine sequence: {}", snapshot.engine.last_sequence);
    println!("relationships: {}", snapshot.engine.relationships.len());
    println!("rooms: {}", snapshot.engine.rooms.len());
    println!("transfers: {}", snapshot.engine.transfers.len());
    println!("inbox: {}", snapshot.inbox.len());
    println!(
        "event cursor: {}:{}",
        snapshot.event_cursor.instance_id, snapshot.event_cursor.sequence
    );
    Ok(())
}

fn show_agent_events(response: AgentResponse, json: bool) -> CliResult<()> {
    let response = agent_error(response)?;
    if json {
        println!("{}", serde_json::to_string(&response)?);
        return Ok(());
    }
    match response {
        AgentResponse::Events { cursor, events } => {
            for event in events {
                println!("{}\t{:?}", event.sequence, event.event);
            }
            println!("event cursor: {}:{}", cursor.instance_id, cursor.sequence);
            Ok(())
        }
        AgentResponse::SnapshotRequired { cursor } => Err(format!(
            "Agent event cursor is no longer usable; fetch a new snapshot (current cursor {}:{})",
            cursor.instance_id, cursor.sequence
        )
        .into()),
        _ => Err("Agent returned an unexpected response".into()),
    }
}

fn show_pairing(response: AgentResponse, json: bool) -> CliResult<()> {
    let response = agent_error(response)?;
    if json {
        println!("{}", serde_json::to_string(&response)?);
        return Ok(());
    }
    let AgentResponse::Pairing { pairing } = response else {
        return Err("Agent returned an unexpected response".into());
    };
    println!("Room code: {}", pairing.room_code);
    println!("Verification code: {}", pairing.verification_code);
    eprintln!(
        "On the Mac, enter the room code in Envoix, then enter the six-digit verification code when prompted."
    );
    eprintln!("Keep envoix-agent running until the device appears in `envoix devices list`.");
    Ok(())
}

fn show_devices(response: AgentResponse, json: bool) -> CliResult<()> {
    let response = agent_error(response)?;
    if json {
        println!("{}", serde_json::to_string(&response)?);
        return Ok(());
    }
    let AgentResponse::Devices { devices } = response else {
        return Err("Agent returned an unexpected response".into());
    };
    if devices.is_empty() {
        println!("No remembered devices.");
        return Ok(());
    }
    for device in devices {
        println!(
            "{}\t{}\tgeneration {}",
            device.id, device.label, device.generation
        );
    }
    Ok(())
}

fn show_revoked_device(response: AgentResponse, json: bool) -> CliResult<()> {
    let response = agent_error(response)?;
    if json {
        println!("{}", serde_json::to_string(&response)?);
        return Ok(());
    }
    let AgentResponse::DeviceRevoked { device } = response else {
        return Err("Agent returned an unexpected response".into());
    };
    println!("Revoked device: {} ({})", device.label, device.id);
    Ok(())
}

fn show_inbox(response: AgentResponse, json: bool) -> CliResult<()> {
    let response = agent_error(response)?;
    if json {
        println!("{}", serde_json::to_string(&response)?);
        return Ok(());
    }
    let AgentResponse::Inbox { items } = response else {
        return Err("Agent returned an unexpected response".into());
    };
    if items.is_empty() {
        println!("Inbox is empty.");
        return Ok(());
    }
    for item in items {
        let paths = item
            .roots
            .iter()
            .map(|root| root.path.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "{}\t{}\t{} bytes\t{}",
            item.received_at_unix_ms, item.from_device_label, item.total_bytes, paths
        );
    }
    Ok(())
}

fn show_latest_inbox(response: AgentResponse, json: bool) -> CliResult<()> {
    let response = agent_error(response)?;
    if json {
        println!("{}", serde_json::to_string(&response)?);
        return Ok(());
    }
    let AgentResponse::Latest { item } = response else {
        return Err("Agent returned an unexpected response".into());
    };
    let item = item.ok_or("Inbox is empty")?;
    for root in item.roots {
        println!("{}", root.path);
    }
    Ok(())
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
        let error = format!(
            "offer requires explicit approval before payload ({} bytes offered, {} bytes allocatable); rerun with --approve-large-transfer",
            total, available
        );
        pending.reject().await;
        return Err(error.into());
    }
    let destination_setup: CliResult<_> = async {
        Ok(match plan.save_mode {
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
        })
    }
    .await;
    let (decision, copy_staging_directory, staging_available) = match destination_setup {
        Ok(setup) => setup,
        Err(error) => {
            pending.close_with_failure().await;
            return Err(error);
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
                preplanned_root_names: None,
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
        PeerSource::Manual { peer, token_ref } => {
            let token = api::acquire_shared_token(token_ref)
                .map_err(|error| api::SessionError::InvalidInput(error.to_string()))?;
            let pairing = PairingConfig::spake2_shared_token(token)?;
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
        PeerSource::Invitation {
            secret_ref, broker, ..
        } => {
            let lease = acquire_invitation(secret_ref)
                .map_err(|error| api::SessionError::InvalidInput(error.to_string()))?;
            let broker = api::parse_broker_addr(broker, relay)?;
            let authentication = OneTimeInvitationAuthentication {
                consumption: lease.consumption(),
            };
            api::send_manifest_v2_via_room_with_authentication(
                broker,
                lease.bootstrap().clone(),
                job,
                state_directory,
                config,
                events,
                cancel,
                &authentication,
            )
            .await
        }
        PeerSource::Mdns {
            token_ref: Some(token_ref),
        } => {
            let token = api::acquire_shared_token(token_ref)
                .map_err(|error| api::SessionError::InvalidInput(error.to_string()))?;
            let pairing = PairingConfig::spake2_shared_token(token)?;
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
        PeerSource::ShowManual { token_ref } => {
            let token = token_ref
                .as_ref()
                .map(|token_ref| {
                    api::acquire_shared_token(token_ref)
                        .map_err(|error| api::SessionError::InvalidInput(error.to_string()))
                })
                .unwrap_or_else(generate_token)?;
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
        PeerSource::Mdns { token_ref } => {
            let token = token_ref
                .as_ref()
                .map(|token_ref| {
                    api::acquire_shared_token(token_ref)
                        .map_err(|error| api::SessionError::InvalidInput(error.to_string()))
                })
                .unwrap_or_else(generate_token)?;
            let pairing = PairingConfig::spake2_shared_token(token.clone())?;
            api::receive_manifest_v2_offer_enable_mdns(
                listen_addrs.clone(),
                config,
                &pairing,
                events,
                move |peer, _relay_urls| eprintln!("receiver: {peer}\ntoken: {token}"),
                cancel,
            )
            .await
        }
        PeerSource::Invitation {
            secret_ref, broker, ..
        } => {
            let lease = acquire_invitation(secret_ref)
                .map_err(|error| api::SessionError::InvalidInput(error.to_string()))?;
            let broker = api::parse_broker_addr(broker, relay)?;
            let authentication = OneTimeInvitationAuthentication {
                consumption: lease.consumption(),
            };
            api::receive_manifest_v2_offer_via_room_with_authentication(
                broker,
                lease.bootstrap().clone(),
                listen_addrs,
                config,
                events,
                cancel,
                &authentication,
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

fn sender_state_directory() -> io::Result<PathBuf> {
    Ok(std::env::current_dir()?.join(".envoix-state-v2"))
}

fn api_client(config_path: Option<&Path>, identity: IdentityConfig) -> CliResult<api::Client> {
    eprintln!("{SPAKE2_EXPERIMENTAL_WARNING}");
    let mut client = api::Client::from_runtime_sources(config_path)?;
    client.identity = identity;
    Ok(client)
}

fn generate_token() -> Result<String, api::SessionError> {
    let mut bytes = [0_u8; 18];
    getrandom::fill(&mut bytes).map_err(|error| {
        api::SessionError::Crypto(format!("token entropy unavailable: {error}"))
    })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

struct CliEvents {
    json: bool,
}

impl EventSink for CliEvents {
    fn on_event(&self, event: TransferEvent) {
        if self.json {
            let value = match event {
                TransferEvent::Diagnostic { message } => serde_json::json!({
                    "kind":"diagnostic",
                    "detail":message,
                    "message":message,
                }),
                TransferEvent::Pairing { step } => {
                    let detail = format!("{step:?}");
                    serde_json::json!({
                        "kind":"pairing",
                        "detail":detail,
                        "step":step,
                    })
                }
                TransferEvent::Connecting => serde_json::json!({
                    "kind":"connecting",
                    "detail":"",
                }),
                TransferEvent::Connected { path } => {
                    let path = path.to_string();
                    serde_json::json!({
                        "kind":"connected",
                        "detail":path,
                        "path":path,
                    })
                }
                TransferEvent::PathChanged { path } => {
                    let path = path.to_string();
                    serde_json::json!({
                        "kind":"path_changed",
                        "detail":path,
                        "path":path,
                    })
                }
                TransferEvent::Progress {
                    transfer_id,
                    bytes_transferred,
                    total_bytes,
                    ..
                } => serde_json::json!({
                    "kind":"progress",
                    "detail":format!("{bytes_transferred}/{total_bytes}"),
                    "transfer_id":transfer_id.to_string(),
                    "bytes_transferred":bytes_transferred,
                    "total_bytes":total_bytes,
                }),
                TransferEvent::ManifestV2Phase {
                    transfer_id,
                    direction,
                    phase,
                } => {
                    let detail = format!("{phase:?}");
                    let phase = match phase {
                        api::ManifestV2ProgressPhase::Transferring => "transferring",
                        api::ManifestV2ProgressPhase::Verifying => "verifying",
                        api::ManifestV2ProgressPhase::Saving => "saving",
                        api::ManifestV2ProgressPhase::WaitingForReceiverSave => {
                            "waiting_for_receiver_save"
                        }
                        api::ManifestV2ProgressPhase::FinalizingDelivery => "finalizing_delivery",
                    };
                    serde_json::json!({
                        "kind":"manifest_v2_phase",
                        "detail":detail,
                        "transfer_id":transfer_id.to_string(),
                        "direction":transfer_direction_wire(direction),
                        "phase":phase,
                    })
                }
                TransferEvent::StageTiming {
                    transfer_id,
                    direction,
                    attempt_id,
                    stage,
                    elapsed_us,
                    delta_us,
                } => serde_json::json!({
                    "kind":"stage_timing",
                    "stage":transfer_stage_wire(stage),
                    "direction":transfer_direction_wire(direction),
                    "attempt_id":attempt_id,
                    "transfer_id":transfer_id.map(|value| value.to_string()),
                    "elapsed_us":elapsed_us,
                    "delta_us":delta_us,
                }),
            };
            println!("{value}");
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
                TransferEvent::StageTiming {
                    transfer_id,
                    direction,
                    attempt_id,
                    stage,
                    elapsed_us,
                    delta_us,
                } => eprintln!(
                    "stage_timing stage={} direction={} attempt_id={} transfer_id={} elapsed_us={} delta_us={}",
                    transfer_stage_wire(stage),
                    transfer_direction_wire(direction),
                    attempt_id,
                    transfer_id
                        .as_ref()
                        .map(ToString::to_string)
                        .as_deref()
                        .unwrap_or("-"),
                    elapsed_us,
                    delta_us,
                ),
            }
        }
    }
}

fn transfer_stage_wire(stage: api::TransferStage) -> &'static str {
    match stage {
        api::TransferStage::SessionStarted => "session_started",
        api::TransferStage::ConnectionReady => "connection_ready",
        api::TransferStage::AuthenticationStarted => "authentication_started",
        api::TransferStage::AuthenticationComplete => "authentication_complete",
        api::TransferStage::ManifestOffer => "manifest_offer",
        api::TransferStage::ManifestAccepted => "manifest_accepted",
        api::TransferStage::FirstPayload => "first_payload",
        api::TransferStage::PayloadComplete => "payload_complete",
        api::TransferStage::DeliveryComplete => "delivery_complete",
        api::TransferStage::Canceled => "canceled",
        api::TransferStage::Failed => "failed",
    }
}

fn transfer_direction_wire(direction: api::TransferDirection) -> &'static str {
    match direction {
        api::TransferDirection::Send => "send",
        api::TransferDirection::Receive => "receive",
    }
}
