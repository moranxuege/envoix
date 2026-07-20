use super::*;
use async_trait::async_trait;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use tokio::io::ReadBuf;
use tokio::sync::mpsc;

#[tokio::test]
async fn read_full_chunk_accumulates_short_reads() {
    let mut reader = ShortRead {
        bytes: b"abcdef",
        position: 0,
        max_read: 2,
    };
    let mut buffer = [0_u8; 5];

    let bytes_read = read_full_chunk(&mut reader, &mut buffer).await.unwrap();

    assert_eq!(bytes_read, 5);
    assert_eq!(&buffer, b"abcde");

    let mut buffer = [0_u8; 5];
    let bytes_read = read_full_chunk(&mut reader, &mut buffer).await.unwrap();

    assert_eq!(bytes_read, 1);
    assert_eq!(&buffer[..bytes_read], b"f");
}

#[tokio::test]
async fn transfers_file_over_frame_connection() {
    let root = unique_test_dir();
    let source_dir = root.join("source");
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    let source_path = source_dir.join("hello.txt");
    tokio::fs::write(&source_path, b"hello over frames")
        .await
        .unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(4)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
                .unwrap()
        }
    });

    let send_summary = TransferEngine::new(4)
        .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
        .await
        .unwrap();
    let receive_summary = receiver.await.unwrap();

    assert_eq!(send_summary.bytes_transferred, 17);
    assert_eq!(receive_summary.bytes_transferred, 17);
    assert_eq!(
        tokio::fs::read(output_dir.join("hello.txt")).await.unwrap(),
        b"hello over frames"
    );
}

/// Complete a transfer of `contents` as `file_name` into `output_dir`.
async fn complete_transfer_once(source_path: &Path, output_dir: &Path) {
    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.to_path_buf();
        async move {
            TransferEngine::new(4)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
        }
    });
    TransferEngine::new(4)
        .send_file(
            &mut sender_connection,
            source_path.to_path_buf(),
            true,
            &NoopEventSink,
        )
        .await
        .unwrap();
    receiver.await.unwrap().unwrap();
}

#[tokio::test]
async fn finalize_writes_completion_receipt() {
    let root = unique_test_dir();
    let source_dir = root.join("source");
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    let source_path = source_dir.join("receipt.txt");
    tokio::fs::write(&source_path, b"receipt me").await.unwrap();

    complete_transfer_once(&source_path, &output_dir).await;

    let receipt = LocalFileStorage::read_receipt(&output_dir, "receipt.txt")
        .await
        .unwrap()
        .expect("finalize writes a receipt");
    assert_eq!(receipt.file_name, "receipt.txt");
    assert_eq!(receipt.file_size, 10);
    assert_eq!(
        receipt.file_hash,
        blake3::hash(b"receipt me").to_hex().to_string()
    );
}

#[tokio::test]
async fn reoffer_with_receipt_completes_without_resend() {
    let root = unique_test_dir();
    let source_dir = root.join("source");
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    let source_path = source_dir.join("moved.bin");
    tokio::fs::write(&source_path, b"published and moved away")
        .await
        .unwrap();

    complete_transfer_once(&source_path, &output_dir).await;
    // The app publishes the file elsewhere and deletes the output copy
    // (what Android does); only the receipt remains.
    tokio::fs::remove_file(output_dir.join("moved.bin"))
        .await
        .unwrap();

    // Re-offer: both sides complete, re-delivering the CompleteAck...
    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(4)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
        }
    });
    let send_summary = TransferEngine::new(4)
        .send_file(&mut sender_connection, source_path, true, &NoopEventSink)
        .await
        .expect("re-offer against a receipt completes");
    let receive_summary = receiver
        .await
        .unwrap()
        .expect("receipted receive completes");
    assert_eq!(send_summary.bytes_transferred, 24);
    assert_eq!(receive_summary.bytes_transferred, 24);

    // ...without recreating the file or writing any temp — zero bytes moved.
    let mut entries = tokio::fs::read_dir(&output_dir).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            name.starts_with(".envoix-receipt."),
            "unexpected file recreated by receipted re-offer: {name}"
        );
    }
}

/// Field bug: a receipt from an EARLIER completed transfer must not
/// pre-empt the resume of a NEW in-flight partial of the same file.
#[tokio::test]
async fn receipt_does_not_preempt_an_in_flight_partial() {
    let root = unique_test_dir();
    let source_dir = root.join("source");
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    let source_path = source_dir.join("again.bin");
    let content = b"resume beats receipt";
    tokio::fs::write(&source_path, content).await.unwrap();

    // Transfer once: receipt written; then the file is published away.
    complete_transfer_once(&source_path, &output_dir).await;
    tokio::fs::remove_file(output_dir.join("again.bin"))
        .await
        .unwrap();

    // A NEW transfer of the same file is mid-flight: plant its partial
    // (first 8 bytes) + resume state, as a pause would leave them.
    let tid = TransferId::new("transfer-partial");
    let state = TransferResumeState {
        transfer_id: tid.clone(),
        file_name: "again.bin".into(),
        file_size: content.len() as u64,
        chunk_size: 4,
        bytes_received: 8,
        next_chunk_index: 2,
        hash_bytes: 0,
        hash_checkpoint: None,
        target_file_name: None,
    };
    LocalFileStorage::write_resume_state(&output_dir, &state)
        .await
        .unwrap();
    let temp = LocalFileStorage::resumable_temp_path(&output_dir, "again.bin", &tid).unwrap();
    tokio::fs::write(&temp, &content[..8]).await.unwrap();

    // Resume: the partial must continue (producing the real file), not the
    // receipt path (which produces nothing).
    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(4)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
        }
    });
    let summary = TransferEngine::new(4)
        .send_file(&mut sender_connection, source_path, true, &NoopEventSink)
        .await
        .expect("resume completes");
    receiver.await.unwrap().expect("receive completes");
    assert_eq!(summary.bytes_transferred, content.len() as u64);
    assert_eq!(
        tokio::fs::read(output_dir.join("again.bin")).await.unwrap(),
        content,
        "the partial path must produce the real file - the receipt path produces none"
    );
}

