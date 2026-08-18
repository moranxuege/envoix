use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::io;
use std::os::unix::fs::FileTypeExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use clap::Parser;
use envoix_client::api::{
    self, DestinationDecisionV2, DestinationRequestV2, EventSink, PendingManifestV2Receive,
    RememberedCredential, RememberedRoomControlRole, RoomControlEvent, RoomControlInvite,
    RoomControlSession, RoomOfferRejection, RoomTransferOffer, TransferEvent, TransferOptions,
    TransferRole,
};
use envoix_client::model::{
    RememberedAttemptOutcome, RememberedGenerationRole, remembered_generation_attempts,
};
use envoix_client::product::{
    AGENT_PROTOCOL_VERSION, AgentRequest, AgentResponse, AgentSettings, AgentStatus, InboxItem,
    InboxRoot, PairingInvitation, PreparedRememberedDevice, ProductStore, RememberedDeviceRecord,
    default_agent_state_directory,
};
use envoix_client::{
    DEFAULT_RELAY_URL, DEFAULT_RENDEZVOUS_BROKER, IdentityConfig, TransferCancelToken,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;

const MAX_AGENT_REQUEST_BYTES: u64 = 64 * 1024;
const REMEMBERED_FALLBACK_TIMEOUT: Duration = Duration::from_secs(35);
const RECEIVER_RETRY_DELAY: Duration = Duration::from_secs(3);
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(5);

#[derive(Debug, Parser)]
#[command(
    name = "envoix-agent",
    version,
    about = "Persistent Envoix receiver and local Inbox service"
)]
struct Cli {
    /// Managed Agent settings written by `envoix agent install`.
    #[arg(long)]
    settings: Option<PathBuf>,
    /// Product metadata, protected credentials, and transfer state.
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// Directory where completed incoming roots are saved.
    #[arg(long)]
    inbox: Option<PathBuf>,
    /// Unix socket used by `envoix agent`, `devices`, and `inbox` commands.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Human-readable name for this Agent host.
    #[arg(long)]
    device_name: Option<String>,
    /// Rendezvous broker shared with the Mac app.
    #[arg(long, default_value = DEFAULT_RENDEZVOUS_BROKER)]
    broker: String,
    /// Relay URL; use `none` to run without a relay.
    #[arg(long, default_value = DEFAULT_RELAY_URL)]
    relay: String,
    /// Optional transport-only runtime TOML.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Increase logging verbosity.
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Clone)]
struct RuntimeConfig {
    state_directory: PathBuf,
    inbox_directory: PathBuf,
    socket_path: PathBuf,
    device_name: String,
    broker: String,
    relay: Option<String>,
}

struct AgentRuntime {
    config: RuntimeConfig,
    client: api::Client,
    store: Mutex<ProductStore>,
    active_receivers: Mutex<HashMap<String, TransferCancelToken>>,
    active_pairings: Mutex<HashSet<String>>,
    shutdown: TransferCancelToken,
    background_tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl AgentRuntime {
    fn status(&self) -> Result<AgentStatus> {
        let paired_devices = lock(&self.store)?.devices().len();
        let active_receivers = lock(&self.active_receivers)?.len();
        let active_pairings = lock(&self.active_pairings)?.len();
        Ok(AgentStatus {
            protocol_version: AGENT_PROTOCOL_VERSION,
            pid: std::process::id(),
            device_name: self.config.device_name.clone(),
            state_directory: self.config.state_directory.display().to_string(),
            inbox_directory: self.config.inbox_directory.display().to_string(),
            broker: self.config.broker.clone(),
            relay: self.config.relay.clone(),
            paired_devices,
            active_receivers,
            active_pairings,
        })
    }

    fn session_config(&self, relay: Option<&str>) -> api::SessionConfig {
        let mut options = TransferOptions::default();
        options.relay = relay.map(str::to_string);
        self.client.session_config(&options)
    }
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    let settings = cli
        .settings
        .as_deref()
        .map(read_agent_settings)
        .transpose()?;
    let state_directory = match cli.state_dir {
        Some(path) => path,
        None => default_agent_state_directory()?,
    };
    let inbox_directory = cli
        .inbox
        .or_else(|| {
            settings
                .as_ref()
                .map(|settings| settings.inbox_directory.clone())
        })
        .unwrap_or_else(|| state_directory.join("inbox"));
    let device_name = cli
        .device_name
        .or_else(|| {
            settings
                .as_ref()
                .map(|settings| settings.device_name.clone())
        })
        .unwrap_or_else(|| "WSL".into());
    let socket_path = cli
        .socket
        .or_else(|| std::env::var_os("ENVOIX_AGENT_SOCKET").map(PathBuf::from))
        .unwrap_or_else(|| state_directory.join("agent.sock"));
    let relay = match cli.relay.trim() {
        "" | "none" | "off" => None,
        value => Some(value.to_string()),
    };

    create_private_directory(&state_directory)?;
    create_directory(&inbox_directory)?;
    let client = agent_client(cli.config.as_deref())?;
    let runtime = Arc::new(AgentRuntime {
        config: RuntimeConfig {
            state_directory: state_directory.clone(),
            inbox_directory,
            socket_path,
            device_name,
            broker: cli.broker,
            relay,
        },
        client,
        store: Mutex::new(ProductStore::open(&state_directory)?),
        active_receivers: Mutex::new(HashMap::new()),
        active_pairings: Mutex::new(HashSet::new()),
        shutdown: TransferCancelToken::new(),
        background_tasks: Mutex::new(Vec::new()),
    });

    let device_ids = lock(&runtime.store)?
        .device_records()
        .into_iter()
        .map(|device| device.id().to_string())
        .collect::<Vec<_>>();
    for device_id in device_ids {
        spawn_remembered_receiver(runtime.clone(), device_id);
    }

    serve(runtime).await
}

fn init_tracing(verbosity: u8) {
    use tracing_subscriber::EnvFilter;
    let default = match verbosity {
        0 => "envoix_agent=info,warn",
        1 => "envoix_agent=debug,envoix=debug,warn",
        _ => "envoix_agent=trace,envoix=trace,iroh=debug,warn",
    };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| default.into()))
        .with_target(false)
        .init();
}

