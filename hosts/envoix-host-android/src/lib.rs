//! envoix-host-android: the Android composition root (L7).
//!
//! One `Host` per process, owned by the Kotlin foreground service — NOT by
//! any Flutter engine or activity (Pillar 7: the process owns transfers).
//! It composes the real durable stack (`envoix-storage-local` +
//! `envoix-operation-store` behind the L3 commit barrier) into the L4
//! runtime, restores every durable card at boot, drains the destructive
//! outbox AFTER restore, and serves the generated BN1/BN3 contracts over a
//! polled JNI lane: read/command frames out, command submissions and duty
//! reports in, platform work orders out to the service executor.
//!
//! The attempt-executor seam ([`executor::PreparedIrohExecutor`]) injects the
//! REAL `envoix-attempt-iroh` engine at composition; its one named deferral is
//! that no frontend flow prepares a launch yet, so it parks until F1/F3 land.

// Edition 2024 spells `no_mangle` as `#[unsafe(no_mangle)]`, so the JNI
// export module needs the attribute-level exception; `deny` + a scoped allow
// keeps every OTHER use of unsafe a hard error. No unsafe BLOCK exists in
// this crate — the exception covers exported-symbol attributes only.
#![deny(unsafe_code)]

mod executor;
mod host;
mod provider;
mod store;
mod stores;

#[cfg(target_os = "android")]
#[allow(unsafe_code)]
mod jni_lane;

pub use executor::PreparedIrohExecutor;
pub use host::{BootError, Host};
pub use provider::HostProvider;
pub use store::HostStore;
pub use stores::{CardStores, LiveStore};