#[tokio::test]
async fn reoffer_with_different_content_is_refused() {
    let root = unique_test_dir();
    let source_dir = root.join("source");
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    let source_path = source_dir.join("swap.bin");
    tokio::fs::write(&source_path, b"original content")
        .await
        .unwrap();

    complete_transfer_once(&source_path, &output_dir).await;
    tokio::fs::remove_file(output_dir.join("swap.bin"))
        .await
        .unwrap();
    // Same name, same size, different bytes.
    tokio::fs::write(&source_path, b"altered contents")
        .await
        .unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(4)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
        }
    });
    let send_result = TransferEngine::new(4)
        .send_file(&mut sender_connection, source_path, true, &NoopEventSink)
        .await;
    let receive_result = receiver.await.unwrap();

    assert!(send_result.is_err(), "sender must not report success");
    let error = receive_result.unwrap_err();
    assert!(
        matches!(&error, CoreError::Storage(m) if m.contains("different content")),
        "unexpected receiver error: {error:?}"
    );
}

#[tokio::test]
async fn fresh_send_ignores_receipt_and_retransfers() {
    let root = unique_test_dir();
    let source_dir = root.join("source");
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    let source_path = source_dir.join("fresh.bin");
    tokio::fs::write(&source_path, b"fresh again")
        .await
        .unwrap();

    complete_transfer_once(&source_path, &output_dir).await;
    tokio::fs::remove_file(output_dir.join("fresh.bin"))
        .await
        .unwrap();

    // resume=false (--fresh): the receipt is ignored, the file re-transfers.
    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(4)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
        }
    });
    TransferEngine::new(4)
        .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
        .await
        .unwrap();
    receiver.await.unwrap().unwrap();
    assert_eq!(
        tokio::fs::read(output_dir.join("fresh.bin")).await.unwrap(),
        b"fresh again"
    );
}

#[tokio::test]
async fn rejects_sender_receiver_chunk_size_mismatch() {
    let root = unique_test_dir();
    let source_dir = root.join("source");
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    let source_path = source_dir.join("mismatch.txt");
    tokio::fs::write(&source_path, b"chunk size mismatch")
        .await
        .unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(8)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
        }
    });

    let send_error = TransferEngine::new(4)
        .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
        .await
        .unwrap_err();
    let receive_error = receiver.await.unwrap().unwrap_err();

    assert!(matches!(
        send_error,
        CoreError::Transport(_) | CoreError::Transfer(_)
    ));
    assert!(matches!(receive_error, CoreError::Transfer(_)));
    assert!(
        !fs::try_exists(output_dir.join("mismatch.txt"))
            .await
            .unwrap()
    );
    assert_no_sidecars(&output_dir).await;
}

#[tokio::test]
async fn resumes_after_receiver_stops_mid_transfer() {
    let root = unique_test_dir();
    let source_dir = root.join("source");
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    let source_path = source_dir.join("resume.txt");
    tokio::fs::write(&source_path, b"resume over two connections")
        .await
        .unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let stopped = std::sync::Arc::new(AtomicBool::new(false));
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        let stopped = stopped.clone();
        async move {
            TransferEngine::new(4)
                .receive_file(
                    &mut receiver_connection,
                    output_dir,
                    &StopAfterBytesSink { bytes: 8, stopped },
                )
                .await
        }
    });

    let send_error = TransferEngine::new(4)
        .send_file(
            &mut sender_connection,
            source_path.clone(),
            false,
            &NoopEventSink,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        send_error,
        CoreError::Transport(_) | CoreError::Transfer(_)
    ));
    match receiver.await {
        Ok(result) => assert!(result.is_err() || stopped.load(Ordering::SeqCst)),
        Err(_) => assert!(stopped.load(Ordering::SeqCst)),
    }

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(4)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
                .unwrap()
        }
    });

    let send_summary = TransferEngine::new(4)
        .send_file(&mut sender_connection, source_path, true, &NoopEventSink)
        .await
        .unwrap();
    let receive_summary = receiver.await.unwrap();

    assert_eq!(send_summary.bytes_transferred, 27);
    assert_eq!(receive_summary.bytes_transferred, 27);
    assert_eq!(
        tokio::fs::read(output_dir.join("resume.txt"))
            .await
            .unwrap(),
        b"resume over two connections"
    );
}

#[tokio::test]
async fn sender_fails_when_receiver_drops_before_ack() {
    // The CompleteAck is the receiver's proof that it finalized, so the sender
    // requires it. If the receiver reads Complete but drops without acking
    // (a crash / network death before finalizing), the sender must report
    // failure - never a false success. In the healthy path this cannot happen:
    // the receiver holds the connection open until the sender closes, so the
    // ack is delivered; the rare true-network-death case is recovered by resume
    // on a retry.
    let root = unique_test_dir();
    let source_dir = root.join("source");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    let source_path = source_dir.join("race.txt");
    tokio::fs::write(&source_path, b"receiver vanishes before acking")
        .await
        .unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn(async move {
        let transfer_id = receive_header_and_resume(&mut receiver_connection).await;
        // Drain chunks, then drop the connection on Complete WITHOUT acking.
        loop {
            match receiver_connection.recv_frame().await.unwrap() {
                Frame::Chunk(_) => {}
                Frame::Complete(complete) => {
                    assert_eq!(complete.transfer_id, transfer_id);
                    break;
                }
                other => panic!("unexpected frame while draining: {other:?}"),
            }
        }
        // receiver_connection is dropped here without a CompleteAck.
    });

    let result = TransferEngine::new(4)
        .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
        .await;
    assert!(
        result.is_err(),
        "sender must fail when the receiver never sends CompleteAck"
    );
    receiver.await.unwrap();
}

