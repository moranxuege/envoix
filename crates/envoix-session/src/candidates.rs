//! Filtering of the candidate addresses we advertise to a peer.
//!
//! Some local addresses are useless or unwanted in a descriptor handed to a
//! remote peer - a LAN address to a WAN peer, a VPN/CGNAT address, a
//! link-local. This filter drops them by CIDR. It is *not* applied blindly:
//! LAN addresses are correct for same-network transfers, so the caller only
//! enables it where appropriate, and the user configures the CIDRs.

use std::fmt;
use std::net::SocketAddr;

use envoix_error::CoreError;
use ipnet::IpNet;

use crate::SessionError;

/// Why the filter rejected a candidate address - reported so a user can see
/// which rule dropped which address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Rejection {
    /// An allow-list is set and the address matched none of its networks.
    NotAllowed,
    /// The address matched a deny network.
    Denied(IpNet),
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAllowed => f.write_str("not in the candidate allow-list"),
            Self::Denied(net) => write!(f, "matched candidate deny rule {net}"),
        }
    }
}

/// An allow/deny filter over advertised candidate addresses, by CIDR.
///
/// `deny` always removes matching addresses. `allow`, when non-empty, is an
/// inclusion whitelist: only addresses inside an allowed network survive
/// (then `deny` still applies). An empty filter keeps everything.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CandidateFilter {
    allow: Vec<IpNet>,
    deny: Vec<IpNet>,
}

impl CandidateFilter {
    /// Build a filter from allow/deny CIDR strings (e.g. `"10.0.0.0/8"`,
    /// `"2409:8a1e::/32"`). A bare IP is accepted as a host route (`/32`,
    /// `/128`).
    pub fn from_lists(allow: &[String], deny: &[String]) -> Result<Self, SessionError> {
        Ok(Self {
            allow: parse_cidrs(allow)?,
            deny: parse_cidrs(deny)?,
        })
    }

    /// Whether this filter would change any address set (nothing configured).
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    /// Whether the filter permits `ip` (used to decide which local interfaces
    /// to bind, so a denied range - e.g. Tailscale - is never used at all).
    pub fn permits_ip(&self, ip: std::net::IpAddr) -> bool {
        self.classify(ip).is_ok()
    }

    /// Keep the addresses this filter permits, logging each drop (and the rule
    /// that caused it) at debug so `-v` reveals why an address is not
    /// advertised.
    pub fn apply(&self, addrs: impl IntoIterator<Item = SocketAddr>) -> Vec<SocketAddr> {
        addrs
            .into_iter()
            .filter(|addr| match self.classify(addr.ip()) {
                Ok(()) => true,
                Err(reason) => {
                    tracing::debug!(
                        target: "envoix",
                        "candidate {addr} not advertised: {reason}"
                    );
                    false
                }
            })
            .collect()
    }

    /// `Ok` if the filter permits `ip`, else the reason it is rejected.
    fn classify(&self, ip: std::net::IpAddr) -> Result<(), Rejection> {
        if !self.allow.is_empty() && !self.allow.iter().any(|net| net.contains(&ip)) {
            return Err(Rejection::NotAllowed);
        }
        match self.deny.iter().find(|net| net.contains(&ip)) {
            Some(net) => Err(Rejection::Denied(*net)),
            None => Ok(()),
        }
    }
}

fn parse_cidrs(entries: &[String]) -> Result<Vec<IpNet>, SessionError> {
    entries
        .iter()
        .map(|entry| {
            entry
                .parse::<IpNet>()
                .or_else(|_| entry.parse::<std::net::IpAddr>().map(IpNet::from))
                .map_err(|_| CoreError::InvalidInput(format!("invalid CIDR or IP: {entry:?}")))
        })
        .collect()
}

#[cfg(test)]
#[path = "candidates_tests.rs"]
mod tests;
