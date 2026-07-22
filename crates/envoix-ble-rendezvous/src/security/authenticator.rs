//! Transport-agnostic authenticated BLE rendezvous state machine.
//!
//! Mirrors the pattern from `envoix-pairing::handshake`: pure crypto logic,
//! no I/O. The caller drives message exchange over whatever transport carries
//! the BLE GATT connection.
//!
//! ## Protocol Flow
//!
//! ```text
//! Initiator                              Responder
//!    |                                      |
//!    |  --- EphemeralPublicKey ---------->  |
//!    |  <-- EphemeralPublicKey ----------   |
//!    |                                      |
//!    |  Both: shared = X25519(sk, pk)       |
//!    |  Both: sas = hash(shared || transcript) % 1_000_000
//!    |                                      |
//!    |  Both display 6-digit SAS           |
//!    |  User confirms match                |
//!    |                                      |
//!    |  --- SasConfirm ------------------>  |
//!    |  <-- SasConfirm ------------------   |
//!    |                                      |
//!    |  Both derive session keys           |
//! ```

use zeroize::ZeroizeOnDrop;

use crate::security::key_exchange::{EphemeralKeyPair, validate_peer_public};
use crate::security::sas::{SasCode, compute_sas};
use crate::security::transcript::build_authenticated_transcript;
use crate::security::mode::BleRendezvousSecurity;
use crate::security::{BleError, INITIATOR_CONFIRM_LABEL, RESPONDER_CONFIRM_LABEL};

/// Constant-time comparison for 32-byte arrays — matches the pattern used
/// in `envoix-pairing` which relies on `blake3::Hash`'s built-in
/// constant-time equality (blake3 1.x uses `subtle` internally).
fn ct_eq_32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    blake3::Hash::from(*a) == blake3::Hash::from(*b)
}

/// Domain separation for BLAKE3 KDF session key derivation.
const SESSION_KEY_CONTEXT: &str = "envoix-ble-session-key-v1";
/// Domain separation for the confirmation MAC.
const CONFIRM_MAC_CONTEXT: &str = "envoix-ble-confirm-mac-v1";

/// An ephemeral public key message exchanged over GATT.
#[derive(Clone, Debug)]
pub struct EphemeralPublicKey {
    pub key: [u8; 32],
}

/// SAS confirmation proof — a keyed-BLAKE3 MAC over the transcript.
/// Sent after the user confirms the codes match.
#[derive(Clone, Debug)]
pub struct SasConfirm {
    pub mac: [u8; 32],
}

/// Derived session keys after successful authentication.
#[derive(Clone, Debug, ZeroizeOnDrop)]
pub struct SessionKeys {
    /// Key for ChaCha20-Poly1305 encryption of envelope payloads
    /// (initiator → responder direction).
    pub initiator_to_responder_key: [u8; 32],
    /// Key for ChaCha20-Poly1305 encryption of envelope payloads
    /// (responder → initiator direction).
    pub responder_to_initiator_key: [u8; 32],
    /// Non-replay nonce prefix (first 8 bytes of the per-message nonce).
    pub nonce_prefix: [u8; 8],
}

// ---------------------------------------------------------------------------
// Initiator
// ---------------------------------------------------------------------------

/// Initiator state after generating the ephemeral key pair.
pub struct InitiatorPending {
    key_pair: EphemeralKeyPair,
    presence_id: [u8; 32],
    params: AuthenticatedParams,
}

/// Parameters that define a specific authenticated rendezvous session.
#[derive(Clone, Debug)]
pub struct AuthenticatedParams {
    pub security_mode: BleRendezvousSecurity,
    pub initiator_identity: Option<[u8; 32]>,
    pub responder_identity: Option<[u8; 32]>,
    pub invite_mode: u8,
    pub directional_role: u8,
    pub exchange_id: Option<[u8; 16]>,
    pub sender_ticket: Option<[u8; 32]>,
    pub receiver_ticket: Option<[u8; 32]>,
    pub broker_context: Option<Vec<u8>>,
    pub expiry: u64,
    pub invitation_digest: [u8; 32],
    pub fragment_max_size: u16,
    pub fragment_timeout_ms: u32,
}