#[tokio::test]
async fn sender_times_out_when_receiver_never_acks_but_keeps_connection_open() {
    let root = unique_test_dir();
    let source_dir = root.join("source");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    let source_path = source_dir.join("stalled-ack.txt");
    tokio::fs::write(&source_path, b"receiver stalls before acking")
        .await
        .unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn(async move {
        let transfer_id = receive_header_and_resume(&mut receiver_connection).await;
        loop {
            match receiver_connection.recv_frame().await.unwrap() {
                Frame::Chunk(_) => {}
                Frame::Complete(complete) => {
                    assert_eq!(complete.transfer_id, transfer_id);
                    break;
                }
                other => panic!("unexpected frame while draining: {other:?}"),
            }
        }
        let _hold_connection = receiver_connection;
        std::future::pending::<()>().await;
    });

    let error = TransferEngine::new(4)
        .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        CoreError::Transfer(message) if message.contains("did not confirm completion")
    ));
    receiver.abort();
    let _ = receiver.await;
}

#[tokio::test]
async fn sender_fails_when_receiver_reports_error_after_complete() {
    // A genuine post-Complete failure on the receiver (hash mismatch, finalize
    // error, ...) is signaled with an Error frame; the sender must surface it
    // as a failure - not swallow it like the benign ack-lost close race.
    let root = unique_test_dir();
    let source_dir = root.join("source");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    let source_path = source_dir.join("rejected.txt");
    tokio::fs::write(&source_path, b"payload the receiver will reject")
        .await
        .unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn(async move {
        let transfer_id = receive_header_and_resume(&mut receiver_connection).await;
        loop {
            match receiver_connection.recv_frame().await.unwrap() {
                Frame::Chunk(_) => {}
                Frame::Complete(complete) => {
                    assert_eq!(complete.transfer_id, transfer_id);
                    break;
                }
                other => panic!("unexpected frame while draining: {other:?}"),
            }
        }
        // Simulate a receiver-side finalize/verify failure after Complete.
        receiver_connection
            .send_frame(Frame::Error(ErrorFrame {
                message: "completed file hash mismatch".into(),
            }))
            .await
            .unwrap();
    });

    let result = TransferEngine::new(4)
        .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
        .await;
    assert!(
        result.is_err(),
        "sender must fail when the receiver reports a post-Complete error"
    );
    receiver.await.unwrap();
}

#[tokio::test]
async fn cancellation_notifies_peer_with_error_frame() {
    let root = unique_test_dir();
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let cancel = TransferCancelToken::new();
    let receiver_cancel = cancel.clone();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(4)
                .receive_file_with_cancel(
                    &mut receiver_connection,
                    output_dir,
                    &NoopEventSink,
                    &receiver_cancel,
                )
                .await
        }
    });

    let transfer_id = TransferId::new("cancel-transfer");
    sender_connection
        .send_frame(Frame::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            role: PeerRole::Sender,
        }))
        .await
        .unwrap();
    expect_ready(sender_connection.recv_frame().await.unwrap()).unwrap();
    sender_connection
        .send_frame(Frame::FileHeader(FileHeader {
            transfer_id: transfer_id.clone(),
            file_name: "cancel.txt".into(),
            file_size: 8,
            chunk_size: 4,
            resume_requested: false,
        }))
        .await
        .unwrap();
    expect_resume_status(
        sender_connection.recv_frame().await.unwrap(),
        &transfer_id,
        4,
    )
    .unwrap();

    cancel.cancel();

    let frame = sender_connection.recv_frame().await.unwrap();
    assert!(matches!(
        frame,
        Frame::Error(ErrorFrame { message }) if message == USER_INTERRUPT_MESSAGE
    ));
    let error = receiver.await.unwrap().unwrap_err();
    assert!(matches!(
        error,
        CoreError::Transfer(message) if message == USER_INTERRUPT_MESSAGE
    ));
}

#[tokio::test]
async fn pause_notifies_peer_with_pause_message() {
    let root = unique_test_dir();
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let cancel = TransferCancelToken::new();
    let receiver_cancel = cancel.clone();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(4)
                .receive_file_with_cancel(
                    &mut receiver_connection,
                    output_dir,
                    &NoopEventSink,
                    &receiver_cancel,
                )
                .await
        }
    });

    let transfer_id = TransferId::new("pause-transfer");
    sender_connection
        .send_frame(Frame::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            role: PeerRole::Sender,
        }))
        .await
        .unwrap();
    expect_ready(sender_connection.recv_frame().await.unwrap()).unwrap();
    sender_connection
        .send_frame(Frame::FileHeader(FileHeader {
            transfer_id: transfer_id.clone(),
            file_name: "pause.txt".into(),
            file_size: 8,
            chunk_size: 4,
            resume_requested: false,
        }))
        .await
        .unwrap();
    expect_resume_status(
        sender_connection.recv_frame().await.unwrap(),
        &transfer_id,
        4,
    )
    .unwrap();

    cancel.pause();

    // The peer learns it was a pause, not a cancel (best-effort frame).
    let frame = sender_connection.recv_frame().await.unwrap();
    assert!(matches!(
        frame,
        Frame::Error(ErrorFrame { message }) if message == USER_PAUSE_MESSAGE
    ));
    // Locally the stop is reported as a pause too.
    let error = receiver.await.unwrap().unwrap_err();
    assert!(matches!(
        error,
        CoreError::Transfer(message) if message == USER_PAUSE_MESSAGE
    ));
}

