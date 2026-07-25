use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use envoix_client::api::{
    Client, IdentityConfig, RoomCloseReason, RoomControlEvent, RoomControlInvite,
    RoomControlSession, RoomLifetimePolicy, RoomOfferRejection, RoomTransferOffer,
    TransferCancelToken, TransferOptions, connect_room_control,
};
use envoix_error::CoreError;
use jni::JNIEnv;
use jni::objects::{GlobalRef, JClass, JObject, JString};
use jni::sys::{jlong, jstring};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::{callback_or_log, emit, java_vm_or_log, jstr, runtime, to_jstring};

const DEFAULT_RENDEZVOUS_BROKER: &str =
    "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445";
const DEFAULT_RELAY_URL: &str = "https://envoix.chkxwlyh.us:8444";

struct ActiveRoom {
    cancel: TransferCancelToken,
    session: Option<Arc<RoomControlSession>>,
    commands: Option<mpsc::UnboundedSender<RoomCommand>>,
    accepting_commands: Arc<AtomicBool>,
    closing: Arc<AtomicBool>,
}

static ACTIVE_ROOMS: OnceLock<Mutex<HashMap<i64, ActiveRoom>>> = OnceLock::new();

fn active_rooms() -> &'static Mutex<HashMap<i64, ActiveRoom>> {
    ACTIVE_ROOMS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Deserialize)]
struct StartParams {
    mode: String,
    input: String,
    display_name: String,
    #[serde(default)]
    identity_path: String,
    #[serde(default)]
    fallback_broker: String,
    #[serde(default)]
    fallback_relay: String,
}

