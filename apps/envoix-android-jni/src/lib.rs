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
    TransferRole, ValidatedInvitation, register_remembered_credential,
};
use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JString, JValue};

static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

const DEFAULT_PAIRING_BROKER: &str =
    "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445";
const DEFAULT_PAIRING_RELAY: &str = "https://envoix.chkxwlyh.us:8444";

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
    let (broker, relay) =
        pairing_invite_endpoints(&jstr(&mut env, &broker), &jstr(&mut env, &relay));
    let relay_urls = relay.into_iter().collect();
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

/// Validate a complete InviteV2 URI against the active flow and retain the
/// private bootstrap only behind an opaque process-memory reference.
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
    let prepared = parse_full_invite_for_role(&input, role).and_then(|validated| {
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
    });
    let json = prepared.unwrap_or_else(|error| invitation_error_json(&error));
    to_jstring(&mut env, &json)
}

/// Validate protected bytes loaded by Android and retain them only in process
/// memory for the next remembered session.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_registerRememberedCredential(
    mut env: JNIEnv,
    _class: JClass,
    opaque_credential: JByteArray,
) -> jni::sys::jstring {
    let json = match env.convert_byte_array(&opaque_credential) {
        Ok(opaque) => match register_remembered_credential(&opaque) {
            Ok(reference) => format!(r#"{{"reference":{}}}"#, json_str(reference.as_str())),
            Err(error) => format!(r#"{{"error":{}}}"#, json_str(&error.to_string())),
        },
        Err(error) => format!(
            r#"{{"error":{}}}"#,
            json_str(&format!("read protected remembered credential: {error}")),
        ),
    };
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

fn parse_full_invite_for_role(
    input: &str,
    role: TransferRole,
) -> Result<ValidatedInvitation, InvitationError> {
    InviteV2::parse_for_role(input, role, unix_now())
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

fn pairing_invite_endpoints(broker: &str, relay: &str) -> (String, Option<String>) {
    let broker = broker.trim();
    let use_default_relay = broker.is_empty();
    let broker = if broker.is_empty() {
        DEFAULT_PAIRING_BROKER
    } else {
        broker
    };
    let relay = match relay.trim() {
        "" if use_default_relay => Some(DEFAULT_PAIRING_RELAY.to_string()),
        "" => None,
        relay => Some(relay.to_string()),
    };
    (broker.to_string(), relay)
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
mod nearby_invite;
mod room_control;

#[cfg(test)]
mod tests {
    use super::{
        Capabilities, DEFAULT_PAIRING_BROKER, DEFAULT_PAIRING_RELAY, InviteV2, TransferRole,
        pairing_invite_endpoints, parse_full_invite_for_role, unix_now,
    };

    #[test]
    fn blank_pairing_endpoints_use_public_defaults() {
        let (broker, relay) = pairing_invite_endpoints(" \n", "\t");

        assert_eq!(broker, DEFAULT_PAIRING_BROKER);
        assert_eq!(relay.as_deref(), Some(DEFAULT_PAIRING_RELAY));
    }

    #[test]
    fn explicit_pairing_endpoints_are_trimmed() {
        let (broker, relay) =
            pairing_invite_endpoints(" broker.example:8500 ", " https://relay.example ");

        assert_eq!(broker, "broker.example:8500");
        assert_eq!(relay.as_deref(), Some("https://relay.example"));
    }

    #[test]
    fn custom_broker_does_not_gain_an_implicit_relay() {
        let (broker, relay) = pairing_invite_endpoints("broker.example:8500", "");

        assert_eq!(broker, "broker.example:8500");
        assert_eq!(relay, None);
    }

    #[test]
    fn role_parser_rejects_naked_invite_v2_room_codes() {
        assert!(parse_full_invite_for_role("123456-k7m4-9v2d", TransferRole::Receiver).is_err());
    }

    #[test]
    fn role_parser_keeps_complete_invite_v2_uris() {
        let invite = InviteV2::create(
            DEFAULT_PAIRING_BROKER.into(),
            vec![DEFAULT_PAIRING_RELAY.into()],
            TransferRole::Sender,
            Capabilities::current(),
            unix_now(),
        )
        .expect("create complete invitation");

        assert!(parse_full_invite_for_role(&invite.payload, TransferRole::Receiver).is_ok());
    }
}
