//! Per-transfer options: transport policy and behavior toggles.

use envoix_session::BindAddrs;

/// Constraint on which data paths a transfer may use.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PathPolicy {
    /// Try direct (hole-punched) first, fall back to the relay when set.
    #[default]
    Auto,
    /// Force the relay data path (no direct/holepunch). Requires a relay.
    RelayOnly,
    /// Force a direct data path: the relay is still used to reach a broker,
    /// but the transfer itself gets no relay fallback (direct-or-fail).
    DirectOnly,
}

/// Options for one transfer. Construct with [`TransferOptions::default`] and
/// set the fields you need; new capabilities become new defaulted fields.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct TransferOptions {
    /// Relay URL for WAN/NAT reachability, e.g. `https://relay.example.com:8444`.
    pub relay: Option<String>,
    /// Which data paths the transfer may use.
    pub path: PathPolicy,
    /// Whether receiver-side resume state may be used (send side).
    pub resume: bool,
    /// Local socket addresses to bind when listening; `None` binds
    /// dual-stack IPv4 + IPv6 with OS-assigned ports (receive side).
    pub listen_addrs: Option<BindAddrs>,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            relay: None,
            path: PathPolicy::Auto,
            resume: true,
            listen_addrs: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_resume_on_auto_path_no_relay() {
        let options = TransferOptions::default();
        assert!(options.resume);
        assert_eq!(options.path, PathPolicy::Auto);
        assert_eq!(options.relay, None);
        assert_eq!(options.listen_addrs, None);
    }
}
