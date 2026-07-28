use super::*;
use crate::PakeStart;

#[test]
fn frame_round_trips() {
    let msg = PakeStart {
        nonce: vec![1, 2, 3],
        msg: vec![4, 5, 6, 7],
    };
    let framed = frame(&msg).unwrap();
    // 4-byte length prefix matching the body.
    let len = u32::from_be_bytes(framed[..4].try_into().unwrap()) as usize;
    assert_eq!(len, framed.len() - 4);
    assert_eq!(unframe::<PakeStart>(&framed[4..]).unwrap(), msg);
}

#[test]
fn unframe_rejects_garbage() {
    assert!(matches!(
        unframe::<PakeStart>(b"not json"),
        Err(PairingError::BadJson(_))
    ));
}
