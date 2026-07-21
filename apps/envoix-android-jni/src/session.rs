use super::*;

/// Create and start a transfer session. `params_json` carries the same fields
/// as `runTransfer` (direction/code/broker/relay/path/chunk_size/candidates/
/// use_room/use_mdns/resume). Notices (snapshots + mailbox courier requests)
/// are delivered to `callback.onEvent` as JSON; returns immediately.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_createSession(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    params_json: JString,
    callback: JObject,
) {
    let json = jstr(&mut env, &params_json);
    let Some(vm) = java_vm_or_log(&env, "createSession") else {
        return;
    };
    let Some(cb) = callback_or_log(&env, &callback, "createSession") else {
        return;
    };

    let (context, extras) = match parse_create_params(&json, CreateMode::Normal) {
        Ok(parsed) => parsed,
        Err(e) => return emit_failed_snapshot(&vm, &cb, "invalid session params", e),
    };
    let _guard = runtime().enter();
    let (session, notices) = match TransferSession::start(context, record_for(id), extras) {
        Ok(session) => session,
        Err(error) => return emit_failed_snapshot(&vm, &cb, "invalid session context", error),
    };
    if !register_session(id, session) {
        return emit_failed_snapshot(&vm, &cb, "session already live or registry unavailable", id);
    }
    spawn_pump(vm, cb, notices);
}

/// Direction as the frontend sends it (lowercase). A typed enum so a typo like
/// `"recieve"` is a loud deserialize error, not a silent fall-through to
/// Receive. JNI-local, so the wire-adjacent `TransferDirection` serde repr
/// (PascalCase, read by the snapshot) stays untouched.
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum CreateDirection {
    Send,
    Receive,
}

impl From<CreateDirection> for TransferDirection {
    fn from(d: CreateDirection) -> Self {
        match d {
            CreateDirection::Send => TransferDirection::Send,
            CreateDirection::Receive => TransferDirection::Receive,
        }
    }
}

/// Android platform extras. The JNI adapter knows these keys; the core keeps
/// them opaque (they round-trip back to an untyped `Value`). Typed here with
/// `deny_unknown_fields` only so a misspelled extras key fails loudly at the
/// boundary instead of silently vanishing.
#[derive(Default, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct AndroidPlatformExtras {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    qr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    saved_uri: Option<String>,
    /// The name a received file was actually published under (may be a
    /// collision-bumped "name (1)"). Durable so it survives a restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    published_name: Option<String>,
    /// Public-artifact evidence used before a private receipt may re-confirm
    /// delivery. It is independent from the core's BLAKE3 transfer proof.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    published_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    published_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    publication_invalid: Option<bool>,
    /// The publication duty's last outcome, currently only `"failed"` — a
    /// received file whose publish to public storage did not complete, so the
    /// UI can surface it and a retry can re-drive. Absent = not-yet / done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    publish: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_recoverable: Option<bool>,
}

/// Which create entry point is parsing — staging additionally requires a source.
#[derive(Clone, Copy)]
enum CreateMode {
    Normal,
    Staging,
}

/// The frontend's flat session-params JSON — the UI-shaped boundary DTO. Every
/// top-level field is REQUIRED: Kotlin's `paramsJson` always emits all of them
/// (an empty string means "use the core default"), so a *missing* field means
/// the two sides drifted and must fail loudly, not default silently.
/// `deny_unknown_fields` catches a renamed / typo'd / extra key the same way.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateParams {
    direction: CreateDirection,
    path: String,
    code: String,
    broker: String,
    relay: String,
    chunk_size: String,
    data_stream_window: String,
    candidates_allow: String,
    candidates_deny: String,
    receipt_server: String,
    use_room: bool,
    use_mdns: bool,
    resume: bool,
    /// Optional: Kotlin omits it when there are no extras (`if length > 0`).
    #[serde(default)]
    platform_extras: Option<AndroidPlatformExtras>,
}

