//! The unified client API (new surface, being built alongside the legacy
//! methods; see `docs/design/client-api.md`).
//!
//! One entry point per operation: a transfer is described by *what* to move,
//! *who* to move it with ([`PeerSource`]), and *how* to connect
//! ([`TransferOptions`]); it is observed through one event stream
//! ([`TransferEvent`]) and controlled through a [`Transfer`] handle.
//!
//! Binding-friendly by construction: no generics, closures, or lifetimes in
//! public signatures, so the surface can be exposed through UniFFI later.

mod event;
mod options;
mod source;
mod transfer;

pub use event::TransferEvent;
pub use options::{PathPolicy, TransferOptions};
pub use source::PeerSource;
pub use transfer::Transfer;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use envoix_qr::QrInvitePayload;
use envoix_session::{
    DEFAULT_CHUNK_SIZE, SessionConfig, TransferCancelToken, TransferDirection, parse_broker_addr,
    send_file_enable_mdns_with_cancel, send_file_manual_with_cancel,
    send_file_via_room_with_cancel,
};

use crate::{IdentityConfig, PairingConfig, PeerDescriptor, PublicError};
use transfer::{EventSender, SessionEventAdapter};

/// Placeholder pairing for room transfers: the room flow derives the real
/// token from the SPAKE2 exchange and overrides this before authentication;
/// it exists only because `SessionConfig` requires a pairing.
const ROOM_PLACEHOLDER_TOKEN: &str = "envoix-room-unused-placeholder";

/// The client: local policy (identity, chunk size) shared by transfers.
///
/// Construct with [`Client::new`] and adjust fields as needed; start
/// transfers with [`Client::send`] and [`Client::receive`]. Both must be
/// called within a tokio runtime.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Client {
    /// Maximum chunk payload size used for transfers.
    pub chunk_size: usize,
    /// iroh endpoint identity policy.
    pub identity: IdentityConfig,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            identity: IdentityConfig::Ephemeral,
        }
    }
}

impl Client {
    /// A client with the default chunk size and an ephemeral identity.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sends `file` to the peer described by `to`.
    ///
    /// Fails fast (before any network activity) on invalid input or a peer
    /// source that cannot be sent to yet; everything later is reported
    /// through the returned [`Transfer`].
    pub fn send(
        &self,
        file: PathBuf,
        to: PeerSource,
        options: TransferOptions,
    ) -> Result<Transfer, PublicError> {
        crate::validate_chunk_size(self.chunk_size)?;
        validate_path_policy(&options)?;
        let (events, event_receiver) = EventSender::channel();
        let cancel = TransferCancelToken::new();
        let sink = Box::new(SessionEventAdapter(events.clone()));
        let resume = options.resume;

        let task = match to {
            PeerSource::Manual { peer, token } => {
                let config = self.session_config(shared_token(&token)?, &options);
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Send,
                    });
                    send_file_manual_with_cancel(peer, file, resume, config, sink, cancel).await
                })
            }
            PeerSource::Invite { invite } => {
                let (peer, token) = resolve_invite(&invite)?;
                let config = self.session_config(shared_token(&token)?, &options);
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Send,
                    });
                    send_file_manual_with_cancel(peer, file, resume, config, sink, cancel).await
                })
            }
            PeerSource::Mdns { token: Some(token) } => {
                let config = self.session_config(shared_token(&token)?, &options);
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Send,
                    });
                    send_file_enable_mdns_with_cancel(file, resume, config, sink, cancel).await
                })
            }
            PeerSource::Mdns { token: None } => {
                return Err(PublicError::InvalidInput(
                    "sending over mDNS requires a token".into(),
                ));
            }
            PeerSource::Room { code, broker } => {
                let broker = parse_broker_addr(&broker, options.relay.as_deref())?;
                let config = self.session_config(shared_token(ROOM_PLACEHOLDER_TOKEN)?, &options);
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Send,
                    });
                    events.emit(TransferEvent::Pairing);
                    send_file_via_room_with_cancel(
                        broker, &code, file, resume, config, sink, cancel,
                    )
                    .await
                })
            }
            PeerSource::ShowManual { .. } | PeerSource::ShowInvite { .. } => {
                return Err(PublicError::InvalidInput(
                    "this peer source listens for a dialer; sending toward it needs \
                     protocol role negotiation and is not supported yet"
                        .into(),
                ));
            }
        };
        Ok(Transfer::new(event_receiver, cancel, task))
    }

    fn session_config(&self, pairing: PairingConfig, options: &TransferOptions) -> SessionConfig {
        SessionConfig {
            chunk_size: self.chunk_size,
            pairing,
            identity: self.identity.clone(),
            relay: options.relay.clone(),
            relay_only: options.path == PathPolicy::RelayOnly,
            direct_only: options.path == PathPolicy::DirectOnly,
        }
    }
}

fn shared_token(token: &str) -> Result<PairingConfig, PublicError> {
    PairingConfig::spake2_shared_token(token)
}

fn validate_path_policy(options: &TransferOptions) -> Result<(), PublicError> {
    if options.path == PathPolicy::RelayOnly && options.relay.is_none() {
        return Err(PublicError::InvalidInput(
            "PathPolicy::RelayOnly requires a relay".into(),
        ));
    }
    Ok(())
}

/// Decodes and validates an invite, returning the peer to dial and the token.
fn resolve_invite(invite: &str) -> Result<(PeerDescriptor, String), PublicError> {
    let to_err = |e| PublicError::InvalidInput(format!("invalid invite: {e}"));
    let payload = QrInvitePayload::decode(invite).map_err(to_err)?;
    payload.validate(unix_now()).map_err(to_err)?;
    let peer = payload.peer_descriptor().map_err(to_err)?;
    Ok((peer, payload.token))
}

/// Current Unix time in whole seconds.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client::new()
    }

    #[test]
    fn send_rejects_producer_sources() {
        for source in [
            PeerSource::ShowManual { token: None },
            PeerSource::ShowInvite { ttl_secs: 300 },
        ] {
            let error = client()
                .send("f.txt".into(), source, TransferOptions::default())
                .unwrap_err();
            assert!(matches!(error, PublicError::InvalidInput(_)));
        }
    }

    #[test]
    fn send_over_mdns_requires_token() {
        let error = client()
            .send(
                "f.txt".into(),
                PeerSource::Mdns { token: None },
                TransferOptions::default(),
            )
            .unwrap_err();
        assert!(matches!(error, PublicError::InvalidInput(_)));
    }

    #[test]
    fn relay_only_requires_relay() {
        let options = TransferOptions {
            path: PathPolicy::RelayOnly,
            ..Default::default()
        };
        let error = client()
            .send(
                "f.txt".into(),
                PeerSource::Room {
                    code: "123456-a-b".into(),
                    broker: "unused".into(),
                },
                options,
            )
            .unwrap_err();
        assert!(matches!(error, PublicError::InvalidInput(_)));
    }

    #[test]
    fn send_rejects_garbage_invite() {
        let error = client()
            .send(
                "f.txt".into(),
                PeerSource::Invite {
                    invite: "not-an-invite".into(),
                },
                TransferOptions::default(),
            )
            .unwrap_err();
        assert!(matches!(error, PublicError::InvalidInput(_)));
    }
}
