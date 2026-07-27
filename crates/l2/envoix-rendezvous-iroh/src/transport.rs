use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use envoix_invite::NamespacedRoomKey;
use envoix_rendezvous::identifiers::RENDEZVOUS_ALPN;
use envoix_rendezvous::{CloseWaiter, PeerConn, Role, RoomRegistry};
use iroh::endpoint::{
    BindOpts, ConnectError, ConnectingError, Connection, ConnectionError, Incoming, RecvStream,
    SendStream, TransportErrorCode, VarInt, presets,
};
use iroh::{Endpoint, EndpointAddr, RelayMap, RelayMode};
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::{
    EndpointConfig, IrohClientConfig, IrohOperation, IrohRendezvousError, IrohServerConfig,
    IrohWait,
};

struct IrohClose(Connection);

impl CloseWaiter for IrohClose {
    fn wait_closed(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            self.0.closed().await;
        })
    }
}

pub async fn bind_endpoint(config: EndpointConfig) -> Result<Endpoint, IrohRendezvousError> {
    let bind_deadline = config.bind_deadline();
    let builder = Endpoint::builder(presets::Minimal)
        .secret_key(config.secret_key)
        .relay_mode(match config.relay {
            None => RelayMode::Disabled,
            Some(url) => RelayMode::Custom(RelayMap::from(url)),
        })
        .clear_address_lookup()
        .clear_ip_transports()
        .alpns(vec![RENDEZVOUS_ALPN.to_vec()])
        .bind_addr_with_opts(config.bind, BindOpts::default().set_is_required(true))
        .map_err(|_| IrohRendezvousError::Transport {
            operation: IrohOperation::Bind,
        })?;
    timeout(bind_deadline, builder.bind())
        .await
        .map_err(|_| IrohRendezvousError::Deadline {
            wait: IrohWait::Bind,
        })?
        .map_err(|_| IrohRendezvousError::Transport {
            operation: IrohOperation::Bind,
        })
}

pub fn endpoint_addr(endpoint: &Endpoint) -> EndpointAddr {
    endpoint.addr()
}

/// Who may occupy a connection slot.
///
/// The adapter owns how a refusal is spoken on the wire; the composition root
/// owns the policy, because a budget belongs to the service that is budgeted
/// and not to the transport that happens to accept for it.
pub trait ConnectionAdmission: Send + Sync + 'static {
    /// Held for as long as the admitted connection is served.
    type Permit: Send + 'static;

    /// Takes a slot if one is free. Must not wait: a caller that cannot be
    /// served now is refused now.
    fn try_admit(&self) -> Option<Self::Permit>;
}

/// What happened to one incoming connection.
pub enum ServeOutcome<'a> {
    /// Admission was refused; the peer was told with CONNECTION_REFUSED.
    Refused,
    Completed(&'a Result<(), IrohRendezvousError>),
}

pub async fn serve_endpoint(
    endpoint: Endpoint,
    registry: Arc<RoomRegistry>,
    config: IrohServerConfig,
    admission: impl ConnectionAdmission,
    observe: impl Fn(ServeOutcome<'_>) + Send + Sync + 'static,
) -> Result<(), IrohRendezvousError> {
    let observe = Arc::new(observe);
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else {
                    break;
                };
                let Some(permit) = admission.try_admit() else {
                    // Refuse rather than let the attempt time out: the caller
                    // learns it was turned away, and learns it immediately.
                    incoming.refuse();
                    observe(ServeOutcome::Refused);
                    continue;
                };
                let registry = registry.clone();
                let observe = observe.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    let result = serve_incoming(incoming, registry, config).await;
                    observe(ServeOutcome::Completed(&result));
                });
            }
            completed = tasks.join_next(), if !tasks.is_empty() => {
                completed
                    .expect("guarded by a non-empty task set")
                    .map_err(|_| IrohRendezvousError::ConnectionTaskFailed)?;
            }
        }
    }
    while let Some(completed) = tasks.join_next().await {
        completed.map_err(|_| IrohRendezvousError::ConnectionTaskFailed)?;
    }
    Ok(())
}