impl CreateParams {
    fn into_context(
        self,
        mode: CreateMode,
    ) -> Result<(SessionContext, Option<serde_json::Value>), String> {
        if self.path.trim().is_empty() {
            return Err("path must not be empty".into());
        }
        if let Some(x) = &self.platform_extras {
            // Kotlin writes the two together; reject a half-pair.
            if x.source_uri.is_some() != x.source_recoverable.is_some() {
                return Err("source_uri and source_recoverable must be set together".into());
            }
        }
        if matches!(mode, CreateMode::Staging) {
            let has_source = self
                .platform_extras
                .as_ref()
                .and_then(|x| x.source_uri.as_deref())
                .is_some_and(|s| !s.is_empty());
            if !has_source {
                return Err("staging session requires a source_uri in platform_extras".into());
            }
        }

        let opt = |s: String| (!s.is_empty()).then_some(s);
        let client = ClientContext {
            chunk_size: opt(self.chunk_size),
            data_stream_window: opt(self.data_stream_window),
            candidates_allow: split_csv(&self.candidates_allow),
            candidates_deny: split_csv(&self.candidates_deny),
            receipt_server: opt(self.receipt_server),
            identity_file: None,
        };

        let mut sources: Vec<PeerSource> = Vec::new();
        if self.use_room {
            sources.push(PeerSource::Room {
                code: self.code.clone(),
                broker: self.broker,
            });
        }
        if self.use_mdns {
            sources.push(PeerSource::Mdns {
                token: Some(self.code),
            });
        }

        let mut options = TransferOptions::default();
        options.relay = opt(self.relay); // empty = default, like the CLI
        options.resume = self.resume;

        let context = SessionContext {
            client,
            params: SessionParams {
                direction: self.direction.into(),
                path: std::path::PathBuf::from(self.path),
                sources,
                options,
                publication_required: false,
            },
        };

        // Hand the extras to the core as an opaque object (it never interprets
        // them); the typing above is purely boundary key-validation.
        let extras = match self.platform_extras {
            Some(x) => Some(serde_json::to_value(&x).map_err(|e| format!("platform_extras: {e}"))?),
            None => None,
        };
        Ok((context, extras))
    }
}

/// Parse + validate the frontend's create-session params (shared by the normal
/// and staging entry points), producing the durable [`SessionContext`] plus the
/// opaque platform extras. A single implementation of parse, conversion, and
/// mode validation.
fn parse_create_params(
    json: &str,
    mode: CreateMode,
) -> Result<(SessionContext, Option<serde_json::Value>), String> {
    let params: CreateParams =
        serde_json::from_str(json).map_err(|e| format!("params JSON: {e}"))?;
    params.into_context(mode)
}

/// Create a SEND session that stages its `content://` source first: the
/// session starts in Preparing and the record is committed before Kotlin
/// copies a byte. Notices flow like `createSession`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_createStagingSession(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    params_json: JString,
    callback: JObject,
) {
    let json = jstr(&mut env, &params_json);
    let Some(vm) = java_vm_or_log(&env, "createStagingSession") else {
        return;
    };
    let Some(cb) = callback_or_log(&env, &callback, "createStagingSession") else {
        return;
    };
    let (context, extras) = match parse_create_params(&json, CreateMode::Staging) {
        Ok(parsed) => parsed,
        Err(e) => return emit_failed_snapshot(&vm, &cb, "invalid session params", e),
    };
    let _guard = runtime().enter();
    let (session, notices) = match TransferSession::start_staging(context, record_for(id), extras) {
        Ok(session) => session,
        Err(error) => return emit_failed_snapshot(&vm, &cb, "invalid session context", error),
    };
    if !register_session(id, session) {
        return emit_failed_snapshot(&vm, &cb, "session already live or registry unavailable", id);
    }
    spawn_pump(vm, cb, notices);
}

fn spawn_pump(
    vm: JavaVM,
    cb: GlobalRef,
    mut notices: tokio::sync::mpsc::UnboundedReceiver<envoix_client::api::driver::SessionNotice>,
) {
    let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    runtime().spawn(async move {
        while let Some(notice) = notices.recv().await {
            emit(&vm, &cb, &notice_json(notice, generation));
        }
    });
}

/// Set the durable record directory. Call once at app start, before sessions.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_initRecords(
    mut env: JNIEnv,
    _class: JClass,
    dir: JString,
) {
    let dir = jstr(&mut env, &dir);
    if RECORDS
        .set(envoix_client::api::record::RecordStore::new(dir))
        .is_err()
    {
        tracing::warn!("initRecords: record store already initialized");
    }
}