fn agent_client(config_path: Option<&Path>) -> Result<api::Client> {
    let mut client = api::Client::from_runtime_sources(config_path)?;
    client.identity = IdentityConfig::Ephemeral;
    Ok(client)
}

fn read_agent_settings(path: &Path) -> Result<AgentSettings> {
    let bytes =
        fs::read(path).with_context(|| format!("read Agent settings {}", path.display()))?;
    let settings: AgentSettings = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse Agent settings {}", path.display()))?;
    settings
        .validate()
        .with_context(|| format!("validate Agent settings {}", path.display()))?;
    Ok(settings)
}

async fn serve(runtime: Arc<AgentRuntime>) -> Result<()> {
    prepare_socket(&runtime.config.socket_path).await?;
    let listener = UnixListener::bind(&runtime.config.socket_path)
        .with_context(|| format!("bind {}", runtime.config.socket_path.display()))?;
    fs::set_permissions(
        &runtime.config.socket_path,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )?;
    let _cleanup = SocketCleanup(runtime.config.socket_path.clone());
    tracing::info!(
        socket = %runtime.config.socket_path.display(),
        inbox = %runtime.config.inbox_directory.display(),
        "Envoix Agent ready"
    );

    let termination = termination_signal();
    tokio::pin!(termination);
    loop {
        tokio::select! {
            signal = &mut termination => {
                signal?;
                tracing::info!("shutting down");
                shutdown_background_tasks(&runtime).await;
                return Ok(());
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let runtime = runtime.clone();
                tokio::spawn(async move {
                    if let Err(error) = serve_connection(runtime, stream).await {
                        tracing::warn!(%error, "local Agent request failed");
                    }
                });
            }
        }
    }
}

async fn termination_signal() -> io::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        signal = tokio::signal::ctrl_c() => signal,
        _ = terminate.recv() => Ok(()),
    }
}

async fn prepare_socket(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Agent socket needs a parent directory"))?;
    create_directory(parent)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_socket() {
        bail!(
            "{} exists and is not a Unix socket; refusing to remove it",
            path.display()
        );
    }
    if UnixStream::connect(path).await.is_ok() {
        bail!(
            "another Envoix Agent is already listening at {}",
            path.display()
        );
    }
    fs::remove_file(path).with_context(|| format!("remove stale socket {}", path.display()))?;
    Ok(())
}

async fn serve_connection(runtime: Arc<AgentRuntime>, stream: UnixStream) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut request_bytes = Vec::new();
    let mut limited = BufReader::new(read).take(MAX_AGENT_REQUEST_BYTES + 1);
    limited.read_until(b'\n', &mut request_bytes).await?;
    let response = if request_bytes.len() as u64 > MAX_AGENT_REQUEST_BYTES {
        AgentResponse::error("request_too_large", "Agent request exceeds 64 KiB")
    } else {
        match serde_json::from_slice::<AgentRequest>(&request_bytes) {
            Ok(request) => handle_request(runtime, request).await,
            Err(error) => AgentResponse::error("invalid_request", error.to_string()),
        }
    };
    let mut response_bytes = serde_json::to_vec(&response)?;
    response_bytes.push(b'\n');
    write.write_all(&response_bytes).await?;
    write.shutdown().await?;
    Ok(())
}

