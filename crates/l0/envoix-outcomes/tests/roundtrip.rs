use envoix_outcomes::{Outcome, OutcomeCode, Phase, Recovery, Retryability, SafeDisplay};
use envoix_types::{
    ArtifactId, AttemptGen, ByteCount, Direction, LandedName, OfferedName, RecordId, RequestId,
    TransferId,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct BoundaryPayload {
    record_id: RecordId,
    attempt_gen: AttemptGen,
    transfer_id: TransferId,
    artifact_id: ArtifactId,
    request_id: RequestId,
    offered_name: OfferedName,
    landed_name: LandedName,
    direction: Direction,
    bytes: ByteCount,
    outcome: Outcome,
}

#[test]
fn typed_identity_outcome_roundtrip() {
    let payload = BoundaryPayload {
        record_id: RecordId::new(u64::MAX - 1),
        attempt_gen: AttemptGen::new(17),
        transfer_id: TransferId::from_bytes([0x11; 16]),
        artifact_id: ArtifactId::from_bytes([0x22; 16]),
        request_id: RequestId::from_bytes([0x33; 16]),
        offered_name: OfferedName::from_untrusted("photo.jpg").unwrap(),
        landed_name: LandedName::new("photo (1).jpg"),
        direction: Direction::Receive,
        bytes: ByteCount::new(u64::MAX),
        outcome: Outcome::new(
            OutcomeCode::PeerLost,
            Phase::Transferring,
            Retryability::Retryable,
            SafeDisplay::new("The peer disconnected."),
        )
        .with_recovery(Recovery::ReconnectPeer),
    };

    let encoded = serde_json::to_string(&payload).expect("typed payload serializes");
    let decoded: BoundaryPayload =
        serde_json::from_str(&encoded).expect("typed payload deserializes");

    assert_eq!(decoded, payload);
    assert!(encoded.contains("11111111111111111111111111111111"));
    assert!(encoded.contains("peer_lost"));
}
