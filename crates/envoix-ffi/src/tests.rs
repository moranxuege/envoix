use super::*;
use envoix_qr::QrInvitePayload;
use envoix_rendezvous::RoomRegistry;
use envoix_rendezvous_iroh::{build_endpoint, endpoint_addr, serve_endpoint};
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
use std::io::ErrorKind;
use std::net::UdpSocket;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock, Weak};
use std::thread;
use std::time::Duration;

enum Msg {
    Invite(String),
    Completed(u64),
    Failed(String),
    Event(FfiTransferEvent),
    Activity(FfiTransferActivityRecord),
}

enum ManifestMsg {
    Event(Box<FfiTransferEvent>),
    Activity(Box<FfiManifestActivityRecord>),
}

async fn ready_addr(ep: &Endpoint) -> EndpointAddr {
    for _ in 0..100 {
        if ep.addr().ip_addrs().next().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    endpoint_addr(ep)
}

struct TestObserver(Sender<Msg>);

impl TransferObserver for TestObserver {
    fn on_invite_ready(&self, invite: String) {
        let _ = self.0.send(Msg::Invite(invite));
    }
    fn on_started(&self, _file_name: String, _total_bytes: u64) {}
    fn on_progress(&self, _transferred: u64, _total: u64) {}
    fn on_completed(&self, bytes: u64) {
        let _ = self.0.send(Msg::Completed(bytes));
    }
    fn on_transfer_failed(&self, _failure: FfiTransferFailure) {}
    fn on_failed(&self, reason: String) {
        let _ = self.0.send(Msg::Failed(reason));
    }
    fn on_transfer_event(&self, event: FfiTransferEvent) {
        let _ = self.0.send(Msg::Event(event));
    }
    fn on_transfer_activity(&self, record: FfiTransferActivityRecord) {
        let _ = self.0.send(Msg::Activity(record));
    }
    fn on_status(&self, _message: String) {}
}

struct TestManifestObserver(Sender<ManifestMsg>);

impl ManifestTransferObserverV2 for TestManifestObserver {
    fn on_manifest_event(&self, event: FfiTransferEvent) {
        let _ = self.0.send(ManifestMsg::Event(Box::new(event)));
    }

    fn on_manifest_activity(&self, record: FfiManifestActivityRecord) {
        let _ = self.0.send(ManifestMsg::Activity(Box::new(record)));
    }
}

struct NoopMailbox;

impl MailboxObserver for NoopMailbox {
    fn on_fetch_receipt(&self, _activity_id: String, _key: String) {}

    fn on_post_receipt(&self, _activity_id: String, _key: String, _blob: Vec<u8>) {}
}

struct NoopMailboxV2;

impl MailboxObserverV2 for NoopMailboxV2 {
    fn on_fetch_receipt(&self, _activity_id: String, _key: String, _server: Option<String>) {}

    fn on_post_receipt(
        &self,
        _activity_id: String,
        _key: String,
        _blob: Vec<u8>,
        _server: Option<String>,
    ) {
    }
}

enum MailboxMsg {
    Fetch,
    Post {
        activity_id: String,
        key: String,
        blob: Vec<u8>,
    },
}

struct TestMailbox(Sender<MailboxMsg>);

impl MailboxObserver for TestMailbox {
    fn on_fetch_receipt(&self, _activity_id: String, _key: String) {
        let _ = self.0.send(MailboxMsg::Fetch);
    }

    fn on_post_receipt(&self, activity_id: String, key: String, blob: Vec<u8>) {
        let _ = self.0.send(MailboxMsg::Post {
            activity_id,
            key,
            blob,
        });
    }
}

enum MailboxV2Msg {
    Fetch {
        activity_id: String,
        key: String,
        server: Option<String>,
    },
    Post,
}

struct TestMailboxV2(Sender<MailboxV2Msg>);

impl MailboxObserverV2 for TestMailboxV2 {
    fn on_fetch_receipt(&self, activity_id: String, key: String, server: Option<String>) {
        let _ = self.0.send(MailboxV2Msg::Fetch {
            activity_id,
            key,
            server,
        });
    }

    fn on_post_receipt(
        &self,
        _activity_id: String,
        _key: String,
        _blob: Vec<u8>,
        _server: Option<String>,
    ) {
        let _ = self.0.send(MailboxV2Msg::Post);
    }
}

struct PauseOnProgressObserver {
    messages: Sender<Msg>,
    session: Weak<EnvoixSession>,
    activity_id: String,
    pause_result: Sender<bool>,
    requested: std::sync::atomic::AtomicBool,
}

impl TransferObserver for PauseOnProgressObserver {
    fn on_invite_ready(&self, invite: String) {
        let _ = self.messages.send(Msg::Invite(invite));
    }
    fn on_started(&self, _file_name: String, _total_bytes: u64) {}
    fn on_progress(&self, _transferred: u64, _total: u64) {
        if !self.requested.swap(true, Ordering::SeqCst) {
            let accepted = self
                .session
                .upgrade()
                .is_some_and(|session| session.pause_activity(self.activity_id.clone()));
            let _ = self.pause_result.send(accepted);
        }
    }
    fn on_completed(&self, bytes: u64) {
        let _ = self.messages.send(Msg::Completed(bytes));
    }
    fn on_transfer_failed(&self, _failure: FfiTransferFailure) {}
    fn on_failed(&self, reason: String) {
        let _ = self.messages.send(Msg::Failed(reason));
    }
    fn on_transfer_event(&self, event: FfiTransferEvent) {
        let _ = self.messages.send(Msg::Event(event));
    }
    fn on_transfer_activity(&self, record: FfiTransferActivityRecord) {
        let _ = self.messages.send(Msg::Activity(record));
    }
    fn on_status(&self, _message: String) {}
}

struct DurablePauseOnProgressObserver {
    messages: Sender<Msg>,
    session: Mutex<Option<Weak<DurableEnvoixSession>>>,
    result: Sender<bool>,
    requested: std::sync::atomic::AtomicBool,
}

impl TransferObserver for DurablePauseOnProgressObserver {
    fn on_invite_ready(&self, invite: String) {
        let _ = self.messages.send(Msg::Invite(invite));
    }

    fn on_started(&self, _file_name: String, _total_bytes: u64) {}

    fn on_progress(&self, _transferred: u64, _total: u64) {
        if self.requested.load(Ordering::SeqCst) {
            return;
        }
        let accepted = self
            .session
            .lock()
            .unwrap()
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|session| session.pause());
        if accepted && !self.requested.swap(true, Ordering::SeqCst) {
            let _ = self.result.send(true);
        }
    }

    fn on_completed(&self, bytes: u64) {
        let _ = self.messages.send(Msg::Completed(bytes));
    }

    fn on_transfer_failed(&self, _failure: FfiTransferFailure) {}

    fn on_failed(&self, reason: String) {
        let _ = self.messages.send(Msg::Failed(reason));
    }

    fn on_transfer_event(&self, event: FfiTransferEvent) {
        let _ = self.messages.send(Msg::Event(event));
    }

    fn on_transfer_activity(&self, record: FfiTransferActivityRecord) {
        let _ = self.messages.send(Msg::Activity(record));
    }

    fn on_status(&self, _message: String) {}
}

static LOOPBACK_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn lock_loopback_tests() -> Option<std::sync::MutexGuard<'static, ()>> {
    if !loopback_transport_available() {
        return None;
    }
    LOOPBACK_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap()
        .into()
}

fn recv_invite(rx: &std::sync::mpsc::Receiver<Msg>, timeout: Duration) -> String {
    loop {
        match rx.recv_timeout(timeout).unwrap() {
            Msg::Invite(invite) => return invite,
            Msg::Failed(reason) => panic!("transfer failed before invite: {reason}"),
            Msg::Completed(_) => panic!("transfer completed before invite"),
            Msg::Event(_) => {}
            Msg::Activity(_) => {}
        }
    }
}

fn loopback_transport_available() -> bool {
    match UdpSocket::bind(("127.0.0.1", 0)) {
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            println!("skipping loopback/queue tests: UDP bind permission denied ({error})");
            false
        }
        Err(error) => panic!("transport pre-check failed: {error}"),
    }
}

fn recv_completed(
    rx: &std::sync::mpsc::Receiver<Msg>,
    timeout: Duration,
) -> (u64, Vec<FfiTransferEvent>) {
    let mut events = Vec::new();
    loop {
        match rx.recv_timeout(timeout).unwrap() {
            Msg::Completed(bytes) => return (bytes, events),
            Msg::Failed(reason) => panic!("transfer failed: {reason}"),
            Msg::Invite(_) => {}
            Msg::Event(event) => events.push(event),
            Msg::Activity(record) => {
                if record.state == FfiTransferActivityState::Failed {
                    panic!("transfer failed: {}", record.diagnostic_message);
                }
            }
        }
    }
}

fn recv_completed_activity(
    rx: &std::sync::mpsc::Receiver<Msg>,
    timeout: Duration,
) -> (u64, Vec<FfiTransferEvent>, FfiTransferActivityRecord) {
    let mut events = Vec::new();
    let mut completed_activity = None;
    let mut completed_activity_count = 0;
    loop {
        match rx.recv_timeout(timeout).unwrap() {
            Msg::Completed(bytes) => {
                assert_eq!(
                    completed_activity_count, 1,
                    "receive should publish exactly one terminal completed activity"
                );
                return (
                    bytes,
                    events,
                    completed_activity.expect("completed activity should precede callback"),
                );
            }
            Msg::Failed(reason) => panic!("transfer failed: {reason}"),
            Msg::Invite(_) => {}
            Msg::Event(event) => events.push(event),
            Msg::Activity(record) => match record.state {
                FfiTransferActivityState::Completed => {
                    assert!(
                        !record.completed_file_path.is_empty(),
                        "completed receive activity must include its committed file path"
                    );
                    completed_activity_count += 1;
                    completed_activity = Some(record);
                }
                FfiTransferActivityState::Failed => {
                    panic!("transfer failed: {}", record.diagnostic_message)
                }
                _ => {}
            },
        }
    }
}

fn recv_manifest_invite(rx: &std::sync::mpsc::Receiver<ManifestMsg>, timeout: Duration) -> String {
    loop {
        match rx.recv_timeout(timeout).unwrap() {
            ManifestMsg::Event(_) => {}
            ManifestMsg::Activity(record) => {
                if record.activity.state == FfiTransferActivityState::Failed {
                    panic!("transfer failed: {}", record.activity.diagnostic_message);
                }
                if !record.activity.invite.is_empty() {
                    return record.activity.invite;
                }
            }
        }
    }
}

fn recv_completed_manifest_activity(
    rx: &std::sync::mpsc::Receiver<ManifestMsg>,
    timeout: Duration,
) -> (Vec<FfiTransferEvent>, FfiManifestActivityRecord) {
    let mut events = Vec::new();
    loop {
        match rx.recv_timeout(timeout).unwrap() {
            ManifestMsg::Event(event) => events.push(*event),
            ManifestMsg::Activity(record) => match record.activity.state {
                FfiTransferActivityState::Completed => return (events, *record),
                FfiTransferActivityState::Failed | FfiTransferActivityState::Canceled => {
                    panic!("transfer failed: {}", record.activity.diagnostic_message)
                }
                _ => {}
            },
        }
    }
}

fn recv_activity(
    rx: &std::sync::mpsc::Receiver<Msg>,
    activity_id: &str,
    timeout: Duration,
) -> FfiTransferActivityRecord {
    loop {
        match rx.recv_timeout(timeout).unwrap() {
            Msg::Activity(record) if record.activity_id == activity_id => return record,
            Msg::Failed(reason) => panic!("transfer failed: {reason}"),
            Msg::Invite(_) | Msg::Completed(_) | Msg::Event(_) | Msg::Activity(_) => {}
        }
    }
}

fn recv_activity_state(
    rx: &std::sync::mpsc::Receiver<Msg>,
    activity_id: &str,
    state: FfiTransferActivityState,
    timeout: Duration,
) -> FfiTransferActivityRecord {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .expect("timed out waiting for activity state");
        match rx.recv_timeout(remaining).unwrap() {
            Msg::Activity(record) if record.activity_id == activity_id && record.state == state => {
                return record;
            }
            Msg::Failed(reason) => panic!("transfer failed: {reason}"),
            Msg::Invite(_) | Msg::Completed(_) | Msg::Event(_) | Msg::Activity(_) => {}
        }
    }
}

