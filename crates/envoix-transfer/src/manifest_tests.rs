use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tempfile::tempdir;
use tokio::sync::mpsc;

use super::*;
use crate::MIN_CHUNK_SIZE;

#[test]
fn source_mapping_requires_every_file_and_no_directory() {
    let manifest = test_manifest(
        "mapping",
        vec![
            directory_entry(0, "Folder"),
            file_entry(1, "Folder/a.txt", b"a"),
        ],
    );
    let missing = ManifestSendRequest::new(manifest.clone(), []);
    assert!(
        matches!(missing, Err(CoreError::InvalidInput(message)) if message.contains("missing=[1]"))
    );

    let unexpected = ManifestSendRequest::new(
        manifest,
        [(0, PathBuf::from("directory")), (1, PathBuf::from("file"))],
    );
    assert!(
        matches!(unexpected, Err(CoreError::InvalidInput(message)) if message.contains("unexpected=[0]"))
    );
}

#[tokio::test]
async fn sender_starts_handshake_before_source_metadata_validation() {
    let missing_source = PathBuf::from("/envoix-test-source-does-not-exist");
    let manifest = test_manifest(
        "handshake-before-source-validation",
        vec![file_entry(0, "missing.bin", b"missing")],
    );
    let request = ManifestSendRequest::new(manifest, [(0, missing_source)]).unwrap();
    let mut connection = HandshakeOnlyManifestConnection::default();

    let result = ManifestTransferEngine::new(MIN_CHUNK_SIZE)
        .send_manifest(&mut connection, request, true, &ManifestNoopEventSink)
        .await;

    assert!(result.is_err());
    assert!(matches!(
        connection.sent.as_slice(),
        [ManifestFrame::Hello(ManifestHelloV1 {
            role: PeerRole::Sender,
            ..
        })]
    ));
}

