use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use envoix_client::api::{NearbyInviteEndpoint, NearbyInviteInbox, start_nearby_invite_inbox};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JClass, JObject, JString};
use jni::sys::{jlong, jstring};
use serde::Deserialize;
use serde_json::json;

use crate::{callback_or_log, emit, java_vm_or_log, jstr, runtime, to_jstring};

struct ActiveNearbyInbox {
    inbox: Option<Arc<NearbyInviteInbox>>,
    callback: Arc<GlobalRef>,
    vm: Arc<jni::JavaVM>,
    live: Arc<AtomicBool>,
    outgoing: HashSet<String>,
}

static ACTIVE_NEARBY_INBOXES: OnceLock<Mutex<HashMap<i64, ActiveNearbyInbox>>> = OnceLock::new();

fn active_nearby_inboxes() -> &'static Mutex<HashMap<i64, ActiveNearbyInbox>> {
    ACTIVE_NEARBY_INBOXES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartParams {
    peer_key: String,
    display_name: String,
    #[serde(default)]
    relay: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NearbyInviteEndpointJson {
    endpoint_id: String,
    #[serde(default)]
    relay_url: Option<String>,
    #[serde(default)]
    direct_addresses: Vec<String>,
}

fn parse_nearby_invite_endpoint(route_json: &str) -> Result<NearbyInviteEndpoint, String> {
    serde_json::from_str::<NearbyInviteEndpointJson>(route_json)
        .map(|endpoint| NearbyInviteEndpoint {
            endpoint_id: endpoint.endpoint_id,
            relay_url: endpoint.relay_url,
            direct_addresses: endpoint.direct_addresses,
        })
        .map_err(|error| format!("invalid nearby endpoint route: {error}"))
}

fn ready_event(endpoint: NearbyInviteEndpoint) -> String {
    json!({
        "event":"ready",
        "endpoint_id":endpoint.endpoint_id,
        "relay_url":endpoint.relay_url,
        "direct_addresses":endpoint.direct_addresses,
    })
    .to_string()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_startNearbyInviteInbox(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    params_json: JString,
    callback: JObject,
) {
    let params_json = jstr(&mut env, &params_json);
    let Some(vm) = java_vm_or_log(&env, "startNearbyInviteInbox") else {
        return;
    };
    let Some(callback) = callback_or_log(&env, &callback, "startNearbyInviteInbox") else {
        return;
    };
    let params: StartParams = match serde_json::from_str(&params_json) {
        Ok(params) => params,
        Err(error) => {
            emit(
                &vm,
                &callback,
                &failed_event(format!("invalid nearby-inbox parameters: {error}")),
            );
            return;
        }
    };
    let vm = Arc::new(vm);
    let callback = Arc::new(callback);
    let live = Arc::new(AtomicBool::new(true));
    let entry = ActiveNearbyInbox {
        inbox: None,
        callback: callback.clone(),
        vm: vm.clone(),
        live: live.clone(),
        outgoing: HashSet::new(),
    };
    let Ok(mut inboxes) = active_nearby_inboxes().lock() else {
        emit(
            &vm,
            &callback,
            &failed_event("nearby-inbox registry unavailable"),
        );
        return;
    };
    if inboxes.contains_key(&id) {
        emit(
            &vm,
            &callback,
            &failed_event("a nearby-inbox generation with this id is already active"),
        );
        return;
    }
    inboxes.insert(id, entry);
    drop(inboxes);

    runtime().spawn(async move {
        let relay = (!params.relay.trim().is_empty()).then(|| params.relay.trim().to_string());
        let inbox =
            match start_nearby_invite_inbox(relay, params.peer_key, params.display_name).await {
                Ok(inbox) => Arc::new(inbox),
                Err(error) => {
                    if live.load(Ordering::Acquire) {
                        emit(vm.as_ref(), callback.as_ref(), &failed_event(error));
                    }
                    remove_inbox_if(id, &live);
                    return;
                }
            };
        let registered = active_nearby_inboxes()
            .lock()
            .ok()
            .and_then(|mut inboxes| {
                inboxes.get_mut(&id).and_then(|entry| {
                    Arc::ptr_eq(&entry.live, &live).then(|| {
                        entry.inbox = Some(inbox.clone());
                    })
                })
            })
            .is_some();
        if !registered || !live.load(Ordering::Acquire) {
            inbox.close().await;
            return;
        }

        let endpoint = inbox.endpoint();
        emit(vm.as_ref(), callback.as_ref(), &ready_event(endpoint));
        loop {
            match inbox.next_invite().await {
                Ok(invite) => {
                    if !live.load(Ordering::Acquire) {
                        break;
                    }
                    emit(
                        vm.as_ref(),
                        callback.as_ref(),
                        &json!({
                            "event":"incoming",
                            "request_id":invite.request_id.to_string(),
                            "sender_endpoint_id":invite.sender_endpoint_id,
                            "sender_peer_key":invite.sender_peer_key,
                            "sender_display_name":invite.sender_display_name,
                            "invite":invite.invite,
                            "expires_at_epoch_seconds":invite.expires_at_unix_secs,
                        })
                        .to_string(),
                    );
                }
                Err(error) => {
                    if live.load(Ordering::Acquire) {
                        emit(vm.as_ref(), callback.as_ref(), &failed_event(error));
                    }
                    break;
                }
            }
        }
        inbox.close().await;
        remove_inbox_if(id, &live);
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_sendNearbyInvite(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    request_id: JString,
    route_json: JString,
    invite: JString,
) -> jstring {
    let request_id = jstr(&mut env, &request_id).trim().to_string();
    let route_json = jstr(&mut env, &route_json);
    let invite = jstr(&mut env, &invite);
    if request_id.is_empty() || request_id.len() > 128 {
        return to_jstring(&mut env, r#"{"error":"invalid nearby request id"}"#);
    }
    let endpoint = match parse_nearby_invite_endpoint(&route_json) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            return to_jstring(&mut env, &json!({"error":error}).to_string());
        }
    };

    let queued = active_nearby_inboxes()
        .lock()
        .map_err(|_| "nearby-inbox registry unavailable".to_string())
        .and_then(|mut inboxes| {
            let entry = inboxes
                .get_mut(&id)
                .ok_or_else(|| "nearby-inbox generation is not active".to_string())?;
            let inbox = entry
                .inbox
                .clone()
                .ok_or_else(|| "nearby-inbox generation is not ready".to_string())?;
            if !entry.outgoing.insert(request_id.clone()) {
                return Err("nearby request id is already active".into());
            }
            Ok((
                inbox,
                entry.vm.clone(),
                entry.callback.clone(),
                entry.live.clone(),
            ))
        });
    let (inbox, vm, callback, live) = match queued {
        Ok(queued) => queued,
        Err(error) => {
            return to_jstring(&mut env, &json!({"error":error}).to_string());
        }
    };

    runtime().spawn(async move {
        let result = inbox.send_invite(&endpoint, &invite).await;
        let deliver = active_nearby_inboxes()
            .lock()
            .ok()
            .and_then(|mut inboxes| {
                inboxes.get_mut(&id).and_then(|entry| {
                    (Arc::ptr_eq(&entry.live, &live) && entry.outgoing.remove(&request_id))
                        .then_some(())
                })
            })
            .is_some();
        if deliver && live.load(Ordering::Acquire) {
            emit(
                vm.as_ref(),
                callback.as_ref(),
                &json!({
                    "event":"send_result",
                    "request_id":request_id,
                    "error":result.err().map(|error| error.to_string()),
                })
                .to_string(),
            );
        }
    });
    to_jstring(&mut env, r#"{"queued":true}"#)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_stopNearbyInviteInbox(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
) {
    let entry = active_nearby_inboxes()
        .lock()
        .ok()
        .and_then(|mut inboxes| inboxes.remove(&id));
    if let Some(entry) = entry {
        entry.live.store(false, Ordering::Release);
        if let Some(inbox) = entry.inbox {
            runtime().spawn(async move {
                inbox.close().await;
            });
        }
    }
}

fn remove_inbox_if(id: i64, live: &Arc<AtomicBool>) {
    if let Ok(mut inboxes) = active_nearby_inboxes().lock()
        && inboxes
            .get(&id)
            .is_some_and(|entry| Arc::ptr_eq(&entry.live, live))
    {
        inboxes.remove(&id);
    }
}

fn failed_event(message: impl std::fmt::Display) -> String {
    json!({
        "event":"failed",
        "message":message.to_string(),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENDPOINT_ID: &str = "abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrst";

    #[test]
    fn android_route_json_matches_the_core_endpoint_contract() {
        let route = parse_nearby_invite_endpoint(
            &json!({
                "endpoint_id":ENDPOINT_ID,
                "relay_url":"https://relay.example",
                "direct_addresses":["192.0.2.4:8443","[2001:db8::1]:8443"],
            })
            .to_string(),
        )
        .expect("parse route");

        assert_eq!(route.endpoint_id, ENDPOINT_ID);
        assert_eq!(route.relay_url.as_deref(), Some("https://relay.example"));
        assert_eq!(
            route.direct_addresses,
            ["192.0.2.4:8443", "[2001:db8::1]:8443"]
        );

        let ready: serde_json::Value =
            serde_json::from_str(&ready_event(route)).expect("ready json");
        assert_eq!(ready["endpoint_id"], ENDPOINT_ID);
        assert_eq!(ready["relay_url"], "https://relay.example");
        assert_eq!(
            ready["direct_addresses"],
            json!(["192.0.2.4:8443", "[2001:db8::1]:8443"])
        );

        let direct_only = parse_nearby_invite_endpoint(
            &json!({
                "endpoint_id":ENDPOINT_ID,
                "relay_url":null,
                "direct_addresses":["192.0.2.4:8443"],
            })
            .to_string(),
        )
        .expect("parse direct-only route");
        let direct_only_ready: serde_json::Value =
            serde_json::from_str(&ready_event(direct_only)).expect("direct-only ready json");
        assert!(direct_only_ready["relay_url"].is_null());
    }

    #[test]
    fn android_route_json_rejects_contract_drift() {
        let error = parse_nearby_invite_endpoint(
            &json!({
                "endpoint_id":ENDPOINT_ID,
                "relay_url":null,
                "direct_addresses":["192.0.2.4:8443"],
                "endpoint":"legacy-field",
            })
            .to_string(),
        )
        .expect_err("unknown fields must fail closed");

        assert!(error.contains("unknown field"));
    }
}
