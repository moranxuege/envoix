//! The room registry: match two peers by room id, then blindly relay bytes.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

use crate::RendezvousError;
use crate::peer::PeerConn;
use crate::protocol::{Join, Paired, Reply, Role};

/// How long a first peer waits in a room for its partner.
const DEFAULT_ROOM_TTL: Duration = Duration::from_secs(300);
/// Hard cap on a single relay session, so a stalled peer can't pin resources.
const RELAY_TTL: Duration = Duration::from_secs(120);
/// Cap on the wait for a peer's first control frame (its Join). A peer that
/// connects and opens a stream but never sends Join is not in any room, so the
/// room TTL cannot reclaim it - without this it would pin a connection slot.
const JOIN_TIMEOUT: Duration = Duration::from_secs(10);
/// Grace period to wait for peers to close after relaying, so buffered data is
/// delivered before the transports are dropped.
const CLOSE_GRACE: Duration = Duration::from_secs(10);
/// Reject a join whose room id is longer than this; room ids are short codes.
const MAX_ROOM_ID_LEN: usize = 128;
/// Cap on concurrently waiting (unpaired) rooms, to bound memory under abuse.
const MAX_WAITING_ROOMS: usize = 4096;

/// A peer parked in a room, waiting for a partner. The parked task KEEPS its
/// own connection (so it can watch it die and evict itself immediately —
/// registry entries are invalidated by the resource they represent, not by the
/// TTL alone); `ready` is the slot through which the second peer hands over
/// its connection, after which the parked task drives the relay.
struct Waiter {
    ready: oneshot::Sender<PeerConn>,
    id: u64,
}

/// Matches peers into rooms. Cheap to share behind an `Arc` across connections.
pub struct RoomRegistry {
    waiting: Mutex<HashMap<String, Waiter>>,
    ttl: Duration,
    /// Monotonic id stamped on each parked waiter, so a timed-out waiter only
    /// removes its own map entry, never a newer waiter that reused the room id.
    next_id: AtomicU64,
}

impl RoomRegistry {
    /// A registry with the default room time-to-live.
    pub fn new() -> Self {
        Self {
            waiting: Mutex::new(HashMap::new()),
            ttl: DEFAULT_ROOM_TTL,
            next_id: AtomicU64::new(0),
        }
    }

