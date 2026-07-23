use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;
use tokio::time::timeout;

use crate::peer::{ParkedActivity, PeerConn};
use crate::{
    ControlError, ControlFrame, IoOperation, NamespacedRoomKey, Paired, RegistryConfig,
    RejectionReason, RendezvousError, Reply, Role, WaitKind,
};

struct Waiter {
    ready: oneshot::Sender<PeerConn>,
    id: u64,
}

pub struct RoomRegistry {
    waiting: Mutex<HashMap<NamespacedRoomKey, Waiter>>,
    config: RegistryConfig,
    next_id: AtomicU64,
}

impl RoomRegistry {
    pub fn new(config: RegistryConfig) -> Self {
        Self {
            waiting: Mutex::new(HashMap::new()),
            config,
            next_id: AtomicU64::new(1),
        }
    }

    pub async fn serve(&self, mut conn: PeerConn) -> Result<(), RendezvousError> {
        let frame = match timeout(
            self.config.join_deadline(),
            conn.read_control(self.config.control()),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                self.reject_and_close(conn, RejectionReason::JoinDeadline)
                    .await;
                return Err(RendezvousError::Rejected(RejectionReason::JoinDeadline));
            }
        };
        let ControlFrame::Join(join) = frame else {
            self.reject_and_close(conn, RejectionReason::InvalidControl)
                .await;
            return Err(ControlError::UnexpectedFrame.into());
        };
        let room_key = join.into_room_key();

        enum Decision {
            Matched(Waiter),
            Parked(oneshot::Receiver<PeerConn>, u64),
            Rejected,
        }

        loop {
            let decision = {
                let mut waiting = self
                    .waiting
                    .lock()
                    .map_err(|_| RendezvousError::RegistryUnavailable)?;
                match waiting.remove(&room_key) {
                    Some(first) => Decision::Matched(first),
                    None if waiting.len() >= self.config.max_waiting_rooms() => Decision::Rejected,
                    None => {
                        let id = self.next_waiter_id()?;
                        let (ready, receiver) = oneshot::channel();
                        waiting.insert(room_key.clone(), Waiter { ready, id });
                        Decision::Parked(receiver, id)
                    }
                }
            };

            match decision {
                Decision::Matched(first) => match first.ready.send(conn) {
                    Ok(()) => return Ok(()),
                    Err(returned) => conn = returned,
                },
                Decision::Parked(receiver, id) => {
                    return self.wait_for_partner(conn, receiver, id, &room_key).await;
                }
                Decision::Rejected => {
                    self.reject_and_close(conn, RejectionReason::WaitingRoomsFull)
                        .await;
                    return Err(RendezvousError::Rejected(RejectionReason::WaitingRoomsFull));
                }
            }
        }
    }

    fn next_waiter_id(&self) -> Result<u64, RendezvousError> {
        self.next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| RendezvousError::WaiterIdExhausted)
    }

    async fn wait_for_partner(
        &self,
        mut conn: PeerConn,
        mut receiver: oneshot::Receiver<PeerConn>,
        id: u64,
        room_key: &NamespacedRoomKey,
    ) -> Result<(), RendezvousError> {
        tokio::select! {
            biased;
            handoff = &mut receiver => match handoff {
                Ok(partner) => run_pair(conn, partner, self.config).await,
                Err(_) => Err(RendezvousError::RegistryUnavailable),
            },
            activity = conn.probe_while_parked() => {
                if !self.evict(room_key, id)?
                    && let Ok(partner) = receiver.try_recv()
                {
                    return run_pair(conn, partner, self.config).await;
                }
                match activity {
                    Ok(ParkedActivity::Data) => {
                        self.reject_and_close(conn, RejectionReason::PeerNotSilent).await;
                        Err(RendezvousError::Rejected(RejectionReason::PeerNotSilent))
                    }
                    Ok(ParkedActivity::Closed) => Err(RendezvousError::PeerClosed),
                    Err(error) => Err(error),
                }
            },
            () = tokio::time::sleep(self.config.room_ttl()) => {
                if !self.evict(room_key, id)?
                    && let Ok(partner) = receiver.try_recv()
                {
                    return run_pair(conn, partner, self.config).await;
                }
                self.expire_and_close(conn).await;
                Err(RendezvousError::Expired)
            }
        }
    }

    fn evict(&self, room_key: &NamespacedRoomKey, id: u64) -> Result<bool, RendezvousError> {
        let mut waiting = self
            .waiting
            .lock()
            .map_err(|_| RendezvousError::RegistryUnavailable)?;
        if waiting.get(room_key).is_some_and(|waiter| waiter.id == id) {
            waiting.remove(room_key);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn reject_and_close(&self, mut conn: PeerConn, reason: RejectionReason) {
        let _ = timeout(
            self.config.join_deadline(),
            conn.write_control(
                &ControlFrame::Reply(Reply::Rejected(reason)),
                self.config.control(),
            ),
        )
        .await;
        close_peer(conn, self.config).await;
    }

    async fn expire_and_close(&self, mut conn: PeerConn) {
        let _ = timeout(
            self.config.join_deadline(),
            conn.write_control(&ControlFrame::Reply(Reply::Expired), self.config.control()),
        )
        .await;
        close_peer(conn, self.config).await;
    }
}

async fn run_pair(
    initiator: PeerConn,
    responder: PeerConn,
    config: RegistryConfig,
) -> Result<(), RendezvousError> {
    let (mut initiator_writer, mut initiator_reader, initiator_close) = initiator.into_parts();
    let (mut responder_writer, mut responder_reader, responder_close) = responder.into_parts();

    timeout(config.join_deadline(), async {
        crate::write_control(
            &mut initiator_writer,
            &ControlFrame::Reply(Reply::Paired(Paired {
                role: Role::Initiator,
            })),
            config.control(),
        )
        .await?;
        crate::write_control(
            &mut responder_writer,
            &ControlFrame::Reply(Reply::Paired(Paired {
                role: Role::Responder,
            })),
            config.control(),
        )
        .await
    })
    .await
    .map_err(|_| RendezvousError::Deadline {
        wait: WaitKind::Join,
    })??;

    let relay = timeout(config.relay_ttl(), async {
        tokio::try_join!(
            copy_direction(&mut initiator_reader, &mut responder_writer),
            copy_direction(&mut responder_reader, &mut initiator_writer),
        )
    })
    .await;

    let _ = timeout(config.close_grace(), async {
        let _ = initiator_writer.shutdown().await;
        let _ = responder_writer.shutdown().await;
    })
    .await;
    let close_result = timeout(config.close_grace(), async {
        tokio::join!(initiator_close.wait_closed(), responder_close.wait_closed());
    })
    .await;

    let relay_result = match relay {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(RendezvousError::Deadline {
            wait: WaitKind::Relay,
        }),
    };
    relay_result?;
    close_result.map_err(|_| RendezvousError::Deadline {
        wait: WaitKind::Close,
    })
}

async fn copy_direction(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
) -> Result<(), RendezvousError> {
    tokio::io::copy(reader, writer)
        .await
        .map_err(|_| RendezvousError::Io {
            operation: IoOperation::Relay,
        })?;
    writer.shutdown().await.map_err(|_| RendezvousError::Io {
        operation: IoOperation::Shutdown,
    })
}

async fn close_peer(conn: PeerConn, config: RegistryConfig) {
    let (mut writer, _reader, close) = conn.into_parts();
    let _ = timeout(config.close_grace(), writer.shutdown()).await;
    let _ = timeout(config.close_grace(), close.wait_closed()).await;
}
