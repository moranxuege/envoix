/// Identifier for the complete Envoix 0.2 protocol set.
pub const PROTOCOL_SET_ID: &str = "envoix/protocol-set/2";

/// QUIC ALPN for the data plane.
pub const DATA_ALPN: &[u8] = b"envoix/2";
/// Data-frame magic. This remains branded `ENVX`; the complete dialect tuple is new.
pub const DATA_MAGIC: &[u8; 4] = b"ENVX";
/// Version carried by every data-frame header.
pub const DATA_WIRE_VERSION: u16 = 2;

/// The data dialect is identified by the whole tuple, never a free-standing version.
pub struct DataDialect;

impl DataDialect {
    pub fn canonical_identifier() -> String {
        format!(
            "alpn={};magic={};wire-version={DATA_WIRE_VERSION}",
            String::from_utf8_lossy(DATA_ALPN),
            String::from_utf8_lossy(DATA_MAGIC),
        )
    }
}
