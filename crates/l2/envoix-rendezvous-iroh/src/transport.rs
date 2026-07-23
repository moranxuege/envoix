use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use envoix_invite::NamespacedRoomKey;
use envoix_rendezvous::identifiers::RENDEZVOUS_ALPN;
use envoix_rendezvous::{CloseWaiter, PeerConn, Role, RoomRegistry};
use iroh::endpoint::{BindOpts, Connection, Incoming, RecvStream, SendStream, VarInt, presets};
use iroh::{Endpoint, EndpointAddr, RelayMap, RelayMode};
use tokio::sync::Semaphore;
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

pub async fn serve_endpoint(
    endpoint: Endpoint,
    registry: Arc<RoomRegistry>,
    config: IrohServerConfig,
) -> Result<(), IrohRendezvousError> {
    let permits = Arc::new(Semaphore::new(config.connection_limit()));
    while let Some(incoming) = endpoint.accept().await {
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            drop(incoming);
            continue;
        };
        let registry = registry.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _ = serve_incoming(incoming, registry, config).await;
        });
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

pub async fn join_room(
    endpoint: &Endpoint,
    broker: EndpointAddr,
    room_key: NamespacedRoomKey,
    config: IrohClientConfig,
) -> Result<BrokerSession, IrohRendezvousError> {
    let connection = timeout(
        config.connect_deadline(),
        endpoint.connect(broker, RENDEZVOUS_ALPN),
    )
    .await
    .map_err(|_| IrohRendezvousError::Deadline {
        wait: IrohWait::Connect,
    })?
    .map_err(|_| IrohRendezvousError::Transport {
        operation: IrohOperation::Connect,
    })?;
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
