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
mod tests {
    use super::*;

    fn addrs(list: &[&str]) -> Vec<SocketAddr> {
        list.iter().map(|s| s.parse().unwrap()).collect()
    }

    #[test]
    fn empty_filter_keeps_everything() {
        let f = CandidateFilter::default();
        assert!(f.is_empty());
        let a = addrs(&["10.0.0.1:1", "1.2.3.4:2"]);
        assert_eq!(f.apply(a.clone()), a);
    }

    #[test]
    fn deny_removes_matching_cidrs() {
        let f =
            CandidateFilter::from_lists(&[], &["10.0.0.0/8".into(), "fe80::/10".into()]).unwrap();
        let kept = f.apply(addrs(&[
            "10.0.0.1:1",
            "1.2.3.4:2",
            "[fe80::1]:3",
            "[2409:8a1e::1]:4",
        ]));
        assert_eq!(kept, addrs(&["1.2.3.4:2", "[2409:8a1e::1]:4"]));
    }

    #[test]
    fn allow_is_an_inclusion_whitelist() {
        // Only the CN2-routed interface's addresses survive.
        let f = CandidateFilter::from_lists(&["203.0.113.0/24".into()], &[]).unwrap();
        let kept = f.apply(addrs(&["203.0.113.9:1", "1.2.3.4:2", "10.0.0.1:3"]));
        assert_eq!(kept, addrs(&["203.0.113.9:1"]));
    }

    #[test]
    fn deny_still_applies_within_allow() {
        let f = CandidateFilter::from_lists(&["203.0.113.0/24".into()], &["203.0.113.9/32".into()])
            .unwrap();
        let kept = f.apply(addrs(&["203.0.113.9:1", "203.0.113.10:2"]));
        assert_eq!(kept, addrs(&["203.0.113.10:2"]));
    }

    #[test]
    fn bare_ip_is_a_host_route() {
        let f = CandidateFilter::from_lists(&[], &["2.0.0.1".into()]).unwrap();
        let kept = f.apply(addrs(&["2.0.0.1:1", "2.0.0.2:2"]));
        assert_eq!(kept, addrs(&["2.0.0.2:2"]));
    }

    #[test]
    fn rejects_garbage_cidr() {
        assert!(CandidateFilter::from_lists(&["not-a-cidr".into()], &[]).is_err());
    }
}
