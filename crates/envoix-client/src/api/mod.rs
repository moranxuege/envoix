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

pub use envoix_types::DataPath;
pub use event::TransferEvent;
pub use options::{PathPolicy, TransferOptions};
pub use source::PeerSource;
pub use transfer::Transfer;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use envoix_qr::{QrInvitePayload, generate_token};
use envoix_session::{
    BindAddrs, DEFAULT_CHUNK_SIZE, SessionConfig, TransferCancelToken, TransferDirection,
    bind_iroh_endpoint_enable_mdns, parse_broker_addr, receive_file_via_room_with_cancel,
    receive_file_with_bound_peer_with_cancel, receive_with_auth_retries_with_cancel,
    send_file_enable_mdns_with_cancel, send_file_manual_with_cancel,
    send_file_via_room_with_cancel,
};

use crate::{IdentityConfig, PairingConfig, PeerDescriptor, PublicError};
use transfer::{EventSender, SessionEventAdapter};

/// Placeholder pairing for room transfers: the room flow derives the real
/// token from the SPAKE2 exchange and overrides this before authentication;
/// it exists only because `SessionConfig` requires a pairing.
const ROOM_PLACEHOLDER_TOKEN: &str = "envoix-room-unused-placeholder";

/// Invite lifetime when the source does not specify one (mDNS listener with
/// a generated token).
const DEFAULT_INVITE_TTL_SECS: u64 = 300;

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

    /// A client configured from an optional TOML config file and environment
    /// overrides (`ENVOIX_CHUNK_SIZE`) - the runtime sources the CLI reads,
    /// without the legacy requirement of supplying a pairing up front.
    pub fn from_runtime_sources(config_path: Option<&Path>) -> Result<Self, PublicError> {
        let mut client = Self::new();
        if let Some(path) = config_path
            && let Some(chunk_size) = crate::RuntimeConfig::read(path)?.chunk_size
        {
            client.chunk_size = crate::parse_chunk_size(&chunk_size)?;
        }
        if let Some(value) = std::env::var_os(crate::ENVOIX_CHUNK_SIZE) {
            let value = value.into_string().map_err(|_| {
                PublicError::InvalidInput(format!("{} is not UTF-8", crate::ENVOIX_CHUNK_SIZE))
            })?;
            client.chunk_size = crate::parse_chunk_size(&value)?;
        }
        crate::validate_chunk_size(client.chunk_size)?;
        Ok(client)
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

    /// Receives one file into the `into` directory from the peer described
    /// by `from`.
    ///
    /// Listening sources report our address (and token/invite) through a
    /// [`TransferEvent::Advertised`] event for the user to hand to the peer.
    /// Fails fast on invalid input or a peer source that cannot be received
    /// from yet.
    pub fn receive(
        &self,
        into: PathBuf,
        from: PeerSource,
        options: TransferOptions,
    ) -> Result<Transfer, PublicError> {
        crate::validate_chunk_size(self.chunk_size)?;
        validate_path_policy(&options)?;
        let (events, event_receiver) = EventSender::channel();
        let cancel = TransferCancelToken::new();
        let sink = Box::new(SessionEventAdapter(events.clone()));
        let listen = options
            .listen_addrs
            .clone()
            .unwrap_or_else(|| BindAddrs::dual_stack(0));

        let task = match from {
            PeerSource::ShowManual { token } => {
                let token = token.map_or_else(new_token, Ok)?;
                let config = self.session_config(shared_token(&token)?, &options);
                let cancel = cancel.clone();
                let on_bound = {
                    let events = events.clone();
                    move |peer: PeerDescriptor| {
                        events.emit(TransferEvent::Advertised {
                            peer,
                            token: Some(token),
                            invite: None,
                        });
                    }
                };
                tokio::spawn(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Receive,
                    });
                    receive_file_with_bound_peer_with_cancel(
                        listen, into, config, sink, on_bound, cancel,
                    )
                    .await
                })
            }
            PeerSource::ShowInvite { ttl_secs } => {
                let token = new_token()?;
                let config = self.session_config(shared_token(&token)?, &options);
                let cancel = cancel.clone();
                let on_bound = advertise_with_invite(events.clone(), token, ttl_secs);
                tokio::spawn(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Receive,
                    });
                    receive_file_with_bound_peer_with_cancel(
                        listen, into, config, sink, on_bound, cancel,
                    )
                    .await
                })
            }
            PeerSource::Mdns { token } => {
                // A provided token is only displayed; a generated one also
                // yields an invite so the sender can be handed a QR.
                let (token, invite_ttl) = match token {
                    Some(token) => (token, None),
                    None => (new_token()?, Some(DEFAULT_INVITE_TTL_SECS)),
                };
                let config = self.session_config(shared_token(&token)?, &options);
                let identity = self.identity.clone();
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Receive,
                    });
                    let endpoint = bind_iroh_endpoint_enable_mdns(listen, &identity).await?;
                    let peer = endpoint.peer_descriptor()?;
                    let invite = invite_ttl.map(|ttl| {
                        QrInvitePayload::new(token.clone(), peer.clone(), unix_now() + ttl).encode()
                    });
                    events.emit(TransferEvent::Advertised {
                        peer,
                        token: Some(token),
                        invite,
                    });
                    receive_with_auth_retries_with_cancel(endpoint, into, config, sink, cancel)
                        .await
                })
            }
            PeerSource::Room { code, broker } => {
                let broker = parse_broker_addr(&broker, options.relay.as_deref())?;
                let config = self.session_config(shared_token(ROOM_PLACEHOLDER_TOKEN)?, &options);
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Receive,
                    });
                    events.emit(TransferEvent::Pairing);
                    receive_file_via_room_with_cancel(
                        broker, &code, listen, into, config, sink, cancel,
                    )
                    .await
                })
            }
            PeerSource::Manual { .. } | PeerSource::Invite { .. } => {
                return Err(PublicError::InvalidInput(
                    "this peer source dials a listener; receiving from it needs \
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

/// Generates a fresh pairing token for listening sources given none.
fn new_token() -> Result<String, PublicError> {
    generate_token().map_err(|e| PublicError::Crypto(format!("failed to generate token: {e}")))
}

/// An `on_bound` callback that advertises the peer together with an encoded
/// invite expiring after `ttl_secs`.
fn advertise_with_invite(
    events: EventSender,
    token: String,
    ttl_secs: u64,
) -> impl FnOnce(PeerDescriptor) + Send {
    move |peer: PeerDescriptor| {
        let invite =
            QrInvitePayload::new(token.clone(), peer.clone(), unix_now() + ttl_secs).encode();
        events.emit(TransferEvent::Advertised {
            peer,
            token: Some(token),
            invite: Some(invite),
        });
    }
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
    fn receive_rejects_consumer_sources() {
        let peer = PeerDescriptor::new("peer", vec!["[::1]:9000".parse().unwrap()]).unwrap();
        for source in [
            PeerSource::Manual {
                peer,
                token: "abcdefghijkl".into(),
            },
            PeerSource::Invite {
                invite: "envoix:whatever".into(),
            },
        ] {
            let error = client()
                .receive("out".into(), source, TransferOptions::default())
                .unwrap_err();
            assert!(matches!(error, PublicError::InvalidInput(_)));
        }
    }

    #[test]
    fn runtime_sources_read_chunk_size_from_config_file() {
        let path = std::env::temp_dir().join(format!(
            "envoix-api-config-{}-chunk.toml",
            std::process::id()
        ));
        std::fs::write(&path, "chunk_size = \"1M\"\n").unwrap();

        let client = Client::from_runtime_sources(Some(&path)).unwrap();

        assert_eq!(client.chunk_size, 1024 * 1024);
        std::fs::remove_file(path).unwrap();
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