fn start_test_broker() -> String {
    let (broker_tx, broker_rx) = channel();
    let _server = thread::spawn(move || {
        let runtime = Runtime::new().unwrap();
        runtime.block_on(async move {
            let server = build_endpoint(
                "127.0.0.1:0".parse().unwrap(),
                SecretKey::generate(),
                RelayMode::Disabled,
            )
            .await
            .unwrap();
            let server_id = server.id();
            let server_addr = *ready_addr(&server)
                .await
                .ip_addrs()
                .next()
                .expect("server should have a direct address");
            broker_tx
                .send(format!("{server_id}@{server_addr}"))
                .unwrap();
            serve_endpoint(server, Arc::new(RoomRegistry::new()), None)
                .await
                .unwrap();
        });
    });
    broker_rx.recv_timeout(Duration::from_secs(10)).unwrap()
}

fn assert_no_nonqueued_activity(
    rx: &std::sync::mpsc::Receiver<Msg>,
    activity_id: &str,
    timeout: Duration,
) {
    let deadline = std::time::Instant::now() + timeout;
    while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(Msg::Activity(record))
                if record.activity_id == activity_id
                    && record.state != FfiTransferActivityState::Queued =>
            {
                panic!("queued activity started unexpectedly: {:?}", record.state);
            }
            Ok(Msg::Failed(reason)) => panic!("transfer failed: {reason}"),
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn snapshot_record(
    session: &EnvoixSession,
    activity_id: &str,
) -> Option<FfiTransferActivityRecord> {
    session
        .list_transfer_activities()
        .into_iter()
        .find(|record| record.activity_id == activity_id)
}

#[test]
fn pairing_invite_payload_round_trips_for_native_clients() {
    let invite = make_pairing_invite(
        FfiInviteRole::Receive,
        "id@127.0.0.1:8445".to_string(),
        "https://relay.example".to_string(),
    )
    .unwrap();

    assert!(invite.payload.starts_with("envoix://pair/"));
    assert_eq!(invite.role, FfiInviteRole::Receive);
    assert_eq!(invite.broker, "id@127.0.0.1:8445");
    assert_eq!(invite.relay, "https://relay.example");

    let parsed = parse_pairing_invite(invite.payload).unwrap();
    assert_eq!(parsed.code, invite.code);
    assert_eq!(parsed.broker, "id@127.0.0.1:8445");
    assert_eq!(parsed.relay, "https://relay.example");
    assert_eq!(parsed.role, FfiInviteRole::Receive);
}

#[test]
fn pairing_invite_uses_hosted_defaults_when_settings_are_blank() {
    let invite = make_pairing_invite(FfiInviteRole::Send, String::new(), String::new()).unwrap();
    assert_eq!(invite.broker, DEFAULT_RENDEZVOUS_BROKER);
    assert_eq!(invite.relay, DEFAULT_RELAY_URL);

    let parsed = parse_pairing_invite(invite.code.clone()).unwrap();
    assert_eq!(parsed.code, invite.code);
    assert!(parsed.broker.is_empty());
    assert!(parsed.relay.is_empty());
    assert_eq!(parsed.role, FfiInviteRole::Unknown);
}

#[test]
fn custom_pairing_broker_does_not_force_default_relay() {
    let invite = make_pairing_invite(
        FfiInviteRole::Unknown,
        "custom@10.0.0.1:8445".to_string(),
        String::new(),
    )
    .unwrap();
    assert_eq!(invite.broker, "custom@10.0.0.1:8445");
    assert!(invite.relay.is_empty());
    assert_eq!(invite.role, FfiInviteRole::Unknown);
}

#[test]
fn pairing_invite_rejects_legacy_direct_invites() {
    let err = parse_pairing_invite("envoix:legacy-direct-payload".to_string()).unwrap_err();
    assert!(
        err.to_string()
            .contains("unsupported Envoix pairing invite scheme")
    );
}

#[test]
fn pairing_invite_rejects_non_envoix_qr_payloads() {
    let err =
        parse_pairing_invite("https://example.com/not-an-envoix-code".to_string()).unwrap_err();
    assert!(err.to_string().contains("pairing code must have the form"));
}

#[test]
fn native_progress_is_rate_limited_but_final_progress_is_delivered() {
    let mut last_progress_ms = 0;
    let mut progress = FfiTransferEvent::empty("activity-1", 1_000);
    progress.kind = FfiTransferEventKind::Progress;
    progress.bytes_transferred = 10;
    progress.total_bytes = 100;
    assert!(should_emit_native_event(&progress, &mut last_progress_ms));

    progress.ts_ms = 1_200;
    progress.bytes_transferred = 20;
    assert!(!should_emit_native_event(&progress, &mut last_progress_ms));

    progress.ts_ms = 1_500;
    progress.bytes_transferred = 30;
    assert!(should_emit_native_event(&progress, &mut last_progress_ms));

    progress.ts_ms = 1_510;
    progress.bytes_transferred = 100;
    assert!(should_emit_native_event(&progress, &mut last_progress_ms));

    let completed = FfiTransferEvent::empty("activity-1", 1_511);
    assert!(should_emit_native_event(&completed, &mut last_progress_ms));
}

#[test]
fn manifest_progress_maps_to_a_structured_native_event() {
    let event = StampedEvent {
        ts_ms: 1_234,
        event: TransferEvent::ManifestProgress {
            manifest_id: envoix_client::ManifestId::new("manifest-1"),
            entry_id: 7,
            entry_bytes: 20,
            entry_total_bytes: 40,
            completed_bytes: 120,
            total_bytes: 200,
        },
    };

    let projected = to_ffi_event(&event, "activity-1");

    assert_eq!(projected.kind, FfiTransferEventKind::Progress);
    assert_eq!(projected.activity_id, "activity-1");
    assert_eq!(projected.transfer_id, "manifest-1");
    assert_eq!(projected.bytes_transferred, 120);
    assert_eq!(projected.total_bytes, 200);
    assert!(projected.diagnostic_message.contains("entry_id=7"));
    assert!(projected.diagnostic_message.contains("entry_bytes=20/40"));
}

#[test]
fn receive_verification_projects_existing_bytes_as_resumed() {
    let receive = StampedEvent {
        ts_ms: 1_234,
        event: TransferEvent::Verified {
            transfer_id: TransferId::new("transfer-existing"),
            direction: TransferDirection::Receive,
            file_name: "existing.bin".to_string(),
            bytes_hashed: 353_224,
        },
    };
    let receive = to_ffi_event(&receive, "activity-receive");
    assert_eq!(receive.kind, FfiTransferEventKind::Verified);
    assert_eq!(receive.bytes_transferred, 353_224);
    assert_eq!(receive.total_bytes, 353_224);
    assert_eq!(receive.bytes_resumed, 353_224);

    let send = StampedEvent {
        ts_ms: 1_235,
        event: TransferEvent::Verified {
            transfer_id: TransferId::new("transfer-send-hash"),
            direction: TransferDirection::Send,
            file_name: "source.bin".to_string(),
            bytes_hashed: 353_224,
        },
    };
    let send = to_ffi_event(&send, "activity-send");
    assert_eq!(send.bytes_resumed, 0);
}

#[test]
fn activity_record_folds_transfer_events() {
    let request = FfiTransferRequest {
        activity_id: "activity-1".to_string(),
        direction: FfiTransferDirection::Send,
        mode: FfiTransferMode::Room,
        file_path: "/tmp/report.pdf".to_string(),
        output_dir: String::new(),
        peer_descriptor: String::new(),
        invite: String::new(),
        code: "135790-amber-comet".to_string(),
        token: String::new(),
        broker: String::new(),
        relay: String::new(),
        config_path: String::new(),
        path_policy: FfiPathPolicy::Auto,
        resume: true,
        publication_required: false,
        limits: FfiTransferLimits {
            max_parallel_transfers: 2,
            ..FfiTransferLimits::default()
        },
        rendezvous: FfiRendezvousPlan::default(),
    };
    let mut record = make_transfer_activity_record(request);
    assert_eq!(record.activity_id, "activity-1");
    assert!(record.attempt_id.is_empty());
    assert_eq!(record.state, FfiTransferActivityState::Queued);
    assert_eq!(record.file_name, "report.pdf");
    assert_eq!(record.limits.max_parallel_transfers, 2);

    let mut started = FfiTransferEvent::empty("activity-1", 10);
    started.kind = FfiTransferEventKind::Started;
    started.direction = FfiTransferDirection::Send;
    started.transfer_id = "tx1".to_string();
    started.file_name = "report.pdf".to_string();
    started.total_bytes = 100;
    record = fold_transfer_activity(record, started);
    assert_eq!(record.state, FfiTransferActivityState::Transferring);
    assert_eq!(record.started_at_ms, 10);
    assert_eq!(record.transfer_id, "tx1");

    let mut completed = FfiTransferEvent::empty("activity-1", 20);
    completed.kind = FfiTransferEventKind::Completed;
    completed.transfer_id = "tx1".to_string();
    completed.bytes_transferred = 100;
    record = fold_transfer_activity(record, completed);
    assert_eq!(record.state, FfiTransferActivityState::Verifying);
    assert_eq!(record.bytes_transferred, 100);
    assert_eq!(record.completed_at_ms, 0);

    record.apply_completed(
        &TransferSummary {
            bytes_transferred: 100,
            transfer_id: TransferId::new("tx1"),
            file_name: "report.pdf".to_string(),
            file_hash: blake3::hash(b"report").to_hex().to_string(),
        },
        21,
        "/tmp/report.pdf".to_string(),
    );
    assert_eq!(record.state, FfiTransferActivityState::Completed);
    assert_eq!(record.completed_at_ms, 21);
    assert_eq!(record.completed_file_path, "/tmp/report.pdf");
}

#[test]
fn confirming_activity_is_finalizing_and_rejects_stop_requests() {
    let activity_id = "confirming-activity".to_string();
    let mut request =
        FfiTransferRequest::send("/tmp/report.pdf".to_string(), FfiTransferMode::Room);
    request.activity_id = activity_id.clone();
    let mut activity = make_transfer_activity_record(request);
    activity.state = FfiTransferActivityState::Verifying;
    activity.diagnostic_message = "confirming".to_string();
    assert!(is_finalizing_activity(&activity));
    assert!(!can_pause_durable_activity(&activity));
    assert!(!can_cancel_durable_activity(&activity));
    assert_eq!(
        transfer_activity_actions(activity.clone()),
        FfiTransferActivityActions {
            can_pause: false,
            can_resume: false,
            can_cancel: false,
            can_delete: false,
            is_finalizing: true,
        }
    );

    let session = EnvoixSession::new_with_settings(EnvoixRuntimeSettings::default());
    let (control, _receiver) = oneshot::channel();
    session.queue.lock().unwrap().active.insert(
        activity_id.clone(),
        ActiveTransfer {
            control: Some(control),
            limit: 1,
            activity,
        },
    );

    assert!(!session.cancel_activity(activity_id.clone()));
    assert!(!session.pause_activity(activity_id));
}

#[test]
fn durable_controls_only_accept_legal_lifecycle_states() {
    let request =
        FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::ShowInvite);
    let mut activity = make_transfer_activity_record(request);
    assert!(can_pause_durable_activity(&activity));
    assert!(can_cancel_durable_activity(&activity));
    assert!(!can_resume_durable_activity(&activity));

    activity.apply_paused(now_ms());
    assert!(!can_pause_durable_activity(&activity));
    assert!(can_cancel_durable_activity(&activity));
    assert!(can_resume_durable_activity(&activity));

    activity.state = FfiTransferActivityState::Completed;
    assert!(!can_pause_durable_activity(&activity));
    assert!(!can_cancel_durable_activity(&activity));
    assert!(!can_resume_durable_activity(&activity));
}

#[test]
fn native_activity_actions_use_structured_retryability() {
    let request =
        FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::ShowInvite);
    let mut activity = make_transfer_activity_record(request);
    activity.state = FfiTransferActivityState::Publishing;
    activity.diagnostic_message = "publish failed: legacy display text".to_string();

    let unavailable = transfer_activity_actions(activity.clone());
    assert!(!unavailable.can_resume);
    assert!(!unavailable.can_cancel);

    activity.retryable = true;
    let retryable = transfer_activity_actions(activity);
    assert!(retryable.can_resume);
    assert!(retryable.can_cancel);
    assert!(!retryable.can_pause);
    assert!(!retryable.can_delete);
    assert!(!retryable.is_finalizing);
}