async fn serve_incoming(
    incoming: Incoming,
    registry: Arc<RoomRegistry>,
    config: IrohServerConfig,
) -> Result<(), IrohRendezvousError> {
    let connection = timeout(config.handshake_deadline(), incoming)
        .await
        .map_err(|_| IrohRendezvousError::Deadline {
            wait: IrohWait::Handshake,
        })?
        .map_err(|_| IrohRendezvousError::Transport {
            operation: IrohOperation::Accept,
        })?;
    let (send, recv) = match timeout(config.handshake_deadline(), connection.accept_bi()).await {
        Ok(Ok(streams)) => streams,
        Ok(Err(_)) => {
            connection.close(VarInt::from_u32(0), b"stream");
            return Err(IrohRendezvousError::Transport {
                operation: IrohOperation::OpenStream,
            });
        }
        Err(_) => {
            connection.close(VarInt::from_u32(0), b"timeout");
            return Err(IrohRendezvousError::Deadline {
                wait: IrohWait::Handshake,
            });
        }
    };
    registry
        .serve(PeerConn::new(send, recv, IrohClose(connection)))
        .await
        .map_err(Into::into)
}

pub struct BrokerSession {
    connection: Connection,
    send: SendStream,
    recv: RecvStream,
    role: Role,
    close_deadline: std::time::Duration,
}

impl BrokerSession {
    pub const fn role(&self) -> Role {
        self.role
    }

    pub fn streams_mut(&mut self) -> (&mut SendStream, &mut RecvStream) {
        (&mut self.send, &mut self.recv)
    }

    pub fn into_parts(self) -> (Connection, SendStream, RecvStream, Role) {
        (self.connection, self.send, self.recv, self.role)
    }

    pub async fn close(mut self) -> Result<(), IrohRendezvousError> {
        let _ = self.send.finish();
        let graceful = timeout(self.close_deadline, async {
            let _ = self.send.stopped().await;
            let mut buffer = [0; 1024];
            loop {
                match self.recv.read(&mut buffer).await {
                    Ok(None) => break,
                    Ok(Some(_)) => {}
                    Err(_) => {
                        return Err(IrohRendezvousError::Transport {
                            operation: IrohOperation::Close,
                        });
                    }
                }
            }
            Ok(())
        })
        .await;
        self.connection.close(VarInt::from_u32(0), b"done");
        match graceful {
            Ok(result) => result,
            Err(_) => Err(IrohRendezvousError::Deadline {
                wait: IrohWait::Close,
            }),
        }
    }
}

/// A refusal and a failure are different answers, and the caller is entitled to
/// know which it got: one says come back later, the other says something is
/// broken. Collapsing them is how a full server comes to look like a dead one.
fn classify_connect_failure(error: &ConnectError) -> IrohRendezvousError {
    let closed = match error {
        ConnectError::Connection { source, .. } => Some(source),
        ConnectError::Connecting {
            source: ConnectingError::ConnectionError { source, .. },
            ..
        } => Some(source),
        _ => None,
    };
    if let Some(ConnectionError::ConnectionClosed(close)) = closed
        && close.error_code == TransportErrorCode::CONNECTION_REFUSED
    {
        return IrohRendezvousError::Refused;
    }
    IrohRendezvousError::Transport {
        operation: IrohOperation::Connect,
    }
}

pub async fn join_room(
    endpoint: &Endpoint,
    broker: EndpointAddr,
    room_key: NamespacedRoomKey,
    config: IrohClientConfig,
) -> Result<BrokerSession, IrohRendezvousError> {
    let connection = match timeout(
        config.connect_deadline(),
        endpoint.connect(broker, RENDEZVOUS_ALPN),
    )
    .await
    {
        Ok(Ok(connection)) => connection,
        Ok(Err(error)) => return Err(classify_connect_failure(&error)),
        Err(_) => {
            return Err(IrohRendezvousError::Deadline {
                wait: IrohWait::Connect,
            });
        }
    };
    let (mut send, mut recv) = match timeout(config.stream_deadline(), connection.open_bi()).await {
        Ok(Ok(streams)) => streams,
        Ok(Err(_)) => {
            connection.close(VarInt::from_u32(0), b"stream");
            return Err(IrohRendezvousError::Transport {
                operation: IrohOperation::OpenStream,
            });
        }
        Err(_) => {
            connection.close(VarInt::from_u32(0), b"timeout");
            return Err(IrohRendezvousError::Deadline {
                wait: IrohWait::Stream,
            });
        }
    };
    let role =
        match envoix_rendezvous::join_room(&mut recv, &mut send, room_key, config.rendezvous())
            .await
        {
            Ok(role) => role,
            Err(error) => {
                let _ = send.finish();
                connection.close(VarInt::from_u32(0), b"join");
                return Err(error.into());
            }
        };
    Ok(BrokerSession {
        connection,
        send,
        recv,
        role,
        close_deadline: config.close_deadline(),
    })
}