#[tokio::test]
async fn peer_pause_message_maps_to_paused_by_peer() {
    // peer_error turns the peer's pause frame into the canonical local text.
    let error = peer_error(ErrorFrame {
        message: USER_PAUSE_MESSAGE.into(),
    });
    assert!(matches!(
        error,
        CoreError::Transfer(message) if message == PEER_PAUSE_MESSAGE
    ));
}

#[tokio::test]
async fn sender_reports_explicit_peer_interrupt() {
    let root = unique_test_dir();
    let source_dir = root.join("source");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    let source_path = source_dir.join("peer-interrupt.txt");
    tokio::fs::write(&source_path, b"peer interrupt")
        .await
        .unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn(async move {
        let transfer_id = receive_header_and_resume(&mut receiver_connection).await;
        loop {
            match receiver_connection.recv_frame().await.unwrap() {
                Frame::Chunk(_) => {}
                Frame::Complete(_) => {
                    receiver_connection
                        .send_frame(Frame::Error(ErrorFrame {
                            message: USER_INTERRUPT_MESSAGE.into(),
                        }))
                        .await
                        .unwrap();
                    break transfer_id;
                }
                frame => panic!("unexpected frame: {frame:?}"),
            }
        }
    });

    let error = TransferEngine::new(4)
        .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
        .await
        .unwrap_err();

    receiver.await.unwrap();
    assert!(matches!(
        error,
        CoreError::Transfer(message) if message == "transfer interrupted by peer"
    ));
}

#[tokio::test]
async fn sender_reports_peer_close_during_send() {
    let root = unique_test_dir();
    let source_dir = root.join("source");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    let source_path = source_dir.join("peer-close.txt");
    tokio::fs::write(&source_path, b"peer closed while sending")
        .await
        .unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn(async move {
        receive_header_and_resume(&mut receiver_connection).await;
    });

    let error = TransferEngine::new(4)
        .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
        .await
        .unwrap_err();

    receiver.await.unwrap();
    assert!(matches!(
        error,
        CoreError::Transfer(message) if message == "connection closed by peer"
    ));
}

#[tokio::test]
async fn corrupted_temp_prefix_restarts_from_zero() {
    let root = unique_test_dir();
    let source_dir = root.join("source");
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    tokio::fs::create_dir_all(&output_dir).await.unwrap();
    let source_path = source_dir.join("corrupt.txt");
    let source_bytes = b"abcdefghij";
    tokio::fs::write(&source_path, source_bytes).await.unwrap();

    let transfer_id = TransferId::new("old-transfer");
    let state = TransferResumeState {
        transfer_id: transfer_id.clone(),
        file_name: "corrupt.txt".into(),
        file_size: source_bytes.len() as u64,
        chunk_size: 5,
        bytes_received: 5,
        next_chunk_index: 1,
        hash_bytes: 5,
        hash_checkpoint: Some(blake3::hash(b"abcde").to_hex().to_string()),
        target_file_name: None,
    };
    LocalFileStorage::write_resume_state(&output_dir, &state)
        .await
        .unwrap();
    let temp_path =
        LocalFileStorage::resumable_temp_path(&output_dir, "corrupt.txt", &transfer_id)
            .unwrap();
    tokio::fs::write(&temp_path, b"xxxxx").await.unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(5)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
                .unwrap()
        }
    });

    let send_summary = TransferEngine::new(5)
        .send_file(&mut sender_connection, source_path, true, &NoopEventSink)
        .await
        .unwrap();
    let receive_summary = receiver.await.unwrap();

    assert_eq!(send_summary.bytes_transferred, source_bytes.len() as u64);
    assert_eq!(receive_summary.bytes_transferred, source_bytes.len() as u64);
    assert_eq!(
        tokio::fs::read(output_dir.join("corrupt.txt"))
            .await
            .unwrap(),
        source_bytes
    );
    assert!(!fs::try_exists(temp_path).await.unwrap());
}

#[tokio::test]
async fn resumes_from_temp_file_when_sidecar_offset_is_stale() {
    let root = unique_test_dir();
    let source_dir = root.join("source");
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    tokio::fs::create_dir_all(&output_dir).await.unwrap();
    let source_path = source_dir.join("stale-sidecar.txt");
    let source_bytes = b"abcdefghij";
    tokio::fs::write(&source_path, source_bytes).await.unwrap();

    let transfer_id = TransferId::new("old-transfer");
    let state = TransferResumeState {
        transfer_id: transfer_id.clone(),
        file_name: "stale-sidecar.txt".into(),
        file_size: source_bytes.len() as u64,
        chunk_size: 5,
        bytes_received: 0,
        next_chunk_index: 0,
        hash_bytes: 0,
        hash_checkpoint: None,
        target_file_name: None,
    };
    LocalFileStorage::write_resume_state(&output_dir, &state)
        .await
        .unwrap();
    let temp_path =
        LocalFileStorage::resumable_temp_path(&output_dir, "stale-sidecar.txt", &transfer_id)
            .unwrap();
    tokio::fs::write(&temp_path, b"abcde").await.unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(5)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
                .unwrap()
        }
    });

    let send_summary = TransferEngine::new(5)
        .send_file(&mut sender_connection, source_path, true, &NoopEventSink)
        .await
        .unwrap();
    let receive_summary = receiver.await.unwrap();

    assert_eq!(send_summary.bytes_transferred, 10);
    assert_eq!(receive_summary.bytes_transferred, 10);
    assert_eq!(
        tokio::fs::read(output_dir.join("stale-sidecar.txt"))
            .await
            .unwrap(),
        source_bytes
    );
}