async fn handle_request(runtime: Arc<AgentRuntime>, request: AgentRequest) -> AgentResponse {
    match handle_request_result(runtime, request).await {
        Ok(response) => response,
        Err(error) => AgentResponse::error("operation_failed", format!("{error:#}")),
    }
}

async fn handle_request_result(
    runtime: Arc<AgentRuntime>,
    request: AgentRequest,
) -> Result<AgentResponse> {
    match request {
        AgentRequest::Status => Ok(AgentResponse::Status {
            status: runtime.status()?,
        }),
        AgentRequest::ListDevices => Ok(AgentResponse::Devices {
            devices: lock(&runtime.store)?.devices(),
        }),
        AgentRequest::ForgetDevice { device } => {
            let forgotten = lock(&runtime.store)?.forget_device(&device)?;
            if let Some(cancel) = lock(&runtime.active_receivers)?.get(&forgotten.id) {
                cancel.cancel();
            }
            Ok(AgentResponse::DeviceForgotten { device: forgotten })
        }
        AgentRequest::ListInbox { limit } => Ok(AgentResponse::Inbox {
            items: lock(&runtime.store)?.inbox(limit),
        }),
        AgentRequest::LatestInbox => Ok(AgentResponse::Latest {
            item: lock(&runtime.store)?.latest_inbox(),
        }),
        AgentRequest::Pair { label } => begin_pairing(runtime, label).await,
    }
}

async fn begin_pairing(runtime: Arc<AgentRuntime>, label: String) -> Result<AgentResponse> {
    if runtime.shutdown.is_cancelled() {
        bail!("Agent is shutting down");
    }
    let prepared = {
        let store = lock(&runtime.store)?;
        store.prepare_device(
            &label,
            &runtime.config.broker,
            runtime.config.relay.as_deref(),
        )?
    };
    {
        let mut pairings = lock(&runtime.active_pairings)?;
        if !pairings.insert(prepared.label().to_ascii_lowercase()) {
            bail!("a pairing for this device label is already active");
        }
    }
    let invitation = match RoomControlInvite::generate(
        runtime.config.broker.clone(),
        runtime.config.relay.clone(),
    ) {
        Ok(invitation) => invitation,
        Err(error) => {
            lock(&runtime.active_pairings)?.remove(&prepared.label().to_ascii_lowercase());
            return Err(error.into());
        }
    };
    let verification_code = match generate_verification_code() {
        Ok(code) => code,
        Err(error) => {
            lock(&runtime.active_pairings)?.remove(&prepared.label().to_ascii_lowercase());
            return Err(error);
        }
    };
    let response = AgentResponse::Pairing {
        pairing: PairingInvitation {
            label: prepared.label().to_string(),
            room_code: invitation.code().to_string(),
            verification_code: verification_code.clone(),
            expires_at_unix_seconds: invitation.expires_at_unix_secs(),
        },
    };
    let task_runtime = runtime.clone();
    spawn_background_task(
        &runtime,
        run_initial_pairing(task_runtime, prepared, invitation, verification_code),
    );
    Ok(response)
}

async fn run_initial_pairing(
    runtime: Arc<AgentRuntime>,
    prepared: PreparedRememberedDevice,
    invitation: RoomControlInvite,
    verification_code: String,
) {
    let label = prepared.label().to_string();
    let device_id = prepared.id().to_string();
    let paired = establish_initial_room(&runtime, prepared, invitation, &verification_code).await;
    lock_or_log(&runtime.active_pairings, "active pairings")
        .map(|mut pairings| pairings.remove(&label.to_ascii_lowercase()));
    let (session, session_cancel) = match paired {
        Ok(paired) => {
            tracing::info!(device = %label, "device verification completed");
            paired
        }
        Err(error) if runtime.shutdown.is_cancelled() => {
            tracing::debug!(device = %label, %error, "initial pairing stopped");
            return;
        }
        Err(error) => {
            tracing::warn!(device = %label, %error, "initial pairing ended");
            return;
        }
    };
    let result = run_room_session(&runtime, &session, &device_id, &label, &session_cancel).await;
    session.shutdown().await;
    if let Err(error) = result
        && !runtime.shutdown.is_cancelled()
        && !session_cancel.is_cancelled()
    {
        tracing::warn!(device = %label, %error, "initial room ended");
    }
    lock_or_log(&runtime.active_receivers, "active receivers")
        .map(|mut active| active.remove(&device_id));
    if !runtime.shutdown.is_cancelled() && !session_cancel.is_cancelled() {
        spawn_remembered_receiver(runtime, device_id);
    }
}

