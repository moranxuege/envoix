//! Bounded Room lifecycle and abuse accounting.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::oneshot;

use envoix_invite::is_room_control_locator;

use crate::peer::{PeerConn, PeerParts};
use crate::protocol::{
    BrokerOutcome, BrokerRejection, Join, Paired, RENDEZVOUS_PROTOCOL_VERSION, Reply, Role,
};
use crate::{
    BootstrapKind, BrokerConfig, InvitationSide, RateLimitConfig, RendezvousError, TransferRole,
};

const ROOM_ID_LEN: usize = 6;
const REMEMBERED_ROOM_ID_PREFIX: &str = "r1_";
const REMEMBERED_ROOM_ID_ENCODED_LEN: usize = 43;

/// Authenticated transport identity plus an optional transport-observed direct
/// address. Relay addresses must never be supplied as `direct_ip`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerSource {
    pub endpoint_id: [u8; 32],
    pub direct_ip: Option<IpAddr>,
}

impl PeerSource {
    pub const fn new(endpoint_id: [u8; 32], direct_ip: Option<IpAddr>) -> Self {
        Self {
            endpoint_id,
            direct_ip,
        }
    }

    const fn anonymous() -> Self {
        Self::new([0; 32], None)
    }
}

/// Fixed-cardinality in-process observability. No source or Room identifier is
/// ever a metric label.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BrokerMetricsSnapshot {
    pub active_rooms: usize,
    pub active_connections: usize,
    pub waiting_creators: usize,
    pub room_connections: usize,
    pub tracked_sources: usize,
    pub matches: u64,
    pub exhausted_rooms: u64,
    pub expired_rooms: u64,
    pub room_not_found_rejections: u64,
    pub room_full_rejections: u64,
    pub room_rate_limit_rejections: u64,
    pub endpoint_rate_limit_rejections: u64,
    pub ip_rate_limit_rejections: u64,
    pub server_busy_rejections: u64,
    pub timeouts: u64,
    pub malformed_joins: u64,
    pub unsupported_versions: u64,
    pub oversized_frames: u64,
}

#[derive(Default)]
struct BrokerMetrics {
    matches: AtomicU64,
    exhausted_rooms: AtomicU64,
    expired_rooms: AtomicU64,
    room_not_found_rejections: AtomicU64,
    room_full_rejections: AtomicU64,
    room_rate_limit_rejections: AtomicU64,
    endpoint_rate_limit_rejections: AtomicU64,
    ip_rate_limit_rejections: AtomicU64,
    server_busy_rejections: AtomicU64,
    timeouts: AtomicU64,
    malformed_joins: AtomicU64,
    unsupported_versions: AtomicU64,
    oversized_frames: AtomicU64,
}