#[tokio::test]
async fn no_resume_ignores_compatible_sidecar() {
    let root = unique_test_dir();
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();
    let source_bytes = b"abcdefghij";
    let old_transfer_id = TransferId::new("old-transfer");
    let state = TransferResumeState {
        transfer_id: old_transfer_id.clone(),
        file_name: "fresh.txt".into(),
        file_size: source_bytes.len() as u64,
        chunk_size: 5,
        bytes_received: 5,
        next_chunk_index: 1,
        hash_bytes: 5,
        hash_checkpoint: Some(blake3::hash(b"abcde").to_hex().to_string()),
        target_file_name: None,
    };
    LocalFileStorage::write_resume_state(&output_dir, &state)
        .await
        .unwrap();
    let temp_path =
        LocalFileStorage::resumable_temp_path(&output_dir, "fresh.txt", &old_transfer_id)
            .unwrap();
    tokio::fs::write(&temp_path, b"abcde").await.unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(5)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
                .unwrap()
        }
    });

    manual_send(
        &mut sender_connection,
        ManualSend {
            transfer_id: "manual-transfer",
            file_name: "fresh.txt",
            source_bytes,
            chunk_size: 5,
            resume_requested: false,
            bytes_to_send: source_bytes,
            complete_hash: blake3::hash(source_bytes).to_hex().to_string(),
            expected_resume_bytes: 0,
        },
    )
    .await;
    let receive_summary = receiver.await.unwrap();

    assert_eq!(receive_summary.bytes_transferred, source_bytes.len() as u64);
    assert_eq!(
        tokio::fs::read(output_dir.join("fresh.txt")).await.unwrap(),
        source_bytes
    );
}

#[tokio::test]
async fn temp_shorter_than_state_rejects_resume_candidate() {
    let root = unique_test_dir();
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();
    let source_bytes = b"abcdefghij";
    let old_transfer_id = TransferId::new("old-transfer");
    let state = TransferResumeState {
        transfer_id: old_transfer_id.clone(),
        file_name: "short-temp.txt".into(),
        file_size: source_bytes.len() as u64,
        chunk_size: 5,
        bytes_received: 5,
        next_chunk_index: 1,
        hash_bytes: 5,
        hash_checkpoint: Some(blake3::hash(b"abcde").to_hex().to_string()),
        target_file_name: None,
    };
    LocalFileStorage::write_resume_state(&output_dir, &state)
        .await
        .unwrap();
    let temp_path =
        LocalFileStorage::resumable_temp_path(&output_dir, "short-temp.txt", &old_transfer_id)
            .unwrap();
    tokio::fs::write(&temp_path, b"abc").await.unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(5)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
                .unwrap()
        }
    });

    manual_send(
        &mut sender_connection,
        ManualSend {
            transfer_id: "manual-transfer",
            file_name: "short-temp.txt",
            source_bytes,
            chunk_size: 5,
            resume_requested: true,
            bytes_to_send: source_bytes,
            complete_hash: blake3::hash(source_bytes).to_hex().to_string(),
            expected_resume_bytes: 0,
        },
    )
    .await;
    let receive_summary = receiver.await.unwrap();

    assert_eq!(receive_summary.bytes_transferred, source_bytes.len() as u64);
    assert_eq!(
        tokio::fs::read(output_dir.join("short-temp.txt"))
            .await
            .unwrap(),
        source_bytes
    );
    assert!(
        LocalFileStorage::read_resume_state(&output_dir, "short-temp.txt", &old_transfer_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!fs::try_exists(temp_path).await.unwrap());
}

#[tokio::test]
async fn inconsistent_resume_index_fails_explicitly() {
    let root = unique_test_dir();
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();
    let source_bytes = b"abcdefghij";
    let old_transfer_id = TransferId::new("old-transfer");
    let state = TransferResumeState {
        transfer_id: old_transfer_id.clone(),
        file_name: "bad-state.txt".into(),
        file_size: source_bytes.len() as u64,
        chunk_size: 5,
        bytes_received: 5,
        next_chunk_index: 7,
        hash_bytes: 5,
        hash_checkpoint: Some(blake3::hash(b"abcde").to_hex().to_string()),
        target_file_name: None,
    };
    LocalFileStorage::write_resume_state(&output_dir, &state)
        .await
        .unwrap();
    let temp_path =
        LocalFileStorage::resumable_temp_path(&output_dir, "bad-state.txt", &old_transfer_id)
            .unwrap();
    tokio::fs::write(&temp_path, b"abcde").await.unwrap();

    let header = FileHeader {
        transfer_id: TransferId::new("new-transfer"),
        file_name: "bad-state.txt".into(),
        file_size: source_bytes.len() as u64,
        chunk_size: 5,
        resume_requested: true,
    };
    let error = prepare_existing_resume_state(&output_dir, &header, state)
        .await
        .unwrap_err();

    assert!(
        matches!(error, CoreError::Transfer(message) if message.contains("inconsistent chunk index"))
    );
    assert!(
        LocalFileStorage::read_resume_state(&output_dir, "bad-state.txt", &old_transfer_id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(fs::try_exists(temp_path).await.unwrap());
}

#[tokio::test]
async fn final_hash_mismatch_does_not_finalize_file() {
    let root = unique_test_dir();
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();
    let source_bytes = b"abcdefghij";

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(5)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
        }
    });

    manual_send(
        &mut sender_connection,
        ManualSend {
            transfer_id: "manual-transfer",
            file_name: "bad-hash.txt",
            source_bytes,
            chunk_size: 5,
            resume_requested: false,
            bytes_to_send: source_bytes,
            complete_hash: "not-the-right-hash".into(),
            expected_resume_bytes: 0,
        },
    )
    .await;
    let receive_error = receiver.await.unwrap().unwrap_err();

    assert!(matches!(receive_error, CoreError::Transfer(_)));
    assert!(
        !fs::try_exists(output_dir.join("bad-hash.txt"))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn already_complete_matching_file_returns_success() {
    let root = unique_test_dir();
    let source_dir = root.join("source");
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    tokio::fs::create_dir_all(&output_dir).await.unwrap();
    let source_path = source_dir.join("done.txt");
    tokio::fs::write(&source_path, b"already done")
        .await
        .unwrap();
    tokio::fs::write(output_dir.join("done.txt"), b"already done")
        .await
        .unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(4)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
                .unwrap()
        }
    });

    let send_summary = TransferEngine::new(4)
        .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
        .await
        .unwrap();
    let receive_summary = receiver.await.unwrap();

    assert_eq!(send_summary.bytes_transferred, 12);
    assert_eq!(receive_summary.bytes_transferred, 12);
}

