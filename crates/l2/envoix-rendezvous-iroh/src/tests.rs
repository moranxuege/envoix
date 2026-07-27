use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{
    ConfigError, ConnectionAdmission, EndpointConfig, IrohClientConfig, IrohRendezvousError,
    IrohServerConfig, ServeOutcome, bind_endpoint, endpoint_addr, join_room, serve_endpoint,
};
use envoix_invite::{NamespacedRoomKey, RoomCode};
use envoix_rendezvous::{ClientConfig, ControlLimits, RegistryConfig, RoomRegistry};
use iroh::SecretKey;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// A connection budget with a fixed number of slots, standing in for the
/// composition root's.
struct Slots(Arc<Semaphore>);

impl ConnectionAdmission for Slots {
    type Permit = OwnedSemaphorePermit;

    fn try_admit(&self) -> Option<Self::Permit> {
        Arc::clone(&self.0).try_acquire_owned().ok()
    }
}

fn endpoint_config() -> EndpointConfig {
    EndpointConfig::new(
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        None,
        SecretKey::generate(),
        Duration::from_secs(2),
    )
    .unwrap()
}

fn core_limits() -> ControlLimits {
    ControlLimits::new(32).unwrap()
}

fn client_config() -> IrohClientConfig {
    IrohClientConfig::new(
        Duration::from_secs(2),
        Duration::from_secs(2),
        Duration::from_secs(2),
        ClientConfig::new(Duration::from_secs(3), core_limits()).unwrap(),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rendezvous_iroh_loopback() {
    let server = bind_endpoint(endpoint_config()).await.unwrap();
    let broker = endpoint_addr(&server);
    let server_shutdown = server.clone();
    let registry = Arc::new(RoomRegistry::new(
        RegistryConfig::new(
            Duration::from_secs(3),
            Duration::from_secs(3),
            Duration::from_secs(2),
            Duration::from_secs(2),
            core_limits(),
            16,
        )
        .unwrap(),
    ));
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let observed = outcomes.clone();
    let server_task = tokio::spawn(serve_endpoint(
        server,
        registry,
        IrohServerConfig::new(Duration::from_secs(2)).unwrap(),
        Slots(Arc::new(Semaphore::new(16))),
        move |outcome| {
            if let ServeOutcome::Completed(result) = outcome {
                observed.lock().unwrap().push(result.clone());
            }
        },
    ));

    let client_a = bind_endpoint(endpoint_config()).await.unwrap();
    let client_b = bind_endpoint(endpoint_config()).await.unwrap();
    let room = RoomCode::parse("123456-amber-comet")
        .unwrap()
        .namespaced_key();
    assert_eq!(room.as_str(), "v2:123456");
    let broker_a = broker.clone();
    let room_a = room.clone();
    let join_a = join_room(&client_a, broker_a, room_a, client_config());
    let join_b = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        join_room(&client_b, broker, room, client_config()).await
    };
    let (session_a, session_b) = tokio::join!(join_a, join_b);
    let mut session_a = session_a.unwrap();
    let mut session_b = session_b.unwrap();
    assert_ne!(session_a.role(), session_b.role());

    let from_a = b"\x00iroh-opaque-a\xff";
    let from_b = b"\xfeiroh-opaque-b\x01";
    session_a.streams_mut().0.write_all(from_a).await.unwrap();
    session_b.streams_mut().0.write_all(from_b).await.unwrap();
    let mut at_a = vec![0; from_b.len()];
    let mut at_b = vec![0; from_a.len()];
    tokio::try_join!(
        session_a.streams_mut().1.read_exact(&mut at_a),
        session_b.streams_mut().1.read_exact(&mut at_b),
    )
    .unwrap();
    assert_eq!(at_a, from_b);
    assert_eq!(at_b, from_a);

    let (closed_a, closed_b) = tokio::join!(session_a.close(), session_b.close());
    closed_a.unwrap();
    closed_b.unwrap();
    server_shutdown.close().await;
    server_task.await.unwrap().unwrap();
    let outcomes = outcomes.lock().unwrap();
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.iter().all(Result::is_ok));
}

/// A caller the budget cannot admit is TOLD, and told at once. A refusal that
/// arrives as a timeout is indistinguishable from a dead server.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_budget_refuses_instead_of_letting_the_caller_time_out() {
    let server = bind_endpoint(endpoint_config()).await.unwrap();
    let broker = endpoint_addr(&server);
    let server_shutdown = server.clone();
    let registry = Arc::new(RoomRegistry::new(
        RegistryConfig::new(
            Duration::from_secs(3),
            Duration::from_secs(3),
            Duration::from_secs(2),
            Duration::from_secs(2),
            core_limits(),
            16,
        )
        .unwrap(),
    ));
    let refusals = Arc::new(Mutex::new(0_usize));
    let counted = refusals.clone();
    // A budget with no free slot: every arrival must be refused.
    let full = Slots(Arc::new(Semaphore::new(0)));
    let server_task = tokio::spawn(serve_endpoint(
        server,
        registry,
        IrohServerConfig::new(Duration::from_secs(2)).unwrap(),
        full,
        move |outcome| {
            if matches!(outcome, ServeOutcome::Refused) {
                *counted.lock().unwrap() += 1;
            }
        },
    ));

    let client = bind_endpoint(endpoint_config()).await.unwrap();
    let room = RoomCode::parse("123456-amber-comet")
        .unwrap()
        .namespaced_key();
    let started = std::time::Instant::now();
    let refused = join_room(&client, broker, room, client_config()).await;
    let elapsed = started.elapsed();

    assert!(
        matches!(refused, Err(IrohRendezvousError::Refused)),
        "a full budget must refuse, not fail vaguely"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "a refusal must not take the connect deadline, took {elapsed:?}"
    );
    client.close().await;
    server_shutdown.close().await;
    server_task.await.unwrap().unwrap();
    assert_eq!(*refusals.lock().unwrap(), 1, "the server counted it too");
}

#[test]
fn iroh_config_rejects_zero_policy() {
    assert!(matches!(
        IrohServerConfig::new(Duration::ZERO),
        Err(ConfigError::ZeroDuration { .. })
    ));
    assert!(matches!(
        IrohClientConfig::new(
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_secs(1),
            ClientConfig::new(Duration::from_secs(1), core_limits()).unwrap(),
        ),
        Err(ConfigError::ZeroDuration { .. })
    ));
    assert!(NamespacedRoomKey::parse("v2:123456-amber-comet").is_err());
}
