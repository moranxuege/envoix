//! One bounded, frame-oriented data session over iroh.

#![forbid(unsafe_code)]

mod config;
mod error;
mod link;

pub mod identifiers;

pub use config::{
    AuthFailureBudget, BindAddresses, ConfigError, CongestionControl, DEFAULT_DATA_STREAM_WINDOW,
    DEFAULT_MAX_AUTH_FAILURES, FlowWindow, MAX_DATA_STREAM_WINDOW, MIN_DATA_STREAM_WINDOW,
    SessionEndpointConfig, SessionTimeouts, SessionTransportConfig, WaitKind,
};
pub use error::{SessionError, SessionOperation};
pub use link::{
    CloseOrdering, ExportedSecret, IrohListener, IrohSessionLink, PathObservation,
    SessionCancellation, SessionLink, dial,
};

#[cfg(test)]
mod tests;