/// Start the BLE authenticated rendezvous as the initiator.
/// Returns the ephemeral public key to send over GATT.
pub fn initiator_start(
    presence_id: [u8; 32],
    params: AuthenticatedParams,
) -> Result<(InitiatorPending, EphemeralPublicKey), BleError> {
    let key_pair = EphemeralKeyPair::generate()?;
    let msg = EphemeralPublicKey {
        key: *key_pair.public_key(),
    };
    Ok((
        InitiatorPending {
            key_pair,
            presence_id,
            params,
        },
        msg,
    ))
}

/// Initiator awaiting the responder's ephemeral public key.
pub struct InitiatorAwaitingSas {
    shared_secret: [u8; 32],
    sas: SasCode,
    transcript: Vec<u8>,
    params: AuthenticatedParams,
    initiator_presence_id: [u8; 32],
    responder_presence_id: [u8; 32],
    initiator_pub: [u8; 32],
    responder_pub: [u8; 32],
}

impl InitiatorPending {
    /// Process the responder's ephemeral public key, compute the shared
    /// secret and SAS code. Returns the SAS to display for user confirmation.
    pub fn receive_responder_key(
        self,
        responder_pub: &[u8; 32],
        responder_presence_id: [u8; 32],
    ) -> Result<(InitiatorAwaitingSas, SasCode), BleError> {
        validate_peer_public(responder_pub)?;

        let initiator_pub = *self.key_pair.public_key();
        let shared_secret = self.key_pair.agree(responder_pub);

        let transcript = build_authenticated_transcript(
            self.params.security_mode,
            &self.presence_id,
            &responder_presence_id,
            &initiator_pub,
            responder_pub,
            self.params.initiator_identity.as_ref(),
            self.params.responder_identity.as_ref(),
            self.params.invite_mode,
            self.params.directional_role,
            self.params.exchange_id.as_ref(),
            self.params.sender_ticket.as_ref(),
            self.params.receiver_ticket.as_ref(),
            self.params.broker_context.as_deref(),
            self.params.expiry,
            &self.params.invitation_digest,
            self.params.fragment_max_size,
            self.params.fragment_timeout_ms,
        );

        let sas = compute_sas(&shared_secret, &transcript);

        Ok((
            InitiatorAwaitingSas {
                shared_secret,
                sas,
                transcript,
                params: self.params,
                initiator_presence_id: self.presence_id,
                responder_presence_id,
                initiator_pub,
                responder_pub: *responder_pub,
            },
            sas,
        ))
    }
}

/// Initiator state after user confirmed the SAS codes match.
/// Generates the confirmation MAC to send to the responder.
pub struct InitiatorConfirming {
    shared_secret: [u8; 32],
    transcript: Vec<u8>,
    params: AuthenticatedParams,
    initiator_presence_id: [u8; 32],
    responder_presence_id: [u8; 32],
    initiator_pub: [u8; 32],
    responder_pub: [u8; 32],
}

impl InitiatorAwaitingSas {
    /// Called when the user confirms that both devices show the same SAS code.
    /// Produces the confirmation MAC to send to the responder.
    pub fn confirm(
        self,
    ) -> Result<(InitiatorConfirming, SasConfirm), BleError> {
        let mac = confirmation_mac(
            &self.shared_secret,
            &self.transcript,
            INITIATOR_CONFIRM_LABEL,
        );
        Ok((
            InitiatorConfirming {
                shared_secret: self.shared_secret,
                transcript: self.transcript,
                params: self.params,
                initiator_presence_id: self.initiator_presence_id,
                responder_presence_id: self.responder_presence_id,
                initiator_pub: self.initiator_pub,
                responder_pub: self.responder_pub,
            },
            SasConfirm { mac },
        ))
    }
}