async fn establish_initial_room(
    runtime: &AgentRuntime,
    prepared: PreparedRememberedDevice,
    invitation: RoomControlInvite,
    verification_code: &str,
) -> Result<(RoomControlSession, TransferCancelToken)> {
    let device_id = prepared.id().to_string();
    let session = api::connect_room_control(
        invitation,
        runtime.config.device_name.clone(),
        true,
        false,
        runtime.session_config(runtime.config.relay.as_deref()),
        &runtime.shutdown,
    )
    .await?;
    let pairing = async {
        session.request_verification(verification_code).await?;
        loop {
            match session.next_event().await? {
                RoomControlEvent::VerificationSucceeded => {
                    let credential = session.pairing_credential().ok_or_else(|| {
                        anyhow!("verified room did not expose a pairing credential")
                    })?;
                    let session_cancel = register_remembered_receiver(runtime, &device_id)?
                        .ok_or_else(|| anyhow!("remembered receiver was already active"))?;
                    let commit_result = (|| -> Result<()> {
                        lock(&runtime.store)?.commit_device(
                            prepared,
                            &credential.to_opaque(),
                            0,
                        )?;
                        Ok(())
                    })();
                    if let Err(error) = commit_result {
                        lock_or_log(&runtime.active_receivers, "active receivers")
                            .map(|mut active| active.remove(&device_id));
                        return Err(error);
                    }
                    return Ok(session_cancel);
                }
                RoomControlEvent::VerificationFailed => {
                    bail!("the one-time device verification code was rejected");
                }
                RoomControlEvent::IncomingOffer(offer) => {
                    session
                        .reject_offer(&offer.offer_id, RoomOfferRejection::Invalid)
                        .await?;
                }
                RoomControlEvent::PeerClosed(reason) => {
                    bail!("peer closed the room before verification: {reason:?}");
                }
                RoomControlEvent::VerificationRequested
                | RoomControlEvent::LifetimeChanged(_)
                | RoomControlEvent::Pong { .. } => {}
                RoomControlEvent::OfferAccepted { .. } | RoomControlEvent::OfferRejected { .. } => {
                    bail!("peer sent an unexpected offer decision during verification");
                }
            }
        }
    }
    .await;
    match pairing {
        Ok(session_cancel) => Ok((session, session_cancel)),
        Err(error) => {
            session.shutdown().await;
            Err(error)
        }
    }
}

async fn receive_invitation_offer(
    runtime: &AgentRuntime,
    bootstrap: api::InvitationBootstrap,
    cancel: &TransferCancelToken,
) -> Result<PendingManifestV2Receive> {
    let relay = runtime.config.relay.as_deref();
    let broker = api::parse_broker_addr(&runtime.config.broker, relay)?;
    let events: Arc<dyn EventSink> = Arc::new(AgentEvents);
    api::receive_manifest_v2_offer_via_room(
        broker,
        bootstrap,
        envoix_client::BindAddrs::dual_stack(0),
        runtime.session_config(relay),
        events,
        cancel,
    )
    .await
    .map_err(Into::into)
}

fn spawn_remembered_receiver(runtime: Arc<AgentRuntime>, device_id: String) {
    if runtime.shutdown.is_cancelled() {
        return;
    }
    let receiver_cancel = match register_remembered_receiver(&runtime, &device_id) {
        Ok(Some(cancel)) => cancel,
        Ok(None) => return,
        Err(error) => {
            tracing::error!(device_id, %error, "remembered receiver could not start");
            return;
        }
    };
    let task_runtime = runtime.clone();
    spawn_background_task(&runtime, async move {
        remembered_receiver_loop(task_runtime.clone(), &device_id, &receiver_cancel).await;
        if let Some(mut active) = lock_or_log(&task_runtime.active_receivers, "active receivers") {
            active.remove(&device_id);
        }
    });
}

fn register_remembered_receiver(
    runtime: &AgentRuntime,
    device_id: &str,
) -> Result<Option<TransferCancelToken>> {
    let mut active = lock(&runtime.active_receivers)?;
    if active.contains_key(device_id) {
        return Ok(None);
    }
    let cancel = TransferCancelToken::new();
    active.insert(device_id.to_string(), cancel.clone());
    Ok(Some(cancel))
}

