//! The two properties D1 exists for: one service's load cannot take another's
//! capacity, and a caller that is turned away is told so.
//!
//! Both tests are written to FAIL if the budgets are merged. Saturating the
//! mailbox here holds every one of its slots, so a shared pool would leave
//! pairing nothing — which is precisely what the assertions below would catch.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

use envoix_invite::RoomCode;
use envoix_protocol::mailbox::identifiers::RECEIPT_HTTP_ROUTE;
use envoix_protocol::mailbox::receipt_slot;
use envoix_rendezvous::{ClientConfig, ControlLimits};
use envoix_rendezvous_iroh::{
    BrokerSession, EndpointConfig, IrohClientConfig, IrohRendezvousError, bind_endpoint, join_room,
};
use envoix_server::{BUDGET_HTTP_ROUTE, BudgetMeter, ServerConfig, ServerHandle, run};
use envoix_types::TransferId;
use iroh::{Endpoint, EndpointAddr, SecretKey};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;

const MAILBOX_SLOTS: usize = 4;
const PAIRING_SLOTS: usize = 2;
const SPRAY: usize = 48;

fn server_config(directory: &TempDir) -> ServerConfig {
    let loopback = |port| SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    let mut config = ServerConfig::operational_defaults();
    config.bind = loopback(0);
    config.mailbox_bind = loopback(0);
    config.diagnostics_bind = loopback(0);
    config.node_key_path = directory.path().join("node.key");
    config.close_grace = Duration::from_secs(2);
    config.bind_deadline = Duration::from_secs(2);
    config.join_deadline = Duration::from_secs(2);
    config.handshake_deadline = Duration::from_secs(2);
    config.max_connections = PAIRING_SLOTS;
    config.mailbox_max_connections = MAILBOX_SLOTS;
    config.diagnostics_max_connections = 2;
    config.pairing_workers = 1;
    config.mailbox_workers = 1;
    config.diagnostics_workers = 1;
    config
}

fn client_config() -> IrohClientConfig {
    IrohClientConfig::new(
        Duration::from_secs(2),
        Duration::from_secs(2),
        Duration::from_secs(2),
        ClientConfig::new(Duration::from_secs(2), ControlLimits::new(64).unwrap()).unwrap(),
    )
    .unwrap()
}

async fn client_endpoint() -> Endpoint {
    bind_endpoint(
        EndpointConfig::new(
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
            None,
            SecretKey::generate(),
            Duration::from_secs(2),
        )
        .unwrap(),
    )
    .await
    .unwrap()
}

/// One peer waiting in a room for a partner who never comes. The server holds
/// its connection — and therefore a pairing slot — for as long as it waits,
/// which is how a real caller occupies the budget.
async fn hold_pairing_slot(broker: EndpointAddr, code: &str) -> JoinHandle<()> {
    let endpoint = client_endpoint().await;
    let room = RoomCode::parse(code).unwrap().namespaced_key();
    let patient = IrohClientConfig::new(
        Duration::from_secs(10),
        Duration::from_secs(10),
        Duration::from_secs(10),
        ClientConfig::new(Duration::from_secs(10), ControlLimits::new(64).unwrap()).unwrap(),
    )
    .unwrap();
    tokio::spawn(async move {
        let _held = join_room(&endpoint, broker, room, patient).await;
        std::future::pending::<()>().await;
    })
}

async fn wait_until(meter: &BudgetMeter, in_flight: usize) {
    for _ in 0..200 {
        if meter.in_flight() == in_flight {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "{} never reached {in_flight} in flight",
        meter.service().as_str()
    );
}

/// Two peers meeting in a room, which is what pairing is for.
async fn pair(broker: &EndpointAddr, code: &str) -> (BrokerSession, BrokerSession) {
    let (first, second) = (client_endpoint().await, client_endpoint().await);
    let room = RoomCode::parse(code).unwrap().namespaced_key();
    let join_first = join_room(&first, broker.clone(), room.clone(), client_config());
    let join_second = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        join_room(&second, broker.clone(), room, client_config()).await
    };
    let (first, second) = tokio::join!(join_first, join_second);
    (first.unwrap(), second.unwrap())
}

