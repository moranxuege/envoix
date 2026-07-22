pub const RECEIPT_HTTP_ROUTE: &str = "/api/envoix/mailbox/v2/receipts/{slot}";
pub const RECEIPT_PAYLOAD_SCHEMA_ID: &str = "envoix/mailbox/receipt-payload/2";
pub const RECEIPT_KIND: &str = "envoix/receipt/v2";
pub const RECEIPT_SLOT_KDF_CONTEXT: &str = "envoix/mailbox/slot/receipt/v2";
pub const RECEIPT_SEAL_KDF_CONTEXT: &str = "envoix/mailbox/seal/receipt/v2";
pub const RECEIPT_AAD_PREFIX: &str = "envoix/mailbox/aad/receipt/v2";

/// Canonical material encoding for mailbox derivations.
pub fn encode_length_prefixed(parts: &[&[u8]]) -> Vec<u8> {
    let capacity = parts.iter().map(|part| 8 + part.len()).sum();
    let mut encoded = Vec::with_capacity(capacity);
    for part in parts {
        encoded.extend_from_slice(&(part.len() as u64).to_be_bytes());
        encoded.extend_from_slice(part);
    }
    encoded
}

pub struct ReceiptSlotDerivation;

impl ReceiptSlotDerivation {
    pub const fn canonical_identifier() -> &'static str {
        "blake3::derive_key(context=envoix/mailbox/slot/receipt/v2;material=len64be(transfer_id)||transfer_id)"
    }
}

pub struct ReceiptSealDerivation;

impl ReceiptSealDerivation {
    pub const fn canonical_identifier() -> &'static str {
        "blake3::derive_key(context=envoix/mailbox/seal/receipt/v2;material=len64be(transfer_id)||transfer_id||len64be(pairing_secret)||pairing_secret)"
    }
}

#[cfg(test)]
mod tests {
    use super::encode_length_prefixed;

    #[test]
    fn length_prefixes_make_field_boundaries_unambiguous() {
        assert_ne!(
            encode_length_prefixed(&[b"ab", b"c"]),
            encode_length_prefixed(&[b"a", b"bc"]),
        );
    }
}
