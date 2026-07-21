use super::*;
use envoix_types::{PROTOCOL_VERSION, PeerRole, TransferId};

#[tokio::test]
async fn frame_round_trip() {
    let (mut writer, mut reader) = tokio::io::duplex(1024);
    let frame = Frame::FileHeader(FileHeader {
        transfer_id: TransferId::new("transfer-1"),
        file_name: "hello.txt".into(),
        file_size: 5,
        chunk_size: 1024,
        resume_requested: true,
    });

    write_frame(&mut writer, &frame).await.unwrap();
    let decoded = read_frame(&mut reader).await.unwrap();

    assert_eq!(decoded, frame);
}

#[tokio::test]
async fn file_header_preserves_sizes_above_i32_max() {
    let (mut writer, mut reader) = tokio::io::duplex(1024);
    let frame = Frame::FileHeader(FileHeader {
        transfer_id: TransferId::new("transfer-large"),
        file_name: "mathematica.zip".into(),
        file_size: i32::MAX as u64 + 1,
        chunk_size: 64 * 1024,
        resume_requested: true,
    });

    write_frame(&mut writer, &frame).await.unwrap();

    assert_eq!(read_frame(&mut reader).await.unwrap(), frame);
}

#[tokio::test]
async fn resumable_v1_frames_round_trip() {
    let frames = vec![
        (
            1,
            Frame::Auth(AuthFrame::Spake2Start(Spake2Start {
                protocol_version: PROTOCOL_VERSION,
                role: PeerRole::Sender,
                nonce: b"sender nonce".to_vec(),
                message: b"sender spake2 message".to_vec(),
            })),
        ),
        (
            1,
            Frame::Auth(AuthFrame::Spake2Message(Spake2Message {
                nonce: b"receiver nonce".to_vec(),
                message: b"receiver spake2 message".to_vec(),
            })),
        ),
        (
            1,
            Frame::Auth(AuthFrame::Spake2Confirm(Spake2Confirm {
                proof: b"confirmation proof".to_vec(),
            })),
        ),
        (
            2,
            Frame::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                role: PeerRole::Sender,
            }),
        ),
        (3, Frame::Ready(Ready)),
        (
            4,
            Frame::FileHeader(FileHeader {
                transfer_id: TransferId::new("transfer-1"),
                file_name: "hello.txt".into(),
                file_size: 128,
                chunk_size: 64,
                resume_requested: true,
            }),
        ),
        (
            5,
            Frame::ResumeStatus(ResumeStatus {
                transfer_id: TransferId::new("transfer-1"),
                next_chunk_index: 2,
                bytes_received: 128,
                prefix_hash: "abc123".into(),
            }),
        ),
        (
            6,
            Frame::Chunk(Chunk {
                transfer_id: TransferId::new("transfer-1"),
                index: 2,
                offset: 128,
                bytes: b"hello".to_vec(),
            }),
        ),
        (
            7,
            Frame::Complete(Complete {
                transfer_id: TransferId::new("transfer-1"),
                file_hash: "abc123".into(),
            }),
        ),
        (
            8,
            Frame::CompleteAck(CompleteAck {
                transfer_id: TransferId::new("transfer-1"),
            }),
        ),
        (
            9,
            Frame::Error(ErrorFrame {
                message: "bad frame".into(),
            }),
        ),
    ];

    for (expected_type, frame) in frames {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &frame).await.unwrap();
        assert_eq!(encoded[6], expected_type);
        writer.write_all(&encoded).await.unwrap();
        assert_eq!(read_frame(&mut reader).await.unwrap(), frame);
    }
}

#[tokio::test]
async fn rejects_oversized_frame() {
    let mut input = frame_bytes(FrameType::Ready, &[]);
    input[8..12].copy_from_slice(&((MAX_FRAME_SIZE as u32) + 1).to_be_bytes());

    let error = read_frame(&mut input.as_slice()).await.unwrap_err();

    assert!(matches!(error, CoreError::Protocol(_)));
}

#[tokio::test]
async fn chunk_payload_is_encoded_as_raw_bytes() {
    let frame = Frame::Chunk(Chunk {
        transfer_id: TransferId::new("transfer-1"),
        index: 7,
        offset: 1024,
        bytes: br#"{"not":"json-expanded"}"#.to_vec(),
    });
    let mut encoded = Vec::new();

    write_frame(&mut encoded, &frame).await.unwrap();

    assert!(encoded.ends_with(br#"{"not":"json-expanded"}"#));
    assert_eq!(read_frame(&mut encoded.as_slice()).await.unwrap(), frame);
}

#[tokio::test]
async fn direct_chunk_writer_round_trips() {
    let expected = Frame::Chunk(Chunk {
        transfer_id: TransferId::new("transfer-1"),
        index: 7,
        offset: 1024,
        bytes: b"hello".to_vec(),
    });
    let mut encoded = Vec::new();

    write_chunk_frame(
        &mut encoded,
        &TransferId::new("transfer-1"),
        7,
        1024,
        b"hello",
    )
    .await
    .unwrap();

    assert_eq!(read_frame(&mut encoded.as_slice()).await.unwrap(), expected);
}

#[tokio::test]
async fn rejects_bad_magic_version_and_type() {
    let mut bad_magic = frame_bytes(FrameType::Ready, &[]);
    bad_magic[0] = b'X';
    assert!(matches!(
        read_frame(&mut bad_magic.as_slice()).await,
        Err(CoreError::Protocol(_))
    ));

    let mut bad_version = frame_bytes(FrameType::Ready, &[]);
    bad_version[5] = 2;
    assert!(matches!(
        read_frame(&mut bad_version.as_slice()).await,
        Err(CoreError::Protocol(_))
    ));

    let bad_type = raw_frame_bytes(255, &[]);
    assert!(matches!(
        read_frame(&mut bad_type.as_slice()).await,
        Err(CoreError::Protocol(_))
    ));
}

#[tokio::test]
async fn rejects_invalid_utf8_and_malformed_payloads() {
    let invalid_utf8 = frame_bytes(FrameType::Error, &[0, 0, 0, 1, 0xff]);
    assert!(matches!(
        read_frame(&mut invalid_utf8.as_slice()).await,
        Err(CoreError::Protocol(_))
    ));

    let malformed = frame_bytes(FrameType::CompleteAck, &[0, 0, 0, 8, b't', b'r']);
    assert!(matches!(
        read_frame(&mut malformed.as_slice()).await,
        Err(CoreError::Protocol(_))
    ));

    let trailing = frame_bytes(FrameType::Ready, &[0]);
    assert!(matches!(
        read_frame(&mut trailing.as_slice()).await,
        Err(CoreError::Protocol(_))
    ));
}

#[tokio::test]
async fn hello_frame_carries_protocol_version_and_role() {
    let (mut writer, mut reader) = tokio::io::duplex(1024);
    let frame = Frame::Hello(Hello {
        protocol_version: PROTOCOL_VERSION,
        role: PeerRole::Sender,
    });

    write_frame(&mut writer, &frame).await.unwrap();

    assert_eq!(read_frame(&mut reader).await.unwrap(), frame);
}

fn frame_bytes(frame_type: FrameType, payload: &[u8]) -> Vec<u8> {
    raw_frame_bytes(frame_type as u8, payload)
}

fn raw_frame_bytes(frame_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&WIRE_VERSION.to_be_bytes());
    bytes.extend_from_slice(&[frame_type, 0]);
    bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}