/// One raw request, so the exact status line a refused caller receives is
/// visible rather than translated by a client library.
async fn request(address: SocketAddr, request: &str, body: &[u8]) -> (TcpStream, String) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    let mut response = vec![0; 512];
    let read = stream.read(&mut response).await.unwrap();
    response.truncate(read);
    (stream, String::from_utf8_lossy(&response).into_owned())
}

fn post_receipt(address: SocketAddr, index: u8) -> (String, Vec<u8>) {
    let slot = receipt_slot(TransferId::from_bytes([index; 16]));
    let path = RECEIPT_HTTP_ROUTE.replace("{slot}", &slot.path_component());
    let body = vec![b'r'; 32];
    (
        format!(
            "POST {path} HTTP/1.1\r\nhost: {address}\r\ncontent-length: {}\r\n\r\n",
            body.len()
        ),
        body,
    )
}

/// Opens `SPRAY` mailbox connections and keeps every one of them, so each
/// admitted connection holds a slot for the rest of the test. Returns the held
/// connections and how many were refused.
async fn spray_receipts(address: SocketAddr) -> (Vec<TcpStream>, usize, usize) {
    let mut held = Vec::new();
    let (mut accepted, mut refused) = (0, 0);
    for index in 0..SPRAY {
        let (head, body) = post_receipt(address, u8::try_from(index).unwrap());
        let (stream, response) = request(address, &head, &body).await;
        if response.starts_with("HTTP/1.1 503 Service Unavailable") {
            assert!(
                response.contains("retry-after"),
                "a refusal must say when to come back: {response}"
            );
            refused += 1;
        } else {
            accepted += 1;
            held.push(stream);
        }
    }
    (held, accepted, refused)
}

/// One service's line of the readout.
fn budget_of<'a>(readout: &'a str, service: &str) -> &'a str {
    let key = format!("{{\"service\":\"{service}\"");
    let start = readout
        .find(&key)
        .unwrap_or_else(|| panic!("{service} missing: {readout}"));
    let end = start + readout[start..].find('}').expect("an object ends");
    &readout[start..=end]
}