#[test]
fn core_info_reports_versioned_native_capabilities() {
    let info = envoix_core_info();
    assert_eq!(info.ffi_api_version, ENVOIX_FFI_API_VERSION);
    assert_eq!(info.core_version, env!("CARGO_PKG_VERSION"));
    assert!(
        info.capabilities
            .contains(&"activity_actions_v1".to_string())
    );
    assert!(
        info.capabilities
            .contains(&"per_session_receipt_endpoint_v1".to_string())
    );
    assert_eq!(
        normalized_receipt_server(" https://receipt.example.test:8460/ ").unwrap(),
        Some("https://receipt.example.test:8460".to_string())
    );
    assert!(normalized_receipt_server("file:///tmp/receipt").is_err());
}

#[test]
fn canonical_activity_preserves_structured_network_failure() {
    let request =
        FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::ShowInvite);
    let context =
        canonical_context_for_request(&EnvoixRuntimeSettings::default(), &request).unwrap();
    let mut activity = make_transfer_activity_record(request);
    let mut session = envoix_client::api::machine::Session::new(TransferDirection::Receive);
    session.state = CanonicalState::Failed;
    session.reason_code = Some(SessionFailureCode::Other);
    session.reason = Some("network connection timed out".to_string());
    let mut failure = TransferError::transport(Phase::Transfer, "network connection timed out")
        .to_failure(Some(TransferDirection::Receive));
    failure.attempt_id = Some("attempt-1".to_string());
    session.failure = Some(failure);

    apply_canonical_snapshot(
        &mut activity,
        &SessionSnapshot {
            seq: 1,
            speed_bps: 0.0,
            avg_bps: 0.0,
            session,
        },
        &context,
        now_ms(),
    );

    assert_eq!(activity.state, FfiTransferActivityState::Failed);
    assert_eq!(activity.failure_code, FfiFailureCode::Timeout);
    assert_eq!(activity.failure_category, FfiFailureCategory::Network);
    assert_eq!(activity.failure_phase, FfiFailurePhase::Transferring);
    assert_eq!(activity.attempt_id, "attempt-1");
    assert!(activity.diagnostic_message.contains("timed out"));
}

#[test]
fn resume_during_pause_transition_is_not_lost() {
    let activity_id = "pause-resume-race".to_string();
    let mut request =
        FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::ShowInvite);
    request.activity_id = activity_id.clone();
    let mut activity = make_transfer_activity_record(request.clone());
    activity.state = FfiTransferActivityState::Transferring;

    let session = EnvoixSession::new_with_settings(EnvoixRuntimeSettings::default());
    let (messages, _rx) = channel();
    let observer: Arc<dyn TransferObserver> = Arc::new(TestObserver(messages));
    let (control, _control_receiver) = oneshot::channel();
    session.queue.lock().unwrap().active.insert(
        activity_id.clone(),
        ActiveTransfer {
            control: Some(control),
            limit: 1,
            activity: activity.clone(),
        },
    );

    assert!(session.pause_activity(activity_id.clone()));
    assert!(session.resume_activity(activity_id.clone()));

    activity.apply_paused(now_ms());
    let notice = finish_transfer_activity(
        &activity_id,
        Some(QueuedTransfer {
            request,
            observer,
            activity,
        }),
        &session.queue,
    )
    .expect("paused activity should be requeued");

    assert_eq!(notice.activity.state, FfiTransferActivityState::Queued);
    assert_eq!(notice.status, "resuming");
    let queue = session.queue.lock().unwrap();
    assert_eq!(queue.pending.len(), 1);
    assert!(!queue.paused.contains_key(&activity_id));
    assert!(!queue.active.contains_key(&activity_id));
}

#[test]
fn cancel_during_pause_transition_overrides_resume() {
    let activity_id = "pause-cancel-race".to_string();
    let mut request =
        FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::ShowInvite);
    request.activity_id = activity_id.clone();
    let mut activity = make_transfer_activity_record(request.clone());
    activity.state = FfiTransferActivityState::Transferring;

    let session = EnvoixSession::new_with_settings(EnvoixRuntimeSettings::default());
    let (messages, _rx) = channel();
    let observer: Arc<dyn TransferObserver> = Arc::new(TestObserver(messages));
    let (control, _control_receiver) = oneshot::channel();
    session.queue.lock().unwrap().active.insert(
        activity_id.clone(),
        ActiveTransfer {
            control: Some(control),
            limit: 1,
            activity: activity.clone(),
        },
    );

    assert!(session.pause_activity(activity_id.clone()));
    assert!(session.resume_activity(activity_id.clone()));
    assert!(session.cancel_activity(activity_id.clone()));

    activity.apply_paused(now_ms());
    let notice = finish_transfer_activity(
        &activity_id,
        Some(QueuedTransfer {
            request,
            observer,
            activity,
        }),
        &session.queue,
    )
    .expect("paused activity should be canceled");

    assert_eq!(notice.activity.state, FfiTransferActivityState::Canceled);
    assert!(!notice.activity.retryable);
    assert_eq!(notice.activity.recovery_action, FfiRecoveryAction::None);
    assert_eq!(notice.status, "canceled");
    let queue = session.queue.lock().unwrap();
    assert!(queue.pending.is_empty());
    assert!(!queue.paused.contains_key(&activity_id));
    assert_eq!(
        queue.history.front().map(|record| record.state),
        Some(FfiTransferActivityState::Canceled)
    );
}

#[test]
fn peer_pause_requeues_while_concurrent_cancel_still_wins() {
    let activity_id = "peer-pause-race".to_string();
    let mut request = FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::Room);
    request.activity_id = activity_id.clone();
    let mut activity = make_transfer_activity_record(request.clone());
    activity.state = FfiTransferActivityState::Transferring;
    activity.apply_peer_paused(now_ms());
    assert_eq!(activity.failure_origin, FfiFailureOrigin::Peer);

    let session = EnvoixSession::new_with_settings(EnvoixRuntimeSettings::default());
    let (messages, _rx) = channel();
    let observer: Arc<dyn TransferObserver> = Arc::new(TestObserver(messages));
    let (control, _control_receiver) = oneshot::channel();
    session.queue.lock().unwrap().active.insert(
        activity_id.clone(),
        ActiveTransfer {
            control: Some(control),
            limit: 1,
            activity: activity.clone(),
        },
    );

    assert!(session.cancel_activity(activity_id.clone()));
    schedule_peer_pause_resume(&session.queue, &activity_id);
    let notice = finish_transfer_activity(
        &activity_id,
        Some(QueuedTransfer {
            request,
            observer,
            activity,
        }),
        &session.queue,
    )
    .expect("peer-paused activity should resolve its pending action");

    assert_eq!(notice.activity.state, FfiTransferActivityState::Canceled);
    assert_eq!(notice.status, "canceled");
    let queue = session.queue.lock().unwrap();
    assert!(queue.pending.is_empty());
    assert!(!queue.paused.contains_key(&activity_id));
    drop(queue);

    let resume_id = "peer-pause-resume".to_string();
    let mut resume_request =
        FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::Room);
    resume_request.activity_id = resume_id.clone();
    let mut resume_activity = make_transfer_activity_record(resume_request.clone());
    resume_activity.apply_peer_paused(now_ms());
    let (messages, _rx) = channel();
    let resume_observer: Arc<dyn TransferObserver> = Arc::new(TestObserver(messages));
    let (control, _control_receiver) = oneshot::channel();
    session.queue.lock().unwrap().active.insert(
        resume_id.clone(),
        ActiveTransfer {
            control: Some(control),
            limit: 1,
            activity: resume_activity.clone(),
        },
    );
    schedule_peer_pause_resume(&session.queue, &resume_id);
    let notice = finish_transfer_activity(
        &resume_id,
        Some(QueuedTransfer {
            request: resume_request,
            observer: resume_observer,
            activity: resume_activity,
        }),
        &session.queue,
    )
    .expect("peer pause should automatically queue a resumed attempt");

    assert_eq!(notice.activity.state, FfiTransferActivityState::Queued);
    assert_eq!(notice.status, "resuming");
    assert_eq!(session.queue.lock().unwrap().pending.len(), 1);
}

#[test]
fn canceled_receive_cleanup_deletes_only_its_exact_partial() {
    let runtime = Runtime::new().unwrap();
    runtime.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let transfer_id = TransferId::new("cleanup-transfer");
        let other_id = TransferId::new("other-transfer");
        let state = envoix_storage::TransferResumeState {
            transfer_id: transfer_id.clone(),
            file_name: "movie.mkv".to_string(),
            file_size: 8,
            chunk_size: 4,
            bytes_received: 4,
            next_chunk_index: 1,
            hash_bytes: 4,
            hash_checkpoint: None,
            target_file_name: None,
        };
        let other_state = envoix_storage::TransferResumeState {
            transfer_id: other_id.clone(),
            ..state.clone()
        };
        LocalFileStorage::write_resume_state(dir.path(), &state)
            .await
            .unwrap();
        LocalFileStorage::write_resume_state(dir.path(), &other_state)
            .await
            .unwrap();
        let target_temp =
            LocalFileStorage::resumable_temp_path(dir.path(), "movie.mkv", &transfer_id).unwrap();
        let other_temp =
            LocalFileStorage::resumable_temp_path(dir.path(), "movie.mkv", &other_id).unwrap();
        std::fs::write(&target_temp, b"abcd").unwrap();
        std::fs::write(&other_temp, b"wxyz").unwrap();

        let request = FfiTransferRequest::receive(
            dir.path().to_string_lossy().into_owned(),
            FfiTransferMode::ShowInvite,
        );
        let mut activity = FfiTransferActivityRecord::from_request(&request, now_ms());
        activity.transfer_id = transfer_id.to_string();
        activity.file_name = "movie.mkv".to_string();
        cleanup_canceled_receive(&request, &activity).await;

        assert!(!target_temp.exists());
        assert!(
            LocalFileStorage::read_resume_state(dir.path(), "movie.mkv", &transfer_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(other_temp.exists());
        assert!(
            LocalFileStorage::read_resume_state(dir.path(), "movie.mkv", &other_id)
                .await
                .unwrap()
                .is_some()
        );
    });
}

#[test]
fn discarding_failed_receive_deletes_retained_partial() {
    let dir = tempfile::tempdir().unwrap();
    let transfer_id = TransferId::new("discard-failed-transfer");
    let state = envoix_storage::TransferResumeState {
        transfer_id: transfer_id.clone(),
        file_name: "failed.bin".to_string(),
        file_size: 4,
        chunk_size: 4,
        bytes_received: 4,
        next_chunk_index: 1,
        hash_bytes: 4,
        hash_checkpoint: None,
        target_file_name: None,
    };
    let runtime = Runtime::new().unwrap();
    runtime
        .block_on(LocalFileStorage::write_resume_state(dir.path(), &state))
        .unwrap();
    let temp =
        LocalFileStorage::resumable_temp_path(dir.path(), &state.file_name, &transfer_id).unwrap();
    std::fs::write(&temp, b"data").unwrap();

    let mut request = FfiTransferRequest::receive(
        dir.path().to_string_lossy().into_owned(),
        FfiTransferMode::ShowInvite,
    );
    request.activity_id = "discard-failed".to_string();
    let mut record = FfiTransferActivityRecord::from_request(&request, now_ms());
    record.state = FfiTransferActivityState::Failed;
    record.transfer_id = transfer_id.to_string();
    record.file_name = state.file_name.clone();
    let session = EnvoixSession::new();
    {
        let mut queue = session.queue.lock().unwrap();
        queue.requests.insert(request.activity_id.clone(), request);
        queue.push_history(record);
    }

    assert!(session.discard_transfer_activity("discard-failed".to_string()));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while temp.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!temp.exists());
    assert!(
        runtime
            .block_on(LocalFileStorage::read_resume_state(
                dir.path(),
                &state.file_name,
                &transfer_id,
            ))
            .unwrap()
            .is_none()
    );
}