/// All persisted transfer records as a JSON array (for restoring cards).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_listRestoreContexts<'a>(
    mut env: JNIEnv<'a>,
    _class: JClass,
) -> jni::sys::jstring {
    let json = match RECORDS.get() {
        Some(store) => {
            let records = runtime().block_on(store.load_all());
            let dtos: Vec<serde_json::Value> = records
                .iter()
                .map(|record| {
                    let mut value = serde_json::to_value(record.restore_context())
                        .unwrap_or(serde_json::Value::Null);
                    // Android-specific card context lives in the opaque
                    // platform_extras (the core does not interpret it); the
                    // JNI glue, which knows the Android keys, flattens the two
                    // the frontend needs onto the DTO.
                    if let (Some(object), Some(extras)) =
                        (value.as_object_mut(), record.platform_extras.as_ref())
                    {
                        for key in [
                            "qr",
                            "saved_uri",
                            "published_name",
                            "published_sha256",
                            "publish",
                            "source_uri",
                        ] {
                            if let Some(text) = extras.get(key).and_then(|v| v.as_str()) {
                                object.insert(key.into(), text.into());
                            }
                        }
                        if let Some(size) = extras.get("published_size").and_then(|v| v.as_u64()) {
                            object.insert("published_size".into(), size.into());
                        }
                        if let Some(invalid) =
                            extras.get("publication_invalid").and_then(|v| v.as_bool())
                        {
                            object.insert("publication_invalid".into(), invalid.into());
                        }
                        if let Some(ok) = extras.get("source_recoverable").and_then(|v| v.as_bool())
                        {
                            object.insert("source_recoverable".into(), ok.into());
                        }
                    }
                    value
                })
                .collect();
            match serde_json::to_string(&dtos) {
                Ok(json) => json,
                Err(error) => {
                    return error_jstring(&mut env, "failed to serialize restore contexts", error);
                }
            }
        }
        None => "[]".into(),
    };
    to_jstring(&mut env, &json)
}

/// Rehydrate a persisted session (no attempt launched; a mid-flight record
/// restores as Paused(Lost); a restored Unconfirmed resumes its mailbox poll).
/// Notices flow to `callback` like `createSession`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_restoreSession(
    env: JNIEnv,
    _class: JClass,
    id: jlong,
    callback: JObject,
) {
    let Some(vm) = java_vm_or_log(&env, "restoreSession") else {
        return;
    };
    let Some(cb) = callback_or_log(&env, &callback, "restoreSession") else {
        return;
    };
    let Some(store) = RECORDS.get() else {
        return emit_failed_snapshot(&vm, &cb, "transfer record store is not initialized", "");
    };
    let record = runtime()
        .block_on(store.load_all())
        .into_iter()
        .find(|r| r.id == id as u64);
    let Some(record) = record else {
        return emit_failed_snapshot(&vm, &cb, "transfer record not found", id);
    };
    let _guard = runtime().enter();
    let (session, notices) = match TransferSession::restore(record, record_for(id)) {
        Ok(session) => session,
        Err(error) => {
            return emit_failed_snapshot(&vm, &cb, "invalid restored session context", error);
        }
    };
    if !register_session(id, session) {
        return emit_failed_snapshot(&vm, &cb, "session already live or registry unavailable", id);
    }
    spawn_pump(vm, cb, notices);
}

/// Route a user intent ("pause" / "resume" / "cancel") to a live session.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_sessionIntent(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    intent: JString,
) {
    let intent = jstr(&mut env, &intent);
    let Ok(map) = sessions().lock() else {
        tracing::warn!(id, intent, "sessionIntent: session registry unavailable");
        return;
    };
    let Some(session) = map.get(&id) else {
        // Not registered (yet): queue by record id - registration drains the
        // queue in order, so an intent can never race a restore and be lost.
        if let Ok(mut pending) = pending_intents().lock() {
            let queue = pending.entry(id).or_default();
            if queue.len() < MAX_PENDING_INTENTS {
                tracing::info!(id, intent, "session not registered; intent queued");
                queue.push(intent);
            } else {
                tracing::warn!(id, intent, "pending intent queue full; dropped");
            }
        }
        return;
    };
    route_intent(id, session, &intent);
}