#[tokio::test]
async fn streamed_hash_rejects_same_size_source_changes() {
    let source = tempdir().unwrap();
    let output = tempdir().unwrap();
    let original = b"original";
    let changed = b"modified";
    let source_path = source.path().join("mutable.bin");
    fs::write(&source_path, changed).await.unwrap();
    let request = ManifestSendRequest::new(
        test_manifest(
            "streamed-source-change",
            vec![file_entry(0, "mutable.bin", original)],
        ),
        [(0, source_path)],
    )
    .unwrap();
    let (mut sender, mut receiver, chunks) = memory_manifest_connection_pair();
    let engine = ManifestTransferEngine::new(MIN_CHUNK_SIZE);

    let (sent, received) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(
            engine.send_manifest(&mut sender, request, true, &ManifestNoopEventSink),
            engine.receive_manifest(
                &mut receiver,
                output.path().to_path_buf(),
                &ManifestNoopEventSink
            )
        )
    })
    .await
    .expect("source-change rejection must not leave either peer waiting");

    assert!(sent.is_err());
    assert!(received.is_err());
    assert!(chunks.load(Ordering::SeqCst) > 0);
    assert!(
        !fs::try_exists(output.path().join("mutable.bin"))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn transfers_tree_and_renames_colliding_roots_without_merging() {
    let source = tempdir().unwrap();
    let output = tempdir().unwrap();
    let photo_bytes = b"new photo";
    let note_bytes = b"new note";
    let photo_source = source.path().join("a.txt");
    let note_source = source.path().join("note.txt");
    fs::write(&photo_source, photo_bytes).await.unwrap();
    fs::write(&note_source, note_bytes).await.unwrap();
    fs::create_dir(output.path().join("Photos")).await.unwrap();
    fs::write(output.path().join("Photos/keep.txt"), b"keep")
        .await
        .unwrap();
    fs::write(output.path().join("note.txt"), b"old note")
        .await
        .unwrap();

    let manifest = test_manifest(
        "tree-transfer",
        vec![
            directory_entry(0, "Photos"),
            file_entry(1, "Photos/a.txt", photo_bytes),
            directory_entry(2, "Photos/Empty"),
            file_entry(3, "note.txt", note_bytes),
        ],
    );
    let request =
        ManifestSendRequest::new(manifest, [(1, photo_source), (3, note_source)]).unwrap();
    let (mut sender, mut receiver, _) = memory_manifest_connection_pair();
    let engine = ManifestTransferEngine::new(MIN_CHUNK_SIZE);
    let (sent, received) = tokio::join!(
        engine.send_manifest(&mut sender, request, true, &ManifestNoopEventSink),
        engine.receive_manifest(
            &mut receiver,
            output.path().to_path_buf(),
            &ManifestNoopEventSink
        )
    );
    let sent = sent.unwrap();
    let received = received.unwrap();
    assert_eq!(sent, received);
    assert_eq!(
        fs::read(output.path().join("Photos (1)/a.txt"))
            .await
            .unwrap(),
        photo_bytes
    );
    assert!(
        fs::metadata(output.path().join("Photos (1)/Empty"))
            .await
            .unwrap()
            .is_dir()
    );
    assert_eq!(
        fs::read(output.path().join("note (1).txt")).await.unwrap(),
        note_bytes
    );
    assert_eq!(
        fs::read(output.path().join("Photos/keep.txt"))
            .await
            .unwrap(),
        b"keep"
    );
    assert_eq!(
        fs::read(output.path().join("note.txt")).await.unwrap(),
        b"old note"
    );
    assert!(
        received
            .entries
            .iter()
            .all(|result| result.status == ManifestEntryResultStatus::Renamed)
    );
}

#[tokio::test]
async fn skips_identical_existing_file_without_payload_chunks() {
    let source = tempdir().unwrap();
    let output = tempdir().unwrap();
    let bytes = b"identical";
    let source_path = source.path().join("same.txt");
    fs::write(&source_path, bytes).await.unwrap();
    fs::write(output.path().join("same.txt"), bytes)
        .await
        .unwrap();
    let manifest = test_manifest(
        "skip-identical",
        vec![
            file_entry(0, "same.txt", bytes),
            directory_entry(1, "Empty"),
        ],
    );
    let request = ManifestSendRequest::new(manifest, [(0, source_path)]).unwrap();
    let (mut sender, mut receiver, chunks) = memory_manifest_connection_pair();
    let engine = ManifestTransferEngine::new(MIN_CHUNK_SIZE);
    let (sent, received) = tokio::join!(
        engine.send_manifest(&mut sender, request, true, &ManifestNoopEventSink),
        engine.receive_manifest(
            &mut receiver,
            output.path().to_path_buf(),
            &ManifestNoopEventSink
        )
    );
    assert!(sent.is_ok(), "sender failed: {sent:?}");
    let received = received.unwrap();
    assert_eq!(chunks.load(Ordering::SeqCst), 0);
    assert_eq!(
        received.entries[0].status,
        ManifestEntryResultStatus::SkippedIdentical
    );
    assert_eq!(
        fs::read(output.path().join("same.txt")).await.unwrap(),
        bytes
    );
}

#[tokio::test]
async fn interrupted_entry_resumes_from_persisted_prefix() {
    let source = tempdir().unwrap();
    let output = tempdir().unwrap();
    let bytes = vec![0x5a; MIN_CHUNK_SIZE * 3 + 7];
    let source_path = source.path().join("large.bin");
    fs::write(&source_path, &bytes).await.unwrap();
    let manifest = test_manifest(
        "resume-entry",
        vec![
            directory_entry(0, "Folder"),
            file_entry(1, "Folder/large.bin", &bytes),
        ],
    );
    let request = ManifestSendRequest::new(manifest, [(1, source_path)]).unwrap();
    let engine = ManifestTransferEngine::new(MIN_CHUNK_SIZE);

    let receiver_cancel = TransferCancelToken::new();
    let stopping_sink = CancelAfterProgressSink {
        cancel: receiver_cancel.clone(),
    };
    let (mut first_sender, mut first_receiver, _) = memory_manifest_connection_pair();
    let first_send = engine.send_manifest(
        &mut first_sender,
        request.clone(),
        true,
        &ManifestNoopEventSink,
    );
    let first_receive = engine.receive_manifest_with_cancel(
        &mut first_receiver,
        output.path().to_path_buf(),
        &stopping_sink,
        &receiver_cancel,
    );
    let (first_send, first_receive) = tokio::join!(first_send, first_receive);
    assert!(first_send.is_err());
    assert!(first_receive.is_err());
    assert!(
        !fs::try_exists(output.path().join("Folder/large.bin"))
            .await
            .unwrap()
    );

    let recording_sink = RecordingManifestSink::default();
    let (mut second_sender, mut second_receiver, _) = memory_manifest_connection_pair();
    let (second_send, second_receive) = tokio::join!(
        engine.send_manifest(&mut second_sender, request, true, &recording_sink),
        engine.receive_manifest(
            &mut second_receiver,
            output.path().to_path_buf(),
            &recording_sink
        )
    );
    assert!(second_send.is_ok(), "sender failed: {second_send:?}");
    assert!(
        second_receive.is_ok(),
        "receiver failed: {second_receive:?}"
    );
    assert_eq!(
        fs::read(output.path().join("Folder/large.bin"))
            .await
            .unwrap(),
        bytes
    );
    assert!(
        !fs::try_exists(output.path().join("Folder (1)"))
            .await
            .unwrap()
    );
    assert!(recording_sink.events().iter().any(|event| matches!(
        event,
        ManifestTransferEvent::EntryStarted { bytes_resumed, .. } if *bytes_resumed > 0
    )));
    let plans = recording_sink.plans();
    assert_eq!(plans.len(), 2);
    assert!(plans.iter().any(|(direction, manifest)| {
        *direction == TransferDirection::Send && manifest.manifest_id.0 == "resume-entry"
    }));
    assert!(plans.iter().any(|(direction, manifest)| {
        *direction == TransferDirection::Receive && manifest.manifest_id.0 == "resume-entry"
    }));
}

#[cfg(unix)]
#[tokio::test]
async fn refuses_symlinked_private_state_directory() {
    use std::os::unix::fs::symlink;

    let source = tempdir().unwrap();
    let output = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let bytes = b"secret";
    let source_path = source.path().join("secret.txt");
    fs::write(&source_path, bytes).await.unwrap();
    symlink(outside.path(), output.path().join(STATE_ROOT_NAME)).unwrap();
    let manifest = test_manifest("unsafe-state", vec![file_entry(0, "secret.txt", bytes)]);
    let request = ManifestSendRequest::new(manifest, [(0, source_path)]).unwrap();
    let (mut sender, mut receiver, _) = memory_manifest_connection_pair();
    let engine = ManifestTransferEngine::new(MIN_CHUNK_SIZE);
    let (sent, received) = tokio::join!(
        engine.send_manifest(&mut sender, request, true, &ManifestNoopEventSink),
        engine.receive_manifest(
            &mut receiver,
            output.path().to_path_buf(),
            &ManifestNoopEventSink
        )
    );
    assert!(sent.is_err());
    assert!(
        matches!(received, Err(CoreError::Storage(message)) if message.contains("safe directory"))
    );
    assert!(
        fs::read_dir(outside.path())
            .await
            .unwrap()
            .next_entry()
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn discard_removes_only_the_selected_manifest_state() {
    let output = tempdir().unwrap();
    let selected = ManifestId::new("selected");
    let kept = ManifestId::new("kept");
    let selected_dir = manifest_state_directory(output.path(), &selected)
        .await
        .unwrap();
    let kept_dir = manifest_state_directory(output.path(), &kept)
        .await
        .unwrap();
    fs::write(selected_dir.join("partial"), b"partial")
        .await
        .unwrap();
    fs::write(kept_dir.join("partial"), b"kept").await.unwrap();
    fs::write(output.path().join("completed.txt"), b"completed")
        .await
        .unwrap();

    discard_manifest_resume_state(output.path(), &selected)
        .await
        .unwrap();

    assert!(!fs::try_exists(selected_dir).await.unwrap());
    assert!(fs::try_exists(kept_dir).await.unwrap());
    assert_eq!(
        fs::read(output.path().join("completed.txt")).await.unwrap(),
        b"completed"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn discard_refuses_symlinked_manifest_state() {
    use std::os::unix::fs::symlink;

    let output = tempdir().unwrap();
    let outside = tempdir().unwrap();
    symlink(outside.path(), output.path().join(STATE_ROOT_NAME)).unwrap();

    let result = discard_manifest_resume_state(output.path(), &ManifestId::new("unsafe")).await;

    assert!(
        matches!(result, Err(CoreError::Storage(message)) if message.contains("safe directory"))
    );
}

fn test_manifest(id: &str, entries: Vec<ManifestEntryV1>) -> ManifestV1 {
    let file_count = entries
        .iter()
        .filter(|entry| entry.kind == ManifestEntryKind::RegularFile)
        .count() as u32;
    let directory_count = entries.len() as u32 - file_count;
    let root_count = entries
        .iter()
        .filter(|entry| !entry.relative_path.contains('/'))
        .count() as u32;
    let total_bytes = entries.iter().map(|entry| entry.size).sum();
    let manifest = ManifestV1 {
        manifest_id: ManifestId::new(id),
        entries,
        file_count,
        directory_count,
        root_count,
        total_bytes,
        hash_algorithm: envoix_protocol::ManifestHashAlgorithm::Blake3_256,
    };
    manifest.validate_structure().unwrap();
    manifest
}

#[test]
fn send_request_round_trips_for_durable_sessions() {
    let request = ManifestSendRequest::new(
        test_manifest(
            "durable-request",
            vec![file_entry(0, "file.bin", b"durable")],
        ),
        [(0, PathBuf::from("/private/source/file.bin"))],
    )
    .unwrap();

    let encoded = serde_json::to_vec(&request).unwrap();
    let decoded: ManifestSendRequest = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded.manifest, request.manifest);
    assert_eq!(decoded.source_paths, request.source_paths);
}

#[tokio::test]
async fn send_request_builds_selected_roots_in_deterministic_tree_order() {
    let selected = tempdir().unwrap();
    let folder = selected.path().join("Album");
    fs::create_dir(&folder).await.unwrap();
    fs::write(folder.join("b.jpg"), b"b").await.unwrap();
    fs::write(folder.join("a.jpg"), b"aa").await.unwrap();
    let note = selected.path().join("note.txt");
    fs::write(&note, b"note").await.unwrap();

    let request = ManifestSendRequest::from_paths(
        ManifestId::new("selected-roots"),
        [folder.clone(), note.clone()],
    )
    .await
    .unwrap();

    assert_eq!(request.manifest.root_count, 2);
    assert_eq!(request.manifest.file_count, 3);
    assert_eq!(request.manifest.directory_count, 1);
    assert_eq!(request.manifest.total_bytes, 7);
    assert_eq!(
        request
            .manifest
            .entries
            .iter()
            .map(|entry| entry.relative_path.as_str())
            .collect::<Vec<_>>(),
        ["Album", "Album/a.jpg", "Album/b.jpg", "note.txt"]
    );
    assert_eq!(request.source_path(1).unwrap(), folder.join("a.jpg"));
    assert_eq!(request.source_path(3).unwrap(), note);
}

#[tokio::test]
async fn send_request_rejects_duplicate_root_names() {
    let selected = tempdir().unwrap();
    let first = selected.path().join("one");
    let second = selected.path().join("two");
    fs::create_dir(&first).await.unwrap();
    fs::create_dir(&second).await.unwrap();
    let first_file = first.join("same.txt");
    let second_file = second.join("same.txt");
    fs::write(&first_file, b"one").await.unwrap();
    fs::write(&second_file, b"two").await.unwrap();

    let result = ManifestSendRequest::from_paths(
        ManifestId::new("duplicate-roots"),
        [first_file, second_file],
    )
    .await;

    assert!(
        matches!(result, Err(CoreError::InvalidInput(message)) if message.contains("same name"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn send_request_rejects_symbolic_links() {
    use std::os::unix::fs::symlink;

    let selected = tempdir().unwrap();
    let source = selected.path().join("source.txt");
    let link = selected.path().join("link.txt");
    fs::write(&source, b"source").await.unwrap();
    symlink(&source, &link).unwrap();

    let result = ManifestSendRequest::from_paths(ManifestId::new("symlink-root"), [link]).await;

    assert!(
        matches!(result, Err(CoreError::InvalidInput(message)) if message.contains("Symbolic links") || message.contains("symbolic links"))
    );
}

fn directory_entry(entry_id: u32, relative_path: &str) -> ManifestEntryV1 {
    ManifestEntryV1 {
        entry_id,
        relative_path: relative_path.into(),
        kind: ManifestEntryKind::Directory,
        size: 0,
        hash: None,
        modified_at_unix_ms: None,
    }
}

fn file_entry(entry_id: u32, relative_path: &str, bytes: &[u8]) -> ManifestEntryV1 {
    ManifestEntryV1 {
        entry_id,
        relative_path: relative_path.into(),
        kind: ManifestEntryKind::RegularFile,
        size: bytes.len() as u64,
        hash: Some(*blake3::hash(bytes).as_bytes()),
        modified_at_unix_ms: None,
    }
}

struct MemoryManifestConnection {
    tx: mpsc::Sender<ManifestFrame>,
    rx: mpsc::Receiver<ManifestFrame>,
    chunks: Arc<AtomicUsize>,
}

#[derive(Default)]
struct HandshakeOnlyManifestConnection {
    sent: Vec<ManifestFrame>,
    returned_hello: bool,
}

#[async_trait]
impl ManifestFrameConnection for HandshakeOnlyManifestConnection {
    async fn send_manifest_frame(&mut self, frame: ManifestFrame) -> Result<(), CoreError> {
        self.sent.push(frame);
        Ok(())
    }

    async fn send_manifest_chunk(
        &mut self,
        _manifest_id: &ManifestId,
        _entry_id: u32,
        _transfer_id: &TransferId,
        _index: u64,
        _offset: u64,
        _bytes: &[u8],
    ) -> Result<(), CoreError> {
        Err(CoreError::Protocol(
            "handshake-only connection cannot send chunks".into(),
        ))
    }

    async fn recv_manifest_frame(&mut self) -> Result<ManifestFrame, CoreError> {
        if self.returned_hello {
            return Err(CoreError::Protocol(
                "handshake-only connection has no further frames".into(),
            ));
        }
        self.returned_hello = true;
        Ok(ManifestFrame::Hello(ManifestHelloV1 {
            protocol_version: MANIFEST_V1_PROTOCOL_VERSION,
            role: PeerRole::Receiver,
        }))
    }
}

fn memory_manifest_connection_pair() -> (
    MemoryManifestConnection,
    MemoryManifestConnection,
    Arc<AtomicUsize>,
) {
    let (sender_tx, receiver_rx) = mpsc::channel(64);
    let (receiver_tx, sender_rx) = mpsc::channel(64);
    let chunks = Arc::new(AtomicUsize::new(0));
    (
        MemoryManifestConnection {
            tx: sender_tx,
            rx: sender_rx,
            chunks: chunks.clone(),
        },
        MemoryManifestConnection {
            tx: receiver_tx,
            rx: receiver_rx,
            chunks: chunks.clone(),
        },
        chunks,
    )
}

#[async_trait]
impl ManifestFrameConnection for MemoryManifestConnection {
    async fn send_manifest_frame(&mut self, frame: ManifestFrame) -> Result<(), CoreError> {
        self.tx
            .send(frame)
            .await
            .map_err(|error| CoreError::Transport(error.to_string()))
    }

    async fn send_manifest_chunk(
        &mut self,
        manifest_id: &ManifestId,
        entry_id: u32,
        transfer_id: &TransferId,
        index: u64,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), CoreError> {
        self.chunks.fetch_add(1, Ordering::SeqCst);
        self.send_manifest_frame(ManifestFrame::Chunk(ManifestChunkV1 {
            manifest_id: manifest_id.clone(),
            entry_id,
            transfer_id: transfer_id.clone(),
            index,
            offset,
            bytes: bytes.to_vec(),
        }))
        .await
    }

    async fn recv_manifest_frame(&mut self) -> Result<ManifestFrame, CoreError> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| CoreError::Transport("memory Manifest connection closed".into()))
    }
}

struct CancelAfterProgressSink {
    cancel: TransferCancelToken,
}

impl ManifestEventSink for CancelAfterProgressSink {
    fn on_manifest_event(&self, event: ManifestTransferEvent) {
        if matches!(
            event,
            ManifestTransferEvent::Progress { entry_bytes, .. } if entry_bytes > 0
        ) {
            self.cancel.cancel();
        }
    }
}

#[derive(Default)]
struct RecordingManifestSink {
    events: Mutex<Vec<ManifestTransferEvent>>,
    plans: Mutex<Vec<(TransferDirection, ManifestV1)>>,
}

impl RecordingManifestSink {
    fn events(&self) -> Vec<ManifestTransferEvent> {
        self.events.lock().unwrap().clone()
    }

    fn plans(&self) -> Vec<(TransferDirection, ManifestV1)> {
        self.plans.lock().unwrap().clone()
    }
}

impl ManifestEventSink for RecordingManifestSink {
    fn on_manifest_plan(&self, direction: TransferDirection, manifest: &ManifestV1) {
        self.plans
            .lock()
            .unwrap()
            .push((direction, manifest.clone()));
    }

    fn on_manifest_event(&self, event: ManifestTransferEvent) {
        self.events.lock().unwrap().push(event);
    }
}