#[test]
fn activity_record_keeps_structured_failure_metadata() {
    let request = FfiTransferRequest {
        activity_id: "activity-fail".to_string(),
        direction: FfiTransferDirection::Receive,
        mode: FfiTransferMode::Room,
        file_path: String::new(),
        output_dir: "/tmp/envoix".to_string(),
        peer_descriptor: String::new(),
        invite: String::new(),
        code: "135790-amber-comet".to_string(),
        token: String::new(),
        broker: String::new(),
        relay: String::new(),
        config_path: String::new(),
        path_policy: FfiPathPolicy::Auto,
        resume: true,
        publication_required: false,
        limits: FfiTransferLimits::default(),
        rendezvous: FfiRendezvousPlan::default(),
    };
    let mut record = make_transfer_activity_record(request);
    let failure = FfiTransferFailure {
        code: FfiFailureCode::PermissionDenied,
        category: FfiFailureCategory::Permission,
        phase: FfiFailurePhase::Committing,
        origin: FfiFailureOrigin::Local,
        direction: FfiTransferDirection::Receive,
        transfer_id: "tx-fail".to_string(),
        attempt_id: "attempt-1".to_string(),
        retryable: true,
        recovery_action: FfiRecoveryAction::ChooseFolder,
        user_message_key: "transfer.permission_denied".to_string(),
        diagnostic_message: "permission denied opening destination folder".to_string(),
    };

    record.apply_failure(&failure, 42);

    assert_eq!(record.state, FfiTransferActivityState::Failed);
    assert_eq!(record.failure_code, FfiFailureCode::PermissionDenied);
    assert_eq!(record.failure_category, FfiFailureCategory::Permission);
    assert_eq!(record.failure_phase, FfiFailurePhase::Committing);
    assert_eq!(record.failure_origin, FfiFailureOrigin::Local);
    assert_eq!(record.user_message_key, "transfer.permission_denied");
    assert_eq!(record.recovery_action, FfiRecoveryAction::ChooseFolder);
    assert!(record.retryable);
}

#[test]
fn ffi_failure_keeps_current_attempt_identity() {
    let request = FfiTransferRequest {
        activity_id: "activity-attempt".to_string(),
        direction: FfiTransferDirection::Send,
        mode: FfiTransferMode::Room,
        file_path: "/tmp/report.pdf".to_string(),
        output_dir: String::new(),
        peer_descriptor: String::new(),
        invite: String::new(),
        code: "135790-amber-comet".to_string(),
        token: String::new(),
        broker: String::new(),
        relay: String::new(),
        config_path: String::new(),
        path_policy: FfiPathPolicy::Auto,
        resume: true,
        publication_required: false,
        limits: FfiTransferLimits::default(),
        rendezvous: FfiRendezvousPlan::default(),
    };
    let mut record = make_transfer_activity_record(request);
    record.attempt_id = "attempt-1".to_string();
    record.transfer_id = "tx-1".to_string();

    let error = TransferError::input("unsupported transfer mode");
    let failure = to_ffi_failure(&error, Some(TransferDirection::Send), &record);

    assert_eq!(failure.transfer_id, "tx-1");
    assert_eq!(failure.attempt_id, "attempt-1");
    assert_eq!(failure.direction, FfiTransferDirection::Send);
    assert_eq!(failure.code, FfiFailureCode::UnsupportedFeature);
}

#[test]
fn runtime_settings_normalize_parallel_transfer_limit() {
    let mut limits = FfiTransferLimits {
        max_parallel_transfers: 4,
        ..FfiTransferLimits::default()
    };
    normalize_transfer_limits(
        &EnvoixRuntimeSettings {
            concurrent_transfers: false,
            ..EnvoixRuntimeSettings::default()
        },
        &mut limits,
    );
    assert_eq!(limits.max_parallel_transfers, 1);

    let mut limits = FfiTransferLimits {
        max_parallel_transfers: 4,
        ..FfiTransferLimits::default()
    };
    normalize_transfer_limits(&EnvoixRuntimeSettings::default(), &mut limits);
    assert_eq!(limits.max_parallel_transfers, 4);

    let mut limits = FfiTransferLimits {
        max_parallel_transfers: 0,
        ..FfiTransferLimits::default()
    };
    normalize_transfer_limits(&EnvoixRuntimeSettings::default(), &mut limits);
    assert_eq!(limits.max_parallel_transfers, 1);
}

#[test]
fn room_rendezvous_plan_retries_through_relay_before_mdns() {
    let mut request = FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::Room);
    request.code = "135790-amber-comet".to_string();

    let sources = peer_sources_for_request(&EnvoixRuntimeSettings::default(), &request)
        .expect("room request should build rendezvous sources");

    assert_eq!(sources.len(), 3);
    assert_eq!(sources[0].mode, FfiTransferMode::Room);
    assert_eq!(sources[0].path_policy_override, None);
    assert_eq!(sources[1].mode, FfiTransferMode::Room);
    assert_eq!(
        sources[1].path_policy_override,
        Some(FfiPathPolicy::RelayOnly)
    );
    assert_eq!(sources[2].mode, FfiTransferMode::Mdns);
    match &sources[2].source {
        PeerSource::Mdns { token } => assert_eq!(token.as_deref(), Some("135790-amber-comet")),
        other => panic!("expected mDNS fallback source, got {other:?}"),
    }
}

#[test]
fn canonical_room_context_uses_auto_path_then_mdns_without_duplicate_sources() {
    let mut request =
        FfiTransferRequest::send("/tmp/envoix.txt".to_string(), FfiTransferMode::Room);
    request.code = "135790-amber-comet".to_string();

    let context = canonical_context_for_request(&EnvoixRuntimeSettings::default(), &request)
        .expect("canonical room context");

    assert_eq!(context.params.sources.len(), 2);
    assert!(matches!(context.params.sources[0], PeerSource::Room { .. }));
    assert!(matches!(context.params.sources[1], PeerSource::Mdns { .. }));
    assert_eq!(context.params.options.path, PathPolicy::Auto);
    assert!(context.params.options.relay.is_some());
}

#[test]
fn fallback_is_allowed_after_connection_but_before_transfer_starts() {
    assert!(can_fallback_after_error(None, false, true));
    assert!(!can_fallback_after_error(None, true, true));
    assert!(!can_fallback_after_error(
        Some(TransferStop::Cancel),
        false,
        true
    ));
    assert!(!can_fallback_after_error(None, false, false));
}

#[test]
fn room_fallback_timeout_only_applies_to_senders() {
    let mut send_request =
        FfiTransferRequest::send("/tmp/envoix-room.txt".to_string(), FfiTransferMode::Room);
    send_request.code = "135790-amber-comet".to_string();
    let mut receive_request =
        FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::Room);
    receive_request.code = "135790-amber-comet".to_string();

    assert_eq!(
        fallback_timeout_for_attempt(&send_request, FfiTransferMode::Room, true),
        Some(ROOM_SEND_FALLBACK_TIMEOUT),
    );
    assert_eq!(
        fallback_timeout_for_attempt(&receive_request, FfiTransferMode::Room, true),
        None,
    );
    assert_eq!(
        fallback_timeout_for_attempt(&send_request, FfiTransferMode::Room, false),
        None,
    );
    assert_eq!(
        fallback_timeout_for_attempt(&send_request, FfiTransferMode::Mdns, true),
        None,
    );
}

#[test]
fn room_rendezvous_plan_skips_room_without_internet() {
    let mut request = FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::Room);
    request.code = "135790-amber-comet".to_string();
    request.rendezvous = FfiRendezvousPlan {
        use_room: true,
        use_mdns: true,
        internet_available: false,
    };

    let sources = peer_sources_for_request(&EnvoixRuntimeSettings::default(), &request)
        .expect("mDNS fallback should remain available without internet");

    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].mode, FfiTransferMode::Mdns);
    match &sources[0].source {
        PeerSource::Mdns { token } => assert_eq!(token.as_deref(), Some("135790-amber-comet")),
        other => panic!("expected mDNS source, got {other:?}"),
    }
}

#[test]
fn room_rendezvous_plan_rejects_disabled_routes() {
    let mut request = FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::Room);
    request.code = "135790-amber-comet".to_string();
    request.rendezvous = FfiRendezvousPlan {
        use_room: true,
        use_mdns: false,
        internet_available: false,
    };

    let error = peer_sources_for_request(&EnvoixRuntimeSettings::default(), &request)
        .expect_err("room without internet and without mDNS must be rejected");

    assert!(error.to_string().contains("internet is unavailable"));
}

#[test]
fn debug_summary_redacts_room_password() {
    let mut request =
        FfiTransferRequest::send("/tmp/report.pdf".to_string(), FfiTransferMode::Room);
    request.code = "123456-amber-comet".to_string();
    let attempts = peer_sources_for_request(&EnvoixRuntimeSettings::default(), &request)
        .expect("room request should build rendezvous sources");

    let summary = request_debug_summary(&EnvoixRuntimeSettings::default(), &request, &attempts);
    let attempt = attempt_debug_summary(0, attempts.len(), &attempts[0]);

    assert!(summary.contains("room=123456"));
    assert!(attempt.contains("room=123456"));
    assert!(!summary.contains("amber-comet"));
    assert!(!attempt.contains("amber-comet"));
}

#[test]
fn invite_debug_summary_reports_endpoint_shape_without_token() {
    let peer = PeerDescriptor::new(
        SecretKey::generate().public().to_string(),
        vec!["127.0.0.1:9000".parse().unwrap()],
    )
    .unwrap();
    let invite = QrInvitePayload::new_with_relay_urls(
        "135790-amber-comet".to_string(),
        peer.clone(),
        vec!["https://relay.example:8444".to_string()],
        999,
    )
    .encode();

    let source = invite_source_debug(&invite);
    let advertised = advertised_endpoint_debug(&peer, Some(&invite));

    assert!(source.contains("source=invite"));
    assert!(source.contains("direct=1"));
    assert!(source.contains("relay=1"));
    assert!(!source.contains("amber-comet"));
    assert!(advertised.contains("direct=1"));
    assert!(advertised.contains("relay=1"));
}

#[test]
fn invite_send_auto_adds_relay_only_retry_when_invite_has_relay() {
    let peer = PeerDescriptor::new(
        SecretKey::generate().public().to_string(),
        vec!["127.0.0.1:9000".parse().unwrap()],
    )
    .unwrap();
    let invite = QrInvitePayload::new_with_relay_urls(
        "135790-amber-comet".to_string(),
        peer,
        vec!["https://relay.example:8444".to_string()],
        999,
    )
    .encode();
    let mut request =
        FfiTransferRequest::send("/tmp/report.pdf".to_string(), FfiTransferMode::Invite);
    request.invite = invite;

    let attempts = peer_sources_for_request(&EnvoixRuntimeSettings::default(), &request)
        .expect("invite request should build attempts");

    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].path_policy_override, None);
    assert_eq!(
        attempts[1].path_policy_override,
        Some(FfiPathPolicy::RelayOnly)
    );
    assert!(attempt_debug_summary(1, 2, &attempts[1]).contains("path=relay-only"));
}