/// The courier's answer to a fetch_receipt notice: the blob (base64), or ""
/// when the mailbox slot was empty. `key` echoes the notice's mailbox key,
/// so the driver can drop answers from a superseded attempt.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_receiptResponse(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    key: JString,
    blob_b64: JString,
) {
    use base64::Engine;
    let key = jstr(&mut env, &key);
    let blob_b64 = jstr(&mut env, &blob_b64);
    let blob = if blob_b64.trim().is_empty() {
        None
    } else {
        match base64::engine::general_purpose::STANDARD.decode(blob_b64.trim()) {
            Ok(blob) => Some(blob),
            Err(error) => {
                tracing::warn!(id, %error, "receiptResponse: invalid base64 blob");
                return;
            }
        }
    };
    let Ok(map) = sessions().lock() else {
        tracing::warn!(id, "receiptResponse: session registry unavailable");
        return;
    };
    let Some(session) = map.get(&id) else {
        tracing::warn!(id, "receiptResponse: session not found");
        return;
    };
    session.receipt_response(key, blob);
}

/// A Preparing send: report staging copy progress (moves the bar only).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_stageProgress(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
    generation: jint,
    bytes: jlong,
) {
    let Ok(map) = sessions().lock() else {
        return;
    };
    if let Some(session) = map.get(&id) {
        session.stage_progress(generation.max(0) as u32, bytes.max(0) as u64);
    }
}

/// A Preparing send: staging finished, launch the first attempt.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_stageComplete(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
    generation: jint,
) {
    let Ok(map) = sessions().lock() else {
        return;
    };
    match map.get(&id) {
        Some(session) => session.stage_complete(generation.max(0) as u32),
        None => tracing::debug!(id, "stageComplete: session not live"),
    }
}

/// A Preparing send: staging failed, fail the transfer with `reason`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_stageFailed(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    generation: jint,
    reason: JString,
) {
    let reason = jstr(&mut env, &reason);
    let Ok(map) = sessions().lock() else {
        return;
    };
    match map.get(&id) {
        Some(session) => session.stage_failed(generation.max(0) as u32, reason),
        None => tracing::debug!(id, "stageFailed: session not live"),
    }
}

/// Replace the frontend-owned card context (QR payload, saved URI, ...)
/// persisted with the transfer's record. Opaque to the core.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_setSessionExtras(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    extras_json: JString,
) -> jni::sys::jstring {
    let raw = jstr(&mut env, &extras_json);
    // Typed at the boundary (deny_unknown_fields): a misspelled/mistyped extras
    // key is a loud error, not a silently-stored one. Re-serialized to an opaque
    // object for the core (which never interprets it). Returns "" on success, an
    // error message otherwise, so the caller can surface a real boundary drift.
    let extras = match serde_json::from_str::<AndroidPlatformExtras>(&raw)
        .and_then(|x| serde_json::to_value(&x))
    {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("setSessionExtras: invalid extras: {e}");
            tracing::warn!(id, %msg);
            return to_jstring(&mut env, &msg);
        }
    };
    let Ok(map) = sessions().lock() else {
        return to_jstring(&mut env, "");
    };
    let Some(session) = map.get(&id) else {
        // A benign race (Kotlin syncs after teardown), not a drift — no error.
        tracing::debug!(id, "setSessionExtras: session not live");
        return to_jstring(&mut env, "");
    };
    session.set_extras(extras);
    to_jstring(&mut env, "")
}

/// Tear a session down. With `discard` (D2, Remove): delete the partial,
/// resume state, and receipt sidecars first. Idempotent.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_destroySession(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
    discard: jboolean,
) {
    let session = sessions().lock().ok().and_then(|mut m| m.remove(&id));
    if let Ok(mut pending) = pending_intents().lock() {
        pending.remove(&id);
    }
    let Some(session) = session else {
        // No live handle - the record is still the authority for existence:
        // Remove must clean the durable artifacts anyway, or the record
        // resurrects the card on the next restore.
        if discard != 0
            && let Some((store, id)) = record_for(id)
        {
            runtime().block_on(envoix_client::api::record::discard_record(&store, id));
        }
        return;
    };
    if discard != 0 {
        session.discard();
    }
    // Dropping the handle closes the command channel; the actor drains the
    // queued discard first, then detaches any live attempt - a silent
    // teardown (the peer sees connection loss, never a cancel) - and exits.
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
