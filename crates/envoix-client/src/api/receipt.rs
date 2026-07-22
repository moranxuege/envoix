//! Sealed completion receipts for the rendezvous mailbox.
//!
//! When the final CompleteAck is lost, the sender ends "unconfirmed" while the
//! receiver actually finished. The receiver posts its completion receipt —
//! sealed — to the rendezvous HTTP endpoint (the mailbox pattern: the rdz is
//! the one party that is always online), and the sender fetches it later to
//! flip unconfirmed → done with no receiver presence at all. The one-shot ack
//! dies with its connection; a mailbox post can retry for hours.
//!
//! Privacy: the rdz stays blind. The mailbox key is `blake3(transfer id)` —
//! retrieval is gated on knowing the high-entropy random transfer id — and the
//! blob is sealed (ChaCha20Poly1305, via the pairing bundle primitives) under a
//! key derived from transfer id + pairing code, so the operator can neither
//! read nor brute-force it: the transfer id contributes the entropy the
//! two-word code lacks. Both peers hold both values; nobody else holds either.

use std::path::Path;

use envoix_storage::TransferReceipt;

use super::error::{ErrorKind, Phase, TransferError};

/// The mailbox kind for completion receipts (see `docs/design/peer-mailbox.md`
/// — the kind namespaces the slot key and is bound into the AAD, so a blob of
/// one kind can never be replayed as another).
const KIND_RECEIPT: &str = "receipt";
/// Key-derivation context (blake3 `derive_key` domain separation).
const RECEIPT_KDF_CONTEXT: &str = "envoix 2026-07-09 receipt-seal v1";

/// The mailbox slot key for a (transfer, kind): hex of
/// `blake3(transfer id ‖ "\n" ‖ kind)`. Possession of the transfer id —
/// random, shared only by the two peers over the authenticated channel — is
/// what gates retrieval; the kind keeps message types in separate slots.
fn mailbox_key(transfer_id: &str, kind: &str) -> String {
    let mut material = Vec::with_capacity(transfer_id.len() + kind.len() + 1);
    material.extend_from_slice(transfer_id.as_bytes());
    material.push(b'\n');
    material.extend_from_slice(kind.as_bytes());
    blake3::hash(&material).to_hex().to_string()
}

/// The AAD for a mailbox kind: binds scheme version + kind into the seal.
fn mailbox_aad(kind: &str) -> Vec<u8> {
    format!("envoix-mailbox-v1:{kind}").into_bytes()
}

/// The mailbox slot key a completion receipt is stored under.
pub fn receipt_mailbox_key(transfer_id: &str) -> String {
    mailbox_key(transfer_id, KIND_RECEIPT)
}

/// The symmetric seal key for a receipt blob.
fn receipt_seal_key(transfer_id: &str, code: &str) -> [u8; 32] {
    let mut material = Vec::with_capacity(transfer_id.len() + code.len() + 1);
    material.extend_from_slice(transfer_id.as_bytes());
    material.push(b'\n');
    material.extend_from_slice(code.as_bytes());
    blake3::derive_key(RECEIPT_KDF_CONTEXT, &material)
}

fn crypto_error(message: impl Into<String>) -> TransferError {
    TransferError {
        phase: Phase::Transfer,
        kind: ErrorKind::Crypto,
        message: message.into(),
        failure_code: None,
    }
}

/// Seal a completion receipt for the mailbox.
pub fn seal_receipt(
    transfer_id: &str,
    code: &str,
    receipt: &TransferReceipt,
) -> Result<Vec<u8>, TransferError> {
    if receipt.transfer_id.0 != transfer_id {
        return Err(crypto_error("receipt transfer id does not match mailbox"));
    }
    envoix_pairing::seal_json(
        &receipt_seal_key(transfer_id, code),
        &mailbox_aad(KIND_RECEIPT),
        receipt,
    )
    .map_err(|e| crypto_error(format!("sealing receipt: {e}")))
}

/// Open and authenticate a mailbox blob. Failure means the blob was not sealed
/// by the paired peer for this transfer (or was corrupted) — never trust it.
pub fn open_receipt(
    transfer_id: &str,
    code: &str,
    blob: &[u8],
) -> Result<TransferReceipt, TransferError> {
    let receipt: TransferReceipt = envoix_pairing::open_json(
        &receipt_seal_key(transfer_id, code),
        &mailbox_aad(KIND_RECEIPT),
        blob,
    )
    .map_err(|e| crypto_error(format!("opening receipt: {e}")))?;
    if receipt.transfer_id.0 != transfer_id {
        return Err(crypto_error("receipt transfer id does not match mailbox"));
    }
    Ok(receipt)
}

/// Open a mailbox blob and verify it against the committed send facts: the
/// receipt's size and BLAKE3 hash must match what this attempt actually sent
/// (the `Complete` frame's hash, recorded on Confirming). No file I/O — the
/// source path may have changed or vanished since the send, and must not be
/// the proof basis.
pub fn verify_receipt_against_fact(
    transfer_id: &str,
    code: &str,
    blob: &[u8],
    sent_hash: &str,
    sent_size: u64,
) -> Result<TransferReceipt, TransferError> {
    let receipt = open_receipt(transfer_id, code, blob)?;
    if receipt.file_size != sent_size {
        return Err(crypto_error(format!(
            "receipt is for {} bytes but {sent_size} were sent",
            receipt.file_size
        )));
    }
    if receipt.file_hash != sent_hash {
        return Err(crypto_error(
            "receipt hash does not match the sent bytes".to_string(),
        ));
    }
    Ok(receipt)
}

/// Open a mailbox blob and verify it against the local source file: the
/// receipt's size and BLAKE3 hash must match the file we sent. Returns the
/// verified receipt — proof the peer finalized exactly our bytes. Fallback
/// for sessions persisted before the committed `sent_hash` fact existed.
pub async fn verify_receipt_against_file(
    transfer_id: &str,
    code: &str,
    blob: &[u8],
    file_path: &Path,
) -> Result<TransferReceipt, TransferError> {
    let receipt = open_receipt(transfer_id, code, blob)?;
    let metadata = tokio::fs::metadata(file_path)
        .await
        .map_err(|e| crypto_error(format!("reading source file: {e}")))?;
    if metadata.len() != receipt.file_size {
        return Err(crypto_error(format!(
            "receipt is for {} bytes but the source file has {}",
            receipt.file_size,
            metadata.len()
        )));
    }
    let hash = hash_file(file_path)
        .await
        .map_err(|e| crypto_error(format!("hashing source file: {e}")))?;
    if hash != receipt.file_hash {
        return Err(crypto_error(
            "receipt hash does not match the source file".to_string(),
        ));
    }
    Ok(receipt)
}

/// Streaming BLAKE3 of a file.
async fn hash_file(path: &Path) -> std::io::Result<String> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
#[path = "receipt_tests.rs"]
mod tests;
