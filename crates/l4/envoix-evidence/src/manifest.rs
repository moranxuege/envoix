use envoix_protocol::identifiers::{DATA_ALPN, DATA_MAGIC, DATA_WIRE_VERSION, PROTOCOL_SET_ID};
use envoix_protocol::mailbox::identifiers::RECEIPT_PAYLOAD_SCHEMA_ID;
use envoix_storage_api::identifiers::OPERATION_ENVELOPE_SCHEMA_ID;
use serde::Serialize;

use crate::identifiers::{EVIDENCE_RUST_ABI_ID, EVIDENCE_TIMELINE_SCHEMA_ID};

/// Wire and protocol identifiers compiled into this build.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProtocolManifest {
    pub set_id: &'static str,
    pub data_alpn: &'static [u8],
    pub data_magic: &'static [u8],
    pub data_wire_version: u16,
}

/// Native ABI and L1 schema identifiers compiled into this build.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct AbiSchemaManifest {
    pub evidence_rust_abi_id: &'static str,
    pub evidence_timeline_schema_id: &'static str,
    pub mailbox_receipt_schema_id: &'static str,
    pub operation_envelope_schema_id: &'static str,
}

/// Typed deployment trust-root slot. It cannot contain an arbitrary string.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "sha256")]
pub enum TrustRootFingerprintSlot {
    Unprovisioned,
    Sha256([u8; 32]),
}

/// Static descriptive build and trust evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct BuildTrustManifest {
    pub package_version: &'static str,
    pub protocol: ProtocolManifest,
    pub abi_schema: AbiSchemaManifest,
    pub trust_root: TrustRootFingerprintSlot,
}

/// The evidence manifest for this compiled package.
///
/// Deployment composition may copy this value and fill the typed SHA-256 slot;
/// the default build never guesses a trust root.
pub const BUILD_TRUST_MANIFEST: BuildTrustManifest = BuildTrustManifest {
    package_version: env!("CARGO_PKG_VERSION"),
    protocol: ProtocolManifest {
        set_id: PROTOCOL_SET_ID,
        data_alpn: DATA_ALPN,
        data_magic: DATA_MAGIC,
        data_wire_version: DATA_WIRE_VERSION,
    },
    abi_schema: AbiSchemaManifest {
        evidence_rust_abi_id: EVIDENCE_RUST_ABI_ID,
        evidence_timeline_schema_id: EVIDENCE_TIMELINE_SCHEMA_ID,
        mailbox_receipt_schema_id: RECEIPT_PAYLOAD_SCHEMA_ID,
        operation_envelope_schema_id: OPERATION_ENVELOPE_SCHEMA_ID,
    },
    trust_root: TrustRootFingerprintSlot::Unprovisioned,
};