async fn remembered_receiver_loop(
    runtime: Arc<AgentRuntime>,
    device_id: &str,
    receiver_cancel: &TransferCancelToken,
) {
    while !runtime.shutdown.is_cancelled() && !receiver_cancel.is_cancelled() {
        let loaded = (|| -> Result<_> {
            let store = lock(&runtime.store)?;
            let record = store
                .device_record(device_id)
                .ok_or_else(|| anyhow!("remembered device metadata is missing"))?;
            let opaque = store.device_credential(device_id)?;
            Ok((record, opaque))
        })();
        let (record, opaque) = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                tracing::error!(device_id, %error, "remembered receiver cannot load device");
                return;
            }
        };
        match connect_remembered_room(&runtime, &record, &opaque, receiver_cancel).await {
            Ok((session, next_generation)) => {
                if let Err(error) = lock(&runtime.store).and_then(|mut store| {
                    store
                        .rotate_device(record.id(), &opaque, next_generation)
                        .map_err(Into::into)
                }) {
                    session.shutdown().await;
                    tracing::error!(device = %record.label(), %error, "remembered generation could not be persisted");
                    return;
                }
                tracing::info!(
                    device = %record.label(),
                    generation = next_generation,
                    "remembered room connected"
                );
                let result = run_room_session(
                    &runtime,
                    &session,
                    record.id(),
                    record.label(),
                    receiver_cancel,
                )
                .await;
                session.shutdown().await;
                if let Err(error) = result
                    && !runtime.shutdown.is_cancelled()
                    && !receiver_cancel.is_cancelled()
                {
                    tracing::warn!(device = %record.label(), %error, "remembered room ended");
                }
            }
            Err(error) => {
                if runtime.shutdown.is_cancelled() || receiver_cancel.is_cancelled() {
                    return;
                }
                tracing::warn!(device = %record.label(), %error, "remembered receiver retrying");
                tokio::select! {
                    _ = tokio::time::sleep(RECEIVER_RETRY_DELAY) => {}
                    _ = runtime.shutdown.cancelled() => return,
                    _ = receiver_cancel.cancelled() => return,
                }
            }
        }
    }
}

async fn connect_remembered_room(
    runtime: &Arc<AgentRuntime>,
    record: &RememberedDeviceRecord,
    opaque: &[u8],
    receiver_cancel: &TransferCancelToken,
) -> Result<(RoomControlSession, u64)> {
    let credential = RememberedCredential::from_opaque(opaque)?;
    let relay = record.relay();
    let generations = remembered_generation_attempts(
        record.generation(),
        record.previous_generation(),
        RememberedGenerationRole::Responder,
    )?;
    let last_index = generations.len() - 1;
    let mut last_error = None;
    for (index, generation) in generations.into_iter().enumerate() {
        let next_generation = generation
            .checked_add(1)
            .ok_or_else(|| anyhow!("remembered credential generation is exhausted"))?;
        let attempt_cancel = TransferCancelToken::new();
        let connect = api::connect_remembered_room_control(
            credential.derive_session(generation),
            record.broker().to_string(),
            relay.map(str::to_string),
            runtime.config.device_name.clone(),
            RememberedRoomControlRole::Responder,
            runtime.session_config(relay),
            &attempt_cancel,
        );
        tokio::pin!(connect);
        let result = if index < last_index {
            tokio::select! {
                result = &mut connect => result,
                _ = tokio::time::sleep(REMEMBERED_FALLBACK_TIMEOUT) => {
                    attempt_cancel.cancel();
                    let _ = (&mut connect).await;
                    last_error = Some(anyhow!(
                        "current remembered generation did not find the peer"
                    ));
                    continue;
                }
                _ = receiver_cancel.cancelled() => {
                    attempt_cancel.cancel();
                    let _ = (&mut connect).await;
                    return Err(anyhow!("remembered room connection cancelled"));
                }
                _ = runtime.shutdown.cancelled() => {
                    attempt_cancel.cancel();
                    let _ = (&mut connect).await;
                    return Err(anyhow!("remembered room connection cancelled"));
                }
            }
        } else {
            tokio::select! {
                result = &mut connect => result,
                _ = receiver_cancel.cancelled() => {
                    attempt_cancel.cancel();
                    let _ = (&mut connect).await;
                    return Err(anyhow!("remembered room connection cancelled"));
                }
                _ = runtime.shutdown.cancelled() => {
                    attempt_cancel.cancel();
                    let _ = (&mut connect).await;
                    return Err(anyhow!("remembered room connection cancelled"));
                }
            }
        };
        match result {
            Ok(session) => return Ok((session, next_generation)),
            Err(error)
                if (RememberedAttemptOutcome {
                    succeeded: false,
                    authenticated: error.peer_authenticated(),
                    canceled: receiver_cancel.is_cancelled() || runtime.shutdown.is_cancelled(),
                })
                .should_stop_fallback() =>
            {
                return Err(error.into_error().into());
            }
            Err(error) => last_error = Some(error.into_error().into()),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("remembered device has no usable generation")))
}

