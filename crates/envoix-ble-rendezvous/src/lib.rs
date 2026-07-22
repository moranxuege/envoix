//! # envoix-ble-rendezvous
//!
//! Authenticated one-time BLE rendezvous carrier for Envoix InviteV2.
//!
//! ## Architecture
//!
//! This crate is the **security and carrier logic** for BLE-based peer
//! discovery and invitation exchange. It is transport-agnostic: the state
//! machines here emit messages and expect messages, but never touch a
//! Bluetooth socket directly. Platform-specific BLE GATT, advertisement,
//! and connection management lives in the Android (Kotlin/JNI) and Apple
//! (Swift/UniFFI) layers.
//!
//! ## Security Model
//!
//! | Mode | Value | Description |
//! |------|-------|-------------|
//! | `Insecure` | 0 | Experimental unauthenticated carrier (no MITM protection) |
//! | `AuthenticatedV1` | 1 | Ephemeral X25519 + 6-digit SAS + transcript binding |
//!
//! `AuthenticatedV1` requires a **user-verified Short Authentication String**
//! (SAS) — a 6-digit code both devices display during first use. The code is
//! derived from an ephemeral Diffie-Hellman key agreement bound to the full
//! protocol transcript. An attacker who intercepts the key exchange cannot
//! produce a matching SAS without the shared secret.
//!
//! ## Crate Structure
//!
//! - [`security`] — Key exchange, SAS, transcript, state machine
//! - [`carrier`] — GATT service definition, envelope fragmentation, carrier state

pub mod carrier;
pub mod security;

pub use security::authenticator::{
    AuthenticatedParams, EphemeralPublicKey, InitiatorAwaitingSas, InitiatorConfirming,
    InitiatorPending, ResponderAwaitingSas, ResponderConfirming, SasConfirm, SessionKeys,
    initiator_start, responder_respond,
};
pub use security::mode::BleRendezvousSecurity;
pub use security::sas::SasCode;