/// Lost-ack window: the sender vanishes right after Complete, before the
/// ack can be delivered. The file is finalized (a durable fact), so the
/// receive must still complete - suppressing Completed here would also
/// suppress the mailbox receipt post that exists for exactly this case.
#[tokio::test]
async fn receiver_completes_when_the_ack_cannot_be_delivered() {
    let root = unique_test_dir();
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();
    let source_bytes = b"abcdefghij";

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(5)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
        }
    });

    let transfer_id = TransferId::new("manual-transfer");
    sender_connection
        .send_frame(Frame::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            role: PeerRole::Sender,
        }))
        .await
        .unwrap();
    expect_ready(sender_connection.recv_frame().await.unwrap()).unwrap();
    sender_connection
        .send_frame(Frame::FileHeader(FileHeader {
            transfer_id: transfer_id.clone(),
            file_name: "data.bin".into(),
            file_size: source_bytes.len() as u64,
            chunk_size: 5,
            resume_requested: false,
        }))
        .await
        .unwrap();
    expect_resume_status(
        sender_connection.recv_frame().await.unwrap(),
        &transfer_id,
        5,
    )
    .unwrap();
    for (index, chunk) in source_bytes.chunks(5).enumerate() {
        sender_connection
            .send_frame(Frame::Chunk(Chunk {
                transfer_id: transfer_id.clone(),
                index: index as u64,
                offset: index as u64 * 5,
                bytes: chunk.to_vec(),
            }))
            .await
            .unwrap();
    }
    sender_connection
        .send_frame(Frame::Complete(Complete {
            transfer_id: transfer_id.clone(),
            file_hash: blake3::hash(source_bytes).to_hex().to_string(),
        }))
        .await
        .unwrap();
    // The sender dies without reading the ack.
    drop(sender_connection);

    let summary = receiver
        .await
        .unwrap()
        .expect("finalized receive must complete");
    assert_eq!(summary.bytes_transferred, source_bytes.len() as u64);
    assert_eq!(
        tokio::fs::read(output_dir.join("data.bin")).await.unwrap(),
        source_bytes
    );
}

/// PR #48 review P1: two concurrent receives of the same name into one
/// directory both selected it, both passed the finalize existence check,
/// and the second rename silently REPLACED the first completed file. The
/// atomic claim refuses the taken name and the loser lands beside it.
#[tokio::test]
async fn concurrent_same_name_receives_never_destroy_each_other() {
    let root = unique_test_dir();
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();
    let bytes_a = b"aaaaaaaaaa";
    let bytes_b = b"bbbbbbbbbb";

    let (mut sender_a, mut receiver_conn_a) = memory_connection_pair();
    let (mut sender_b, mut receiver_conn_b) = memory_connection_pair();
    let recv_a = tokio::spawn({
        let dir = output_dir.clone();
        async move {
            TransferEngine::new(5)
                .receive_file(&mut receiver_conn_a, dir, &NoopEventSink)
                .await
                .unwrap()
        }
    });
    let recv_b = tokio::spawn({
        let dir = output_dir.clone();
        async move {
            TransferEngine::new(5)
                .receive_file(&mut receiver_conn_b, dir, &NoopEventSink)
                .await
                .unwrap()
        }
    });

    // Both offers use the SAME name; neither sees a collision at start
    // (the dir is empty), so both try to claim "c.bin" at finalize.
    tokio::join!(
        manual_send(
            &mut sender_a,
            ManualSend {
                transfer_id: "transfer-a",
                file_name: "c.bin",
                source_bytes: bytes_a,
                chunk_size: 5,
                resume_requested: false,
                bytes_to_send: bytes_a,
                complete_hash: blake3::hash(bytes_a).to_hex().to_string(),
                expected_resume_bytes: 0,
            },
        ),
        manual_send(
            &mut sender_b,
            ManualSend {
                transfer_id: "transfer-b",
                file_name: "c.bin",
                source_bytes: bytes_b,
                chunk_size: 5,
                resume_requested: false,
                bytes_to_send: bytes_b,
                complete_hash: blake3::hash(bytes_b).to_hex().to_string(),
                expected_resume_bytes: 0,
            },
        ),
    );
    let summary_a = recv_a.await.unwrap();
    let summary_b = recv_b.await.unwrap();

    let landed_a = tokio::fs::read(output_dir.join(&summary_a.file_name))
        .await
        .unwrap();
    let landed_b = tokio::fs::read(output_dir.join(&summary_b.file_name))
        .await
        .unwrap();
    assert_ne!(
        summary_a.file_name, summary_b.file_name,
        "distinct landed names"
    );
    assert_eq!(landed_a, bytes_a, "receive A intact under its landed name");
    assert_eq!(landed_b, bytes_b, "receive B intact under its landed name");
}

