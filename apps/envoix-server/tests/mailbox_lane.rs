use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use envoix_mailbox::{HttpReceiptMailbox, MailboxClientError};
use envoix_protocol::mailbox::{SealedReceipt, receipt_slot};
use envoix_server::{ServerConfig, run};
use envoix_types::TransferId;
use tempfile::TempDir;

fn server_config(directory: &TempDir) -> ServerConfig {
    let mut config = ServerConfig::operational_defaults();
    config.bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    config.mailbox_bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    config.node_key_path = directory.path().join("node.key");
    config.close_grace = Duration::from_secs(2);
    config.bind_deadline = Duration::from_secs(2);
    config.mailbox_ttl = Duration::from_millis(50);
    config.mailbox_max_blob_size = 16;
    config.mailbox_max_key_length = 64;
    config.mailbox_max_entries = 1;
    config
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mailbox_bounds_ttl_capacity_and_restart_are_enforced() {
    let directory = TempDir::new().unwrap();
    let config = server_config(&directory);
    let server = run(config.clone()).await.unwrap();
    let mailbox =
        HttpReceiptMailbox::new(&format!("http://{}", server.mailbox_bound_addr())).unwrap();
    let first_slot = receipt_slot(TransferId::from_bytes([0x11; 16]));
    let second_slot = receipt_slot(TransferId::from_bytes([0x22; 16]));
    let blob = SealedReceipt::from_bytes(vec![0x33; 16]).unwrap();
    let oversized = SealedReceipt::from_bytes(vec![0x44; 17]).unwrap();

    mailbox.post(first_slot, &blob).await.unwrap();
    assert_eq!(mailbox.poll(first_slot).await.unwrap(), Some(blob.clone()));
    assert_eq!(
        mailbox.post(first_slot, &oversized).await,
        Err(MailboxClientError::UnexpectedStatus(413))
    );
    assert_eq!(
        mailbox.post(second_slot, &blob).await,
        Err(MailboxClientError::UnexpectedStatus(429))
    );

    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(mailbox.poll(first_slot).await.unwrap(), None);
    mailbox.post(second_slot, &blob).await.unwrap();
    server.shutdown().await.unwrap();

    let restarted = run(config).await.unwrap();
    let restarted_mailbox =
        HttpReceiptMailbox::new(&format!("http://{}", restarted.mailbox_bound_addr())).unwrap();
    assert_eq!(restarted_mailbox.poll(second_slot).await.unwrap(), None);
    restarted.shutdown().await.unwrap();
}