#[test]
fn ffi_queue_respects_serial_runtime_setting() {
    if !loopback_transport_available() {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let first_dir = dir.path().join("first");
    let second_dir = dir.path().join("second");
    std::fs::create_dir_all(&first_dir).unwrap();
    std::fs::create_dir_all(&second_dir).unwrap();

    let session = EnvoixSession::new_with_settings(EnvoixRuntimeSettings {
        concurrent_transfers: false,
        ..EnvoixRuntimeSettings::default()
    });

    let (first_tx, _first_rx) = channel();
    let mut first = FfiTransferRequest::receive(
        first_dir.to_str().unwrap().to_string(),
        FfiTransferMode::ShowInvite,
    );
    first.activity_id = "serial-first".to_string();
    first.limits.max_parallel_transfers = 2;
    session
        .start_transfer(first, Arc::new(TestObserver(first_tx)))
        .unwrap();

    let (second_tx, second_rx) = channel();
    let mut second = FfiTransferRequest::receive(
        second_dir.to_str().unwrap().to_string(),
        FfiTransferMode::ShowInvite,
    );
    second.activity_id = "serial-second".to_string();
    second.limits.max_parallel_transfers = 2;
    session
        .start_transfer(second, Arc::new(TestObserver(second_tx)))
        .unwrap();

    let queued = recv_activity(&second_rx, "serial-second", Duration::from_secs(2));
    assert_eq!(queued.state, FfiTransferActivityState::Queued);
    assert_eq!(queued.limits.max_parallel_transfers, 1);
    assert_eq!(
        snapshot_record(&session, "serial-second").map(|record| record.state),
        Some(FfiTransferActivityState::Queued)
    );
    assert_no_nonqueued_activity(&second_rx, "serial-second", Duration::from_millis(200));

    assert!(session.cancel_activity("serial-second".to_string()));
    assert!(session.cancel_activity("serial-first".to_string()));
}

#[test]
fn ffi_queue_holds_and_cancels_pending_activity() {
    if !loopback_transport_available() {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let first_dir = dir.path().join("first");
    let second_dir = dir.path().join("second");
    std::fs::create_dir_all(&first_dir).unwrap();
    std::fs::create_dir_all(&second_dir).unwrap();

    let session = EnvoixSession::new();

    let (first_tx, _first_rx) = channel();
    let mut first = FfiTransferRequest::receive(
        first_dir.to_str().unwrap().to_string(),
        FfiTransferMode::ShowInvite,
    );
    first.activity_id = "queue-first".to_string();
    first.limits.max_parallel_transfers = 1;
    session
        .start_transfer(first, Arc::new(TestObserver(first_tx)))
        .unwrap();

    let (second_tx, second_rx) = channel();
    let mut second = FfiTransferRequest::receive(
        second_dir.to_str().unwrap().to_string(),
        FfiTransferMode::ShowInvite,
    );
    second.activity_id = "queue-second".to_string();
    second.limits.max_parallel_transfers = 1;
    session
        .start_transfer(second, Arc::new(TestObserver(second_tx)))
        .unwrap();

    let queued = recv_activity(&second_rx, "queue-second", Duration::from_secs(2));
    assert_eq!(queued.state, FfiTransferActivityState::Queued);
    assert!(snapshot_record(&session, "queue-first").is_some());
    assert_eq!(
        snapshot_record(&session, "queue-second").map(|record| record.state),
        Some(FfiTransferActivityState::Queued)
    );
    assert_no_nonqueued_activity(&second_rx, "queue-second", Duration::from_millis(200));

    assert!(session.cancel_activity("queue-second".to_string()));
    let canceled = recv_activity(&second_rx, "queue-second", Duration::from_secs(2));
    assert_eq!(canceled.state, FfiTransferActivityState::Canceled);
    assert_eq!(
        snapshot_record(&session, "queue-second").map(|record| record.state),
        Some(FfiTransferActivityState::Canceled)
    );
    assert_eq!(
        session
            .get_transfer_activity("queue-second".to_string())
            .map(|record| record.state),
        Some(FfiTransferActivityState::Canceled)
    );
    assert_eq!(session.clear_transfer_history(), 1);
    assert!(snapshot_record(&session, "queue-second").is_none());

    assert!(session.cancel_activity("queue-first".to_string()));
}

#[test]
fn ffi_queue_discards_pending_activity() {
    if !loopback_transport_available() {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let first_dir = dir.path().join("first");
    let second_dir = dir.path().join("second");
    std::fs::create_dir_all(&first_dir).unwrap();
    std::fs::create_dir_all(&second_dir).unwrap();

    let session = EnvoixSession::new();

    let (first_tx, _first_rx) = channel();
    let mut first = FfiTransferRequest::receive(
        first_dir.to_str().unwrap().to_string(),
        FfiTransferMode::ShowInvite,
    );
    first.activity_id = "discard-first".to_string();
    first.limits.max_parallel_transfers = 1;
    session
        .start_transfer(first, Arc::new(TestObserver(first_tx)))
        .unwrap();

    let (second_tx, second_rx) = channel();
    let mut second = FfiTransferRequest::receive(
        second_dir.to_str().unwrap().to_string(),
        FfiTransferMode::ShowInvite,
    );
    second.activity_id = "discard-second".to_string();
    second.limits.max_parallel_transfers = 1;
    session
        .start_transfer(second, Arc::new(TestObserver(second_tx)))
        .unwrap();

    let queued = recv_activity(&second_rx, "discard-second", Duration::from_secs(2));
    assert_eq!(queued.state, FfiTransferActivityState::Queued);
    assert!(snapshot_record(&session, "discard-second").is_some());

    assert!(session.discard_transfer_activity("discard-second".to_string()));
    let canceled = recv_activity(&second_rx, "discard-second", Duration::from_secs(2));
    assert_eq!(canceled.state, FfiTransferActivityState::Canceled);
    assert!(snapshot_record(&session, "discard-second").is_none());
    assert!(
        session
            .get_transfer_activity("discard-second".to_string())
            .is_none()
    );
    assert!(!session.discard_transfer_activity("discard-second".to_string()));

    assert!(session.cancel_activity("discard-first".to_string()));
}

#[test]
fn ffi_queue_pauses_and_resumes_pending_activity() {
    if !loopback_transport_available() {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let first_dir = dir.path().join("first");
    let second_dir = dir.path().join("second");
    std::fs::create_dir_all(&first_dir).unwrap();
    std::fs::create_dir_all(&second_dir).unwrap();

    let session = EnvoixSession::new();

    let (first_tx, _first_rx) = channel();
    let mut first = FfiTransferRequest::receive(
        first_dir.to_str().unwrap().to_string(),
        FfiTransferMode::ShowInvite,
    );
    first.activity_id = "pause-first".to_string();
    first.limits.max_parallel_transfers = 1;
    session
        .start_transfer(first, Arc::new(TestObserver(first_tx)))
        .unwrap();

    let (second_tx, second_rx) = channel();
    let mut second = FfiTransferRequest::receive(
        second_dir.to_str().unwrap().to_string(),
        FfiTransferMode::ShowInvite,
    );
    second.activity_id = "pause-second".to_string();
    second.limits.max_parallel_transfers = 1;
    session
        .start_transfer(second, Arc::new(TestObserver(second_tx)))
        .unwrap();

    let queued = recv_activity(&second_rx, "pause-second", Duration::from_secs(2));
    assert_eq!(queued.state, FfiTransferActivityState::Queued);

    assert!(session.pause_activity("pause-second".to_string()));
    let paused = recv_activity(&second_rx, "pause-second", Duration::from_secs(2));
    assert_eq!(paused.state, FfiTransferActivityState::Paused);
    assert_eq!(paused.recovery_action, FfiRecoveryAction::Resume);
    assert_eq!(
        snapshot_record(&session, "pause-second").map(|record| record.state),
        Some(FfiTransferActivityState::Paused)
    );

    assert!(session.resume_activity("pause-second".to_string()));
    let requeued = recv_activity(&second_rx, "pause-second", Duration::from_secs(2));
    assert_eq!(requeued.state, FfiTransferActivityState::Queued);
    assert!(requeued.attempt_id.is_empty());
    assert_eq!(
        snapshot_record(&session, "pause-second").map(|record| record.state),
        Some(FfiTransferActivityState::Queued)
    );
    assert_no_nonqueued_activity(&second_rx, "pause-second", Duration::from_millis(200));

    assert!(session.cancel_activity("pause-second".to_string()));
    assert!(session.cancel_activity("pause-first".to_string()));
}

/// Rewrites an invite's direct addresses to loopback, keeping the port, so
/// the transfer stays on the local machine (mirrors the CLI loopback test).
fn loopback_invite(invite: &str) -> String {
    let mut payload = QrInvitePayload::decode(invite).unwrap();
    let port = payload.peer.direct_addrs[0].port();
    payload.peer.direct_addrs = vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)];
    payload.encode()
}

#[test]
fn ffi_qr_invite_loopback() {
    let Some(_loopback_guard) = lock_loopback_tests() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("received");
    std::fs::create_dir_all(&output_dir).unwrap();
    let source = dir.path().join("hello.txt");
    let text = b"hello from the ffi bridge";
    std::fs::write(&source, text).unwrap();

    let receiver = EnvoixSession::new();
    let (rtx, rrx) = channel();
    receiver
        .receive(
            output_dir.to_str().unwrap().to_string(),
            Arc::new(TestObserver(rtx)),
        )
        .unwrap();

    let invite = loopback_invite(&recv_invite(&rrx, Duration::from_secs(10)));

    // Let the receiver's accept loop start before dialing.
    std::thread::sleep(Duration::from_millis(300));

    let sender = EnvoixSession::new();
    let (stx, srx) = channel();
    sender
        .send_invite(
            invite,
            source.to_str().unwrap().to_string(),
            Arc::new(TestObserver(stx)),
        )
        .unwrap();

    let (_, sender_events) = recv_completed(&srx, Duration::from_secs(15));
    let (bytes, receiver_events, receiver_activity) =
        recv_completed_activity(&rrx, Duration::from_secs(15));

    let completed_path = output_dir.join("hello.txt");
    assert_eq!(bytes, text.len() as u64);
    assert_eq!(
        receiver_activity.completed_file_path,
        completed_path.to_string_lossy()
    );
    assert_eq!(std::fs::read(completed_path).unwrap(), text);
    assert!(
        sender_events
            .iter()
            .any(|event| event.kind == FfiTransferEventKind::Binding
                && event.direction == FfiTransferDirection::Send
                && event.mode == FfiTransferMode::Invite)
    );
    assert!(
        receiver_events
            .iter()
            .any(|event| event.kind == FfiTransferEventKind::Started
                && event.direction == FfiTransferDirection::Receive
                && event.file_name == "hello.txt")
    );
}

#[test]
fn durable_ffi_invite_loopback_persists_canonical_completion() {
    let Some(_loopback_guard) = lock_loopback_tests() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("received");
    let receive_records = dir.path().join("receive-records");
    let send_records = dir.path().join("send-records");
    std::fs::create_dir_all(&output_dir).unwrap();
    let source = dir.path().join("durable.txt");
    let text = b"canonical durable ffi loopback";
    std::fs::write(&source, text).unwrap();

    let (rtx, rrx) = channel();
    let mut receive_request = FfiTransferRequest::receive(
        output_dir.to_string_lossy().into_owned(),
        FfiTransferMode::ShowInvite,
    );
    receive_request.activity_id = "durable-receive".to_string();
    let receiver = start_durable_transfer(
        EnvoixRuntimeSettings::default(),
        receive_request,
        receive_records.to_string_lossy().into_owned(),
        Arc::new(TestObserver(rtx)),
        Arc::new(NoopMailbox),
    )
    .unwrap();

    let invite = loopback_invite(&recv_invite(&rrx, Duration::from_secs(10)));
    thread::sleep(Duration::from_millis(300));

    let (stx, srx) = channel();
    let mut send_request = FfiTransferRequest::send(
        source.to_string_lossy().into_owned(),
        FfiTransferMode::Invite,
    );
    send_request.activity_id = "durable-send".to_string();
    send_request.invite = invite;
    let sender = start_durable_transfer(
        EnvoixRuntimeSettings::default(),
        send_request,
        send_records.to_string_lossy().into_owned(),
        Arc::new(TestObserver(stx)),
        Arc::new(NoopMailbox),
    )
    .unwrap();

    let (sent, _) = recv_completed(&srx, Duration::from_secs(20));
    let (received, _, receive_activity) = recv_completed_activity(&rrx, Duration::from_secs(20));
    let completed_path = output_dir.join("durable.txt");
    assert_eq!(sent, text.len() as u64);
    assert_eq!(received, text.len() as u64);
    assert_eq!(std::fs::read(&completed_path).unwrap(), text);
    assert_eq!(
        receive_activity.completed_file_path,
        completed_path.to_string_lossy()
    );

    let receive_history =
        list_durable_transfer_records(receive_records.to_string_lossy().into_owned()).unwrap();
    let send_history =
        list_durable_transfer_records(send_records.to_string_lossy().into_owned()).unwrap();
    assert_eq!(receive_history.len(), 1);
    assert_eq!(receive_history[0].activity_id, "durable-receive");
    assert_eq!(
        receive_history[0].state,
        FfiTransferActivityState::Completed
    );
    assert_eq!(send_history.len(), 1);
    assert_eq!(send_history[0].activity_id, "durable-send");
    assert_eq!(send_history[0].state, FfiTransferActivityState::Completed);

    drop(sender);
    drop(receiver);
}

#[test]
fn durable_ffi_room_existing_file_completion_preserves_resumed_bytes() {
    let Some(_loopback_guard) = lock_loopback_tests() else {
        return;
    };
    let broker = start_test_broker();
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("received");
    let receive_records = dir.path().join("receive-records");
    let send_records = dir.path().join("send-records");
    std::fs::create_dir_all(&output_dir).unwrap();
    let source = dir.path().join("existing.txt");
    let text = b"durable existing-file accounting";
    std::fs::write(&source, text).unwrap();
    let settings = EnvoixRuntimeSettings {
        server_url: broker,
        relay_url: String::new(),
        ..EnvoixRuntimeSettings::default()
    };

    let (rtx, rrx) = channel();
    let mut receive_request = FfiTransferRequest::receive(
        output_dir.to_string_lossy().into_owned(),
        FfiTransferMode::Room,
    );
    receive_request.activity_id = "existing-receive-seed".to_string();
    receive_request.code = "existing-file-room-seed".to_string();
    let receiver = start_durable_transfer_v2(
        settings.clone(),
        receive_request,
        receive_records.to_string_lossy().into_owned(),
        "https://receipt.example.test".to_string(),
        Arc::new(TestObserver(rtx)),
        Arc::new(NoopMailboxV2),
    )
    .unwrap();
    thread::sleep(Duration::from_millis(200));

    let (stx, srx) = channel();
    let mut send_request =
        FfiTransferRequest::send(source.to_string_lossy().into_owned(), FfiTransferMode::Room);
    send_request.activity_id = "existing-send-seed".to_string();
    send_request.code = "existing-file-room-seed".to_string();
    let sender = start_durable_transfer_v2(
        settings.clone(),
        send_request,
        send_records.to_string_lossy().into_owned(),
        "https://receipt.example.test".to_string(),
        Arc::new(TestObserver(stx)),
        Arc::new(NoopMailboxV2),
    )
    .unwrap();
    recv_completed(&srx, Duration::from_secs(20));
    recv_completed_activity(&rrx, Duration::from_secs(20));
    drop(sender);
    drop(receiver);

    let (rtx, rrx) = channel();
    let mut receive_request = FfiTransferRequest::receive(
        output_dir.to_string_lossy().into_owned(),
        FfiTransferMode::Room,
    );
    receive_request.activity_id = "existing-receive-repeat".to_string();
    receive_request.code = "existing-file-room-repeat".to_string();
    let receiver = start_durable_transfer_v2(
        settings.clone(),
        receive_request,
        receive_records.to_string_lossy().into_owned(),
        "https://receipt.example.test".to_string(),
        Arc::new(TestObserver(rtx)),
        Arc::new(NoopMailboxV2),
    )
    .unwrap();
    thread::sleep(Duration::from_millis(200));

    let (stx, srx) = channel();
    let mut send_request =
        FfiTransferRequest::send(source.to_string_lossy().into_owned(), FfiTransferMode::Room);
    send_request.activity_id = "existing-send-repeat".to_string();
    send_request.code = "existing-file-room-repeat".to_string();
    let sender = start_durable_transfer_v2(
        settings,
        send_request,
        send_records.to_string_lossy().into_owned(),
        "https://receipt.example.test".to_string(),
        Arc::new(TestObserver(stx)),
        Arc::new(NoopMailboxV2),
    )
    .unwrap();

    let (_, sender_events) = recv_completed(&srx, Duration::from_secs(20));
    let (_, receiver_events, receiver_activity) =
        recv_completed_activity(&rrx, Duration::from_secs(20));
    assert_eq!(receiver_activity.bytes_resumed, text.len() as u64);
    assert!(receiver_events.iter().any(|event| {
        event.kind == FfiTransferEventKind::Verified && event.bytes_resumed == text.len() as u64
    }));
    assert!(!receiver_events.iter().any(|event| {
        matches!(
            event.kind,
            FfiTransferEventKind::Started | FfiTransferEventKind::Progress
        )
    }));
    assert!(sender_events.iter().any(|event| {
        event.kind == FfiTransferEventKind::Started && event.bytes_resumed == text.len() as u64
    }));

    let receive_history =
        list_durable_transfer_records(receive_records.to_string_lossy().into_owned()).unwrap();
    let repeated = receive_history
        .iter()
        .find(|activity| activity.activity_id == "existing-receive-repeat")
        .expect("repeated receive activity should be persisted");
    assert_eq!(repeated.bytes_resumed, text.len() as u64);

    drop(sender);
    drop(receiver);
}

#[test]
fn durable_manifest_receiver_existing_single_file_preserves_resumed_bytes() {
    let Some(_loopback_guard) = lock_loopback_tests() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("received");
    let receive_records = dir.path().join("receive-records");
    let send_records = dir.path().join("send-records");
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&receive_records).unwrap();
    std::fs::create_dir_all(&send_records).unwrap();
    let source = dir.path().join("existing.txt");
    let text = b"Manifest receiver existing-file accounting";
    std::fs::write(&source, text).unwrap();

    for suffix in ["seed", "repeat"] {
        let (rtx, rrx) = channel();
        let mut receive_request = FfiTransferRequest::receive(
            output_dir.to_string_lossy().into_owned(),
            FfiTransferMode::ShowInvite,
        );
        receive_request.activity_id = format!("manifest-existing-receive-{suffix}");
        let receiver = start_durable_manifest_receive_v2(
            EnvoixRuntimeSettings::default(),
            receive_request,
            receive_records.to_string_lossy().into_owned(),
            Arc::new(TestManifestObserver(rtx)),
        )
        .unwrap();
        let invite = loopback_invite(&recv_manifest_invite(&rrx, Duration::from_secs(10)));
        thread::sleep(Duration::from_millis(300));

        let (stx, srx) = channel();
        let mut send_request = FfiTransferRequest::send(
            source.to_string_lossy().into_owned(),
            FfiTransferMode::Invite,
        );
        send_request.activity_id = format!("manifest-existing-send-{suffix}");
        send_request.invite = invite;
        let sender = start_durable_transfer_v2(
            EnvoixRuntimeSettings::default(),
            send_request,
            send_records.to_string_lossy().into_owned(),
            "https://receipt.example.test".to_string(),
            Arc::new(TestObserver(stx)),
            Arc::new(NoopMailboxV2),
        )
        .unwrap();

        let (_, sender_events) = recv_completed(&srx, Duration::from_secs(20));
        let (receiver_events, receiver_activity) =
            recv_completed_manifest_activity(&rrx, Duration::from_secs(20));
        if suffix == "repeat" {
            assert_eq!(receiver_activity.activity.bytes_resumed, text.len() as u64);
            assert!(receiver_events.iter().any(|event| {
                event.kind == FfiTransferEventKind::Verified
                    && event.bytes_resumed == text.len() as u64
            }));
            assert!(!receiver_events.iter().any(|event| {
                matches!(
                    event.kind,
                    FfiTransferEventKind::Started | FfiTransferEventKind::Progress
                )
            }));
            assert!(sender_events.iter().any(|event| {
                event.kind == FfiTransferEventKind::Started
                    && event.bytes_resumed == text.len() as u64
            }));
        }

        drop(sender);
        drop(receiver);
    }

    let history =
        list_durable_manifest_records(receive_records.to_string_lossy().into_owned()).unwrap();
    let repeated = history
        .iter()
        .find(|record| record.activity.activity_id == "manifest-existing-receive-repeat")
        .expect("repeated Manifest receive activity should be persisted");
    assert_eq!(repeated.activity.bytes_resumed, text.len() as u64);
}

#[test]
fn durable_invite_pause_reuses_the_scanned_endpoint_and_token() {
    let Some(_loopback_guard) = lock_loopback_tests() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("received");
    let receive_records = dir.path().join("receive-records");
    let send_records = dir.path().join("send-records");
    std::fs::create_dir_all(&output_dir).unwrap();
    let source = dir.path().join("invite-pause.bin");
    let payload = vec![0x4a; 16 * 1024 * 1024];
    std::fs::write(&source, &payload).unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "chunk_size = \"256K\"\n").unwrap();
    // A non-empty broker setting suppresses the hosted relay default. This
    // keeps the regression on the original QR's direct endpoint/port.
    let settings = EnvoixRuntimeSettings {
        server_url: "unused-local-broker".to_string(),
        relay_url: String::new(),
        config_path: config.to_string_lossy().into_owned(),
        ..EnvoixRuntimeSettings::default()
    };

    let (rtx, rrx) = channel();
    let mut receive_request = FfiTransferRequest::receive(
        output_dir.to_string_lossy().into_owned(),
        FfiTransferMode::ShowInvite,
    );
    receive_request.activity_id = "invite-pause-receive".to_string();
    let receiver = start_durable_transfer(
        settings.clone(),
        receive_request,
        receive_records.to_string_lossy().into_owned(),
        Arc::new(TestObserver(rtx)),
        Arc::new(NoopMailbox),
    )
    .unwrap();
    let invite = loopback_invite(&recv_invite(&rrx, Duration::from_secs(10)));
    thread::sleep(Duration::from_millis(300));

    let (stx, srx) = channel();
    let (pause_tx, pause_rx) = channel();
    let send_observer = Arc::new(DurablePauseOnProgressObserver {
        messages: stx,
        session: Mutex::new(None),
        result: pause_tx,
        requested: std::sync::atomic::AtomicBool::new(false),
    });
    let mut send_request = FfiTransferRequest::send(
        source.to_string_lossy().into_owned(),
        FfiTransferMode::Invite,
    );
    send_request.activity_id = "invite-pause-send".to_string();
    send_request.invite = invite;
    let sender = start_durable_transfer(
        settings,
        send_request,
        send_records.to_string_lossy().into_owned(),
        send_observer.clone(),
        Arc::new(NoopMailbox),
    )
    .unwrap();
    *send_observer.session.lock().unwrap() = Some(Arc::downgrade(&sender));

    assert!(
        pause_rx
            .recv_timeout(Duration::from_secs(20))
            .expect("progress should trigger an invite pause")
    );
    recv_activity_state(
        &srx,
        "invite-pause-send",
        FfiTransferActivityState::Paused,
        Duration::from_secs(20),
    );
    assert!(sender.resume());

    let (sent, _) = recv_completed(&srx, Duration::from_secs(45));
    let (received, events, _activity) = recv_completed_activity(&rrx, Duration::from_secs(45));
    assert_eq!(sent, payload.len() as u64);
    assert_eq!(received, payload.len() as u64);
    assert!(
        events
            .iter()
            .any(|event| event.kind == FfiTransferEventKind::Advertised),
        "the receiver must re-advertise the stable listener after peer pause"
    );
    assert_eq!(
        std::fs::read(output_dir.join("invite-pause.bin")).unwrap(),
        payload
    );

    drop(sender);
    drop(receiver);
}

