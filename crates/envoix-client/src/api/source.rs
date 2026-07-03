//! How the two peers of a transfer find and authenticate each other.

use envoix_protocol::PeerDescriptor;

/// How to find and authenticate the peer of a transfer.
///
/// Consumer variants ([`Manual`](Self::Manual), [`Invite`](Self::Invite)) hold
/// the peer's address and dial it. Producer variants
/// ([`ShowManual`](Self::ShowManual), [`ShowInvite`](Self::ShowInvite)) listen
/// and advertise their own address through a
/// [`TransferEvent::Advertised`](super::TransferEvent::Advertised) event for
/// the user to hand to the peer. [`Mdns`](Self::Mdns) discovers or advertises
/// on the local network; [`Room`](Self::Room) pairs both sides through a
/// rendezvous broker with a short code.
///
/// The design makes every variant valid for both sending and receiving; the
/// current wire protocol still ties the dialer to the file sender, so today
/// consumer variants work for `send`, producer variants for `receive`, and
/// `Mdns`/`Room` for both. Unsupported combinations fail fast with
/// `InvalidInput` before any network activity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerSource {
    /// Dial a peer descriptor obtained out of band, authenticating with a
    /// shared token (>= 12 ASCII bytes).
    Manual {
        /// The peer to dial, as printed by the listening side.
        peer: PeerDescriptor,
        /// Shared SPAKE2 pairing token.
        token: String,
    },
    /// Dial the producer of a scanned/pasted invite string, which carries the
    /// peer address, token, and expiry.
    Invite {
        /// The invite string, e.g. from a QR code.
        invite: String,
    },
    /// Listen and report our own descriptor (and token) for the peer to dial.
    ShowManual {
        /// Shared pairing token; generated when `None`.
        token: Option<String>,
    },
    /// Listen and report an invite string (for QR display) for the peer.
    ShowInvite {
        /// Invite lifetime before senders reject it as expired.
        ttl_secs: u64,
    },
    /// Discover (when dialing) or advertise (when listening) over LAN mDNS.
    Mdns {
        /// Shared pairing token. Required to dial; generated for a listener
        /// when `None`, in which case an invite is also produced.
        token: Option<String>,
    },
    /// Pair through a rendezvous broker using a short code; the transfer
    /// token is derived from the SPAKE2 exchange, so none is supplied.
    Room {
        /// Short pairing code shared between the two sides.
        code: String,
        /// Broker address, `<endpoint-id>@<ip:port>`.
        broker: String,
    },
}
