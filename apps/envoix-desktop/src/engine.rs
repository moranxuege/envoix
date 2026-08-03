//! Drives `envoix-client` on a background tokio runtime and reports progress
//! to the UI thread.
//!
//! The demo covers one route: a directional invitation through the deployed
//! rendezvous, which is what the mobile clients ship with.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{SystemTime, UNIX_EPOCH};

use envoix_client::api::{
    self, CanonicalTransferJob, DestinationDecisionV2, DestinationRequestV2, EventSink,
    InvitationConsumption, PendingManifestV2Receive, TransferEvent, TransferRole,
    acquire_invitation,
};
use envoix_client::{IdentityConfig, TransferCancelToken};
use tokio::sync::oneshot;

/// Deployed broker and relay, matching `Endpoints` in
/// `android/app/src/main/java/dev/envoix/app/TransferRepository.kt`.
pub const BROKER: &str =
    "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445";
pub const RELAY: &str = "https://envoix.chkxwlyh.us:8444";

pub enum UiEvent {
    /// The local side created an invitation and is waiting in its room.
    Invite {
        payload: String,
        room_code: String,
    },
    Status(String),
    Connected(String),
    Offer(OfferSummary),
    Progress {
        bytes: u64,
        total: u64,
    },
    Phase(String),
    Finished {
        entries: usize,
        bytes: u64,
    },
    Failed(String),
}

#[derive(Clone)]
pub struct OfferSummary {
    pub roots: Vec<String>,
    pub files: u64,
    pub directories: u64,
    pub bytes: u64,
}

/// Identifies one transfer for the lifetime of the process. Events carry it so
/// the UI can route them to the right card while several run at once.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TransferId(pub(crate) u64);

/// Per-transfer control the UI needs after the transfer has started.
struct Handle {
    cancel: TransferCancelToken,
    /// Fired by the UI to release a receive that is parked on its offer.
    accept: Option<oneshot::Sender<bool>>,
}

pub struct Engine {
    runtime: tokio::runtime::Runtime,
    events: Receiver<(TransferId, UiEvent)>,
    sink: Sender<(TransferId, UiEvent)>,
    context: egui::Context,
    next_id: u64,
    handles: HashMap<TransferId, Handle>,
}

impl Engine {
    pub fn new(context: egui::Context) -> Self {
        let (sink, events) = channel();
        Self {
            runtime: tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime"),
            events,
            sink,
            context,
            next_id: 0,
            handles: HashMap::new(),
        }
    }

    pub fn poll(&self) -> impl Iterator<Item = (TransferId, UiEvent)> + '_ {
        self.events.try_iter()
    }

    pub fn cancel(&self, transfer: TransferId) {
        if let Some(handle) = self.handles.get(&transfer) {
            handle.cancel.cancel();
        }
    }

    pub fn accept_offer(&mut self, transfer: TransferId) {
        if let Some(handle) = self.handles.get_mut(&transfer)
            && let Some(accept) = handle.accept.take()
        {
            let _ = accept.send(true);
        }
    }

    fn next_transfer(&mut self) -> (TransferId, TransferCancelToken) {
        let id = TransferId(self.next_id);
        self.next_id += 1;
        // Each transfer owns its token: a cancelled one stays cancelled, so a
        // shared token would abort every later transfer the instant it began.
        let cancel = TransferCancelToken::new();
        self.handles.insert(
            id,
            Handle {
                cancel: cancel.clone(),
                accept: None,
            },
        );
        (id, cancel)
    }

    /// Creates a receiver-side invitation, waits for a sender, then parks on the
    /// authenticated offer until the UI accepts it.
    pub fn start_receive(&mut self, save_directory: PathBuf) -> TransferId {
        let (id, cancel) = self.next_transfer();
        let sink = Sink::new(id, self.sink.clone(), self.context.clone());
        let (accept_tx, accept_rx) = oneshot::channel();
        if let Some(handle) = self.handles.get_mut(&id) {
            handle.accept = Some(accept_tx);
        }
        self.runtime.spawn(async move {
            if let Err(error) = receive(save_directory, sink.clone(), cancel, accept_rx).await {
                sink.fail(error);
            }
        });
        id
    }

    /// Joins an invitation payload produced by the peer and sends the selection.
    pub fn start_send(&mut self, files: Vec<PathBuf>, invite: String) -> TransferId {
        let (id, cancel) = self.next_transfer();
        let sink = Sink::new(id, self.sink.clone(), self.context.clone());
        self.runtime.spawn(async move {
            if let Err(error) = send(files, invite, sink.clone(), cancel).await {
                sink.fail(error);
            }
        });
        id
    }
}