async fn run_room_session(
    runtime: &Arc<AgentRuntime>,
    session: &RoomControlSession,
    device_id: &str,
    device_label: &str,
    receiver_cancel: &TransferCancelToken,
) -> Result<()> {
    loop {
        let event = tokio::select! {
            event = session.next_event() => event?,
            _ = runtime.shutdown.cancelled() => return Ok(()),
            _ = receiver_cancel.cancelled() => return Ok(()),
        };
        match event {
            RoomControlEvent::IncomingOffer(offer) => {
                if receiver_cancel.is_cancelled() {
                    return Ok(());
                }
                match receive_room_offer(runtime, session, device_id, device_label, offer).await {
                    Ok(item) => tracing::info!(
                        device = device_label,
                        item = %item.id,
                        "incoming room transfer received"
                    ),
                    Err(error) => tracing::warn!(
                        device = device_label,
                        %error,
                        "incoming room transfer failed"
                    ),
                }
            }
            RoomControlEvent::PeerClosed(_) => return Ok(()),
            RoomControlEvent::LifetimeChanged(_)
            | RoomControlEvent::Pong { .. }
            | RoomControlEvent::VerificationSucceeded => {}
            RoomControlEvent::VerificationRequested | RoomControlEvent::VerificationFailed => {
                bail!("peer attempted device verification after pairing completed");
            }
            RoomControlEvent::OfferAccepted { .. } | RoomControlEvent::OfferRejected { .. } => {
                bail!("peer sent an unexpected decision for an Agent receive-only room");
            }
        }
    }
}

async fn receive_room_offer(
    runtime: &Arc<AgentRuntime>,
    session: &RoomControlSession,
    device_id: &str,
    device_label: &str,
    offer: RoomTransferOffer,
) -> Result<InboxItem> {
    create_directory(&runtime.config.inbox_directory)?;
    let target_directory = fs::canonicalize(&runtime.config.inbox_directory)?;
    let available = api::local_allocatable_bytes(&target_directory)?;
    if offer.total_bytes > api::AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES
        || offer.total_bytes > available / 2
    {
        session
            .reject_offer(&offer.offer_id, RoomOfferRejection::Declined)
            .await?;
        bail!(
            "offer requires explicit approval ({} bytes offered, {} bytes allocatable)",
            offer.total_bytes,
            available
        );
    }

    let bootstrap = api::parse_invitation_for_role(
        &offer.transfer_invite,
        TransferRole::Receiver,
        unix_seconds()?,
    )?
    .into_bootstrap();
    let receive_cancel = TransferCancelToken::new();
    let task_cancel = receive_cancel.clone();
    let task_runtime = runtime.clone();
    let receiver = tokio::spawn(async move {
        tokio::select! {
            result = receive_invitation_offer(&task_runtime, bootstrap, &task_cancel) => result,
            _ = task_runtime.shutdown.cancelled() => {
                task_cancel.cancel();
                Err(anyhow!("Agent shut down while waiting for the transfer sender"))
            }
        }
    });
    tokio::task::yield_now().await;

    if let Err(error) = session.accept_offer(&offer.offer_id).await {
        receive_cancel.cancel();
        let _ = receiver.await;
        return Err(error.into());
    }
    if let Err(error) = session.set_local_transfer_active(true).await {
        receive_cancel.cancel();
        let _ = receiver.await;
        return Err(error.into());
    }
    let pending = match receiver.await.context("transfer receiver task failed") {
        Ok(Ok(pending)) => pending,
        Err(error) => {
            let _ = session.set_local_transfer_active(false).await;
            return Err(error);
        }
        Ok(Err(error)) => {
            let _ = session.set_local_transfer_active(false).await;
            return Err(error);
        }
    };
    let manifest = &pending.offer().manifest;
    let totals = &manifest.totals;
    let item_count = u64::from(totals.file_count) + u64::from(totals.directory_count);
    let expected_root_names = manifest
        .roots
        .iter()
        .take(3)
        .map(|root| root.requested_name.as_str())
        .collect::<Vec<_>>();
    let offer_matches_manifest = u64::from(offer.item_count) == item_count
        && offer.directory_count == totals.directory_count
        && offer.total_bytes == totals.total_plaintext_bytes
        && offer.root_names.len() == expected_root_names.len()
        && offer
            .root_names
            .iter()
            .map(String::as_str)
            .eq(expected_root_names);
    if !offer_matches_manifest {
        pending.reject().await;
        let _ = session.set_local_transfer_active(false).await;
        bail!("authenticated file list did not match the accepted room offer");
    }
    let result = receive_to_inbox(runtime, device_id, device_label, pending).await;
    let inactive = session.set_local_transfer_active(false).await;
    if let Err(error) = inactive {
        return Err(error.into());
    }
    result
}

