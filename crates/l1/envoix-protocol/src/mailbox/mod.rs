//! Cryptographic receipt-mailbox protocol.

pub mod identifiers;

mod receipt;

pub use receipt::{
    MAX_SEALED_RECEIPT_SIZE, MailboxProtocolError, ReceiptPayload, ReceiptSlot, SealedReceipt,
    open_receipt, receipt_slot, seal_receipt,
};