/// Consumes the one-time invitation once the peer authenticates, mirroring the
/// CLI's handler.
struct OneTimeInvitation {
    consumption: InvitationConsumption,
}

impl api::AuthenticationHandler for OneTimeInvitation {
    fn on_authenticated(
        &self,
        _outcome: api::AuthenticationOutcome,
    ) -> Result<(), api::SessionError> {
        self.consumption.consume();
        Ok(())
    }
}

#[derive(Clone)]
struct Sink {
    transfer: TransferId,
    sender: Sender<(TransferId, UiEvent)>,
    context: egui::Context,
}

impl Sink {
    fn new(
        transfer: TransferId,
        sender: Sender<(TransferId, UiEvent)>,
        context: egui::Context,
    ) -> Arc<Self> {
        Arc::new(Self {
            transfer,
            sender,
            context,
        })
    }

    fn emit(&self, event: UiEvent) {
        let _ = self.sender.send((self.transfer, event));
        self.context.request_repaint();
    }

    fn status(&self, message: impl Into<String>) {
        self.emit(UiEvent::Status(message.into()));
    }

    fn fail(&self, message: impl Into<String>) {
        self.emit(UiEvent::Failed(message.into()));
    }
}

impl EventSink for Sink {
    fn on_event(&self, event: TransferEvent) {
        match event {
            TransferEvent::Connecting => self.status("Connecting"),
            TransferEvent::Connected { path } => {
                self.emit(UiEvent::Connected(path.to_string()));
            }
            TransferEvent::PathChanged { path } => {
                self.emit(UiEvent::Connected(path.to_string()));
            }
            TransferEvent::Pairing { step } => self.status(format!("Pairing: {step:?}")),
            TransferEvent::Progress {
                bytes_transferred,
                total_bytes,
                ..
            } => self.emit(UiEvent::Progress {
                bytes: bytes_transferred,
                total: total_bytes,
            }),
            TransferEvent::ManifestV2Phase { phase, .. } => {
                self.emit(UiEvent::Phase(phase_label(phase).into()));
            }
            TransferEvent::Diagnostic { .. } | TransferEvent::StageTiming { .. } => {}
        }
    }
}