/// Crash-repair: finalize commits the file before the receipt, so a death
/// in between leaves possession without proof. The existing-final recovery
/// path must recreate the receipt - PostReceipt seals from it, and without
/// it the confirmation duty is undischargeable forever.
#[tokio::test]
async fn existing_final_recovery_repairs_a_missing_receipt() {
    let root = unique_test_dir();
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();
    let source_bytes = b"abcdefghij";
    // The crash left the final file but no receipt sidecar.
    tokio::fs::write(output_dir.join("data.bin"), source_bytes)
        .await
        .unwrap();
    assert!(
        LocalFileStorage::read_receipt(&output_dir, "data.bin")
            .await
            .unwrap()
            .is_none()
    );

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(5)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
                .unwrap()
        }
    });

    manual_send(
        &mut sender_connection,
        ManualSend {
            transfer_id: "manual-transfer",
            file_name: "data.bin",
            source_bytes,
            chunk_size: 5,
            resume_requested: true,
            bytes_to_send: source_bytes,
            complete_hash: blake3::hash(source_bytes).to_hex().to_string(),
            expected_resume_bytes: source_bytes.len() as u64,
        },
    )
    .await;
    receiver.await.unwrap();

    let receipt = LocalFileStorage::read_receipt(&output_dir, "data.bin")
        .await
        .unwrap()
        .expect("recovery repaired the receipt");
    assert_eq!(
        receipt.file_hash,
        blake3::hash(source_bytes).to_hex().to_string()
    );
}

/// Field bug (rooms 223606/135499): a fresh send of an already-present
/// file "completed" in 308ms off the existing final - the fresh request
/// was silently ignored. A fresh offer must move real bytes and land
/// beside the original under a free name.
#[tokio::test]
async fn fresh_offer_beside_existing_final_lands_real_copy_under_new_name() {
    let root = unique_test_dir();
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();
    let source_bytes = b"abcdefghij";
    tokio::fs::write(output_dir.join("data.bin"), source_bytes)
        .await
        .unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(5)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
                .unwrap()
        }
    });

    manual_send(
        &mut sender_connection,
        ManualSend {
            transfer_id: "manual-transfer",
            file_name: "data.bin",
            source_bytes,
            chunk_size: 5,
            resume_requested: false,
            bytes_to_send: source_bytes,
            complete_hash: blake3::hash(source_bytes).to_hex().to_string(),
            expected_resume_bytes: 0,
        },
    )
    .await;
    let receive_summary = receiver.await.unwrap();

    assert_eq!(
        receive_summary.bytes_transferred,
        source_bytes.len() as u64,
        "fresh offer must transfer real bytes, not answer from the final"
    );
    assert_eq!(
        tokio::fs::read(output_dir.join("data (1).bin"))
            .await
            .unwrap(),
        source_bytes,
        "fresh copy lands beside the original under a uniquified name"
    );
    assert_eq!(
        tokio::fs::read(output_dir.join("data.bin")).await.unwrap(),
        source_bytes,
        "the original file is untouched"
    );
    let receipt = LocalFileStorage::read_receipt(&output_dir, "data (1).bin")
        .await
        .unwrap()
        .expect("receipt is keyed by the file that actually exists");
    assert_eq!(receipt.file_size, source_bytes.len() as u64);
}

/// The pause+resume half of the same bug: an interrupted fresh re-receive
/// records its landing name, and the resumed attempt (resume_requested is
/// true on attempt 2+) must continue into that name - not be instantly
/// answered by the same-name final that made it uniquify in the first place.
#[tokio::test]
async fn interrupted_fresh_offer_resumes_into_recorded_target() {
    let root = unique_test_dir();
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();
    let source_bytes = b"abcdefghij";
    tokio::fs::write(output_dir.join("data.bin"), source_bytes)
        .await
        .unwrap();
    // The state an interrupted fresh attempt leaves behind: 5 of 10 bytes
    // in the temp, landing name recorded.
    let old_transfer_id = TransferId::new("old-transfer");
    let state = TransferResumeState {
        transfer_id: old_transfer_id.clone(),
        file_name: "data.bin".into(),
        file_size: source_bytes.len() as u64,
        chunk_size: 5,
        bytes_received: 5,
        next_chunk_index: 1,
        hash_bytes: 5,
        hash_checkpoint: Some(blake3::hash(b"abcde").to_hex().to_string()),
        target_file_name: Some("data (1).bin".into()),
    };
    LocalFileStorage::write_resume_state(&output_dir, &state)
        .await
        .unwrap();
    let temp_path =
        LocalFileStorage::resumable_temp_path(&output_dir, "data.bin", &old_transfer_id)
            .unwrap();
    tokio::fs::write(&temp_path, b"abcde").await.unwrap();

    let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
    let receiver = tokio::spawn({
        let output_dir = output_dir.clone();
        async move {
            TransferEngine::new(5)
                .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                .await
                .unwrap()
        }
    });

    manual_send(
        &mut sender_connection,
        ManualSend {
            transfer_id: "manual-transfer",
            file_name: "data.bin",
            source_bytes,
            chunk_size: 5,
            resume_requested: true,
            bytes_to_send: source_bytes,
            complete_hash: blake3::hash(source_bytes).to_hex().to_string(),
            expected_resume_bytes: 5,
        },
    )
    .await;
    let receive_summary = receiver.await.unwrap();

    assert_eq!(receive_summary.bytes_transferred, source_bytes.len() as u64);
    assert_eq!(
        tokio::fs::read(output_dir.join("data (1).bin"))
            .await
            .unwrap(),
        source_bytes,
        "resume continues into the recorded landing name"
    );
    assert_eq!(
        tokio::fs::read(output_dir.join("data.bin")).await.unwrap(),
        source_bytes,
        "the original file is untouched"
    );
}

#[tokio::test]
async fn unique_final_name_skips_taken_names() {
    let root = unique_test_dir();
    let output_dir = root.join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();
    tokio::fs::write(output_dir.join("a.txt"), b"x")
        .await
        .unwrap();
    tokio::fs::write(output_dir.join("a (1).txt"), b"x")
        .await
        .unwrap();
    assert_eq!(
        unique_final_name(&output_dir, "a.txt").await.unwrap(),
        "a (2).txt"
    );
    assert_eq!(
        unique_final_name(&output_dir, "noext").await.unwrap(),
        "noext (1)"
    );
    assert_eq!(
        unique_final_name(&output_dir, ".dotfile").await.unwrap(),
        ".dotfile (1)"
    );
}