impl InitiatorConfirming {
    /// Verify the responder's confirmation MAC.
    /// On success, returns the derived session keys.
    pub fn verify_responder(
        self,
        responder_confirm: &SasConfirm,
    ) -> Result<SessionKeys, BleError> {
        let expected = confirmation_mac(
            &self.shared_secret,
            &self.transcript,
            RESPONDER_CONFIRM_LABEL,
        );
        if ct_eq_32(&expected, &responder_confirm.mac) {
            Ok(derive_session_keys(
                &self.shared_secret,
                &self.initiator_pub,
                &self.responder_pub,
            ))
        } else {
            Err(BleError::SasConfirmMismatch)
        }
    }
}

// ---------------------------------------------------------------------------
// Responder
// ---------------------------------------------------------------------------

/// Responder state after computing the SAS, awaiting user confirmation.
pub struct ResponderAwaitingSas {
    shared_secret: [u8; 32],
    sas: SasCode,
    transcript: Vec<u8>,
    params: AuthenticatedParams,
    initiator_presence_id: [u8; 32],
    responder_presence_id: [u8; 32],
    initiator_pub: [u8; 32],
    responder_pub: [u8; 32],
}

/// Start the BLE authenticated rendezvous as the responder, given the
/// initiator's ephemeral public key. Returns the responder's ephemeral
/// public key to send back over GATT, plus the SAS code for display.
pub fn responder_respond(
    presence_id: [u8; 32],
    params: AuthenticatedParams,
    initiator_pub: &[u8; 32],
    initiator_presence_id: [u8; 32],
) -> Result<(ResponderAwaitingSas, EphemeralPublicKey, SasCode), BleError> {
    validate_peer_public(initiator_pub)?;

    let key_pair = EphemeralKeyPair::generate()?;
    let responder_pub = *key_pair.public_key();
    let shared_secret = key_pair.agree(initiator_pub);

    let transcript = build_authenticated_transcript(
        params.security_mode,
        &initiator_presence_id,
        &presence_id,
        initiator_pub,
        &responder_pub,
        params.initiator_identity.as_ref(),
        params.responder_identity.as_ref(),
        params.invite_mode,
        params.directional_role,
        params.exchange_id.as_ref(),
        params.sender_ticket.as_ref(),
        params.receiver_ticket.as_ref(),
        params.broker_context.as_deref(),
        params.expiry,
        &params.invitation_digest,
        params.fragment_max_size,
        params.fragment_timeout_ms,
    );

    let sas = compute_sas(&shared_secret, &transcript);

    Ok((
        ResponderAwaitingSas {
            shared_secret,
            sas,
            transcript,
            params,
            initiator_presence_id,
            responder_presence_id: presence_id,
            initiator_pub: *initiator_pub,
            responder_pub,
        },
        EphemeralPublicKey { key: responder_pub },
        sas,
    ))
}

impl ResponderAwaitingSas {
    /// Called when the user confirms the SAS codes match.
    /// Returns the SAS confirm message to send to the initiator.
    pub fn confirm(
        self,
    ) -> Result<(ResponderConfirming, SasConfirm), BleError> {
        let mac = confirmation_mac(
            &self.shared_secret,
            &self.transcript,
            RESPONDER_CONFIRM_LABEL,
        );
        Ok((
            ResponderConfirming {
                shared_secret: self.shared_secret,
                transcript: self.transcript,
                initiator_pub: self.initiator_pub,
                responder_pub: self.responder_pub,
            },
            SasConfirm { mac },
        ))
    }
}

/// Responder state awaiting the initiator's confirmation MAC.
pub struct ResponderConfirming {
    shared_secret: [u8; 32],
    transcript: Vec<u8>,
    initiator_pub: [u8; 32],
    responder_pub: [u8; 32],
}