#[test]
fn durable_restore_marks_interrupted_attempt_lost_and_remove_discards_exact_state() {
    let Some(_loopback_guard) = lock_loopback_tests() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("received");
    let records_dir = dir.path().join("records");
    std::fs::create_dir_all(&output_dir).unwrap();

    let mut request = FfiTransferRequest::receive(
        output_dir.to_string_lossy().into_owned(),
        FfiTransferMode::ShowInvite,
    );
    request.activity_id = "restore-interrupted".to_string();
    let context =
        canonical_context_for_request(&EnvoixRuntimeSettings::default(), &request).unwrap();
    let transfer_id = TransferId::new("restore-transfer");
    let other_id = TransferId::new("other-transfer");
    let mut session = envoix_client::api::machine::Session::new(TransferDirection::Receive);
    session.state = CanonicalState::Transferring;
    session.transfer_id = Some(transfer_id.to_string());
    session.file_name = Some("resume.bin".to_string());
    session.bytes = 4;
    session.total = 8;
    let record = TransferRecord {
        version: envoix_client::api::record::RECORD_VERSION,
        id: stable_record_id(&request.activity_id),
        created_ms: now_ms(),
        updated_ms: now_ms(),
        context,
        session,
        platform_extras: Some(
            serde_json::json!({ "external_record_id": request.activity_id.clone() }),
        ),
    };
    let resume_state = envoix_storage::TransferResumeState {
        transfer_id: transfer_id.clone(),
        file_name: "resume.bin".to_string(),
        file_size: 8,
        chunk_size: 4,
        bytes_received: 4,
        next_chunk_index: 1,
        hash_bytes: 4,
        hash_checkpoint: None,
        target_file_name: None,
    };
    let other_state = envoix_storage::TransferResumeState {
        transfer_id: other_id.clone(),
        ..resume_state.clone()
    };
    durable_runtime().unwrap().block_on(async {
        RecordStore::new(&records_dir).save(&record).await.unwrap();
        LocalFileStorage::write_resume_state(&output_dir, &resume_state)
            .await
            .unwrap();
        LocalFileStorage::write_resume_state(&output_dir, &other_state)
            .await
            .unwrap();
    });
    let partial =
        LocalFileStorage::resumable_temp_path(&output_dir, "resume.bin", &transfer_id).unwrap();
    let other_partial =
        LocalFileStorage::resumable_temp_path(&output_dir, "resume.bin", &other_id).unwrap();
    std::fs::write(&partial, b"abcd").unwrap();
    std::fs::write(&other_partial, b"wxyz").unwrap();

    let (tx, rx) = channel();
    let restored = restore_durable_transfer(
        request.activity_id.clone(),
        records_dir.to_string_lossy().into_owned(),
        Arc::new(TestObserver(tx)),
        Arc::new(NoopMailbox),
    )
    .unwrap();
    let paused = recv_activity_state(
        &rx,
        &request.activity_id,
        FfiTransferActivityState::Paused,
        Duration::from_secs(5),
    );
    assert_eq!(paused.failure_code, FfiFailureCode::NetworkLost);
    assert_eq!(paused.recovery_action, FfiRecoveryAction::Resume);
    assert!(paused.diagnostic_message.contains("app restart"));

    assert!(restored.remove());
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let records =
            list_durable_transfer_records(records_dir.to_string_lossy().into_owned()).unwrap();
        if records.is_empty() && !partial.exists() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "remove did not delete the durable record and exact partial"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert!(other_partial.exists());
    assert!(
        durable_runtime()
            .unwrap()
            .block_on(LocalFileStorage::read_resume_state(
                &output_dir,
                "resume.bin",
                &other_id,
            ))
            .unwrap()
            .is_some()
    );
}