async fn diagnostics(handle: &ServerHandle) -> String {
    let address = handle.diagnostics_bound_addr();
    let (_stream, response) = request(
        address,
        &format!("GET {BUDGET_HTTP_ROUTE} HTTP/1.1\r\nhost: {address}\r\n\r\n"),
        b"",
    )
    .await;
    response
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn receipt_spray_cannot_starve_pairing() {
    let directory = TempDir::new().unwrap();
    let server = run(server_config(&directory)).await.unwrap();
    let broker = server.endpoint_addr().clone();

    let (held, accepted, refused) = spray_receipts(server.mailbox_bound_addr()).await;
    assert_eq!(
        accepted, MAILBOX_SLOTS,
        "the mailbox must admit exactly its own budget"
    );
    assert_eq!(
        refused,
        SPRAY - MAILBOX_SLOTS,
        "every caller beyond the budget must be refused, not queued"
    );

    // With the mailbox holding every slot it owns, pairing must still pair
    // inside its own deadline. A shared pool would have nothing left here.
    let started = Instant::now();
    let (first, second) = pair(&broker, "123456-amber-comet").await;
    let elapsed = started.elapsed();
    assert_ne!(first.role(), second.role());
    assert!(
        elapsed < Duration::from_secs(2),
        "pairing took {elapsed:?} while the mailbox was saturated"
    );

    let readout = diagnostics(&server).await;
    let mailbox = budget_of(&readout, "mailbox");
    assert!(
        mailbox.contains(&format!(
            "\"capacity\":{MAILBOX_SLOTS},\"in_flight\":{MAILBOX_SLOTS}"
        )),
        "the mailbox budget should read as full: {mailbox}"
    );
    let pairing = budget_of(&readout, "pairing");
    assert!(
        pairing.contains("\"admitted\":2,\"refused\":0"),
        "pairing should have spent only its own budget: {pairing}"
    );

    // Dropped rather than closed: this test is about who was admitted, and a
    // graceful close is the rendezvous crate's own subject.
    drop(held);
    drop((first, second));
    server.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_admission_isolation() {
    let directory = TempDir::new().unwrap();
    let server = run(server_config(&directory)).await.unwrap();
    let broker = server.endpoint_addr().clone();
    let mailbox_address = server.mailbox_bound_addr();

    // A saturated mailbox refuses in its own name, at once, and says so.
    let (held, _, refused) = spray_receipts(mailbox_address).await;
    assert!(refused > 0);
    let (head, body) = post_receipt(mailbox_address, 200);
    let started = Instant::now();
    let (_stream, response) = request(mailbox_address, &head, &body).await;
    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "a refusal must not look like a timeout"
    );

    // Diagnostics answers while the mailbox is full: the operator surface is
    // not a hostage of the service being asked about.
    assert!(diagnostics(&server).await.starts_with("HTTP/1.1 200 OK"));

    // And pairing admits, because pairing's budget is untouched by mailbox
    // load. Two waiting peers fill it exactly.
    let first = hold_pairing_slot(broker.clone(), "123456-amber-comet").await;
    let second = hold_pairing_slot(broker.clone(), "222222-amber-comet").await;
    wait_until(&server.meters()[0], PAIRING_SLOTS).await;

    // The other direction: with pairing now full, a further peer is REFUSED —
    // told, not left to time out — while the mailbox admits again as soon as
    // its own slots are free.
    let third = client_endpoint().await;
    let other_room = RoomCode::parse("333333-amber-comet")
        .unwrap()
        .namespaced_key();
    let started = Instant::now();
    let turned_away = join_room(&third, broker.clone(), other_room, client_config()).await;
    assert!(
        matches!(turned_away, Err(IrohRendezvousError::Refused)),
        "a full pairing budget must refuse in its own name"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the refusal must arrive before any deadline would"
    );

    drop(held);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (head, body) = post_receipt(mailbox_address, 201);
    let (_stream, response) = request(mailbox_address, &head, &body).await;
    assert!(
        response.starts_with("HTTP/1.1 204"),
        "freed mailbox slots must be usable while pairing is full: {response}"
    );

    let readout = diagnostics(&server).await;
    let pairing = budget_of(&readout, "pairing");
    assert!(
        pairing.contains("\"capacity\":2,\"in_flight\":2,\"admitted\":2,\"refused\":1"),
        "{pairing}"
    );

    third.close().await;
    first.abort();
    second.abort();
    server.shutdown().await.unwrap();
}

/// Each service's work runs on its own workers, so a budget is CPU as well as
/// admission. Merge the runtimes and these names collapse into one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_service_is_served_by_its_own_workers() {
    let directory = TempDir::new().unwrap();
    let server = run(server_config(&directory)).await.unwrap();
    let broker = server.endpoint_addr().clone();

    let (head, body) = post_receipt(server.mailbox_bound_addr(), 7);
    let (_stream, response) = request(server.mailbox_bound_addr(), &head, &body).await;
    assert!(response.starts_with("HTTP/1.1 204"), "{response}");
    let (first, second) = pair(&broker, "123456-amber-comet").await;
    drop((first, second));
    assert!(diagnostics(&server).await.starts_with("HTTP/1.1 200 OK"));

    let workers: Vec<&str> = server
        .meters()
        .iter()
        .map(|meter| meter.worker().expect("every service served something"))
        .collect();
    let mut distinct = workers.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        3,
        "services shared an executor: {workers:?}"
    );

    server.shutdown().await.unwrap();
}