struct MemoryFrameConnection {
    tx: mpsc::Sender<Frame>,
    rx: mpsc::Receiver<Frame>,
}

fn memory_connection_pair() -> (MemoryFrameConnection, MemoryFrameConnection) {
    let (sender_tx, receiver_rx) = mpsc::channel(16);
    let (receiver_tx, sender_rx) = mpsc::channel(16);

    (
        MemoryFrameConnection {
            tx: sender_tx,
            rx: sender_rx,
        },
        MemoryFrameConnection {
            tx: receiver_tx,
            rx: receiver_rx,
        },
    )
}

struct ManualSend<'a> {
    transfer_id: &'a str,
    file_name: &'a str,
    source_bytes: &'a [u8],
    chunk_size: u64,
    resume_requested: bool,
    bytes_to_send: &'a [u8],
    complete_hash: String,
    expected_resume_bytes: u64,
}

async fn manual_send(connection: &mut MemoryFrameConnection, request: ManualSend<'_>) {
    let transfer_id = TransferId::new(request.transfer_id);
    connection
        .send_frame(Frame::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            role: PeerRole::Sender,
        }))
        .await
        .unwrap();
    expect_ready(connection.recv_frame().await.unwrap()).unwrap();
    connection
        .send_frame(Frame::FileHeader(FileHeader {
            transfer_id: transfer_id.clone(),
            file_name: request.file_name.into(),
            file_size: request.source_bytes.len() as u64,
            chunk_size: request.chunk_size,
            resume_requested: request.resume_requested,
        }))
        .await
        .unwrap();
    let resume_status = expect_resume_status(
        connection.recv_frame().await.unwrap(),
        &transfer_id,
        request.chunk_size as usize,
    )
    .unwrap();
    assert_eq!(resume_status.bytes_received, request.expected_resume_bytes);

    let mut offset = resume_status.bytes_received;
    for (index, chunk) in (resume_status.next_chunk_index..).zip(
        request.bytes_to_send[resume_status.bytes_received as usize..]
            .chunks(request.chunk_size as usize),
    ) {
        connection
            .send_frame(Frame::Chunk(Chunk {
                transfer_id: transfer_id.clone(),
                index,
                offset,
                bytes: chunk.to_vec(),
            }))
            .await
            .unwrap();
        offset += chunk.len() as u64;
    }
    connection
        .send_frame(Frame::Complete(Complete {
            transfer_id: transfer_id.clone(),
            file_hash: request.complete_hash.clone(),
        }))
        .await
        .unwrap();
    if request.complete_hash == blake3::hash(request.source_bytes).to_hex().as_str() {
        expect_complete_ack(connection.recv_frame().await.unwrap(), &transfer_id).unwrap();
    }
}

async fn receive_header_and_resume(connection: &mut MemoryFrameConnection) -> TransferId {
    expect_sender_hello(connection.recv_frame().await.unwrap()).unwrap();
    connection.send_frame(Frame::Ready(Ready)).await.unwrap();
    let header = expect_file_header(connection.recv_frame().await.unwrap()).unwrap();
    send_resume_status(connection, &header.transfer_id, 0, 0, String::new())
        .await
        .unwrap();
    header.transfer_id
}

async fn assert_no_sidecars(output_dir: &Path) {
    if !fs::try_exists(output_dir).await.unwrap() {
        return;
    }

    let mut entries = fs::read_dir(output_dir).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        assert!(
            !(name.ends_with(".json") || name.ends_with(".part")),
            "unexpected sidecar: {name}"
        );
    }
}

#[async_trait]
impl FrameConnection for MemoryFrameConnection {
    async fn send_frame(&mut self, frame: Frame) -> Result<(), CoreError> {
        self.tx
            .send(frame)
            .await
            .map_err(|error| CoreError::Transport(error.to_string()))
    }

    async fn send_chunk(
        &mut self,
        transfer_id: &TransferId,
        index: u64,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), CoreError> {
        self.send_frame(Frame::Chunk(Chunk {
            transfer_id: transfer_id.clone(),
            index,
            offset,
            bytes: bytes.to_vec(),
        }))
        .await
    }

    async fn recv_frame(&mut self) -> Result<Frame, CoreError> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| CoreError::Transport("memory connection closed".into()))
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct StopAfterBytesSink {
    bytes: u64,
    stopped: std::sync::Arc<AtomicBool>,
}

impl EventSink for StopAfterBytesSink {
    fn on_event(&self, event: TransferEvent) {
        if let TransferEvent::Progress {
            bytes_transferred, ..
        } = event
            && bytes_transferred >= self.bytes
            && bytes_transferred > 0
        {
            self.stopped.store(true, Ordering::SeqCst);
            panic!("simulated receiver stop after {bytes_transferred} bytes");
        }
    }
}

struct ShortRead<'a> {
    bytes: &'a [u8],
    position: usize,
    max_read: usize,
}

impl AsyncRead for ShortRead<'_> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.position >= self.bytes.len() || buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let bytes_to_read = self
            .max_read
            .min(self.bytes.len() - self.position)
            .min(buffer.remaining());
        let end = self.position + bytes_to_read;
        buffer.put_slice(&self.bytes[self.position..end]);
        self.position = end;
        Poll::Ready(Ok(()))
    }
}

struct TestDir(tempfile::TempDir);

impl std::ops::Deref for TestDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.0.path()
    }
}

impl AsRef<Path> for TestDir {
    fn as_ref(&self) -> &Path {
        self.0.path()
    }
}

fn unique_test_dir() -> TestDir {
    TestDir(
        tempfile::Builder::new()
            .prefix("envoix-transfer-test-")
            .tempdir()
            .unwrap(),
    )
}