#[test]
fn durable_room_pause_resumes_from_initiating_side_only() {
    let Some(_loopback_guard) = lock_loopback_tests() else {
        return;
    };
    let broker = start_test_broker();
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("received");
    std::fs::create_dir_all(&output_dir).unwrap();
    let source = dir.path().join("durable-pause.bin");
    let payload = vec![0x6d; 32 * 1024 * 1024];
    std::fs::write(&source, &payload).unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "chunk_size = \"256K\"\n").unwrap();

    let settings = EnvoixRuntimeSettings {
        server_url: broker,
        relay_url: String::new(),
        config_path: config.to_string_lossy().into_owned(),
        ..EnvoixRuntimeSettings::default()
    };
    let code = "975310-durable-pause".to_string();
    let receive_records = dir.path().join("receive-records");
    let send_records = dir.path().join("send-records");

    let (rtx, rrx) = channel();
    let (mailbox_tx, mailbox_rx) = channel();
    let mut receive_request = FfiTransferRequest::receive(
        output_dir.to_string_lossy().into_owned(),
        FfiTransferMode::Room,
    );
    receive_request.activity_id = "durable-pause-receive".to_string();
    receive_request.code = code.clone();
    let receiver = start_durable_transfer(
        settings.clone(),
        receive_request,
        receive_records.to_string_lossy().into_owned(),
        Arc::new(TestObserver(rtx)),
        Arc::new(TestMailbox(mailbox_tx)),
    )
    .unwrap();

    thread::sleep(Duration::from_millis(200));

    let (stx, srx) = channel();
    let (pause_tx, pause_rx) = channel();
    let send_observer = Arc::new(DurablePauseOnProgressObserver {
        messages: stx,
        session: Mutex::new(None),
        result: pause_tx,
        requested: std::sync::atomic::AtomicBool::new(false),
    });
    let mut send_request =
        FfiTransferRequest::send(source.to_string_lossy().into_owned(), FfiTransferMode::Room);
    send_request.activity_id = "durable-pause-send".to_string();
    send_request.code = code;
    let sender = start_durable_transfer(
        settings,
        send_request,
        send_records.to_string_lossy().into_owned(),
        send_observer.clone(),
        Arc::new(NoopMailbox),
    )
    .unwrap();
    *send_observer.session.lock().unwrap() = Some(Arc::downgrade(&sender));

    assert!(
        pause_rx
            .recv_timeout(Duration::from_secs(20))
            .expect("progress should trigger a durable pause")
    );
    recv_activity_state(
        &srx,
        "durable-pause-send",
        FfiTransferActivityState::Paused,
        Duration::from_secs(20),
    );
    assert!(sender.resume());

    let (sent, _) = recv_completed(&srx, Duration::from_secs(45));
    let (received, events, _activity) = recv_completed_activity(&rrx, Duration::from_secs(45));
    assert_eq!(sent, payload.len() as u64);
    assert_eq!(received, payload.len() as u64);
    assert_eq!(
        std::fs::read(output_dir.join("durable-pause.bin")).unwrap(),
        payload
    );
    assert!(
        events
            .iter()
            .filter(|event| event.kind == FfiTransferEventKind::Binding)
            .count()
            >= 2,
        "the peer must automatically launch a second rendezvous attempt"
    );
    match mailbox_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("completed room receive should post a sealed receipt")
    {
        MailboxMsg::Post {
            activity_id,
            key,
            blob,
        } => {
            assert_eq!(activity_id, "durable-pause-receive");
            assert_eq!(key.len(), 64);
            assert!(!blob.is_empty());
        }
        MailboxMsg::Fetch => panic!("receiver should post, not fetch, a receipt"),
    }
    assert!(receiver.receipt_posted());
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let records = durable_runtime()
            .unwrap()
            .block_on(RecordStore::new(&receive_records).load_all());
        if records
            .first()
            .is_some_and(|record| record.session.facts.proof_delivered)
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "receipt POST acknowledgement was not persisted"
        );
        thread::sleep(Duration::from_millis(20));
    }

    drop(sender);
    drop(receiver);
}

#[test]
fn durable_unconfirmed_restore_completes_from_verified_mailbox_receipt() {
    let Some(_loopback_guard) = lock_loopback_tests() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let records_dir = dir.path().join("records");
    let source = dir.path().join("mailbox.bin");
    let payload = b"mailbox completion proof";
    std::fs::write(&source, payload).unwrap();
    let transfer_id = "transfer-mailbox-proof";
    let code = "864209-mailbox-proof";

    let mut request =
        FfiTransferRequest::send(source.to_string_lossy().into_owned(), FfiTransferMode::Room);
    request.activity_id = "mailbox-unconfirmed".to_string();
    request.code = code.to_string();
    request.broker = "ignored@127.0.0.1:9".to_string();
    let mut context =
        canonical_context_for_request(&EnvoixRuntimeSettings::default(), &request).unwrap();
    context.client.receipt_server = Some("https://receipt.example.test:8460/session".to_string());
    let mut machine = envoix_client::api::machine::Session::new(TransferDirection::Send);
    machine.state = CanonicalState::Unconfirmed;
    machine.transfer_id = Some(transfer_id.to_string());
    machine.file_name = Some("mailbox.bin".to_string());
    machine.bytes = payload.len() as u64;
    machine.total = payload.len() as u64;
    let created_ms = now_ms();
    durable_runtime()
        .unwrap()
        .block_on(RecordStore::new(&records_dir).save(&TransferRecord {
            version: envoix_client::api::record::RECORD_VERSION,
            id: stable_record_id(&request.activity_id),
            created_ms,
            updated_ms: created_ms,
            context,
            session: machine,
            platform_extras: Some(
                serde_json::json!({ "external_record_id": request.activity_id.clone() }),
            ),
        }))
        .unwrap();
    let receipt = envoix_storage::TransferReceipt {
        transfer_id: TransferId::new(transfer_id),
        file_name: "mailbox.bin".to_string(),
        file_size: payload.len() as u64,
        file_hash: blake3::hash(payload).to_hex().to_string(),
    };
    let blob = envoix_client::api::receipt::seal_receipt(transfer_id, code, &receipt).unwrap();

    let (activity_tx, activity_rx) = channel();
    let (mailbox_tx, mailbox_rx) = channel();
    let restored = restore_durable_transfer_v2(
        request.activity_id.clone(),
        records_dir.to_string_lossy().into_owned(),
        Arc::new(TestObserver(activity_tx)),
        Arc::new(TestMailboxV2(mailbox_tx)),
    )
    .unwrap();
    let fetch = mailbox_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("restored unconfirmed transfer should poll the mailbox");
    match fetch {
        MailboxV2Msg::Fetch {
            activity_id,
            key,
            server,
        } => {
            assert_eq!(activity_id, request.activity_id);
            assert_eq!(
                key,
                envoix_client::api::receipt::receipt_mailbox_key(transfer_id)
            );
            assert_eq!(
                server.as_deref(),
                Some("https://receipt.example.test:8460/session")
            );
        }
        MailboxV2Msg::Post => panic!("send should fetch, not post, a receipt"),
    }
    assert!(restored.receipt_response(blob));

    let completed = recv_activity_state(
        &activity_rx,
        &request.activity_id,
        FfiTransferActivityState::Completed,
        Duration::from_secs(5),
    );
    assert_eq!(completed.bytes_transferred, payload.len() as u64);
    let history =
        list_durable_transfer_records(records_dir.to_string_lossy().into_owned()).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].state, FfiTransferActivityState::Completed);
    assert_eq!(history[0].created_at_ms, created_ms);
}

#[test]
fn durable_staged_receive_is_not_completed_until_native_publication() {
    let Some(_loopback_guard) = lock_loopback_tests() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let records_dir = dir.path().join("records");
    let staging = dir.path().join("staging");
    std::fs::create_dir_all(&staging).unwrap();
    let staged_file = staging.join("published.bin");
    std::fs::write(&staged_file, b"published bytes").unwrap();

    let mut request = FfiTransferRequest::receive(
        staging.to_string_lossy().into_owned(),
        FfiTransferMode::ShowInvite,
    );
    request.activity_id = "awaiting-publication".to_string();
    request.publication_required = true;
    let context =
        canonical_context_for_request(&EnvoixRuntimeSettings::default(), &request).unwrap();
    let mut machine = envoix_client::api::machine::Session::new(TransferDirection::Receive);
    machine.state = CanonicalState::AwaitingPublication;
    machine.publication_required = true;
    machine.transfer_id = Some("transfer-publication".to_string());
    machine.file_name = Some("published.bin".to_string());
    machine.bytes = 15;
    machine.total = 15;
    machine.completed_file_path = Some(staged_file.to_string_lossy().into_owned());
    let timestamp = now_ms();
    durable_runtime()
        .unwrap()
        .block_on(RecordStore::new(&records_dir).save(&TransferRecord {
            version: envoix_client::api::record::RECORD_VERSION,
            id: stable_record_id(&request.activity_id),
            created_ms: timestamp,
            updated_ms: timestamp,
            context,
            session: machine,
            platform_extras: Some(
                serde_json::json!({ "external_record_id": request.activity_id.clone() }),
            ),
        }))
        .unwrap();

    let (tx, rx) = channel();
    let restored = restore_durable_transfer(
        request.activity_id.clone(),
        records_dir.to_string_lossy().into_owned(),
        Arc::new(TestObserver(tx)),
        Arc::new(NoopMailbox),
    )
    .unwrap();
    let publishing = recv_activity_state(
        &rx,
        &request.activity_id,
        FfiTransferActivityState::Publishing,
        Duration::from_secs(5),
    );
    assert_eq!(
        publishing.completed_file_path,
        staged_file.to_string_lossy()
    );
    assert_eq!(publishing.completed_at_ms, 0);

    let final_uri = "content://downloads/envoix/published.bin";
    assert!(restored.publication_succeeded(final_uri.to_string()));
    let completed = recv_activity_state(
        &rx,
        &request.activity_id,
        FfiTransferActivityState::Completed,
        Duration::from_secs(5),
    );
    assert_eq!(completed.completed_file_path, final_uri);
    assert!(completed.completed_at_ms > 0);
    let history =
        list_durable_transfer_records(records_dir.to_string_lossy().into_owned()).unwrap();
    assert_eq!(history[0].state, FfiTransferActivityState::Completed);
    assert_eq!(history[0].completed_file_path, final_uri);
}