impl ResponderConfirming {
    /// Verify the initiator's confirmation MAC.
    /// On success, returns the derived session keys.
    pub fn verify_initiator(
        self,
        initiator_confirm: &SasConfirm,
    ) -> Result<SessionKeys, BleError> {
        let expected = confirmation_mac(
            &self.shared_secret,
            &self.transcript,
            INITIATOR_CONFIRM_LABEL,
        );
        if ct_eq_32(&expected, &initiator_confirm.mac) {
            Ok(derive_session_keys(
                &self.shared_secret,
                &self.initiator_pub,
                &self.responder_pub,
            ))
        } else {
            Err(BleError::SasConfirmMismatch)
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Keyed-BLAKE3 MAC over the transcript, tagged with a role-specific label
/// so that initiator_confirm and responder_confirm are distinct.
fn confirmation_mac(shared_secret: &[u8; 32], transcript: &[u8], label: &[u8]) -> [u8; 32] {
    let confirm_key = blake3::derive_key(CONFIRM_MAC_CONTEXT, shared_secret);
    let mut h = blake3::Hasher::new_keyed(&confirm_key);
    h.update(transcript);
    h.update(&(label.len() as u64).to_be_bytes());
    h.update(label);
    let hash = h.finalize();
    *hash.as_bytes()
}

/// Derive separate directional session keys + nonce prefix from the shared
/// secret and both public keys.
fn derive_session_keys(
    shared_secret: &[u8; 32],
    initiator_pub: &[u8; 32],
    responder_pub: &[u8; 32],
) -> SessionKeys {
    // Derive a master secret from the shared secret bound to both public keys.
    let mut kdf_input = Vec::with_capacity(32 + 32 + 32);
    kdf_input.extend_from_slice(shared_secret);
    kdf_input.extend_from_slice(initiator_pub);
    kdf_input.extend_from_slice(responder_pub);

    let master = blake3::derive_key(SESSION_KEY_CONTEXT, &kdf_input);

    // Split into two directional keys + nonce prefix using BLAKE3 KDF
    // with different context strings.
    let i_key = blake3::derive_key(
        "envoix-ble-dir-i2r-v1",
        &master,
    );
    let r_key = blake3::derive_key(
        "envoix-ble-dir-r2i-v1",
        &master,
    );
    let nonce = blake3::derive_key(
        "envoix-ble-nonce-prefix-v1",
        &master,
    );

    SessionKeys {
        initiator_to_responder_key: i_key,
        responder_to_initiator_key: r_key,
        nonce_prefix: {
            let mut prefix = [0u8; 8];
            prefix.copy_from_slice(&nonce[..8]);
            prefix
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_params() -> AuthenticatedParams {
        AuthenticatedParams {
            security_mode: BleRendezvousSecurity::AuthenticatedV1,
            initiator_identity: None,
            responder_identity: None,
            invite_mode: 1,
            directional_role: 0,
            exchange_id: None,
            sender_ticket: None,
            receiver_ticket: None,
            broker_context: None,
            expiry: 0,
            invitation_digest: [0xAAu8; 32],
            fragment_max_size: 512,
            fragment_timeout_ms: 30_000,
        }
    }

    fn test_presence_id(seed: u8) -> [u8; 32] {
        [seed; 32]
    }

    /// Drive a full successful authenticated handshake.
    fn run() -> Result<(SessionKeys, SessionKeys), BleError> {
        let params_i = test_params();
        let params_r = test_params();

        let i_pid = test_presence_id(0x11);
        let r_pid = test_presence_id(0x22);

        // Initiator generates its ephemeral key
        let (i_pending, i_pub) = initiator_start(i_pid, params_i)?;

        // Responder receives initiator's key, generates its own
        let (r_awaiting, r_pub, r_sas) = responder_respond(r_pid, params_r, &i_pub.key, i_pid)?;

        // Initiator receives responder's key, computes SAS
        let (i_awaiting, i_sas) = i_pending.receive_responder_key(&r_pub.key, r_pid)?;

        // Both should have the same SAS code
        assert_eq!(i_sas, r_sas);

        // User confirms on both sides
        let (i_confirming, i_confirm) = i_awaiting.confirm()?;
        let (r_confirming, r_confirm) = r_awaiting.confirm()?;

        // Exchange confirmations
        let i_keys = i_confirming.verify_responder(&r_confirm)?;
        let r_keys = r_confirming.verify_initiator(&i_confirm)?;

        Ok((i_keys, r_keys))
    }

    #[test]
    fn full_handshake_produces_matching_keys() {
        let (i_keys, r_keys) = run().unwrap();
        assert_eq!(
            i_keys.initiator_to_responder_key,
            r_keys.initiator_to_responder_key
        );
        assert_eq!(
            i_keys.responder_to_initiator_key,
            r_keys.responder_to_initiator_key
        );
        assert_eq!(i_keys.nonce_prefix, r_keys.nonce_prefix);
    }

    #[test]
    fn directional_keys_are_distinct() {
        let (i_keys, _) = run().unwrap();
        assert_ne!(
            i_keys.initiator_to_responder_key,
            i_keys.responder_to_initiator_key
        );
    }

    #[test]
    fn mismatched_ephemeral_key_causes_different_sas() {
        let params = test_params();
        let params2 = test_params();
        let i_pid = test_presence_id(0x11);
        let r_pid = test_presence_id(0x22);

        // Two different initiators with different keys
        let (i1_pending, i1_pub) = initiator_start(i_pid, params)?;
        let (i2_pending, i2_pub) = initiator_start(i_pid, params2)?;

        let (r_awaiting, r_pub, r_sas) =
            responder_respond(r_pid, test_params(), &i1_pub.key, i_pid)?;

        let (i_awaiting, i_sas) = i1_pending.receive_responder_key(&r_pub.key, r_pid)?;
        assert_eq!(i_sas, r_sas); // Same key — same SAS

        // Different initiator key — different SAS
        let (i2_awaiting, i2_sas) = i2_pending.receive_responder_key(&r_pub.key, r_pid)?;
        assert_ne!(i2_sas, i_sas);
    }

    #[test]
    fn tampered_confirm_mac_is_rejected() {
        let params = test_params();
        let i_pid = test_presence_id(0x11);
        let r_pid = test_presence_id(0x22);

        let (i_pending, i_pub) = initiator_start(i_pid, params.clone())?;
        let (r_awaiting, r_pub, _r_sas) =
            responder_respond(r_pid, params, &i_pub.key, i_pid)?;
        let (i_awaiting, _i_sas) = i_pending.receive_responder_key(&r_pub.key, r_pid)?;

        let (i_confirming, _i_confirm) = i_awaiting.confirm()?;
        let (_r_confirming, mut r_confirm) = r_awaiting.confirm()?;

        // Tamper with the responder's confirmation
        r_confirm.mac[0] ^= 0x01;

        assert!(matches!(
            i_confirming.verify_responder(&r_confirm),
            Err(BleError::SasConfirmMismatch)
        ));
    }

    #[test]
    fn wrong_transcript_causes_sas_mismatch() {
        // If one side uses different params, SAS won't match (detected by user).
        let params_i = test_params();
        let mut params_r = test_params();
        params_r.invite_mode = 2; // Different invite mode

        let i_pid = test_presence_id(0x11);
        let r_pid = test_presence_id(0x22);

        let (i_pending, i_pub) = initiator_start(i_pid, params_i)?;
        let (_r_awaiting, _r_pub, r_sas) =
            responder_respond(r_pid, params_r, &i_pub.key, i_pid)?;
        let (_i_awaiting, i_sas) = i_pending.receive_responder_key(&_r_pub.key, r_pid)?;

        // Different parameters produce different SAS codes
        assert_ne!(i_sas, r_sas);
    }
}
