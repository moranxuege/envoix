use super::*;

const K: &[u8] = b"a 32-byte-ish spake2 shared key!"; // stand-in for SPAKE2 K
const AAD: &[u8] = b"sender-role";

#[test]
fn round_trips_bytes() {
    let sealed = seal(K, AAD, b"hello peer").unwrap();
    // nonce(12) + ciphertext + 16-byte tag, never the plaintext in clear.
    assert!(sealed.len() >= NONCE_LEN + 16);
    assert!(!sealed.windows(10).any(|w| w == b"hello peer"));
    assert_eq!(open(K, AAD, &sealed).unwrap(), b"hello peer");
}

#[test]
fn wrong_key_fails() {
    let sealed = seal(K, AAD, b"secret").unwrap();
    assert!(matches!(
        open(b"a different 32-ish wrong key!!!!", AAD, &sealed),
        Err(PairingError::Decrypt)
    ));
}

#[test]
fn wrong_aad_fails() {
    // A bundle sealed for one role cannot be opened as another - this is
    // what stops a relay reflecting a peer's ciphertext back as its own.
    let sealed = seal(K, b"initiator", b"secret").unwrap();
    assert!(matches!(
        open(K, b"responder", &sealed),
        Err(PairingError::Decrypt)
    ));
}

#[test]
fn tampered_ciphertext_fails() {
    let mut sealed = seal(K, AAD, b"secret").unwrap();
    let last = sealed.len() - 1;
    sealed[last] ^= 0x01;
    assert!(matches!(open(K, AAD, &sealed), Err(PairingError::Decrypt)));
}

#[test]
fn truncated_fails() {
    assert!(matches!(
        open(K, AAD, &[0u8; 4]),
        Err(PairingError::Malformed)
    ));
}

#[test]
fn fresh_nonce_each_seal() {
    // Same key + aad + plaintext must not produce identical ciphertext.
    assert_ne!(seal(K, AAD, b"x").unwrap(), seal(K, AAD, b"x").unwrap());
}

#[test]
fn json_round_trips() {
    let value = (
        "endpoint-id-abc".to_string(),
        vec!["10.0.0.1:9000".to_string()],
    );
    let sealed = seal_json(K, AAD, &value).unwrap();
    let got: (String, Vec<String>) = open_json(K, AAD, &sealed).unwrap();
    assert_eq!(got, value);
}