#[test]
fn durable_publication_failure_and_replacement_survive_restart() {
    let Some(_loopback_guard) = lock_loopback_tests() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let records_dir = dir.path().join("records");
    let staging = dir.path().join("staging");
    std::fs::create_dir_all(&staging).unwrap();
    let staged_file = staging.join("recover.bin");
    std::fs::write(&staged_file, b"recover bytes").unwrap();

    let mut request = FfiTransferRequest::receive(
        staging.to_string_lossy().into_owned(),
        FfiTransferMode::ShowInvite,
    );
    request.activity_id = "publication-recovery".to_string();
    request.publication_required = true;
    let context =
        canonical_context_for_request(&EnvoixRuntimeSettings::default(), &request).unwrap();
    let mut machine = envoix_client::api::machine::Session::new(TransferDirection::Receive);
    machine.state = CanonicalState::AwaitingPublication;
    machine.publication_required = true;
    machine.transfer_id = Some("transfer-publication-recovery".to_string());
    machine.file_name = Some("recover.bin".to_string());
    machine.bytes = 13;
    machine.total = 13;
    machine.completed_file_path = Some(staged_file.to_string_lossy().into_owned());
    let timestamp = now_ms();
    durable_runtime()
        .unwrap()
        .block_on(RecordStore::new(&records_dir).save(&TransferRecord {
            version: envoix_client::api::record::RECORD_VERSION,
            id: stable_record_id(&request.activity_id),
            created_ms: timestamp,
            updated_ms: timestamp,
            context,
            session: machine,
            platform_extras: Some(
                serde_json::json!({ "external_record_id": request.activity_id.clone() }),
            ),
        }))
        .unwrap();

    let (tx, _rx) = channel();
    let restored = restore_durable_transfer(
        request.activity_id.clone(),
        records_dir.to_string_lossy().into_owned(),
        Arc::new(TestObserver(tx)),
        Arc::new(NoopMailbox),
    )
    .unwrap();
    assert_eq!(
        restored.activity().state,
        FfiTransferActivityState::Publishing
    );
    assert!(restored.set_publication_target(FfiNativePublicationTarget {
        destination_path: "/first/destination".to_string(),
        bookmark: vec![1, 2, 3],
    }));
    assert_eq!(
        restored.publication_target(),
        Some(FfiNativePublicationTarget {
            destination_path: "/first/destination".to_string(),
            bookmark: vec![1, 2, 3],
        })
    );
    let failure = FfiTransferFailure {
        code: FfiFailureCode::DestinationConflict,
        category: FfiFailureCategory::Storage,
        phase: FfiFailurePhase::Committing,
        origin: FfiFailureOrigin::Local,
        direction: FfiTransferDirection::Receive,
        transfer_id: "transfer-publication-recovery".to_string(),
        attempt_id: "publication-attempt".to_string(),
        retryable: true,
        recovery_action: FfiRecoveryAction::ChooseFolder,
        user_message_key: "transfer.publish_failed".to_string(),
        diagnostic_message: "destination is unavailable".to_string(),
    };
    assert!(!restored.publication_failed(FfiTransferFailure {
        retryable: false,
        ..failure.clone()
    }));
    assert!(restored.publication_failed(failure.clone()));
    assert_eq!(
        restored.activity().state,
        FfiTransferActivityState::Publishing
    );
    assert_eq!(restored.activity().failure_code, failure.code);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let records = durable_runtime()
            .unwrap()
            .block_on(RecordStore::new(&records_dir).load_all());
        let publication = records.first().and_then(native_publication_metadata);
        if publication
            .as_ref()
            .and_then(|value| value.failure.as_ref())
            == Some(&failure)
        {
            assert_eq!(
                publication.unwrap().target.unwrap().destination_path,
                "/first/destination"
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "publication failure was not persisted"
        );
        thread::sleep(Duration::from_millis(20));
    }

    drop(restored);
    let (tx, _rx) = channel();
    let restarted = restore_durable_transfer(
        request.activity_id,
        records_dir.to_string_lossy().into_owned(),
        Arc::new(TestObserver(tx)),
        Arc::new(NoopMailbox),
    )
    .unwrap();
    let restarted_activity = restarted.activity();
    assert_eq!(
        restarted_activity.state,
        FfiTransferActivityState::Publishing
    );
    assert_eq!(restarted_activity.failure_code, failure.code);
    assert!(restarted_activity.retryable);
    assert_eq!(
        restarted_activity.recovery_action,
        FfiRecoveryAction::ChooseFolder
    );
    assert_eq!(
        restarted.publication_target(),
        Some(FfiNativePublicationTarget {
            destination_path: "/first/destination".to_string(),
            bookmark: vec![1, 2, 3],
        })
    );

    assert!(
        restarted.set_publication_target(FfiNativePublicationTarget {
            destination_path: " /replacement/destination ".to_string(),
            bookmark: vec![4, 5, 6],
        })
    );
    let cleared = restarted.activity();
    assert_eq!(cleared.state, FfiTransferActivityState::Publishing);
    assert_eq!(cleared.failure_code, FfiFailureCode::Unknown);
    assert!(!cleared.retryable);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let records = durable_runtime()
            .unwrap()
            .block_on(RecordStore::new(&records_dir).load_all());
        let publication = records.first().and_then(native_publication_metadata);
        if publication
            .as_ref()
            .is_some_and(|value| value.failure.is_none())
        {
            let target = publication.unwrap().target.unwrap();
            assert_eq!(target.destination_path, "/replacement/destination");
            assert_eq!(target.bookmark, vec![4, 5, 6]);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "replacement publication target was not persisted"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn canceling_durable_publication_discards_only_its_staged_artifacts() {
    let Some(_loopback_guard) = lock_loopback_tests() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let records_dir = dir.path().join("records");
    let staging = dir.path().join("staging");
    std::fs::create_dir_all(&staging).unwrap();
    let staged_file = staging.join("cancel.bin");
    let unrelated_file = staging.join("keep.bin");
    let payload = b"cancel staged bytes";
    std::fs::write(&staged_file, payload).unwrap();
    std::fs::write(&unrelated_file, b"keep staged bytes").unwrap();

    let transfer_id = TransferId::new("transfer-cancel-publication");
    let unrelated_id = TransferId::new("transfer-unrelated-publication");
    durable_runtime().unwrap().block_on(async {
        LocalFileStorage::write_receipt(
            &staging,
            &envoix_storage::TransferReceipt {
                transfer_id: transfer_id.clone(),
                file_name: "cancel.bin".into(),
                file_size: payload.len() as u64,
                file_hash: "cancel-hash".into(),
            },
        )
        .await
        .unwrap();
        LocalFileStorage::write_receipt(
            &staging,
            &envoix_storage::TransferReceipt {
                transfer_id: unrelated_id.clone(),
                file_name: "keep.bin".into(),
                file_size: 17,
                file_hash: "keep-hash".into(),
            },
        )
        .await
        .unwrap();
    });

    let mut request = FfiTransferRequest::receive(
        staging.to_string_lossy().into_owned(),
        FfiTransferMode::ShowInvite,
    );
    request.activity_id = "cancel-awaiting-publication".to_string();
    request.publication_required = true;
    let context =
        canonical_context_for_request(&EnvoixRuntimeSettings::default(), &request).unwrap();
    let mut machine = envoix_client::api::machine::Session::new(TransferDirection::Receive);
    machine.state = CanonicalState::AwaitingPublication;
    machine.publication_required = true;
    machine.transfer_id = Some(transfer_id.to_string());
    machine.file_name = Some("cancel.bin".to_string());
    machine.bytes = payload.len() as u64;
    machine.total = payload.len() as u64;
    machine.completed_file_path = Some(staged_file.to_string_lossy().into_owned());
    let timestamp = now_ms();
    durable_runtime()
        .unwrap()
        .block_on(RecordStore::new(&records_dir).save(&TransferRecord {
            version: envoix_client::api::record::RECORD_VERSION,
            id: stable_record_id(&request.activity_id),
            created_ms: timestamp,
            updated_ms: timestamp,
            context,
            session: machine,
            platform_extras: Some(
                serde_json::json!({ "external_record_id": request.activity_id.clone() }),
            ),
        }))
        .unwrap();

    let (tx, rx) = channel();
    let restored = restore_durable_transfer(
        request.activity_id.clone(),
        records_dir.to_string_lossy().into_owned(),
        Arc::new(TestObserver(tx)),
        Arc::new(NoopMailbox),
    )
    .unwrap();
    recv_activity_state(
        &rx,
        &request.activity_id,
        FfiTransferActivityState::Publishing,
        Duration::from_secs(5),
    );

    assert!(restored.cancel());
    let canceled = recv_activity_state(
        &rx,
        &request.activity_id,
        FfiTransferActivityState::Canceled,
        Duration::from_secs(5),
    );
    assert_eq!(canceled.failure_code, FfiFailureCode::UserCanceled);
    assert!(!staged_file.exists());
    assert!(unrelated_file.exists());
    durable_runtime().unwrap().block_on(async {
        assert!(
            LocalFileStorage::read_receipt(&staging, "cancel.bin")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            LocalFileStorage::read_receipt(&staging, "keep.bin")
                .await
                .unwrap()
                .unwrap()
                .transfer_id,
            unrelated_id
        );
    });
    let history =
        list_durable_transfer_records(records_dir.to_string_lossy().into_owned()).unwrap();
    assert_eq!(history[0].state, FfiTransferActivityState::Canceled);
}

#[test]
fn ffi_room_loopback() {
    let Some(_loopback_guard) = lock_loopback_tests() else {
        return;
    };
    let broker = start_test_broker();

    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("received");
    std::fs::create_dir_all(&output_dir).unwrap();
    let source = dir.path().join("room.txt");
    let text = b"hello from ffi room";
    std::fs::write(&source, text).unwrap();

    let settings = EnvoixRuntimeSettings {
        server_url: broker,
        relay_url: String::new(),
        ..EnvoixRuntimeSettings::default()
    };
    let code = "135790-amber-comet".to_string();

    let receiver = EnvoixSession::new_with_settings(settings.clone());
    let (rtx, rrx) = channel();
    let mut receive_request = FfiTransferRequest::receive(
        output_dir.to_str().unwrap().to_string(),
        FfiTransferMode::Room,
    );
    receive_request.code = code.clone();
    receiver
        .start_transfer(receive_request, Arc::new(TestObserver(rtx)))
        .unwrap();

    thread::sleep(Duration::from_millis(200));

    let sender = EnvoixSession::new_with_settings(settings);
    let (stx, srx) = channel();
    let mut send_request =
        FfiTransferRequest::send(source.to_str().unwrap().to_string(), FfiTransferMode::Room);
    send_request.code = code;
    sender
        .start_transfer(send_request, Arc::new(TestObserver(stx)))
        .unwrap();

    let (_, sender_events) = recv_completed(&srx, Duration::from_secs(20));
    let (bytes, receiver_events, receiver_activity) =
        recv_completed_activity(&rrx, Duration::from_secs(20));

    let completed_path = output_dir.join("room.txt");
    assert_eq!(bytes, text.len() as u64);
    assert_eq!(
        receiver_activity.completed_file_path,
        completed_path.to_string_lossy()
    );
    assert_eq!(std::fs::read(completed_path).unwrap(), text);
    assert!(
        sender_events
            .iter()
            .any(|event| event.kind == FfiTransferEventKind::Pairing
                && event.pairing_step == FfiPairingStep::Exchanged)
    );
    assert!(
        receiver_events
            .iter()
            .any(|event| event.kind == FfiTransferEventKind::Started
                && event.file_name == "room.txt")
    );
}

#[test]
fn ffi_room_pause_resumes_from_one_side_and_preserves_file() {
    let Some(_loopback_guard) = lock_loopback_tests() else {
        return;
    };
    let broker = start_test_broker();
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("received");
    std::fs::create_dir_all(&output_dir).unwrap();
    let source = dir.path().join("pause-resume.bin");
    let payload = vec![0x5a; 32 * 1024 * 1024];
    std::fs::write(&source, &payload).unwrap();

    let settings = EnvoixRuntimeSettings {
        server_url: broker,
        relay_url: String::new(),
        ..EnvoixRuntimeSettings::default()
    };
    let code = "246810-cobalt-bridge".to_string();
    let receiver_id = "pause-receiver".to_string();
    let sender_id = "pause-sender".to_string();

    let receiver = Arc::new(EnvoixSession::new_with_settings(settings.clone()));
    let (rtx, rrx) = channel();
    let mut receive_request = FfiTransferRequest::receive(
        output_dir.to_str().unwrap().to_string(),
        FfiTransferMode::Room,
    );
    receive_request.activity_id = receiver_id;
    receive_request.code = code.clone();
    receiver
        .start_transfer(receive_request, Arc::new(TestObserver(rtx)))
        .unwrap();

    thread::sleep(Duration::from_millis(200));

    let sender = Arc::new(EnvoixSession::new_with_settings(settings));
    let (stx, srx) = channel();
    let (pause_tx, pause_rx) = channel();
    let observer = PauseOnProgressObserver {
        messages: stx,
        session: Arc::downgrade(&sender),
        activity_id: sender_id.clone(),
        pause_result: pause_tx,
        requested: std::sync::atomic::AtomicBool::new(false),
    };
    let mut send_request =
        FfiTransferRequest::send(source.to_str().unwrap().to_string(), FfiTransferMode::Room);
    send_request.activity_id = sender_id.clone();
    send_request.code = code;
    sender
        .start_transfer(send_request, Arc::new(observer))
        .unwrap();

    assert!(
        pause_rx
            .recv_timeout(Duration::from_secs(20))
            .expect("progress should trigger a pause")
    );
    recv_activity_state(
        &srx,
        &sender_id,
        FfiTransferActivityState::Paused,
        Duration::from_secs(20),
    );
    assert!(sender.resume_activity(sender_id));

    let (sent, _) = recv_completed(&srx, Duration::from_secs(45));
    let (received, _, activity) = recv_completed_activity(&rrx, Duration::from_secs(45));
    assert_eq!(sent, payload.len() as u64);
    assert_eq!(received, payload.len() as u64);
    assert!(!activity.completed_file_path.is_empty());
    assert_eq!(
        std::fs::read(output_dir.join("pause-resume.bin")).unwrap(),
        payload
    );
}
