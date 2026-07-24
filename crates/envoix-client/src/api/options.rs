//! Per-route transport options. Job behavior is sealed in the manifest.

use envoix_session::BindAddrs;

use super::transport::TransportPreference;

/// Constraint on which iroh data paths a transfer may use.
///
/// Provider selection is configured independently through
/// [`TransferOptions::transport`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PathPolicy {
    /// Try direct (hole-punched) first, fall back to the relay when set.
    #[default]
    Auto,
    /// Force the relay data path (no direct/holepunch). Requires a relay.
    RelayOnly,
    /// Force a direct data path: the relay is still used to reach a broker,
    /// but the transfer itself gets no relay fallback (direct-or-fail).
    ///
    /// Currently gated off at the CLI (the `--direct-only` flag errors): a
    /// relay-free direct path between two NATed peers is not achievable, because
    /// iroh's hole-punching (a QUIC NAT-traversal extension) runs over a
    /// connection that must first be established through the relay. The variant
    /// and its plumbing remain for when the story changes; see
    /// `docs/design/client-api.md` 5.5.
    DirectOnly,
}

/// Options for one transfer. Construct with [`TransferOptions::default`] and
/// set the fields you need; new capabilities become new defaulted fields.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct TransferOptions {
    /// Which provider should establish the data channel. This is independent
    /// of [`Self::path`], which applies only after iroh is selected.
    #[serde(default)]
    pub transport: TransportPreference,
    /// Relay URL for WAN/NAT reachability, e.g. `https://relay.example.com:8444`.
    pub relay: Option<String>,
    /// Which data paths the transfer may use.
    pub path: PathPolicy,
    /// Local socket addresses to bind when listening; `None` binds
    /// dual-stack IPv4 + IPv6 with OS-assigned ports (receive side).
    pub listen_addrs: Option<BindAddrs>,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            transport: TransportPreference::Automatic,
            relay: None,
            path: PathPolicy::Auto,
            listen_addrs: None,
        }
    }
}