impl BrokerMetrics {
    fn rejection(&self, outcome: BrokerOutcome) {
        let counter = match outcome {
            BrokerOutcome::RoomNotFound => &self.room_not_found_rejections,
            BrokerOutcome::RoomFull => &self.room_full_rejections,
            BrokerOutcome::RoomRateLimited => &self.room_rate_limit_rejections,
            BrokerOutcome::EndpointRateLimited => &self.endpoint_rate_limit_rejections,
            BrokerOutcome::IpRateLimited => &self.ip_rate_limit_rejections,
            BrokerOutcome::ServerBusy => &self.server_busy_rejections,
            _ => return,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

struct Waiter {
    ready: oneshot::Sender<MatchedPeer>,
    id: u64,
    join: Join,
}

struct MatchedPeer {
    conn: PeerConn,
    join: Join,
}

enum CreatorDecision {
    Parked {
        ready_rx: oneshot::Receiver<MatchedPeer>,
        waiter_id: u64,
        expires_at: Instant,
        room_guard: RoomSlotGuard,
    },
    Rejected(BrokerRejection),
}

#[derive(Clone, Copy)]
enum RoomStatus {
    Live,
    Expired { purge_at: Instant },
    Exhausted { purge_at: Instant },
}

struct RoomState {
    instance: u64,
    expires_at: Instant,
    attempts: u32,
    attempt_rate: TokenBucket,
    active_connections: usize,
    waiter: Option<Waiter>,
    status: RoomStatus,
    short_code: bool,
}

#[derive(Default)]
struct RegistryState {
    rooms: HashMap<String, RoomState>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum IpPrefix {
    V4([u8; 3]),
    V6([u8; 8]),
}

impl From<IpAddr> for IpPrefix {
    fn from(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V4(ip) => {
                let [a, b, c, _] = ip.octets();
                Self::V4([a, b, c])
            }
            IpAddr::V6(ip) => {
                let octets = ip.octets();
                Self::V6(octets[..8].try_into().expect("IPv6 /64 is eight bytes"))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum SourceKey {
    Endpoint([u8; 32]),
    Ip(IpAddr),
    Prefix(IpPrefix),
}

struct SourceState {
    bucket: TokenBucket,
    active_connections: usize,
    last_seen: Instant,
}

struct SourceTracker {
    entries: HashMap<SourceKey, SourceState>,
    config: BrokerConfig,
}

impl SourceTracker {
    fn new(config: BrokerConfig) -> Self {
        Self {
            entries: HashMap::new(),
            config,
        }
    }

    fn admit(
        tracker: &Arc<Mutex<Self>>,
        source: PeerSource,
        now: Instant,
    ) -> Result<SourceGuard, BrokerRejection> {
        let mut tracker_lock = tracker.lock().expect("source tracker mutex");
        let mut keys = vec![(
            SourceKey::Endpoint(source.endpoint_id),
            SourceKind::Endpoint,
        )];
        if let Some(ip) = source.direct_ip {
            keys.push((SourceKey::Ip(ip), SourceKind::Ip));
            keys.push((SourceKey::Prefix(ip.into()), SourceKind::Prefix));
        }
        tracker_lock.ensure_capacity(&keys, now)?;

        let endpoint_key = SourceKey::Endpoint(source.endpoint_id);
        if tracker_lock
            .entries
            .get(&endpoint_key)
            .is_some_and(|state| {
                state.active_connections >= tracker_lock.config.max_connections_per_endpoint
            })
        {
            return Err(tracker_lock.rejection(
                BrokerOutcome::EndpointRateLimited,
                tracker_lock.config.unavailable_retry_after,
            ));
        }

        for (key, kind) in keys {
            let policy = tracker_lock.policy(kind);
            let state = tracker_lock
                .entries
                .entry(key)
                .or_insert_with(|| SourceState {
                    bucket: TokenBucket::new(policy, now),
                    active_connections: 0,
                    last_seen: now,
                });
            state.last_seen = now;
            if let Err(retry_after) = state.bucket.take(now) {
                let outcome = match kind {
                    SourceKind::Endpoint => BrokerOutcome::EndpointRateLimited,
                    SourceKind::Ip | SourceKind::Prefix => BrokerOutcome::IpRateLimited,
                };
                return Err(tracker_lock.rejection(outcome, retry_after));
            }
        }

        tracker_lock
            .entries
            .get_mut(&endpoint_key)
            .expect("endpoint source was inserted")
            .active_connections += 1;
        drop(tracker_lock);
        Ok(SourceGuard {
            tracker: tracker.clone(),
            endpoint_key,
        })
    }

    fn observe_direct(&mut self, ip: IpAddr, now: Instant) -> Result<(), BrokerRejection> {
        let keys = [
            (SourceKey::Ip(ip), SourceKind::Ip),
            (SourceKey::Prefix(ip.into()), SourceKind::Prefix),
        ];
        self.ensure_capacity(&keys, now)?;
        for (key, kind) in keys {
            let policy = self.policy(kind);
            let state = self.entries.entry(key).or_insert_with(|| SourceState {
                bucket: TokenBucket::new(policy, now),
                active_connections: 0,
                last_seen: now,
            });
            state.last_seen = now;
            if let Err(retry_after) = state.bucket.take(now) {
                return Err(self.rejection(BrokerOutcome::IpRateLimited, retry_after));
            }
        }
        Ok(())
    }

    fn ensure_capacity(
        &mut self,
        keys: &[(SourceKey, SourceKind)],
        now: Instant,
    ) -> Result<(), BrokerRejection> {
        let missing = keys
            .iter()
            .filter(|(key, _)| !self.entries.contains_key(key))
            .count();
        if self.entries.len() + missing <= self.config.max_source_states {
            return Ok(());
        }
        let ttl = self.config.source_state_ttl;
        self.entries.retain(|_, state| {
            state.active_connections > 0 || now.saturating_duration_since(state.last_seen) < ttl
        });
        if self.entries.len() + missing > self.config.max_source_states {
            return Err(self.rejection(
                BrokerOutcome::ServerBusy,
                self.config.unavailable_retry_after,
            ));
        }
        Ok(())
    }

    fn policy(&self, kind: SourceKind) -> RateLimitConfig {
        match kind {
            SourceKind::Endpoint => self.config.endpoint_join_rate,
            SourceKind::Ip => self.config.ip_join_rate,
            SourceKind::Prefix => self.config.subnet_join_rate,
        }
    }

    fn rejection(&self, outcome: BrokerOutcome, retry_after: Duration) -> BrokerRejection {
        BrokerRejection {
            outcome,
            retry_after: bounded_retry_after(retry_after, self.config.max_retry_after),
        }
    }
}

#[derive(Clone, Copy)]
enum SourceKind {
    Endpoint,
    Ip,
    Prefix,
}

struct SourceGuard {
    tracker: Arc<Mutex<SourceTracker>>,
    endpoint_key: SourceKey,
}

impl Drop for SourceGuard {
    fn drop(&mut self) {
        let Ok(mut tracker) = self.tracker.lock() else {
            return;
        };
        if let Some(state) = tracker.entries.get_mut(&self.endpoint_key) {
            state.active_connections = state.active_connections.saturating_sub(1);
        }
    }
}

struct RoomSlotGuard {
    state: Arc<Mutex<RegistryState>>,
    room_id: String,
    instance: u64,
}

impl Drop for RoomSlotGuard {
    fn drop(&mut self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if let Some(room) = state.rooms.get_mut(&self.room_id)
            && room.instance == self.instance
        {
            room.active_connections = room.active_connections.saturating_sub(1);
        }
    }
}

#[derive(Clone, Copy)]
struct TokenBucket {
    tokens: f64,
    updated: Instant,
    policy: RateLimitConfig,
}

impl TokenBucket {
    fn new(policy: RateLimitConfig, now: Instant) -> Self {
        Self {
            tokens: f64::from(policy.burst),
            updated: now,
            policy,
        }
    }

    fn take(&mut self, now: Instant) -> Result<(), Duration> {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        let refill_per_second = f64::from(self.policy.events) / self.policy.period.as_secs_f64();
        self.tokens = (self.tokens + elapsed * refill_per_second).min(f64::from(self.policy.burst));
        self.updated = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            return Ok(());
        }
        Err(Duration::from_secs_f64(
            (1.0 - self.tokens) / refill_per_second,
        ))
    }
}

/// Matches one creator and one joiner while retaining bounded Room abuse state.
pub struct RoomRegistry {
    state: Arc<Mutex<RegistryState>>,
    sources: Arc<Mutex<SourceTracker>>,
    config: BrokerConfig,
    metrics: Arc<BrokerMetrics>,
    next_id: AtomicU64,
}

impl RoomRegistry {
    pub fn new() -> Self {
        Self::with_config(BrokerConfig::default()).expect("default broker config")
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        let config = BrokerConfig {
            room_ttl: ttl,
            ..BrokerConfig::default()
        };
        Self::with_config(config).expect("custom room TTL")
    }

    pub fn with_config(config: BrokerConfig) -> Result<Self, &'static str> {
        config.validate()?;
        Ok(Self {
            state: Arc::new(Mutex::new(RegistryState::default())),
            sources: Arc::new(Mutex::new(SourceTracker::new(config.clone()))),
            config,
            metrics: Arc::new(BrokerMetrics::default()),
            next_id: AtomicU64::new(0),
        })
    }

    pub fn config(&self) -> &BrokerConfig {
        &self.config
    }

    pub fn metrics_snapshot(&self) -> BrokerMetricsSnapshot {
        let now = Instant::now();
        let mut state = self.state.lock().expect("registry mutex");
        self.sweep_rooms(&mut state, now);
        let active_rooms = state
            .rooms
            .values()
            .filter(|room| matches!(room.status, RoomStatus::Live))
            .count();
        let waiting_creators = state
            .rooms
            .values()
            .filter(|room| room.waiter.is_some())
            .count();
        let room_connections = state
            .rooms
            .values()
            .map(|room| room.active_connections)
            .sum();
        let sources = self.sources.lock().expect("source tracker mutex");
        let tracked_sources = sources.entries.len();
        let active_connections = sources
            .entries
            .iter()
            .filter_map(|(key, source)| {
                matches!(key, SourceKey::Endpoint(_)).then_some(source.active_connections)
            })
            .sum();
        BrokerMetricsSnapshot {
            active_rooms,
            active_connections,
            waiting_creators,
            room_connections,
            tracked_sources,
            matches: self.metrics.matches.load(Ordering::Relaxed),
            exhausted_rooms: self.metrics.exhausted_rooms.load(Ordering::Relaxed),
            expired_rooms: self.metrics.expired_rooms.load(Ordering::Relaxed),
            room_not_found_rejections: self
                .metrics
                .room_not_found_rejections
                .load(Ordering::Relaxed),
            room_full_rejections: self.metrics.room_full_rejections.load(Ordering::Relaxed),
            room_rate_limit_rejections: self
                .metrics
                .room_rate_limit_rejections
                .load(Ordering::Relaxed),
            endpoint_rate_limit_rejections: self
                .metrics
                .endpoint_rate_limit_rejections
                .load(Ordering::Relaxed),
            ip_rate_limit_rejections: self
                .metrics
                .ip_rate_limit_rejections
                .load(Ordering::Relaxed),
            server_busy_rejections: self.metrics.server_busy_rejections.load(Ordering::Relaxed),
            timeouts: self.metrics.timeouts.load(Ordering::Relaxed),
            malformed_joins: self.metrics.malformed_joins.load(Ordering::Relaxed),
            unsupported_versions: self.metrics.unsupported_versions.load(Ordering::Relaxed),
            oversized_frames: self.metrics.oversized_frames.load(Ordering::Relaxed),
        }
    }

    pub fn record_transport_busy(&self) {
        self.metrics.rejection(BrokerOutcome::ServerBusy);
    }

    /// Debit IP and prefix buckets when a direct path appears after Join. This
    /// never accepts relay addresses and does not delay the Join path.
    pub fn observe_direct_ip(&self, ip: IpAddr) -> Result<(), BrokerRejection> {
        let result = self
            .sources
            .lock()
            .expect("source tracker mutex")
            .observe_direct(ip, Instant::now());
        if let Err(rejection) = &result {
            self.metrics.rejection(rejection.outcome);
        }
        result
    }

    /// Test/in-memory entry point. Production transports should use
    /// [`Self::serve_from`] with their authenticated endpoint identity.
    pub async fn serve(&self, conn: PeerConn) -> Result<(), RendezvousError> {
        self.serve_from(conn, PeerSource::anonymous()).await
    }

    pub async fn serve_from(
        &self,
        mut conn: PeerConn,
        source: PeerSource,
    ) -> Result<(), RendezvousError> {
        let source_guard = match SourceTracker::admit(&self.sources, source, Instant::now()) {
            Ok(guard) => guard,
            Err(rejection) => {
                // Let a well-behaved peer finish its already-started Join write
                // before closing the read half, so it can reliably receive the
                // structured rejection. The same Join deadline and body cap
                // keep a limited slowloris bounded.
                let _ = tokio::time::timeout(
                    self.config.join_timeout,
                    conn.read_control_with_limit::<serde_json::Value>(self.config.max_frame_body),
                )
                .await;
                return self.reject(conn, rejection).await;
            }
        };
        conn.hold(source_guard);

        let join: Join = match tokio::time::timeout(
            self.config.join_timeout,
            conn.read_control_with_limit(self.config.max_frame_body),
        )
        .await
        {
            Err(_) => {
                self.metrics.timeouts.fetch_add(1, Ordering::Relaxed);
                return self
                    .reject(conn, self.rejection(BrokerOutcome::MalformedJoin, None))
                    .await;
            }
            Ok(Err(RendezvousError::FrameTooLarge)) => {
                self.metrics
                    .oversized_frames
                    .fetch_add(1, Ordering::Relaxed);
                self.metrics.malformed_joins.fetch_add(1, Ordering::Relaxed);
                return self
                    .reject(conn, self.rejection(BrokerOutcome::MalformedJoin, None))
                    .await;
            }
            Ok(Err(RendezvousError::BadMessage(_))) => {
                self.metrics.malformed_joins.fetch_add(1, Ordering::Relaxed);
                return self
                    .reject(conn, self.rejection(BrokerOutcome::MalformedJoin, None))
                    .await;
            }
            Ok(Err(error)) => return Err(error),
            Ok(Ok(join)) => join,
        };

        if let Err(outcome) = validate_join(&join) {
            if outcome == BrokerOutcome::UnsupportedVersion {
                self.metrics
                    .unsupported_versions
                    .fetch_add(1, Ordering::Relaxed);
            } else {
                self.metrics.malformed_joins.fetch_add(1, Ordering::Relaxed);
            }
            return self.reject(conn, self.rejection(outcome, None)).await;
        }

        let room_id = join.room_id.clone();
        let room_log_label = if is_remembered_room_id(&room_id) {
            "<remembered>"
        } else if is_room_control_locator(&room_id) {
            "<room-control>"
        } else {
            room_id.as_str()
        };
        tracing::Span::current().record("room", tracing::field::display(room_log_label));
        tracing::info!(
            side = ?join.invitation_side,
            transfer_role = ?join.transfer_role,
            "joined"
        );

        match join.invitation_side {
            InvitationSide::Creator => self.serve_creator(conn, join).await,
            InvitationSide::Joiner => self.serve_joiner(conn, join).await,
        }
    }

    async fn serve_creator(&self, mut conn: PeerConn, join: Join) -> Result<(), RendezvousError> {
        let room_id = join.room_id.clone();
        let now = Instant::now();
        let decision = {
            let mut state = self.state.lock().expect("registry mutex");
            self.sweep_rooms(&mut state, now);
            if !state.rooms.contains_key(&room_id) {
                let waiting = state
                    .rooms
                    .values()
                    .filter(|room| room.waiter.is_some())
                    .count();
                if state.rooms.len() >= self.config.max_room_states
                    || waiting >= self.config.max_waiting_creators
                {
                    CreatorDecision::Rejected(self.rejection(
                        BrokerOutcome::ServerBusy,
                        Some(self.config.unavailable_retry_after),
                    ))
                } else {
                    let instance = self.next_id.fetch_add(1, Ordering::Relaxed);
                    state.rooms.insert(
                        room_id.clone(),
                        RoomState {
                            instance,
                            expires_at: now + self.config.room_ttl,
                            attempts: 0,
                            attempt_rate: TokenBucket::new(self.config.room_attempt_rate, now),
                            active_connections: 0,
                            waiter: None,
                            status: RoomStatus::Live,
                            short_code: !is_remembered_room_id(&room_id),
                        },
                    );
                    park_creator(&mut state, &self.state, &room_id, &join, &self.next_id)
                }
            } else {
                let room = state.rooms.get(&room_id).expect("Room exists");
                let rejection = match room.status {
                    RoomStatus::Expired { .. } => Some(BrokerOutcome::RoomExpired),
                    RoomStatus::Exhausted { .. } => Some(BrokerOutcome::RoomUnderAttack),
                    RoomStatus::Live
                        if room.waiter.is_some()
                            || room.active_connections >= self.config.max_connections_per_room =>
                    {
                        Some(BrokerOutcome::RoomFull)
                    }
                    RoomStatus::Live => None,
                };
                if let Some(outcome) = rejection {
                    CreatorDecision::Rejected(
                        self.rejection(
                            outcome,
                            matches!(outcome, BrokerOutcome::RoomFull)
                                .then_some(self.config.unavailable_retry_after),
                        ),
                    )
                } else {
                    park_creator(&mut state, &self.state, &room_id, &join, &self.next_id)
                }
            }
        };
        match decision {
            CreatorDecision::Rejected(rejection) => self.reject(conn, rejection).await,
            CreatorDecision::Parked {
                ready_rx,
                waiter_id,
                expires_at,
                room_guard,
            } => {
                conn.hold(room_guard);
                tracing::debug!(waiter_id, "creator parked");
                self.wait_for_joiner(conn, join, ready_rx, waiter_id, expires_at)
                    .await
            }
        }
    }

    async fn serve_joiner(&self, conn: PeerConn, join: Join) -> Result<(), RendezvousError> {
        let room_id = join.room_id.clone();
        let now = Instant::now();
        let mut conn = Some(conn);
        let rejection = {
            let mut state = self.state.lock().expect("registry mutex");
            self.sweep_rooms(&mut state, now);
            if let Some(room) = state.rooms.get_mut(&room_id) {
                let status_rejection = match room.status {
                    RoomStatus::Expired { .. } => Some(BrokerOutcome::RoomExpired),
                    RoomStatus::Exhausted { .. } => Some(BrokerOutcome::RoomUnderAttack),
                    RoomStatus::Live => None,
                };
                if let Some(outcome) = status_rejection {
                    Some(self.rejection(outcome, None))
                } else if room.short_code && room.attempts >= self.config.room_attempt_limit {
                    self.exhaust_room(room, now);
                    Some(self.rejection(BrokerOutcome::RoomUnderAttack, None))
                } else if room.active_connections >= self.config.max_connections_per_room {
                    Some(self.rejection(
                        BrokerOutcome::RoomFull,
                        Some(self.config.unavailable_retry_after),
                    ))
                } else if room
                    .waiter
                    .as_ref()
                    .is_none_or(|waiter| joins_match(&waiter.join, &join).is_none())
                {
                    Some(self.rejection(
                        BrokerOutcome::RoomNotFound,
                        Some(self.config.unavailable_retry_after),
                    ))
                } else if room.short_code {
                    match room.attempt_rate.take(now) {
                        Ok(()) => self.handoff_joiner(room, &room_id, &join, &mut conn, now),
                        Err(retry_after) => {
                            Some(self.rejection(BrokerOutcome::RoomRateLimited, Some(retry_after)))
                        }
                    }
                } else {
                    self.handoff_joiner(room, &room_id, &join, &mut conn, now)
                }
            } else {
                Some(self.rejection(
                    BrokerOutcome::RoomNotFound,
                    Some(self.config.unavailable_retry_after),
                ))
            }
        };

        if let Some(rejection) = rejection {
            self.reject(conn.expect("connection was not handed off"), rejection)
                .await
        } else {
            tracing::info!("matched creator and joiner");
            Ok(())
        }
    }

    fn handoff_joiner(
        &self,
        room: &mut RoomState,
        room_id: &str,
        join: &Join,
        conn: &mut Option<PeerConn>,
        now: Instant,
    ) -> Option<BrokerRejection> {
        let waiter = room.waiter.take().expect("compatible waiter exists");
        room.active_connections += 1;
        let mut handed_conn = conn.take().expect("joiner connection is available");
        handed_conn.hold(RoomSlotGuard {
            state: self.state.clone(),
            room_id: room_id.to_string(),
            instance: room.instance,
        });
        let handed = waiter.ready.send(MatchedPeer {
            conn: handed_conn,
            join: join.clone(),
        });
        match handed {
            Ok(()) => {
                room.attempts += 1;
                self.metrics.matches.fetch_add(1, Ordering::Relaxed);
                if room.short_code && room.attempts >= self.config.room_attempt_limit {
                    self.exhaust_room(room, now);
                }
                None
            }
            Err(returned) => {
                *conn = Some(returned.conn);
                Some(self.rejection(
                    BrokerOutcome::RoomNotFound,
                    Some(self.config.unavailable_retry_after),
                ))
            }
        }
    }

    fn exhaust_room(&self, room: &mut RoomState, now: Instant) {
        if matches!(room.status, RoomStatus::Live) {
            room.status = RoomStatus::Exhausted {
                purge_at: now + self.config.room_tombstone_ttl,
            };
            self.metrics.exhausted_rooms.fetch_add(1, Ordering::Relaxed);
        }
    }

    async fn wait_for_joiner(
        &self,
        mut conn: PeerConn,
        join: Join,
        mut ready_rx: oneshot::Receiver<MatchedPeer>,
        waiter_id: u64,
        expires_at: Instant,
    ) -> Result<(), RendezvousError> {
        let room_id = join.room_id.clone();
        let wait = expires_at.saturating_duration_since(Instant::now());
        tokio::select! {
            biased;
            handoff = &mut ready_rx => match handoff {
                Ok(partner) => {
                    run_pair(conn, &join, partner.conn, &partner.join, &self.config, &self.metrics).await
                }
                Err(_) => {
                    let expired = self.room_is_expired(&room_id);
                    if expired {
                        let _ = conn.write_control(&Reply::Expired).await;
                        Err(RendezvousError::Expired)
                    } else {
                        Err(RendezvousError::Rejected(BrokerOutcome::ServerBusy))
                    }
                }
            },
            probe = conn.read_control_with_limit::<Join>(self.config.max_frame_body) => {
                self.evict_waiter(&room_id, waiter_id);
                match probe {
                    Ok(_) => {
                        self.metrics.malformed_joins.fetch_add(1, Ordering::Relaxed);
                        Err(RendezvousError::Rejected(BrokerOutcome::MalformedJoin))
                    }
                    Err(error) => Err(error),
                }
            },
            () = tokio::time::sleep(wait) => {
                self.expire_room(&room_id, waiter_id, Instant::now());
                let _ = conn.write_control(&Reply::Expired).await;
                close_connection(conn, self.config.close_grace).await;
                self.metrics.timeouts.fetch_add(1, Ordering::Relaxed);
                Err(RendezvousError::Expired)
            }
        }
    }

    fn evict_waiter(&self, room_id: &str, waiter_id: u64) {
        let mut state = self.state.lock().expect("registry mutex");
        if let Some(room) = state.rooms.get_mut(room_id)
            && room
                .waiter
                .as_ref()
                .is_some_and(|waiter| waiter.id == waiter_id)
        {
            room.waiter.take();
        }
    }

    fn expire_room(&self, room_id: &str, waiter_id: u64, now: Instant) {
        let mut state = self.state.lock().expect("registry mutex");
        let Some(room) = state.rooms.get_mut(room_id) else {
            return;
        };
        if room
            .waiter
            .as_ref()
            .is_some_and(|waiter| waiter.id == waiter_id)
            && matches!(room.status, RoomStatus::Live)
        {
            room.waiter.take();
            room.status = RoomStatus::Expired {
                purge_at: now + self.config.room_tombstone_ttl,
            };
            self.metrics.expired_rooms.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn room_is_expired(&self, room_id: &str) -> bool {
        self.state
            .lock()
            .expect("registry mutex")
            .rooms
            .get(room_id)
            .is_some_and(|room| matches!(room.status, RoomStatus::Expired { .. }))
    }

    fn sweep_rooms(&self, state: &mut RegistryState, now: Instant) {
        for room in state.rooms.values_mut() {
            if matches!(room.status, RoomStatus::Live) && now >= room.expires_at {
                room.waiter.take();
                room.status = RoomStatus::Expired {
                    purge_at: now + self.config.room_tombstone_ttl,
                };
                self.metrics.expired_rooms.fetch_add(1, Ordering::Relaxed);
            }
        }
        state.rooms.retain(|_, room| {
            let purge_at = match room.status {
                RoomStatus::Live => return true,
                RoomStatus::Expired { purge_at } | RoomStatus::Exhausted { purge_at } => purge_at,
            };
            room.active_connections > 0 || now < purge_at
        });
    }

    fn rejection(&self, outcome: BrokerOutcome, retry_after: Option<Duration>) -> BrokerRejection {
        BrokerRejection {
            outcome,
            retry_after: retry_after
                .and_then(|retry| bounded_retry_after(retry, self.config.max_retry_after)),
        }
    }

    async fn reject(
        &self,
        mut conn: PeerConn,
        rejection: BrokerRejection,
    ) -> Result<(), RendezvousError> {
        self.metrics.rejection(rejection.outcome);
        tracing::info!(outcome = rejection.outcome.code(), "join rejected");
        let outcome = rejection.outcome;
        let _ = tokio::time::timeout(
            self.config.slow_frame_timeout,
            conn.write_control(&Reply::Rejected(rejection)),
        )
        .await;
        close_connection(conn, self.config.close_grace).await;
        Err(RendezvousError::Rejected(outcome))
    }
}

impl Default for RoomRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn park_creator(
    registry: &mut RegistryState,
    shared_state: &Arc<Mutex<RegistryState>>,
    room_id: &str,
    join: &Join,
    next_id: &AtomicU64,
) -> CreatorDecision {
    let room = registry.rooms.get_mut(room_id).expect("Room exists");
    let waiter_id = next_id.fetch_add(1, Ordering::Relaxed);
    let (ready_tx, ready_rx) = oneshot::channel();
    room.waiter = Some(Waiter {
        ready: ready_tx,
        id: waiter_id,
        join: join.clone(),
    });
    room.active_connections += 1;
    CreatorDecision::Parked {
        ready_rx,
        waiter_id,
        expires_at: room.expires_at,
        room_guard: RoomSlotGuard {
            state: shared_state.clone(),
            room_id: room_id.to_string(),
            instance: room.instance,
        },
    }
}

fn bounded_retry_after(retry: Duration, max: Duration) -> Option<u64> {
    if retry.is_zero() {
        return None;
    }
    let retry = retry.min(max);
    Some(
        retry
            .as_secs()
            .saturating_add(u64::from(retry.subsec_nanos() > 0)),
    )
}

fn validate_join(join: &Join) -> Result<(), BrokerOutcome> {
    if join.version != RENDEZVOUS_PROTOCOL_VERSION {
        return Err(BrokerOutcome::UnsupportedVersion);
    }
    let invitation_room =
        join.room_id.len() == ROOM_ID_LEN && join.room_id.bytes().all(|byte| byte.is_ascii_digit());
    let remembered_room = is_remembered_room_id(&join.room_id);
    let room_control = is_room_control_locator(&join.room_id);
    if !invitation_room && !remembered_room && !room_control {
        return Err(BrokerOutcome::MalformedJoin);
    }
    match join.invitation_side {
        InvitationSide::Creator => {
            let valid_methods = if room_control {
                join.bootstrap_methods == [BootstrapKind::RoomCode]
                    && join.transfer_role == TransferRole::Receiver
            } else if remembered_room {
                join.bootstrap_methods == [BootstrapKind::FullTicket]
                    && join.transfer_role == TransferRole::Receiver
            } else {
                join.bootstrap_methods == [BootstrapKind::FullTicket, BootstrapKind::RoomCode]
            };
            if !valid_methods || join.selected_bootstrap_method.is_some() {
                return Err(BrokerOutcome::MalformedJoin);
            }
        }
        InvitationSide::Joiner => {
            let valid_selection = if room_control {
                join.selected_bootstrap_method == Some(BootstrapKind::RoomCode)
                    && join.transfer_role == TransferRole::Sender
            } else if remembered_room {
                join.selected_bootstrap_method == Some(BootstrapKind::FullTicket)
                    && join.transfer_role == TransferRole::Sender
            } else {
                join.selected_bootstrap_method.is_some()
            };
            if !join.bootstrap_methods.is_empty() || !valid_selection {
                return Err(BrokerOutcome::MalformedJoin);
            }
        }
    }
    Ok(())
}

fn is_remembered_room_id(value: &str) -> bool {
    value
        .strip_prefix(REMEMBERED_ROOM_ID_PREFIX)
        .is_some_and(|encoded| {
            encoded.len() == REMEMBERED_ROOM_ID_ENCODED_LEN
                && encoded
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        })
}

fn joins_match(first: &Join, second: &Join) -> Option<BootstrapKind> {
    let (creator, joiner) = match (first.invitation_side, second.invitation_side) {
        (InvitationSide::Creator, InvitationSide::Joiner) => (first, second),
        (InvitationSide::Joiner, InvitationSide::Creator) => (second, first),
        _ => return None,
    };
    if creator.transfer_role.complement() != joiner.transfer_role {
        return None;
    }
    let selected = joiner.selected_bootstrap_method?;
    creator
        .bootstrap_methods
        .contains(&selected)
        .then_some(selected)
}

async fn run_pair(
    mut first: PeerConn,
    first_join: &Join,
    mut second: PeerConn,
    second_join: &Join,
    config: &BrokerConfig,
    metrics: &BrokerMetrics,
) -> Result<(), RendezvousError> {
    let selected = joins_match(first_join, second_join)
        .ok_or(RendezvousError::Rejected(BrokerOutcome::RoomNotFound))?;
    let first_role = role_for(first_join);
    let second_role = role_for(second_join);
    tokio::time::timeout(
        config.slow_frame_timeout,
        first.write_control(&Reply::Paired(Paired {
            role: first_role,
            selected_bootstrap_method: selected,
        })),
    )
    .await
    .map_err(|_| RendezvousError::Timeout)??;
    tokio::time::timeout(
        config.slow_frame_timeout,
        second.write_control(&Reply::Paired(Paired {
            role: second_role,
            selected_bootstrap_method: selected,
        })),
    )
    .await
    .map_err(|_| RendezvousError::Timeout)??;

    let PeerParts {
        writer: first_writer,
        reader: first_reader,
        close: first_close,
        lifetimes: _first_lifetimes,
    } = first.into_parts();
    let PeerParts {
        writer: second_writer,
        reader: second_reader,
        close: second_close,
        lifetimes: _second_lifetimes,
    } = second.into_parts();
    let relay = tokio::time::timeout(config.relay_ttl, async {
        tokio::join!(
            relay_frames(first_reader, second_writer, config),
            relay_frames(second_reader, first_writer, config),
        )
    })
    .await;
    match relay {
        Err(_) => {
            metrics.timeouts.fetch_add(1, Ordering::Relaxed);
        }
        Ok((left, right)) => {
            record_relay_result(metrics, &left);
            record_relay_result(metrics, &right);
        }
    }
    let _ = tokio::time::timeout(config.close_grace, async {
        tokio::join!(first_close.wait_closed(), second_close.wait_closed())
    })
    .await;
    Ok(())
}

fn role_for(join: &Join) -> Role {
    match join.invitation_side {
        InvitationSide::Creator => Role::Responder,
        InvitationSide::Joiner => Role::Initiator,
    }
}

async fn relay_frames<R, W>(
    mut reader: R,
    mut writer: W,
    config: &BrokerConfig,
) -> Result<(), RendezvousError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    loop {
        let mut length = [0_u8; 4];
        match tokio::time::timeout(config.relay_idle_timeout, reader.read_exact(&mut length)).await
        {
            Err(_) => return Err(RendezvousError::Timeout),
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                let _ = writer.shutdown().await;
                return Ok(());
            }
            Ok(Err(error)) => return Err(error.into()),
            Ok(Ok(_)) => {}
        }
        let body_length = u32::from_be_bytes(length) as usize;
        if body_length > config.max_frame_body {
            return Err(RendezvousError::FrameTooLarge);
        }
        let mut body = vec![0_u8; body_length];
        tokio::time::timeout(config.slow_frame_timeout, reader.read_exact(&mut body))
            .await
            .map_err(|_| RendezvousError::Timeout)??;
        tokio::time::timeout(config.slow_frame_timeout, async {
            writer.write_all(&length).await?;
            writer.write_all(&body).await?;
            writer.flush().await
        })
        .await
        .map_err(|_| RendezvousError::Timeout)??;
    }
}

fn record_relay_result(metrics: &BrokerMetrics, result: &Result<(), RendezvousError>) {
    match result {
        Err(RendezvousError::Timeout) => {
            metrics.timeouts.fetch_add(1, Ordering::Relaxed);
        }
        Err(RendezvousError::FrameTooLarge) => {
            metrics.oversized_frames.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }
}

async fn close_connection(conn: PeerConn, close_grace: Duration) {
    let PeerParts {
        mut writer,
        reader: _reader,
        close,
        lifetimes: _lifetimes,
    } = conn.into_parts();
    let _ = writer.shutdown().await;
    let _ = tokio::time::timeout(close_grace, close.wait_closed()).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn test_config() -> BrokerConfig {
        let generous = RateLimitConfig {
            events: 100,
            period: Duration::from_secs(60),
            burst: 100,
        };
        BrokerConfig {
            endpoint_join_rate: generous,
            ip_join_rate: generous,
            subnet_join_rate: generous,
            ..BrokerConfig::default()
        }
    }

    #[test]
    fn token_bucket_reports_exact_boundary() {
        let now = Instant::now();
        let mut bucket = TokenBucket::new(
            RateLimitConfig {
                events: 2,
                period: Duration::from_secs(10),
                burst: 1,
            },
            now,
        );
        assert_eq!(bucket.take(now), Ok(()));
        assert_eq!(bucket.take(now), Err(Duration::from_secs(5)));
        assert_eq!(bucket.take(now + Duration::from_secs(5)), Ok(()));
    }

    #[test]
    fn prefixes_aggregate_at_v4_24_and_v6_64() {
        assert_eq!(
            IpPrefix::from(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))),
            IpPrefix::from(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 254)))
        );
        assert_ne!(
            IpPrefix::from(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))),
            IpPrefix::from(IpAddr::V4(Ipv4Addr::new(192, 0, 3, 1)))
        );
        assert_eq!(
            IpPrefix::from(IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 1, 2, 3, 4, 5, 6))),
            IpPrefix::from(IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 1, 2, 9, 8, 7, 6)))
        );
    }

    #[test]
    fn retry_after_is_rounded_up_and_capped() {
        assert_eq!(
            bounded_retry_after(Duration::from_millis(1), Duration::from_secs(5)),
            Some(1)
        );
        assert_eq!(
            bounded_retry_after(Duration::from_secs(8), Duration::from_secs(5)),
            Some(5)
        );
        assert_eq!(
            bounded_retry_after(Duration::ZERO, Duration::from_secs(5)),
            None
        );
    }

    #[test]
    fn endpoint_rate_and_concurrency_are_isolated() {
        let now = Instant::now();
        let mut config = test_config();
        config.endpoint_join_rate = RateLimitConfig {
            events: 1,
            period: Duration::from_secs(60),
            burst: 1,
        };
        let tracker = Arc::new(Mutex::new(SourceTracker::new(config.clone())));
        let source_a = PeerSource::new([1; 32], None);
        let source_b = PeerSource::new([2; 32], None);
        drop(SourceTracker::admit(&tracker, source_a, now).unwrap());
        assert_eq!(
            SourceTracker::admit(&tracker, source_a, now)
                .err()
                .unwrap()
                .outcome,
            BrokerOutcome::EndpointRateLimited
        );
        assert!(SourceTracker::admit(&tracker, source_b, now).is_ok());

        config.max_connections_per_endpoint = 1;
        config.endpoint_join_rate = RateLimitConfig {
            events: 10,
            period: Duration::from_secs(60),
            burst: 10,
        };
        let tracker = Arc::new(Mutex::new(SourceTracker::new(config)));
        let first = SourceTracker::admit(&tracker, source_a, now).unwrap();
        assert_eq!(
            SourceTracker::admit(&tracker, source_a, now)
                .err()
                .unwrap()
                .outcome,
            BrokerOutcome::EndpointRateLimited
        );
        drop(first);
        assert!(SourceTracker::admit(&tracker, source_a, now).is_ok());
    }

    #[test]
    fn direct_ips_debit_individual_and_looser_prefix_buckets() {
        let now = Instant::now();
        let mut config = test_config();
        config.subnet_join_rate = RateLimitConfig {
            events: 1,
            period: Duration::from_secs(60),
            burst: 1,
        };
        let tracker = Arc::new(Mutex::new(SourceTracker::new(config)));
        let first = PeerSource::new([1; 32], Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))));
        let same_prefix = PeerSource::new([2; 32], Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 99))));
        let other_prefix = PeerSource::new([3; 32], Some(IpAddr::V4(Ipv4Addr::new(192, 0, 3, 1))));
        assert!(SourceTracker::admit(&tracker, first, now).is_ok());
        assert_eq!(
            SourceTracker::admit(&tracker, same_prefix, now)
                .err()
                .unwrap()
                .outcome,
            BrokerOutcome::IpRateLimited
        );
        assert!(SourceTracker::admit(&tracker, other_prefix, now).is_ok());
    }

    #[test]
    fn source_tracking_refuses_growth_past_its_cap() {
        let now = Instant::now();
        let mut config = test_config();
        config.max_source_states = 3;
        let tracker = Arc::new(Mutex::new(SourceTracker::new(config)));
        let direct = PeerSource::new([1; 32], Some(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1))));
        drop(SourceTracker::admit(&tracker, direct, now).unwrap());
        assert_eq!(tracker.lock().unwrap().entries.len(), 3);
        assert_eq!(
            SourceTracker::admit(&tracker, PeerSource::new([2; 32], None), now)
                .err()
                .unwrap()
                .outcome,
            BrokerOutcome::ServerBusy
        );
        assert_eq!(tracker.lock().unwrap().entries.len(), 3);
    }

    #[test]
    fn source_state_expires_at_the_exact_ttl_boundary() {
        let now = Instant::now();
        let mut config = test_config();
        config.max_source_states = 3;
        config.source_state_ttl = Duration::from_secs(10);
        let tracker = Arc::new(Mutex::new(SourceTracker::new(config)));
        let first = PeerSource::new([1; 32], Some(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1))));
        drop(SourceTracker::admit(&tracker, first, now).unwrap());
        assert!(
            SourceTracker::admit(
                &tracker,
                PeerSource::new([2; 32], None),
                now + Duration::from_secs(10)
            )
            .is_ok()
        );
        assert_eq!(tracker.lock().unwrap().entries.len(), 1);
    }

    #[test]
    fn room_expiry_and_tombstone_purge_use_inclusive_boundaries() {
        let now = Instant::now();
        let config = BrokerConfig {
            room_tombstone_ttl: Duration::from_secs(5),
            ..test_config()
        };
        let registry = RoomRegistry::with_config(config.clone()).unwrap();
        let mut state = RegistryState::default();
        state.rooms.insert(
            "580001".into(),
            RoomState {
                instance: 1,
                expires_at: now,
                attempts: 0,
                attempt_rate: TokenBucket::new(config.room_attempt_rate, now),
                active_connections: 0,
                waiter: None,
                status: RoomStatus::Live,
                short_code: true,
            },
        );
        registry.sweep_rooms(&mut state, now);
        assert!(matches!(
            state.rooms["580001"].status,
            RoomStatus::Expired { .. }
        ));
        registry.sweep_rooms(&mut state, now + config.room_tombstone_ttl);
        assert!(!state.rooms.contains_key("580001"));
    }
}