#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
enum RoomCommand {
    Offer {
        offer_id: String,
        transfer_invite: String,
        root_names: Vec<String>,
        item_count: u32,
        total_bytes: u64,
    },
    Respond {
        offer_id: String,
        accept: bool,
    },
    Policy {
        policy: String,
    },
    Close {
        reason: String,
    },
    Ping {
        nonce: u64,
    },
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_generateRoomControlInvite(
    mut env: JNIEnv,
    _class: JClass,
    broker: JString,
    relay: JString,
) -> jstring {
    let broker = fallback(&jstr(&mut env, &broker), DEFAULT_RENDEZVOUS_BROKER);
    let relay = fallback_optional(&jstr(&mut env, &relay), DEFAULT_RELAY_URL);
    let value = RoomControlInvite::generate(broker, relay)
        .map(invite_json)
        .unwrap_or_else(error_json);
    to_jstring(&mut env, &value.to_string())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_parseRoomControlInvite(
    mut env: JNIEnv,
    _class: JClass,
    input: JString,
    fallback_broker: JString,
    fallback_relay: JString,
) -> jstring {
    let input = jstr(&mut env, &input);
    let broker = fallback(&jstr(&mut env, &fallback_broker), DEFAULT_RENDEZVOUS_BROKER);
    let relay = fallback_optional(&jstr(&mut env, &fallback_relay), DEFAULT_RELAY_URL);
    let value = RoomControlInvite::parse(&input, broker, relay)
        .map(invite_json)
        .unwrap_or_else(error_json);
    to_jstring(&mut env, &value.to_string())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_startRoomControlSession(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    params_json: JString,
    callback: JObject,
) {
    let params_json = jstr(&mut env, &params_json);
    let Some(vm) = java_vm_or_log(&env, "startRoomControlSession") else {
        return;
    };
    let Some(callback) = callback_or_log(&env, &callback, "startRoomControlSession") else {
        return;
    };
    let params: StartParams = match serde_json::from_str(&params_json) {
        Ok(params) => params,
        Err(error) => {
            emit(
                &vm,
                &callback,
                &failed_event(format!("invalid room-control parameters: {error}")),
            );
            return;
        }
    };
    if !matches!(params.mode.as_str(), "host" | "join") {
        emit(
            &vm,
            &callback,
            &failed_event("room-control mode must be host or join"),
        );
        return;
    }

    let cancel = TransferCancelToken::new();
    let accepting_commands = Arc::new(AtomicBool::new(true));
    let closing = Arc::new(AtomicBool::new(false));
    let vm = Arc::new(vm);
    let callback = Arc::new(callback);
    let entry = ActiveRoom {
        cancel: cancel.clone(),
        session: None,
        commands: None,
        accepting_commands: accepting_commands.clone(),
        closing: closing.clone(),
    };
    let Ok(mut rooms) = active_rooms().lock() else {
        emit(&vm, &callback, &failed_event("room registry unavailable"));
        return;
    };
    if rooms.contains_key(&id) {
        emit(
            &vm,
            &callback,
            &failed_event("a room session with this id is already active"),
        );
        return;
    }
    rooms.insert(id, entry);
    drop(rooms);

    runtime().spawn(async move {
        let result = connect_from_params(&params, &cancel).await;
        let session = match result {
            Ok(session) => Arc::new(session),
            Err(error) => {
                if !closing.load(Ordering::Acquire) {
                    emit(vm.as_ref(), callback.as_ref(), &failed_event(error));
                }
                remove_room_if(id, &closing);
                return;
            }
        };
        let (command_sender, command_receiver) = mpsc::unbounded_channel();
        let registered = active_rooms()
            .lock()
            .ok()
            .and_then(|mut rooms| {
                rooms.get_mut(&id).and_then(|entry| {
                    Arc::ptr_eq(&entry.closing, &closing).then(|| {
                        entry.session = Some(session.clone());
                        entry.commands = Some(command_sender);
                    })
                })
            })
            .is_some();
        if !registered || closing.load(Ordering::Acquire) {
            let _ = session.close(RoomCloseReason::Backgrounded).await;
            return;
        }
        let command_task = tokio::spawn(run_command_fifo(
            id,
            session.clone(),
            command_receiver,
            closing.clone(),
            vm.clone(),
            callback.clone(),
        ));
        emit(
            vm.as_ref(),
            callback.as_ref(),
            &json!({
                "notice":"room_control",
                "state":"connected",
                "peer_name":session.peer_name(),
                "creator":session.is_creator(),
                "policy":"idle_15_minutes",
            })
            .to_string(),
        );

        loop {
            match session.next_event().await {
                Ok(RoomControlEvent::PeerClosed(reason)) => {
                    accepting_commands.store(false, Ordering::Release);
                    closing.store(true, Ordering::Release);
                    emit(vm.as_ref(), callback.as_ref(), &closed_event(reason));
                    break;
                }
                Ok(event) => emit(
                    vm.as_ref(),
                    callback.as_ref(),
                    &event_json(event).to_string(),
                ),
                Err(error) => {
                    accepting_commands.store(false, Ordering::Release);
                    if !closing.load(Ordering::Acquire) {
                        let event = match error {
                            CoreError::Io(_) | CoreError::Transport(_) => {
                                closed_event(RoomCloseReason::NetworkLost)
                            }
                            other => failed_event(other),
                        };
                        emit(vm.as_ref(), callback.as_ref(), &event);
                    }
                    break;
                }
            }
        }
        command_task.abort();
        remove_room_if(id, &closing);
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_sendRoomControlCommand(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    command_json: JString,
) -> jstring {
    let command_json = jstr(&mut env, &command_json);
    let command: RoomCommand = match serde_json::from_str(&command_json) {
        Ok(command) => command,
        Err(error) => {
            return to_jstring(
                &mut env,
                &error_json(format!("invalid room-control command: {error}")).to_string(),
            );
        }
    };
    let queued = active_rooms()
        .lock()
        .map_err(|_| "room registry unavailable".to_string())
        .and_then(|rooms| {
            let entry = rooms
                .get(&id)
                .ok_or_else(|| "room control is not connected".to_string())?;
            enqueue_room_command(entry, command)
        });
    if let Err(error) = queued {
        return to_jstring(&mut env, &error_json(error).to_string());
    }
    to_jstring(&mut env, r#"{"queued":true}"#)
}

fn enqueue_room_command(entry: &ActiveRoom, command: RoomCommand) -> Result<(), String> {
    if !entry.accepting_commands.load(Ordering::Acquire) {
        return Err("room control is closing".into());
    }
    let terminal_close = matches!(
        &command,
        RoomCommand::Close { reason } if parse_close_reason(reason).is_ok()
    );
    let commands = entry
        .commands
        .as_ref()
        .ok_or_else(|| "room control is not connected".to_string())?;
    commands
        .send(command)
        .map_err(|_| "room command queue is closed".to_string())?;
    if terminal_close {
        entry.accepting_commands.store(false, Ordering::Release);
    }
    Ok(())
}

async fn run_command_fifo(
    id: i64,
    session: Arc<RoomControlSession>,
    mut commands: mpsc::UnboundedReceiver<RoomCommand>,
    closing: Arc<AtomicBool>,
    vm: Arc<jni::JavaVM>,
    callback: Arc<GlobalRef>,
) {
    while let Some(command) = commands.recv().await {
        if closing.load(Ordering::Acquire) {
            break;
        }
        let requested_close = match &command {
            RoomCommand::Close { reason } => parse_close_reason(reason).ok(),
            _ => None,
        };
        if requested_close.is_some() {
            closing.store(true, Ordering::Release);
        }
        let result = execute_command(&session, &command).await;
        if let Some(reason) = requested_close {
            if let Err(error) = result {
                tracing::debug!(%error, "room control peer close notification failed");
            }
            emit(vm.as_ref(), callback.as_ref(), &closed_event(reason));
            remove_room_if(id, &closing);
            break;
        }
        if let Err(error) = result {
            emit(
                vm.as_ref(),
                callback.as_ref(),
                &command_failed_event(&command, error),
            );
            continue;
        }
        match command {
            RoomCommand::Respond { offer_id, accept } => {
                emit(
                    vm.as_ref(),
                    callback.as_ref(),
                    &offer_response_sent_event(&offer_id, accept),
                );
            }
            RoomCommand::Policy { policy } => {
                emit(
                    vm.as_ref(),
                    callback.as_ref(),
                    &json!({
                        "notice":"room_control",
                        "state":"policy_changed",
                        "policy":policy,
                    })
                    .to_string(),
                );
            }
            RoomCommand::Close { .. } => unreachable!("valid closes return above"),
            _ => {}
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_cancelRoomControlSession(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
) {
    let entry = active_rooms()
        .lock()
        .ok()
        .and_then(|mut rooms| rooms.remove(&id));
    if let Some(entry) = entry {
        entry.accepting_commands.store(false, Ordering::Release);
        entry.closing.store(true, Ordering::Release);
        entry.cancel.cancel();
        if let Some(session) = entry.session {
            runtime().spawn(async move {
                let _ = session.close(RoomCloseReason::Backgrounded).await;
            });
        }
    }
}

async fn connect_from_params(
    params: &StartParams,
    cancel: &TransferCancelToken,
) -> Result<RoomControlSession, CoreError> {
    let broker = fallback(&params.fallback_broker, DEFAULT_RENDEZVOUS_BROKER);
    let relay = fallback_optional(&params.fallback_relay, DEFAULT_RELAY_URL);
    let invite = RoomControlInvite::parse(&params.input, broker, relay)?;
    let mut client = Client::default();
    if !params.identity_path.trim().is_empty() {
        client.identity = IdentityConfig::Persistent(PathBuf::from(params.identity_path.trim()));
    }
    let mut options = TransferOptions::default();
    options.relay = invite.relay().map(str::to_string);
    connect_room_control(
        invite,
        params.display_name.clone(),
        params.mode == "host",
        client.session_config(&options),
        cancel,
    )
    .await
}

async fn execute_command(
    session: &RoomControlSession,
    command: &RoomCommand,
) -> Result<(), CoreError> {
    match command {
        RoomCommand::Offer {
            offer_id,
            transfer_invite,
            root_names,
            item_count,
            total_bytes,
        } => {
            session
                .offer_transfer(RoomTransferOffer {
                    offer_id: offer_id.clone(),
                    transfer_invite: transfer_invite.clone(),
                    root_names: root_names.clone(),
                    item_count: *item_count,
                    total_bytes: *total_bytes,
                })
                .await
        }
        RoomCommand::Respond { offer_id, accept } if *accept => {
            session.accept_offer(offer_id).await
        }
        RoomCommand::Respond { offer_id, .. } => {
            session
                .reject_offer(offer_id, RoomOfferRejection::Declined)
                .await
        }
        RoomCommand::Policy { policy } => session.set_policy(parse_policy(policy)?).await,
        RoomCommand::Close { reason } => session.close(parse_close_reason(reason)?).await,
        RoomCommand::Ping { nonce } => session.ping(*nonce).await,
    }
}

fn invite_json(invite: RoomControlInvite) -> Value {
    json!({
        "code":invite.code(),
        "payload":invite.payload(),
        "broker":invite.broker(),
        "relay":invite.relay(),
        "expires_at_epoch_ms":invite.expires_at_unix_secs().saturating_mul(1_000),
    })
}

fn event_json(event: RoomControlEvent) -> Value {
    match event {
        RoomControlEvent::IncomingOffer(offer) => json!({
            "notice":"room_control",
            "state":"incoming_offer",
            "offer":{
                "id":offer.offer_id,
                "transfer_invite":offer.transfer_invite,
                "root_names":offer.root_names,
                "item_count":offer.item_count,
                "total_bytes":offer.total_bytes,
            }
        }),
        RoomControlEvent::OfferAccepted { offer_id } => json!({
            "notice":"room_control",
            "state":"offer_accepted",
            "offer_id":offer_id,
        }),
        RoomControlEvent::OfferRejected { offer_id, reason } => json!({
            "notice":"room_control",
            "state":"offer_rejected",
            "offer_id":offer_id,
            "reason":rejection_wire(reason),
        }),
        RoomControlEvent::PolicyChanged(policy) => json!({
            "notice":"room_control",
            "state":"policy_changed",
            "policy":policy_wire(policy),
        }),
        RoomControlEvent::PeerClosed(reason) => json!({
            "notice":"room_control",
            "state":"closed",
            "reason":close_reason_wire(reason),
        }),
        RoomControlEvent::Pong { nonce } => json!({
            "notice":"room_control",
            "state":"pong",
            "nonce":nonce,
        }),
    }
}

fn failed_event(error: impl std::fmt::Display) -> String {
    json!({
        "notice":"room_control",
        "state":"failed",
        "message":error.to_string(),
    })
    .to_string()
}

fn command_failed_event(command: &RoomCommand, error: impl std::fmt::Display) -> String {
    let (kind, offer_id) = match command {
        RoomCommand::Offer { offer_id, .. } => ("offer", Some(offer_id.as_str())),
        RoomCommand::Respond { offer_id, .. } => ("respond", Some(offer_id.as_str())),
        RoomCommand::Policy { .. } => ("policy", None),
        RoomCommand::Close { .. } => ("close", None),
        RoomCommand::Ping { .. } => ("ping", None),
    };
    json!({
        "notice":"room_control",
        "state":"command_failed",
        "command":kind,
        "offer_id":offer_id,
        "message":error.to_string(),
    })
    .to_string()
}

fn offer_response_sent_event(offer_id: &str, accepted: bool) -> String {
    json!({
        "notice":"room_control",
        "state":"offer_response_sent",
        "offer_id":offer_id,
        "accepted":accepted,
    })
    .to_string()
}

fn closed_event(reason: RoomCloseReason) -> String {
    json!({
        "notice":"room_control",
        "state":"closed",
        "reason":close_reason_wire(reason),
    })
    .to_string()
}

fn error_json(error: impl std::fmt::Display) -> Value {
    json!({"error":error.to_string()})
}

fn fallback(value: &str, default: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        default.to_string()
    } else {
        value.to_string()
    }
}

fn fallback_optional(value: &str, default: &str) -> Option<String> {
    Some(fallback(value, default))
}

fn parse_policy(value: &str) -> Result<RoomLifetimePolicy, CoreError> {
    match value {
        "idle_15_minutes" => Ok(RoomLifetimePolicy::Idle15Minutes),
        "until_foreground_ends" => Ok(RoomLifetimePolicy::UntilForegroundEnds),
        _ => Err(CoreError::InvalidInput(
            "unknown room lifetime policy".into(),
        )),
    }
}

fn policy_wire(policy: RoomLifetimePolicy) -> &'static str {
    match policy {
        RoomLifetimePolicy::Idle15Minutes => "idle_15_minutes",
        RoomLifetimePolicy::UntilForegroundEnds => "until_foreground_ends",
    }
}

fn parse_close_reason(value: &str) -> Result<RoomCloseReason, CoreError> {
    match value {
        "user_ended" => Ok(RoomCloseReason::UserEnded),
        "idle_expired" => Ok(RoomCloseReason::IdleExpired),
        "invitation_expired" => Ok(RoomCloseReason::InvitationExpired),
        "peer_ended" => Ok(RoomCloseReason::PeerEnded),
        "backgrounded" => Ok(RoomCloseReason::Backgrounded),
        "network_lost" => Ok(RoomCloseReason::NetworkLost),
        "protocol_failure" => Ok(RoomCloseReason::ProtocolFailure),
        _ => Err(CoreError::InvalidInput("unknown room close reason".into())),
    }
}

fn close_reason_wire(reason: RoomCloseReason) -> &'static str {
    match reason {
        RoomCloseReason::UserEnded => "user_ended",
        RoomCloseReason::IdleExpired => "idle_expired",
        RoomCloseReason::InvitationExpired => "invitation_expired",
        RoomCloseReason::PeerEnded => "peer_ended",
        RoomCloseReason::Backgrounded => "backgrounded",
        RoomCloseReason::NetworkLost => "network_lost",
        RoomCloseReason::ProtocolFailure => "protocol_failure",
    }
}

fn rejection_wire(reason: RoomOfferRejection) -> &'static str {
    match reason {
        RoomOfferRejection::Declined => "declined",
        RoomOfferRejection::Busy => "busy",
        RoomOfferRejection::Expired => "expired",
        RoomOfferRejection::Invalid => "invalid",
    }
}

fn remove_room_if(id: i64, generation: &Arc<AtomicBool>) {
    if let Ok(mut rooms) = active_rooms().lock()
        && rooms
            .get(&id)
            .is_some_and(|entry| Arc::ptr_eq(&entry.closing, generation))
    {
        rooms.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_json_contains_epoch_milliseconds() {
        let invite = RoomControlInvite::parse(
            "envoix://room/R123456-amber-comet?broker=test&expires=42",
            "fallback",
            None,
        )
        .unwrap();
        assert_eq!(invite_json(invite)["expires_at_epoch_ms"], 42_000);
    }

    #[test]
    fn command_preserves_opaque_offer_id() {
        let command: RoomCommand = serde_json::from_str(
            r#"{"command":"offer","offer_id":"opaque_7","transfer_invite":"x","root_names":[],"item_count":0,"total_bytes":0}"#,
        )
        .unwrap();
        assert!(matches!(
            command,
            RoomCommand::Offer { offer_id, .. } if offer_id == "opaque_7"
        ));
    }

    #[test]
    fn respond_ack_and_failure_keep_offer_correlation() {
        let command: RoomCommand =
            serde_json::from_str(r#"{"command":"respond","offer_id":"opaque_9","accept":true}"#)
                .unwrap();
        let failure: Value =
            serde_json::from_str(&command_failed_event(&command, "network lost")).unwrap();
        assert_eq!(failure["state"], "command_failed");
        assert_eq!(failure["command"], "respond");
        assert_eq!(failure["offer_id"], "opaque_9");

        let acknowledgment: Value =
            serde_json::from_str(&offer_response_sent_event("opaque_9", true)).unwrap();
        assert_eq!(acknowledgment["state"], "offer_response_sent");
        assert_eq!(acknowledgment["offer_id"], "opaque_9");
        assert_eq!(acknowledgment["accepted"], true);
    }

    #[test]
    fn command_queue_preserves_respond_before_close_and_rejects_later_work() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let entry = ActiveRoom {
            cancel: TransferCancelToken::new(),
            session: None,
            commands: Some(sender),
            accepting_commands: Arc::new(AtomicBool::new(true)),
            closing: Arc::new(AtomicBool::new(false)),
        };
        enqueue_room_command(
            &entry,
            RoomCommand::Respond {
                offer_id: "offer_1".into(),
                accept: true,
            },
        )
        .unwrap();
        enqueue_room_command(
            &entry,
            RoomCommand::Close {
                reason: "user_ended".into(),
            },
        )
        .unwrap();
        assert!(
            enqueue_room_command(
                &entry,
                RoomCommand::Policy {
                    policy: "idle_15_minutes".into(),
                },
            )
            .is_err()
        );
        assert!(matches!(
            receiver.try_recv().unwrap(),
            RoomCommand::Respond { offer_id, accept }
                if offer_id == "offer_1" && accept
        ));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            RoomCommand::Close { reason } if reason == "user_ended"
        ));
        assert!(receiver.try_recv().is_err());
    }
}
