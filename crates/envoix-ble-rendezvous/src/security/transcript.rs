use crate::security::mode::BleRendezvousSecurity;
use envoix_types::PROTOCOL_VERSION;

/// Domain separation prefix for all BLE rendezvous hashes.
pub const BLE_DOMAIN: &[u8] = b"envoix-ble-rendezvous-v1";

/// Protocol version for the BLE rendezvous carrier itself (distinct from the
/// wire protocol version used by the QUIC transfer layer).
pub const BLE_PROTOCOL_VERSION: u16 = 1;

/// Builds the length-prefixed authenticated transcript that both sides hash
/// into the SAS and bind into derived session keys.
///
/// Every field is tagged with its length (u64 big-endian) before the value,
/// matching the convention used in `envoix-pairing` and `envoix-auth`.
pub struct Transcript {
    buf: Vec<u8>,
}

impl Transcript {
    pub fn new() -> Self {
        let mut buf = Vec::new();
        // Domain separation
        buf.extend_from_slice(&(BLE_DOMAIN.len() as u64).to_be_bytes());
        buf.extend_from_slice(BLE_DOMAIN);
        Self { buf }
    }

    /// Append a length-prefixed byte slice.
    pub fn append(&mut self, bytes: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        self.buf.extend_from_slice(bytes);
        self
    }

    /// Append a length-prefixed u8.
    pub fn append_u8(&mut self, v: u8) -> &mut Self {
        self.append(&[v])
    }

    /// Append a length-prefixed u16 (big-endian).
    pub fn append_u16(&mut self, v: u16) -> &mut Self {
        self.append(&v.to_be_bytes())
    }

    /// Append a length-prefixed u32 (big-endian).
    pub fn append_u32(&mut self, v: u32) -> &mut Self {
        self.append(&v.to_be_bytes())
    }

    /// Append a length-prefixed u64 (big-endian).
    pub fn append_u64(&mut self, v: u64) -> &mut Self {
        self.append(&v.to_be_bytes())
    }

    /// Append an optional byte slice (length = 0 if None).
    pub fn append_opt(&mut self, bytes: Option<&[u8]>) -> &mut Self {
        match bytes {
            Some(b) => self.append(b),
            None => {
                self.buf
                    .extend_from_slice(&(0u64).to_be_bytes());
                self
            }
        }
    }

    /// Consume and return the complete transcript bytes.
    pub fn finalize(self) -> Vec<u8> {
        self.buf
    }
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the full authenticated transcript as required by the issue spec:
///
///   security mode and protocol version
///   both rotating presence/session identifiers
///   both ephemeral public keys
///   both authenticated identities (when available)
///   InviteV2 mode and directional/exchange roles
///   exchange_id and both direction tickets (when applicable)
///   broker/service context and expiry
///   complete bounded invitation digest
///   fragment/envelope parameters
pub fn build_authenticated_transcript(
    security_mode: BleRendezvousSecurity,
    initiator_presence_id: &[u8; 32],
    responder_presence_id: &[u8; 32],
    initiator_ephemeral_pub: &[u8; 32],
    responder_ephemeral_pub: &[u8; 32],
    initiator_identity: Option<&[u8; 32]>,
    responder_identity: Option<&[u8; 32]>,
    invite_mode: u8,
    directional_role: u8,
    exchange_id: Option<&[u8; 16]>,
    sender_ticket: Option<&[u8; 32]>,
    receiver_ticket: Option<&[u8; 32]>,
    broker_context: Option<&[u8]>,
    expiry: u64,
    invitation_digest: &[u8; 32],
    fragment_max_size: u16,
    fragment_timeout_ms: u32,
) -> Vec<u8> {
    let mut t = Transcript::new();

    // Security mode & protocol versions
    t.append_u8(security_mode.to_byte())
        .append_u16(BLE_PROTOCOL_VERSION)
        .append_u32(PROTOCOL_VERSION);

    // Presence identifiers (rotating, per-session)
    t.append(initiator_presence_id)
        .append(responder_presence_id);

    // Ephemeral public keys
    t.append(initiator_ephemeral_pub)
        .append(responder_ephemeral_pub);

    // Optional pinned identities (trusted device)
    t.append_opt(initiator_identity.map(|id| id.as_slice()))
        .append_opt(responder_identity.map(|id| id.as_slice()));

    // InviteV2 mode and role
    t.append_u8(invite_mode)
        .append_u8(directional_role);

    // Exchange metadata
    t.append_opt(exchange_id.map(|id| id.as_slice()))
        .append_opt(sender_ticket.map(|t| t.as_slice()))
        .append_opt(receiver_ticket.map(|t| t.as_slice()));

    // Broker context and expiry
    t.append_opt(broker_context)
        .append_u64(expiry);

    // Bounded invitation digest
    t.append(invitation_digest);

    // Fragment parameters
    t.append_u16(fragment_max_size)
        .append_u32(fragment_timeout_ms);

    t.finalize()
}