async fn receive_to_inbox(
    runtime: &AgentRuntime,
    device_id: &str,
    device_label: &str,
    pending: PendingManifestV2Receive,
) -> Result<InboxItem> {
    fs::create_dir_all(&runtime.config.inbox_directory)?;
    let target_directory = fs::canonicalize(&runtime.config.inbox_directory)?;
    let available = api::local_allocatable_bytes(&target_directory)?;
    let manifest = pending.offer().manifest.clone();
    let total = manifest.totals.total_plaintext_bytes;
    let exceptional = total > api::AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES || total > available / 2;
    if exceptional {
        pending.reject().await;
        bail!(
            "offer requires explicit approval ({} bytes offered, {} bytes allocatable)",
            total,
            available
        );
    }
    let summary = pending
        .receive(
            DestinationRequestV2 {
                target_directory,
                copy_staging_directory: None,
                decision: DestinationDecisionV2::UseDirectSave,
                target_allocatable_bytes: Some(available),
                staging_allocatable_bytes: None,
                stable_object_identity: true,
                exceptional_transfer_approved: false,
                preplanned_root_names: None,
            },
            runtime.config.state_directory.join("transfer-state-v2"),
            &runtime.shutdown,
        )
        .await?;
    if summary.saved_root_paths.len() != manifest.roots.len() {
        bail!("receiver completion did not report every saved root");
    }
    let roots = summary
        .saved_root_paths
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .context("saved Inbox root has no UTF-8 file name")?;
            Ok(InboxRoot {
                name: name.to_string(),
                path: path.display().to_string(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let item = InboxItem {
        id: format!(
            "{}_{}",
            URL_SAFE_NO_PAD.encode(manifest.job_id.0),
            manifest.generation
        ),
        received_at_unix_ms: unix_millis()?,
        from_device_id: device_id.to_string(),
        from_device_label: device_label.to_string(),
        roots,
        file_count: manifest.totals.file_count,
        directory_count: manifest.totals.directory_count,
        total_bytes: total,
    };
    lock(&runtime.store)?.append_inbox(item.clone())?;
    Ok(item)
}

struct AgentEvents;

impl EventSink for AgentEvents {
    fn on_event(&self, event: TransferEvent) {
        tracing::debug!(?event, "transfer event");
    }
}

struct SocketCleanup(PathBuf);

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| anyhow!("Agent state lock is poisoned"))
}

fn lock_or_log<'a, T>(mutex: &'a Mutex<T>, label: &str) -> Option<MutexGuard<'a, T>> {
    match mutex.lock() {
        Ok(guard) => Some(guard),
        Err(_) => {
            tracing::error!(label, "Agent state lock is poisoned");
            None
        }
    }
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))
}

fn create_directory(path: &Path) -> io::Result<()> {
    let existed = path.exists();
    fs::create_dir_all(path)?;
    if !existed {
        fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    }
    Ok(())
}

fn spawn_background_task<F>(runtime: &Arc<AgentRuntime>, future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let Some(mut tasks) = lock_or_log(&runtime.background_tasks, "background tasks") else {
        return;
    };
    if runtime.shutdown.is_cancelled() {
        return;
    }
    tasks.retain(|task| !task.is_finished());
    tasks.push(tokio::spawn(future));
}

async fn shutdown_background_tasks(runtime: &AgentRuntime) {
    runtime.shutdown.cancel();
    let tasks = lock_or_log(&runtime.background_tasks, "background tasks")
        .map(|mut tasks| std::mem::take(&mut *tasks))
        .unwrap_or_default();
    let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE_PERIOD;
    for mut task in tasks {
        match tokio::time::timeout_at(deadline, &mut task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) if error.is_cancelled() => {}
            Ok(Err(error)) => tracing::warn!(%error, "Agent background task failed"),
            Err(_) => {
                task.abort();
                let _ = task.await;
            }
        }
    }
}

fn unix_seconds() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs())
}

fn generate_verification_code() -> Result<String> {
    const RANGE: u32 = 1_000_000;
    const UNBIASED_LIMIT: u32 = u32::MAX - (u32::MAX % RANGE);
    loop {
        let value = getrandom::u32()
            .map_err(|error| anyhow!("generate device verification code: {error}"))?;
        if value < UNBIASED_LIMIT {
            return Ok(format!("{:06}", value % RANGE));
        }
    }
}