fn phase_label(phase: api::ManifestV2ProgressPhase) -> &'static str {
    match phase {
        api::ManifestV2ProgressPhase::Transferring => "Transferring",
        api::ManifestV2ProgressPhase::Verifying => "Verifying",
        api::ManifestV2ProgressPhase::Saving => "Saving",
        api::ManifestV2ProgressPhase::WaitingForReceiverSave => "Waiting for receiver to save",
        api::ManifestV2ProgressPhase::FinalizingDelivery => "Finalizing delivery",
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

fn client() -> Result<api::Client, String> {
    let mut client = api::Client::from_runtime_sources(None).map_err(|error| error.to_string())?;
    client.identity = IdentityConfig::Ephemeral;
    Ok(client)
}

fn transfer_options(relay: Option<String>) -> api::TransferOptions {
    let mut options = api::TransferOptions::default();
    options.relay = relay;
    // Escape hatch for venues where hole punching cannot succeed, and the
    // switch that separates a transport problem from an application one: the
    // relay path carries QUIC over the relay's TCP connection instead of the
    // host's UDP socket.
    if std::env::var_os("ENVOIX_DESKTOP_RELAY_ONLY").is_some() {
        options.path = api::PathPolicy::RelayOnly;
    }
    options
}

async fn receive(
    save_directory: PathBuf,
    sink: Arc<Sink>,
    cancel: TransferCancelToken,
    accept: oneshot::Receiver<bool>,
) -> Result<(), String> {
    tokio::fs::create_dir_all(&save_directory)
        .await
        .map_err(|error| format!("cannot create {}: {error}", save_directory.display()))?;
    let target_directory = tokio::fs::canonicalize(&save_directory)
        .await
        .map_err(|error| error.to_string())?;
    let available =
        api::local_allocatable_bytes(&target_directory).map_err(|error| error.to_string())?;

    let created = api::create_invitation(
        BROKER.to_string(),
        vec![RELAY.to_string()],
        TransferRole::Receiver,
        unix_now(),
    )
    .map_err(|error| error.to_string())?;
    sink.emit(UiEvent::Invite {
        payload: created.payload.clone(),
        room_code: created.room_code.to_string(),
    });

    let source = api::PeerSource::invitation(created.into_bootstrap(), BROKER.to_string())
        .map_err(|error| error.to_string())?;
    let api::PeerSource::Invitation { secret_ref, .. } = &source else {
        return Err("invitation source expected".into());
    };
    let lease = acquire_invitation(secret_ref).map_err(|error| error.to_string())?;
    let broker = api::parse_broker_addr(BROKER, Some(RELAY)).map_err(|error| error.to_string())?;
    let authentication = OneTimeInvitation {
        consumption: lease.consumption(),
    };

    sink.status("Waiting for a sender");
    let config = client()?.session_config(&transfer_options(Some(RELAY.to_string())));
    let events: Arc<dyn EventSink> = sink.clone();
    let pending: PendingManifestV2Receive =
        api::receive_manifest_v2_offer_via_room_with_authentication(
            broker,
            lease.bootstrap().clone(),
            envoix_client::BindAddrs::dual_stack(0),
            config,
            events,
            &cancel,
            &authentication,
        )
        .await
        .map_err(|error| error.to_string())?;

    let manifest = &pending.offer().manifest;
    let total = manifest.totals.total_plaintext_bytes;
    sink.emit(UiEvent::Offer(OfferSummary {
        roots: manifest
            .roots
            .iter()
            .map(|root| root.requested_name.clone())
            .collect(),
        files: u64::from(manifest.totals.file_count),
        directories: u64::from(manifest.totals.directory_count),
        bytes: total,
    }));

    if !accept.await.unwrap_or(false) {
        pending.reject().await;
        return Err("offer declined".into());
    }

    let state_directory = target_directory.join(".envoix-state-v2");
    let summary = pending
        .receive(
            DestinationRequestV2 {
                target_directory,
                copy_staging_directory: None,
                decision: DestinationDecisionV2::UseDirectSave,
                target_allocatable_bytes: Some(available),
                staging_allocatable_bytes: None,
                stable_object_identity: true,
                exceptional_transfer_approved: true,
                preplanned_root_names: None,
            },
            state_directory,
            &cancel,
        )
        .await
        .map_err(|error| error.to_string())?;

    sink.emit(UiEvent::Finished {
        entries: summary.data_plane.entry_results.len(),
        bytes: total,
    });
    Ok(())
}

async fn send(
    files: Vec<PathBuf>,
    invite: String,
    sink: Arc<Sink>,
    cancel: TransferCancelToken,
) -> Result<(), String> {
    let validated = api::parse_invitation_for_role(invite.trim(), TransferRole::Sender, unix_now())
        .map_err(|error| error.to_string())?;
    let public = &validated.invitation().public_context;
    let broker = public.broker.clone();
    let relay = public.relay_urls.first().cloned();

    sink.status("Preparing");
    let mut job = CanonicalTransferJob::new(api::CompressionPolicyV2::Smart)
        .map_err(|error| error.to_string())?;
    for file in files {
        job.add_local_path(file)
            .await
            .map_err(|error| error.to_string())?;
    }
    job.prepare_all().await.map_err(|error| error.to_string())?;
    if job.lifecycle() != api::JobLifecycle::ReadyToSend {
        return Err(format!(
            "source preparation needs a decision: {:?}",
            job.source_selections()
        ));
    }
    job.seal_for_send().map_err(|error| error.to_string())?;
    let total = job
        .manifest()
        .map(|manifest| manifest.totals.total_plaintext_bytes)
        .unwrap_or_default();

    let source = api::PeerSource::invitation(validated.into_bootstrap(), broker.clone())
        .map_err(|error| error.to_string())?;
    let api::PeerSource::Invitation { secret_ref, .. } = &source else {
        return Err("invitation source expected".into());
    };
    let lease = acquire_invitation(secret_ref).map_err(|error| error.to_string())?;
    let broker_addr =
        api::parse_broker_addr(&broker, relay.as_deref()).map_err(|error| error.to_string())?;
    let authentication = OneTimeInvitation {
        consumption: lease.consumption(),
    };

    sink.status("Joining the room");
    let config = client()?.session_config(&transfer_options(relay));
    let events: Arc<dyn EventSink> = sink.clone();
    let state_directory = std::env::temp_dir().join("envoix-desktop-send-state");
    let summary = api::send_manifest_v2_via_room_with_authentication(
        broker_addr,
        lease.bootstrap().clone(),
        &job,
        state_directory,
        config,
        events,
        &cancel,
        &authentication,
    )
    .await
    .map_err(|error| error.to_string())?;

    sink.emit(UiEvent::Finished {
        entries: summary.data_plane.entry_results.len(),
        bytes: total,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Waits for a transfer's invitation payload, failing rather than hanging.
    fn await_invite(events: &Receiver<(TransferId, UiEvent)>) -> String {
        for _ in 0..600 {
            while let Ok((_, event)) = events.try_recv() {
                match event {
                    UiEvent::Invite { payload, .. } => return payload,
                    UiEvent::Failed(message) => panic!("receiver failed early: {message}"),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("no invitation within 30s");
    }

    /// The route the UI drives, end to end, through the deployed rendezvous.
    ///
    /// Ignored by default: it is the only test here that needs a host outside
    /// this repository to be alive. Run it deliberately with
    /// `cargo test -p envoix-desktop -- --ignored --nocapture`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "requires the deployed rendezvous to be reachable"]
    async fn a_file_crosses_the_deployed_rendezvous() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let source = workspace.path().join("payload.bin");
        let payload: Vec<u8> = (0..64 * 1024).map(|index| (index % 251) as u8).collect();
        std::fs::write(&source, &payload).expect("write payload");
        let save_directory = workspace.path().join("received");

        let context = egui::Context::default();
        let (receiver_tx, receiver_events) = channel();
        let receiver_sink = Sink::new(TransferId(0), receiver_tx, context.clone());
        let (sender_tx, sender_events) = channel();
        let sender_sink = Sink::new(TransferId(1), sender_tx, context);
        let cancel = TransferCancelToken::new();

        // The offer is accepted unconditionally here; the UI gates it on a click.
        let (accept, accept_rx) = oneshot::channel();
        accept.send(true).expect("arm accept");

        let receiving = tokio::spawn({
            let sink = receiver_sink.clone();
            let cancel = cancel.clone();
            let save_directory = save_directory.clone();
            async move { receive(save_directory, sink, cancel, accept_rx).await }
        });

        let invite = tokio::task::spawn_blocking(move || await_invite(&receiver_events))
            .await
            .expect("invite task");

        let sending = tokio::spawn({
            let sink = sender_sink.clone();
            let cancel = cancel.clone();
            async move { send(vec![source], invite, sink, cancel).await }
        });

        let (sent, received) = tokio::join!(sending, receiving);
        sent.expect("send task").expect("send failed");
        received.expect("receive task").expect("receive failed");

        drop(sender_events);
        let landed = std::fs::read(save_directory.join("payload.bin")).expect("received file");
        assert_eq!(landed, payload, "received bytes differ from the source");
    }

    fn env_path(key: &str) -> PathBuf {
        PathBuf::from(std::env::var(key).unwrap_or_else(|_| panic!("{key} must be set")))
    }

    /// Receiving half of the cross-platform interop pair. Publishes its
    /// invitation to `ENVOIX_INTEROP_INVITE` so a peer in another process, and
    /// potentially on another platform, can join it.
    ///
    /// Run together with `interop_send`; neither half is meaningful alone.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "interop half: pair with interop_send"]
    async fn interop_receive() {
        let invite_file = env_path("ENVOIX_INTEROP_INVITE");
        let save_directory = env_path("ENVOIX_INTEROP_SAVE");

        let (events_tx, events) = channel();
        let sink = Sink::new(TransferId(0), events_tx, egui::Context::default());
        let cancel = TransferCancelToken::new();
        let (accept, accept_rx) = oneshot::channel();
        accept.send(true).expect("arm accept");

        let receiving = tokio::spawn({
            let sink = sink.clone();
            let cancel = cancel.clone();
            let save_directory = save_directory.clone();
            async move { receive(save_directory, sink, cancel, accept_rx).await }
        });

        let invite = tokio::task::spawn_blocking(move || await_invite(&events))
            .await
            .expect("invite task");
        std::fs::write(&invite_file, invite.as_bytes()).expect("publish invite");
        println!("published invite to {}", invite_file.display());

        receiving
            .await
            .expect("receive task")
            .expect("receive failed");
        println!("receive completed");
    }

    /// Sending half of the interop pair. Waits for `interop_receive` to publish
    /// its invitation, then sends `ENVOIX_INTEROP_SOURCE` into it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "interop half: pair with interop_receive"]
    async fn interop_send() {
        let invite_file = env_path("ENVOIX_INTEROP_INVITE");
        let source = env_path("ENVOIX_INTEROP_SOURCE");

        let invite = tokio::task::spawn_blocking(move || {
            for _ in 0..600 {
                if let Ok(text) = std::fs::read_to_string(&invite_file)
                    && text.starts_with("envoix://")
                {
                    return text.trim().to_owned();
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            panic!("peer published no invitation within 30s");
        })
        .await
        .expect("invite task");

        let (events_tx, _events) = channel();
        let sink = Sink::new(TransferId(0), events_tx, egui::Context::default());
        let cancel = TransferCancelToken::new();
        send(vec![source], invite, sink, cancel)
            .await
            .expect("send failed");
        println!("send completed");
    }

    /// The platform filesystem primitives the receiver's save path depends on.
    ///
    /// Isolates a port problem in `std::fs` from one in the transfer logic:
    /// `symlink_metadata` on Windows inspects reparse data, and the destination
    /// planner canonicalises both the target and each saved entry.
    #[test]
    fn platform_filesystem_primitives_work() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let file = workspace.path().join("probe.bin");
        std::fs::write(&file, b"probe").expect("write probe");

        assert!(
            std::fs::symlink_metadata(&file)
                .expect("symlink_metadata on a file")
                .is_file()
        );
        assert!(
            std::fs::symlink_metadata(workspace.path())
                .expect("symlink_metadata on a directory")
                .is_dir()
        );
        std::fs::canonicalize(&file).expect("canonicalize a file");
        std::fs::canonicalize(workspace.path()).expect("canonicalize a directory");
    }

    /// Waits for one engine to publish an invitation for `transfer`.
    fn wait_for_invite(engine: &mut Engine, transfer: TransferId) -> Result<String, String> {
        for _ in 0..600 {
            for (id, event) in engine.poll().collect::<Vec<_>>() {
                if id != transfer {
                    continue;
                }
                match event {
                    UiEvent::Invite { payload, .. } => return Ok(payload),
                    UiEvent::Failed(message) => return Err(message),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err("no invitation within 30s".into())
    }

    /// Drains both engines once, reporting whether payload moved and how many
    /// sides ended, split by outcome so a cancel cannot be confused with a
    /// completion that beat it. Releases any offer the way the Accept button does.
    #[derive(Default)]
    struct Drained {
        moved: bool,
        failed: usize,
        finished: usize,
    }

    impl Drained {
        fn terminal(&self) -> usize {
            self.failed + self.finished
        }
    }

    fn drain(receiver: &mut Engine, sender: &mut Engine) -> Drained {
        let mut drained = Drained::default();
        let events: Vec<(TransferId, UiEvent)> = receiver
            .poll()
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(id, event)| (id, event, true))
            .chain(
                sender
                    .poll()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|(id, event)| (id, event, false)),
            )
            .map(|(id, event, _)| (id, event))
            .collect();
        for (id, event) in events {
            match event {
                UiEvent::Progress { bytes, .. } if bytes > 0 => drained.moved = true,
                UiEvent::Offer(_) => receiver.accept_offer(id),
                UiEvent::Failed(_) => drained.failed += 1,
                UiEvent::Finished { .. } => drained.finished += 1,
                _ => {}
            }
        }
        drained
    }

    fn drive_round(receiver: &mut Engine, sender: &mut Engine) -> Result<(), String> {
        let mut terminal = 0;
        for _ in 0..1200 {
            let drained = drain(receiver, sender);
            if drained.failed > 0 {
                return Err("a side failed".into());
            }
            terminal += drained.terminal();
            if terminal >= 2 {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err("round did not finish within 60s".into())
    }

    /// A demo does not stop after one transfer. This drives two rounds through
    /// the same pair of engines, which is where reused state bites: a cancel
    /// token, once cancelled, stays cancelled, and the accept channel is
    /// single-use.
    #[test]
    #[ignore = "requires the deployed rendezvous to be reachable"]
    fn two_transfers_in_a_row() {
        let context = egui::Context::default();
        let mut receiver = Engine::new(context.clone());
        let mut sender = Engine::new(context);
        let workspace = tempfile::tempdir().expect("tempdir");

        for round in 0..2_u8 {
            let save_directory = workspace.path().join(format!("recv-{round}"));
            let source = workspace.path().join(format!("source-{round}.bin"));
            let payload: Vec<u8> = (0..32 * 1024)
                .map(|index| ((index + usize::from(round)) % 251) as u8)
                .collect();
            std::fs::write(&source, &payload).expect("write source");

            let incoming = receiver.start_receive(save_directory.clone());
            let invite = wait_for_invite(&mut receiver, incoming)
                .unwrap_or_else(|error| panic!("round {round}: {error}"));
            sender.start_send(vec![source], invite);
            drive_round(&mut receiver, &mut sender)
                .unwrap_or_else(|error| panic!("round {round}: {error}"));

            let landed = std::fs::read(save_directory.join(format!("source-{round}.bin")))
                .unwrap_or_else(|error| panic!("round {round} received file: {error}"));
            assert_eq!(landed, payload, "round {round} bytes differ");
        }
    }

    /// Two transfers running at the same time, which is what the card list
    /// exists for. Each must land its own bytes; a shared cancel token or a
    /// mis-routed event would cross them over.
    #[test]
    #[ignore = "requires the deployed rendezvous to be reachable"]
    fn two_transfers_at_once() {
        let context = egui::Context::default();
        let mut receiver = Engine::new(context.clone());
        let mut sender = Engine::new(context);
        let workspace = tempfile::tempdir().expect("tempdir");

        let mut expected = Vec::new();
        for lane in 0..2_u8 {
            let save_directory = workspace.path().join(format!("lane-{lane}"));
            let source = workspace.path().join(format!("lane-{lane}.bin"));
            // Distinct contents, so a crossed transfer fails the comparison.
            let payload: Vec<u8> = (0..48 * 1024)
                .map(|index| ((index * 7 + usize::from(lane) * 101) % 251) as u8)
                .collect();
            std::fs::write(&source, &payload).expect("write source");

            let incoming = receiver.start_receive(save_directory.clone());
            let invite = wait_for_invite(&mut receiver, incoming)
                .unwrap_or_else(|error| panic!("lane {lane}: {error}"));
            sender.start_send(vec![source], invite);
            expected.push((save_directory.join(format!("lane-{lane}.bin")), payload));
        }

        // Both lanes are in flight together; wait for four terminal events.
        let mut terminal = 0;
        for _ in 0..2400 {
            let drained = drain(&mut receiver, &mut sender);
            assert_eq!(drained.failed, 0, "a concurrent lane failed");
            terminal += drained.terminal();
            if terminal >= 4 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(terminal >= 4, "concurrent lanes did not both finish");

        for (path, payload) in expected {
            let landed =
                std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            assert_eq!(
                landed,
                payload,
                "{} received the wrong bytes",
                path.display()
            );
        }
    }

    /// A demo that goes wrong has to recover. Cancelling mid-flight must end
    /// both sides rather than hang, and must not poison the engines: a cancelled
    /// `TransferCancelToken` stays cancelled, so a stale one would abort every
    /// later attempt the instant it started.
    #[test]
    #[ignore = "requires the deployed rendezvous to be reachable"]
    fn cancelling_mid_transfer_leaves_the_engines_usable() {
        let context = egui::Context::default();
        let mut receiver = Engine::new(context.clone());
        let mut sender = Engine::new(context);
        let workspace = tempfile::tempdir().expect("tempdir");

        let source = workspace.path().join("large.bin");
        std::fs::write(&source, vec![7_u8; 32 * 1024 * 1024]).expect("write source");
        let incoming = receiver.start_receive(workspace.path().join("abandoned"));
        let invite = wait_for_invite(&mut receiver, incoming).expect("first invitation");
        let outgoing = sender.start_send(vec![source], invite);

        let mut moved = false;
        let mut early_finished = 0;
        for _ in 0..1800 {
            let drained = drain(&mut receiver, &mut sender);
            moved |= drained.moved;
            early_finished += drained.finished;
            if moved {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(moved, "no payload moved, so there was nothing to cancel");
        assert_eq!(
            early_finished, 0,
            "the transfer completed before a cancel could be issued; \
             raise the payload size so the cancel lands mid-flight"
        );

        sender.cancel(outgoing);
        receiver.cancel(incoming);

        let mut terminal = 0;
        let mut failed = 0;
        for _ in 0..1200 {
            let drained = drain(&mut receiver, &mut sender);
            terminal += drained.terminal();
            failed += drained.failed;
            if terminal >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            terminal >= 2,
            "cancelled transfer never settled on both sides (saw {terminal})"
        );
        assert!(
            failed >= 1,
            "both sides reported success despite the cancel, so nothing was cancelled"
        );

        // The engines must still be good for a fresh round.
        let save_directory = workspace.path().join("after-cancel");
        let source = workspace.path().join("small.bin");
        let payload: Vec<u8> = (0..32 * 1024).map(|index| (index % 251) as u8).collect();
        std::fs::write(&source, &payload).expect("write source");
        let incoming = receiver.start_receive(save_directory.clone());
        let invite = wait_for_invite(&mut receiver, incoming).expect("second invitation");
        sender.start_send(vec![source], invite);
        drive_round(&mut receiver, &mut sender).expect("transfer after a cancel");

        let landed = std::fs::read(save_directory.join("small.bin")).expect("received file");
        assert_eq!(landed, payload, "bytes differ after a cancelled round");
    }
}