    /// A registry with a custom room time-to-live (mostly for tests).
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            waiting: Mutex::new(HashMap::new()),
            ttl,
            next_id: AtomicU64::new(0),
        }
    }

    /// Serve one peer connection: read its [`Join`], then either park it as the
    /// first peer of a room or, if a peer already waits there, pair the two and
    /// relay between them. The parked (first) peer's task keeps its connection
    /// and drives the relay once the second hands its connection over; the
    /// second peer's task returns after the handoff.
    pub async fn serve(&self, mut conn: PeerConn) -> Result<(), RendezvousError> {
        let Join { room_id } = tokio::time::timeout(JOIN_TIMEOUT, conn.read_control())
            .await
            .map_err(|_| RendezvousError::Rejected("no join received within timeout"))??;
        if room_id.is_empty() || room_id.len() > MAX_ROOM_ID_LEN {
            return Err(RendezvousError::Rejected("room id length out of range"));
        }
        // Record the room id onto the ambient connection span (set up by the
        // transport layer), so every event below - and the peer-address line the
        // transport emits asynchronously - correlates by room without repeating it.
        tracing::Span::current().record("room", tracing::field::display(&room_id));
        tracing::info!("joined");

        // Decide under the lock (no await held), then act once it's released, so
        // two peers arriving at once can't both park and miss each other.
        enum Decision {
            Matched(Waiter),
            Parked(oneshot::Receiver<PeerConn>, u64),
            Rejected(&'static str),
        }
        loop {
            let decision = {
                let mut waiting = self.waiting.lock().expect("registry mutex");
                match waiting.remove(&room_id) {
                    Some(first) => Decision::Matched(first),
                    None if waiting.len() >= MAX_WAITING_ROOMS => {
                        Decision::Rejected("too many waiting rooms")
                    }
                    None => {
                        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                        let (ready_tx, ready_rx) = oneshot::channel();
                        waiting.insert(room_id.clone(), Waiter { ready: ready_tx, id });
                        tracing::debug!(id, "parked (waiting for partner)");
                        Decision::Parked(ready_rx, id)
                    }
                }
            };

            match decision {
                // We are the second peer: hand our connection to the parked
                // task, which drives the relay. If the waiter vanished between
                // the map removal and the handoff (its connection died at that
                // instant), loop around and park ourselves instead of failing a
                // live peer against a corpse.
                Decision::Matched(first) => match first.ready.send(conn) {
                    Ok(()) => {
                        tracing::info!("matched two peers");
                        return Ok(());
                    }
                    Err(returned) => {
                        tracing::debug!("waiter left during handoff; retrying");
                        conn = returned;
                        continue;
                    }
                },
                // We are the first peer: wait for a partner while watching our
                // own connection, or expire.
                Decision::Parked(ready_rx, id) => {
                    return self.wait_for_partner(conn, ready_rx, id, &room_id).await;
                }
                Decision::Rejected(reason) => {
                    tracing::warn!(reason, "rejected");
                    return Err(RendezvousError::Rejected(reason));
                }
            }
        }
    }

    /// Parked-peer wait: a partner's connection arrives through `ready_rx` (we
    /// then drive the relay), the TTL elapses, or our own connection dies. The
    /// peer must stay silent between `Join` and `Paired`, so anything read from
    /// it while parked - data or EOF - evicts the waiter immediately: a room
    /// slot must never outlive the connection it represents (the "dead slot"
    /// bug: a cancelled peer lingered until the TTL, consumed the next join,
    /// and left its real partner parked in an emptied room).
    async fn wait_for_partner(
        &self,
        mut conn: PeerConn,
        mut ready_rx: oneshot::Receiver<PeerConn>,
        id: u64,
        room_id: &str,
    ) -> Result<(), RendezvousError> {
        tokio::select! {
            // Prefer the handoff when several branches are ready at once.
            biased;
            handoff = &mut ready_rx => match handoff {
                Ok(partner) => {
                    tracing::info!("matched two peers");
                    run_pair(conn, partner).await
                }
                // The sender half only drops without a send if the registry
                // itself is being torn down.
                Err(_) => Err(RendezvousError::Rejected("registry shut down")),
            },
            probe = conn.read_control::<Join>() => {
                if !self.evict(room_id, id)
                    && let Ok(partner) = ready_rx.try_recv()
                {
                    // Lost the race: a partner was handed over while we were
                    // evicting. Serve it rather than dropping it; if our side is
                    // truly dead the relay fails fast and the partner sees a
                    // clean error instead of a silent orphan.
                    return run_pair(conn, partner).await;
                }
                match probe {
                    Ok(_) => {
                        tracing::info!(id, "evicted (sent data while waiting)");
                        Err(RendezvousError::Rejected("sent data while waiting"))
                    }
                    Err(_) => {
                        tracing::info!(id, "evicted (connection closed while waiting)");
                        Err(RendezvousError::Rejected("connection closed while waiting"))
                    }
                }
            },
            () = tokio::time::sleep(self.ttl) => {
                if !self.evict(room_id, id)
                    && let Ok(partner) = ready_rx.try_recv()
                {
                    return run_pair(conn, partner).await;
                }
                // Tell the peer *why* we are closing, so it can report "no peer
                // joined" instead of a bare connection drop. Best-effort: if it
                // is lost the peer falls back to the connection-closed path.
                let _ = conn.write_control(&Reply::Expired).await;
                let (mut writer, _reader, close) = conn.into_parts();
                let _ = writer.shutdown().await;
                let _ = tokio::time::timeout(CLOSE_GRACE, close.wait_closed()).await;
                tracing::info!(id, "expired (no partner within ttl)");
                Err(RendezvousError::Expired)
            }
        }
    }

    /// Remove our own waiter entry; `false` if a match already claimed it (or a
    /// newer waiter reused the room id).
    fn evict(&self, room_id: &str, id: u64) -> bool {
        let mut waiting = self.waiting.lock().expect("registry mutex");
        if waiting.get(room_id).is_some_and(|w| w.id == id) {
            waiting.remove(room_id);
            true
        } else {
            false
        }
    }
}

impl Default for RoomRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Tell each peer its role, then relay raw bytes both ways until both sides
/// close (or the relay deadline elapses). The keep-alive handles are held for
/// the whole relay so the transports stay open.
async fn run_pair(initiator: PeerConn, responder: PeerConn) -> Result<(), RendezvousError> {
    let (mut iw, mut ir, i_close) = initiator.into_parts();
    let (mut rw, mut rr, r_close) = responder.into_parts();

    crate::io::write_framed(
        &mut iw,
        &Reply::Paired(Paired {
            role: Role::Initiator,
        }),
    )
    .await?;
    crate::io::write_framed(
        &mut rw,
        &Reply::Paired(Paired {
            role: Role::Responder,
        }),
    )
    .await?;

    // Blind relay: the SPAKE2 + sealed-descriptor traffic flows through
    // opaquely. When one side finishes (EOF), propagate it as a clean shutdown
    // of the other side's writer so the peer drains all data, rather than seeing
    // the connection torn down mid-read.
    let _ = tokio::time::timeout(RELAY_TTL, async {
        tokio::join!(
            async {
                let _ = tokio::io::copy(&mut ir, &mut rw).await;
                let _ = rw.shutdown().await;
            },
            async {
                let _ = tokio::io::copy(&mut rr, &mut iw).await;
                let _ = iw.shutdown().await;
            },
        )
    })
    .await;

    // Keep both transports open until the peers close them (after draining), so
    // their last buffered bytes are delivered before we drop the connections.
    let _ = tokio::time::timeout(CLOSE_GRACE, async {
        tokio::join!(i_close.wait_closed(), r_close.wait_closed())
    })
    .await;
    Ok(())
}