fn unix_millis() -> Result<u64> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before the Unix epoch")?
            .as_millis(),
    )
    .context("system clock exceeds supported range")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opaque_credential() -> Vec<u8> {
        let mut credential = b"ENVR\x01".to_vec();
        credential.extend_from_slice(&[0x42; 32]);
        credential
    }

    #[test]
    fn agent_uses_a_distinct_identity_for_each_endpoint() {
        assert_eq!(
            agent_client(None).unwrap().identity,
            IdentityConfig::Ephemeral
        );
    }

    #[test]
    fn managed_settings_are_validated_when_loaded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.json");
        fs::write(
            &path,
            br#"{"version":1,"device_name":" WSL","inbox_directory":"/tmp/inbox"}"#,
        )
        .unwrap();

        let error = read_agent_settings(&path).unwrap_err();

        assert!(error.to_string().contains("validate Agent settings"));
    }

    #[tokio::test]
    async fn empty_runtime_reports_status_and_empty_inbox() {
        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state");
        let runtime = Arc::new(AgentRuntime {
            config: RuntimeConfig {
                state_directory: state_directory.clone(),
                inbox_directory: state_directory.join("inbox"),
                socket_path: state_directory.join("agent.sock"),
                device_name: "test-wsl".into(),
                broker: "broker".into(),
                relay: None,
            },
            client: api::Client::default(),
            store: Mutex::new(ProductStore::open(&state_directory).unwrap()),
            active_receivers: Mutex::new(HashMap::new()),
            active_pairings: Mutex::new(HashSet::new()),
            shutdown: TransferCancelToken::new(),
            background_tasks: Mutex::new(Vec::new()),
        });

        let response = handle_request(runtime.clone(), AgentRequest::Status).await;
        let AgentResponse::Status { status } = response else {
            panic!("unexpected response")
        };
        assert_eq!(status.device_name, "test-wsl");
        assert_eq!(status.protocol_version, AGENT_PROTOCOL_VERSION);
        assert_eq!(status.paired_devices, 0);
        assert_eq!(
            handle_request(runtime, AgentRequest::LatestInbox).await,
            AgentResponse::Latest { item: None }
        );
    }

    #[tokio::test]
    async fn forgetting_device_cancels_its_remembered_receiver() {
        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state");
        let mut store = ProductStore::open(&state_directory).unwrap();
        let pending = store.prepare_device("MacBook", "broker", None).unwrap();
        let device_id = pending.id().to_string();
        store
            .commit_device(pending, &opaque_credential(), 0)
            .unwrap();
        let receiver_cancel = TransferCancelToken::new();
        let runtime = Arc::new(AgentRuntime {
            config: RuntimeConfig {
                state_directory: state_directory.clone(),
                inbox_directory: state_directory.join("inbox"),
                socket_path: state_directory.join("agent.sock"),
                device_name: "test-wsl".into(),
                broker: "broker".into(),
                relay: None,
            },
            client: api::Client::default(),
            store: Mutex::new(store),
            active_receivers: Mutex::new(HashMap::from([(
                device_id.clone(),
                receiver_cancel.clone(),
            )])),
            active_pairings: Mutex::new(HashSet::new()),
            shutdown: TransferCancelToken::new(),
            background_tasks: Mutex::new(Vec::new()),
        });

        let response = handle_request(
            runtime.clone(),
            AgentRequest::ForgetDevice {
                device: "macbook".into(),
            },
        )
        .await;

        let AgentResponse::DeviceForgotten { device } = response else {
            panic!("unexpected response")
        };
        assert_eq!(device.id, device_id);
        assert!(receiver_cancel.is_cancelled());
        assert!(lock(&runtime.store).unwrap().devices().is_empty());
    }

    #[tokio::test]
    async fn socket_preparation_never_removes_a_regular_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.sock");
        fs::write(&path, b"keep me").unwrap();

        let error = prepare_socket(&path).await.unwrap_err();

        assert!(error.to_string().contains("refusing to remove"));
        assert_eq!(fs::read(path).unwrap(), b"keep me");
    }

    #[test]
    fn existing_custom_directory_permissions_are_preserved() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();

        create_directory(directory.path()).unwrap();

        assert_eq!(
            fs::metadata(directory.path()).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn verification_code_is_always_six_ascii_digits() {
        for _ in 0..32 {
            let code = generate_verification_code().unwrap();
            assert_eq!(code.len(), 6);
            assert!(code.bytes().all(|byte| byte.is_ascii_digit()));
        }
    }
}
