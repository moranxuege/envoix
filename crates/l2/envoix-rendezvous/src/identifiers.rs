pub const RENDEZVOUS_ALPN: &[u8] = b"envoix-rendezvous/2";
pub const RENDEZVOUS_MAGIC: &[u8; 4] = b"ENVR";
pub const RENDEZVOUS_WIRE_VERSION: u16 = 2;

pub struct RendezvousDialect;

impl RendezvousDialect {
    pub fn canonical_identifier() -> String {
        format!(
            "alpn={};magic={};wire-version={RENDEZVOUS_WIRE_VERSION}",
            String::from_utf8_lossy(RENDEZVOUS_ALPN),
            String::from_utf8_lossy(RENDEZVOUS_MAGIC),
        )
    }
}
