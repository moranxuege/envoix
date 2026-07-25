//! JNI bridge that runs an Envoix transfer **in the Android app's process** and
//! streams its event JSON to a Kotlin callback.
//!
//! Running in-process (rather than exec'ing the CLI as a subprocess) is required
//! on Android: the network stack reaches the platform through the JVM - DNS
//! (`hickory` → `ConnectivityManager`), interface enumeration (`netdev`), and the
//! TLS trust store (`rustls-platform-verifier`) all read the Android context via
//! `ndk_context`. [`initContext`] wires the VM + app context in once; without it
//! those crates panic ("android context was not initialized").

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use envoix_client::api::{
    Capabilities, InvitationBootstrap, InvitationError, InviteSecretRef, InviteV2, PeerSource,
    RoomCode, TransferRole, ValidatedInvitation,
};
use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};

static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn runtime() -> &'static tokio::runtime::Runtime {
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime")
    })
}

/// Wire the Android VM + application context into `ndk_context`. Must be called
/// once (e.g. from `Application.onCreate`) before any transfer runs.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_initContext(
    env: JNIEnv,
    _class: JClass,
    context: JObject,
) {
    let Ok(vm) = env.get_java_vm() else {
        tracing::warn!("initContext: failed to get JavaVM");
        return;
    };
    let Ok(ctx) = env.new_global_ref(&context) else {
        tracing::warn!("initContext: failed to create global context ref");
        return;
    };
    unsafe {
        ndk_context::initialize_android_context(
            vm.get_java_vm_pointer() as *mut _,
            ctx.as_obj().as_raw() as *mut _,
        );
    }
    // Keep the global ref alive for the whole process, since ndk_context holds a
    // raw pointer to it.
    std::mem::forget(ctx);
}

/// Generate a directional InviteV2 for `role` ("send"/"receive").
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_generateInvite(
    mut env: JNIEnv,
    _class: JClass,
    role: JString,
    broker: JString,
    relay: JString,
) -> jni::sys::jstring {
    let role = match jstr(&mut env, &role).as_str() {
        "send" => TransferRole::Sender,
        "receive" => TransferRole::Receiver,
        _ => return to_jstring(&mut env, r#"{"error":"role must be send or receive"}"#),
    };
    let broker = jstr(&mut env, &broker);
    let relay = jstr(&mut env, &relay);
    let relay_urls = (!relay.is_empty()).then_some(relay).into_iter().collect();
    let json = match InviteV2::create(
        broker.clone(),
        relay_urls,
        role,
        Capabilities::current(),
        unix_now(),
    ) {
        Ok(invite) => {
            let public = &invite.invitation().public_context;
            let room_code = invite.room_code.canonical().to_string();
            let payload = invite.payload.clone();
            let creator_role = invite.creator_role;
            let joiner_role = invite.joiner_role;
            let expires_at = invite.expires_at;
            let relay = public.relay_urls.first().cloned();
            let reference = match store_invitation(invite.into_bootstrap(), broker.clone()) {
                Ok(reference) => reference,
                Err(error) => return to_jstring(&mut env, &invitation_error_json(&error)),
            };
            format!(
                r#"{{"code":{},"payload":{},"reference":{},"broker":{},"relay":{},"creatorRole":{},"joinerRole":{},"expiresAt":{}}}"#,
                json_str(&room_code),
                json_str(&payload),
                reference_json(&reference),
                json_str(&broker),
                relay
                    .as_deref()
                    .map(json_str)
                    .unwrap_or_else(|| "null".to_string()),
                json_str(role_name(creator_role)),
                json_str(role_name(joiner_role)),
                expires_at,
            )
        }
        Err(error) => invitation_error_json(&error),
    };
    to_jstring(&mut env, &json)
}

/// Parse a full payload for deep-link routing without returning its credential.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_parseInvite(
    mut env: JNIEnv,
    _class: JClass,
    input: JString,
) -> jni::sys::jstring {
    let input = jstr(&mut env, &input);
    let json = InviteV2::parse(&input, unix_now())
        .map(|invite| parsed_invite_json(&invite))
        .unwrap_or_else(|error| invitation_error_json(&error));
    to_jstring(&mut env, &json)
}

