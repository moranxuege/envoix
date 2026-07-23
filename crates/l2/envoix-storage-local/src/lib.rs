//! Crash-atomic local filesystem implementation of the Envoix storage contract.

#![forbid(unsafe_code)]

mod local;

pub use local::{LocalStorage, LocalStorageError, LocalTransaction, LocalWriterLease};

#[cfg(test)]
mod tests;
