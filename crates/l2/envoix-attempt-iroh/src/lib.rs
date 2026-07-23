//! One authenticated, generation-stamped transfer attempt over a session link.

#![forbid(unsafe_code)]

mod error;
mod executor;

pub use error::AttemptError;
pub use executor::{
    AttemptControl, AttemptHandle, AttemptTimeouts, AttemptTransferSpec, SharedAttemptSupervisor,
    spawn_iroh_receiver, spawn_receiver, spawn_sender,
};

#[cfg(test)]
mod tests;