/// Validate a full invitation or Room Code against the active flow and retain
/// the private bootstrap only behind an opaque process-memory reference.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_parseInviteForRole(
    mut env: JNIEnv,
    _class: JClass,
    input: JString,
    role: JString,
) -> jni::sys::jstring {
    let input = jstr(&mut env, &input);
    let role = match jstr(&mut env, &role).as_str() {
        "send" => TransferRole::Sender,
        "receive" => TransferRole::Receiver,
        _ => return to_jstring(&mut env, r#"{"error":"role must be send or receive"}"#),
    };
    let prepared = if input.starts_with("envoix:") {
        InviteV2::parse_for_role(&input, role, unix_now()).and_then(|validated| {
            let public = &validated.invitation().public_context;
            let broker = public.broker.clone();
            let relay = public.relay_urls.first().cloned();
            let creator_role = public.creator_transfer_role;
            let joiner_role = public.joiner_transfer_role;
            let expires_at = public.expires_at;
            store_invitation(validated.into_bootstrap(), broker.clone()).map(|reference| {
                prepared_invite_json(
                    &reference,
                    &broker,
                    relay.as_deref(),
                    creator_role,
                    joiner_role,
                    expires_at,
                )
            })
        })
    } else {
        RoomCode::parse(&input).and_then(|room_code| {
            store_invitation(
                InvitationBootstrap::room_code_joiner(room_code, role),
                String::new(),
            )
            .map(|reference| prepared_invite_json(&reference, "", None, role.complement(), role, 0))
        })
    };
    let json = prepared.unwrap_or_else(|error| invitation_error_json(&error));
    to_jstring(&mut env, &json)
}

/// Strictly normalize canonical or separator-free Room Code input.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_normalizeRoomCode(
    mut env: JNIEnv,
    _class: JClass,
    input: JString,
) -> jni::sys::jstring {
    let input = jstr(&mut env, &input);
    let json = RoomCode::parse(&input)
        .map(|code| format!(r#"{{"code":{}}}"#, json_str(code.canonical())))
        .unwrap_or_else(|error| invitation_error_json(&error));
    to_jstring(&mut env, &json)
}

fn store_invitation(
    bootstrap: InvitationBootstrap,
    broker: String,
) -> Result<InviteSecretRef, InvitationError> {
    match PeerSource::invitation(bootstrap, broker)
        .map_err(|_| InvitationError::AuthenticationFailed)?
    {
        PeerSource::Invitation { secret_ref, .. } => Ok(secret_ref),
        _ => unreachable!("invitation constructor returned a non-invitation source"),
    }
}

fn reference_json(reference: &InviteSecretRef) -> String {
    serde_json::to_string(reference).expect("invitation reference is JSON serializable")
}

fn parsed_invite_json(invite: &ValidatedInvitation) -> String {
    let public = &invite.invitation().public_context;
    format!(
        r#"{{"broker":{},"relay":{},"creatorRole":{},"joinerRole":{},"expiresAt":{}}}"#,
        json_str(&public.broker),
        public
            .relay_urls
            .first()
            .map(|value| json_str(value))
            .unwrap_or_else(|| "null".to_string()),
        json_str(role_name(public.creator_transfer_role)),
        json_str(role_name(public.joiner_transfer_role)),
        public.expires_at,
    )
}

fn prepared_invite_json(
    reference: &InviteSecretRef,
    broker: &str,
    relay: Option<&str>,
    creator_role: TransferRole,
    joiner_role: TransferRole,
    expires_at: u64,
) -> String {
    format!(
        r#"{{"reference":{},"broker":{},"relay":{},"creatorRole":{},"joinerRole":{},"expiresAt":{}}}"#,
        reference_json(reference),
        json_str(broker),
        relay.map(json_str).unwrap_or_else(|| "null".to_string()),
        json_str(role_name(creator_role)),
        json_str(role_name(joiner_role)),
        expires_at,
    )
}

fn invitation_error_json(error: &InvitationError) -> String {
    format!(
        r#"{{"error":{},"errorCode":{}}}"#,
        json_str(&error.to_string()),
        json_str(error.code().as_str()),
    )
}

fn role_name(role: TransferRole) -> &'static str {
    match role {
        TransferRole::Sender => "send",
        TransferRole::Receiver => "receive",
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn to_jstring(env: &mut JNIEnv, s: &str) -> jni::sys::jstring {
    env.new_string(s)
        .map(|s| s.into_raw())
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "failed to allocate Java string");
            std::ptr::null_mut()
        })
}

fn jstr(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(|s| s.into()).unwrap_or_else(|error| {
        tracing::warn!(%error, "failed to read Java string");
        String::new()
    })
}

/// Call `callback.onEvent(json)`, attaching this thread to the JVM as needed.
fn emit(vm: &JavaVM, cb: &GlobalRef, json: &str) {
    let Ok(mut env) = vm.attach_current_thread() else {
        tracing::warn!("failed to attach thread to JVM for callback");
        return;
    };
    if let Ok(js) = env.new_string(json) {
        let _ = env.call_method(
            cb,
            "onEvent",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&js)],
        );
    } else {
        tracing::warn!("failed to allocate callback JSON string");
    }
}

/// Minimal JSON string encoding for the synthetic error event.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn java_vm_or_log(env: &JNIEnv, context: &str) -> Option<JavaVM> {
    match env.get_java_vm() {
        Ok(vm) => Some(vm),
        Err(error) => {
            tracing::warn!(%error, "{context}: failed to get JavaVM");
            None
        }
    }
}

fn callback_or_log(env: &JNIEnv, callback: &JObject, context: &str) -> Option<GlobalRef> {
    match env.new_global_ref(callback) {
        Ok(callback) => Some(callback),
        Err(error) => {
            tracing::warn!(%error, "{context}: failed to create callback global ref");
            None
        }
    }
}

mod logging;
mod manifest_v2;
