use envoix_outcomes::OutcomeCode;
use envoix_protocol::{
    Abort, Chunk, Complete, CompleteAck, ContentHash, DecodeError, EncodeError, Field, FileHeader,
    Frame, FrameKind, Hello, IngressState, MAX_CHUNK_SIZE, MAX_FRAME_SIZE, MAX_OFFERED_NAME_SIZE,
    ProtocolReason, Ready, ResumeMode, ResumeStatus, decode_frame, encode_frame, encoded_frame_len,
};
use envoix_types::{ByteCount, OfferedName, TransferId};

const TRANSFER_ID_BYTES: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];

const HELLO_FIXTURE: &[u8] = &[
    0x45, 0x4e, 0x56, 0x58, 0x00, 0x02, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00,
];
const READY_FIXTURE: &[u8] = &[
    0x45, 0x4e, 0x56, 0x58, 0x00, 0x02, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00,
];
const FILE_HEADER_FIXTURE: &[u8] = &[
    0x45, 0x4e, 0x56, 0x58, 0x00, 0x02, 0x04, 0x00, 0x00, 0x00, 0x00, 0x26, 0x00, 0x01, 0x02, 0x03,
    0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x00, 0x00, 0x00, 0x05,
    0x61, 0x2e, 0x74, 0x78, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x01, 0x00,
    0x00, 0x01,
];
const RESUME_STATUS_FIXTURE: &[u8] = &[
    0x45, 0x4e, 0x56, 0x58, 0x00, 0x02, 0x05, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0x01, 0x02, 0x03,
    0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
const RESUME_STATUS_WITH_HASH_FIXTURE: &[u8] = &[
    0x45, 0x4e, 0x56, 0x58, 0x00, 0x02, 0x05, 0x00, 0x00, 0x00, 0x00, 0x41, 0x00, 0x01, 0x02, 0x03,
    0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x01, 0x11, 0x11, 0x11,
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
];
const CHUNK_FIXTURE: &[u8] = &[
    0x45, 0x4e, 0x56, 0x58, 0x00, 0x02, 0x06, 0x00, 0x00, 0x00, 0x00, 0x27, 0x00, 0x01, 0x02, 0x03,
    0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x03,
    0xde, 0xad, 0xbe,
];
const EMPTY_CHUNK_FIXTURE: &[u8] = &[
    0x45, 0x4e, 0x56, 0x58, 0x00, 0x02, 0x06, 0x00, 0x00, 0x00, 0x00, 0x24, 0x00, 0x01, 0x02, 0x03,
    0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
const COMPLETE_FIXTURE: &[u8] = &[
    0x45, 0x4e, 0x56, 0x58, 0x00, 0x02, 0x07, 0x00, 0x00, 0x00, 0x00, 0x30, 0x00, 0x01, 0x02, 0x03,
    0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x22, 0x22, 0x22, 0x22,
    0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22,
    0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22,
];
const COMPLETE_ACK_FIXTURE: &[u8] = &[
    0x45, 0x4e, 0x56, 0x58, 0x00, 0x02, 0x08, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x01, 0x02, 0x03,
    0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];
const ABORT_FIXTURE: &[u8] = &[
    0x45, 0x4e, 0x56, 0x58, 0x00, 0x02, 0x09, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01,
];
const ABORT_WITH_ID_FIXTURE: &[u8] = &[
    0x45, 0x4e, 0x56, 0x58, 0x00, 0x02, 0x09, 0x00, 0x00, 0x00, 0x00, 0x12, 0x01, 0x00, 0x01, 0x02,
    0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x02,
];

fn transfer_id() -> TransferId {
    TransferId::from_bytes(TRANSFER_ID_BYTES)
}

#[test]
fn common_header_reports_a_bounded_total_length() {
    let encoded = encode_frame(&Frame::Hello(Hello)).unwrap();
    let header: &[u8; envoix_protocol::HEADER_LEN] =
        encoded[..envoix_protocol::HEADER_LEN].try_into().unwrap();
    assert_eq!(encoded_frame_len(header).unwrap(), encoded.len());

    let mut oversized = *header;
    oversized[8..12].copy_from_slice(&((MAX_FRAME_SIZE as u32) + 1).to_be_bytes());
    assert!(matches!(
        encoded_frame_len(&oversized),
        Err(DecodeError::FrameTooLarge { .. })
    ));
}

fn fixtures() -> Vec<(Frame, IngressState, &'static [u8])> {
    vec![
        (Frame::Hello(Hello), IngressState::AwaitHello, HELLO_FIXTURE),
        (Frame::Ready(Ready), IngressState::AwaitReady, READY_FIXTURE),
        (
            Frame::FileHeader(FileHeader {
                transfer_id: transfer_id(),
                offered_name: OfferedName::from_untrusted("a.txt").unwrap(),
                file_size: ByteCount::new(5),
                chunk_size: ByteCount::new(64 * 1024),
                resume: ResumeMode::Allowed,
            }),
            IngressState::AwaitFileHeader,
            FILE_HEADER_FIXTURE,
        ),
        (
            Frame::ResumeStatus(ResumeStatus {
                transfer_id: transfer_id(),
                next_chunk_index: 0,
                bytes_received: ByteCount::new(0),
                prefix_hash: None,
            }),
            IngressState::AwaitResumeStatus,
            RESUME_STATUS_FIXTURE,
        ),
        (
            Frame::ResumeStatus(ResumeStatus {
                transfer_id: transfer_id(),
                next_chunk_index: 2,
                bytes_received: ByteCount::new(128 * 1024),
                prefix_hash: Some(ContentHash::from_bytes([0x11; 32])),
            }),
            IngressState::AwaitResumeStatus,
            RESUME_STATUS_WITH_HASH_FIXTURE,
        ),
        (
            Frame::Chunk(Chunk {
                transfer_id: transfer_id(),
                index: 1,
                offset: ByteCount::new(3),
                bytes: vec![0xde, 0xad, 0xbe],
            }),
            IngressState::ReceivingData,
            CHUNK_FIXTURE,
        ),
        (
            Frame::Chunk(Chunk {
                transfer_id: transfer_id(),
                index: 2,
                offset: ByteCount::new(0),
                bytes: Vec::new(),
            }),
            IngressState::ReceivingData,
            EMPTY_CHUNK_FIXTURE,
        ),
        (
            Frame::Complete(Complete {
                transfer_id: transfer_id(),
                file_hash: ContentHash::from_bytes([0x22; 32]),
            }),
            IngressState::ReceivingData,
            COMPLETE_FIXTURE,
        ),
        (
            Frame::CompleteAck(CompleteAck {
                transfer_id: transfer_id(),
            }),
            IngressState::AwaitCompleteAck,
            COMPLETE_ACK_FIXTURE,
        ),
        (
            Frame::Abort(Abort {
                transfer_id: None,
                reason: ProtocolReason::Cancelled,
            }),
            IngressState::AwaitReady,
            ABORT_FIXTURE,
        ),
        (
            Frame::Abort(Abort {
                transfer_id: Some(transfer_id()),
                reason: ProtocolReason::Paused,
            }),
            IngressState::AwaitReady,
            ABORT_WITH_ID_FIXTURE,
        ),
    ]
}

#[test]
fn wire_conformance_and_malformed_ingress() {
    for (frame, state, fixture) in fixtures() {
        assert_eq!(encode_frame(&frame).unwrap(), fixture);
        assert_eq!(decode_frame(fixture, state).unwrap(), frame);
    }

    for (_, state, fixture) in fixtures() {
        for truncated_at in 0..fixture.len() {
            assert!(decode_frame(&fixture[..truncated_at], state).is_err());
        }
    }

    assert!(matches!(
        decode_frame(&HELLO_FIXTURE[..11], IngressState::AwaitHello),
        Err(DecodeError::TruncatedHeader { actual: 11 })
    ));
    assert!(matches!(
        decode_frame(
            &FILE_HEADER_FIXTURE[..FILE_HEADER_FIXTURE.len() - 1],
            IngressState::AwaitFileHeader,
        ),
        Err(DecodeError::TruncatedFrame { .. })
    ));

    let mut oversized_frame = HELLO_FIXTURE.to_vec();
    oversized_frame[6] = FrameKind::Chunk.wire_id();
    oversized_frame[8..12].copy_from_slice(&((MAX_FRAME_SIZE as u32) + 1).to_be_bytes());
    assert!(matches!(
        decode_frame(&oversized_frame, IngressState::ReceivingData),
        Err(DecodeError::FrameTooLarge { .. })
    ));

    let mut oversized_chunk_field = vec![
        0x45, 0x4e, 0x56, 0x58, 0x00, 0x02, 0x06, 0x00, 0x00, 0x00, 0x00, 0x24,
    ];
    oversized_chunk_field.extend_from_slice(&[0; 16]);
    oversized_chunk_field.extend_from_slice(&[0; 8]);
    oversized_chunk_field.extend_from_slice(&[0; 8]);
    oversized_chunk_field.extend_from_slice(&((MAX_CHUNK_SIZE as u32) + 1).to_be_bytes());
    assert!(matches!(
        decode_frame(&oversized_chunk_field, IngressState::ReceivingData),
        Err(DecodeError::FieldTooLarge {
            field: Field::ChunkBytes,
            ..
        })
    ));

    assert!(matches!(
        decode_frame(CHUNK_FIXTURE, IngressState::AwaitReady),
        Err(DecodeError::WrongState {
            state: IngressState::AwaitReady,
            kind: FrameKind::Chunk,
        })
    ));
    let wrong_state_truncated = &oversized_chunk_field[..12];
    assert!(matches!(
        decode_frame(wrong_state_truncated, IngressState::AwaitReady),
        Err(DecodeError::WrongState { .. })
    ));

    let mut unknown_type = HELLO_FIXTURE.to_vec();
    unknown_type[6] = 0xff;
    assert!(matches!(
        decode_frame(&unknown_type, IngressState::AwaitHello),
        Err(DecodeError::UnknownFrameType { wire_id: 0xff })
    ));
    unknown_type[6] = 1;
    assert!(matches!(
        decode_frame(&unknown_type, IngressState::AwaitHello),
        Err(DecodeError::UnknownFrameType { wire_id: 1 })
    ));

    let mut nonzero_reserved = HELLO_FIXTURE.to_vec();
    nonzero_reserved[7] = 1;
    assert_eq!(
        decode_frame(&nonzero_reserved, IngressState::AwaitHello),
        Err(DecodeError::NonZeroReservedByte { actual: 1 })
    );

    let mut invalid_name = FILE_HEADER_FIXTURE.to_vec();
    invalid_name[32] = b'/';
    assert_eq!(
        decode_frame(&invalid_name, IngressState::AwaitFileHeader),
        Err(DecodeError::InvalidOfferedName)
    );
    let mut invalid_utf8 = FILE_HEADER_FIXTURE.to_vec();
    invalid_utf8[32] = 0xff;
    assert_eq!(
        decode_frame(&invalid_utf8, IngressState::AwaitFileHeader),
        Err(DecodeError::InvalidUtf8 {
            field: Field::OfferedName,
        })
    );

    let mut trailing = HELLO_FIXTURE.to_vec();
    trailing.push(0);
    assert_eq!(
        decode_frame(&trailing, IngressState::AwaitHello),
        Err(DecodeError::TrailingFrameBytes { count: 1 })
    );

    let mut unknown_reason = ABORT_FIXTURE.to_vec();
    *unknown_reason.last_mut().unwrap() = 0xff;
    assert_eq!(
        decode_frame(&unknown_reason, IngressState::AwaitReady),
        Err(DecodeError::UnknownProtocolReason { wire_id: 0xff })
    );
}

#[test]
fn wrong_identifier_rejected() {
    let mut wrong_magic = HELLO_FIXTURE.to_vec();
    wrong_magic[0] = b'X';
    assert!(matches!(
        decode_frame(&wrong_magic, IngressState::AwaitHello),
        Err(DecodeError::WrongMagic { .. })
    ));

    let mut wrong_version = HELLO_FIXTURE.to_vec();
    wrong_version[5] = 1;
    assert_eq!(
        decode_frame(&wrong_version, IngressState::AwaitHello),
        Err(DecodeError::UnsupportedVersion { actual: 1 })
    );
}

#[test]
fn max_chunk_and_name_boundaries_are_enforced() {
    let maximum_chunk = Frame::Chunk(Chunk {
        transfer_id: transfer_id(),
        index: u64::MAX,
        offset: ByteCount::new(u64::MAX),
        bytes: vec![0xa5; MAX_CHUNK_SIZE],
    });
    let encoded = encode_frame(&maximum_chunk).unwrap();
    assert_eq!(
        decode_frame(&encoded, IngressState::ReceivingData).unwrap(),
        maximum_chunk
    );

    let oversized_chunk = Frame::Chunk(Chunk {
        transfer_id: transfer_id(),
        index: 0,
        offset: ByteCount::new(0),
        bytes: vec![0; MAX_CHUNK_SIZE + 1],
    });
    assert!(matches!(
        encode_frame(&oversized_chunk),
        Err(EncodeError::FieldTooLarge {
            field: Field::ChunkBytes,
            ..
        })
    ));

    let maximum_name = "n".repeat(MAX_OFFERED_NAME_SIZE);
    let header = Frame::FileHeader(FileHeader {
        transfer_id: transfer_id(),
        offered_name: OfferedName::from_untrusted(&maximum_name).unwrap(),
        file_size: ByteCount::new(0),
        chunk_size: ByteCount::new(1),
        resume: ResumeMode::Disabled,
    });
    let encoded = encode_frame(&header).unwrap();
    assert_eq!(
        decode_frame(&encoded, IngressState::AwaitFileHeader).unwrap(),
        header
    );

    let oversized_name = "n".repeat(MAX_OFFERED_NAME_SIZE + 1);
    assert!(
        OfferedName::from_untrusted(oversized_name).is_err(),
        "the owner type makes an over-bound protocol name unrepresentable"
    );
}

#[test]
fn abort_is_typed_and_legal_from_every_ingress_state() {
    let frame = Frame::Abort(Abort {
        transfer_id: Some(transfer_id()),
        reason: ProtocolReason::Paused,
    });
    assert_eq!(
        ProtocolReason::Paused.outcome_code(),
        Some(OutcomeCode::Paused)
    );
    assert_eq!(ProtocolReason::IntegrityMismatch.outcome_code(), None);

    let encoded = encode_frame(&frame).unwrap();
    for state in [
        IngressState::AwaitHello,
        IngressState::AwaitReady,
        IngressState::AwaitFileHeader,
        IngressState::AwaitResumeStatus,
        IngressState::ReceivingData,
        IngressState::AwaitCompleteAck,
    ] {
        assert_eq!(decode_frame(&encoded, state).unwrap(), frame);
    }
}
