use std::sync::Arc;

use envoix_client::api::{
    NearbyInvite, NearbyInviteEndpoint, NearbyInviteInbox,
    start_nearby_invite_inbox as start_core_nearby_invite_inbox,
};

use crate::{DEFAULT_RELAY_URL, EnvoixError, non_empty, op_err, spawn_on_ffi_runtime};

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiNearbyInvite {
    pub request_id: u64,
    pub sender_endpoint_id: String,
    pub sender_peer_key: String,
    pub sender_display_name: String,
    pub invite: String,
    pub expires_at_epoch_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiNearbyInviteEndpoint {
    pub endpoint_id: String,
    pub relay_url: Option<String>,
    pub direct_addresses: Vec<String>,
}

#[derive(uniffi::Object)]
pub struct FfiNearbyInviteInbox {
    inbox: Arc<NearbyInviteInbox>,
}

#[uniffi::export]
impl FfiNearbyInviteInbox {
    pub fn endpoint(&self) -> FfiNearbyInviteEndpoint {
        project_endpoint(self.inbox.endpoint())
    }

    pub async fn next_invite(&self) -> Result<FfiNearbyInvite, EnvoixError> {
        let inbox = self.inbox.clone();
        spawn_on_ffi_runtime(async move {
            inbox
                .next_invite()
                .await
                .map(project_invite)
                .map_err(op_err)
        })
        .await
    }

    pub async fn send_invite(
        &self,
        endpoint: FfiNearbyInviteEndpoint,
        invite: String,
    ) -> Result<(), EnvoixError> {
        let inbox = self.inbox.clone();
        let endpoint = core_endpoint(endpoint);
        spawn_on_ffi_runtime(
            async move { inbox.send_invite(&endpoint, &invite).await.map_err(op_err) },
        )
        .await
    }

    pub async fn shutdown(&self) -> Result<(), EnvoixError> {
        let inbox = self.inbox.clone();
        spawn_on_ffi_runtime(async move {
            inbox.close().await;
            Ok::<(), EnvoixError>(())
        })
        .await
    }
}

#[uniffi::export]
pub async fn start_nearby_invite_inbox(
    relay: String,
    peer_key: String,
    display_name: String,
) -> Result<Arc<FfiNearbyInviteInbox>, EnvoixError> {
    let relay = Some(non_empty(&relay).unwrap_or(DEFAULT_RELAY_URL).to_string());
    spawn_on_ffi_runtime(async move {
        start_core_nearby_invite_inbox(relay, peer_key, display_name)
            .await
            .map(|inbox| {
                Arc::new(FfiNearbyInviteInbox {
                    inbox: Arc::new(inbox),
                })
            })
            .map_err(op_err)
    })
    .await
}

fn project_endpoint(endpoint: NearbyInviteEndpoint) -> FfiNearbyInviteEndpoint {
    FfiNearbyInviteEndpoint {
        endpoint_id: endpoint.endpoint_id,
        relay_url: endpoint.relay_url,
        direct_addresses: endpoint.direct_addresses,
    }
}

fn core_endpoint(endpoint: FfiNearbyInviteEndpoint) -> NearbyInviteEndpoint {
    NearbyInviteEndpoint {
        endpoint_id: endpoint.endpoint_id,
        relay_url: endpoint.relay_url,
        direct_addresses: endpoint.direct_addresses,
    }
}

fn project_invite(invite: NearbyInvite) -> FfiNearbyInvite {
    FfiNearbyInvite {
        request_id: invite.request_id,
        sender_endpoint_id: invite.sender_endpoint_id,
        sender_peer_key: invite.sender_peer_key,
        sender_display_name: invite.sender_display_name,
        invite: invite.invite,
        expires_at_epoch_ms: invite.expires_at_unix_secs.saturating_mul(1_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_projection_preserves_sender_and_saturates_expiry_millis() {
        let projected = project_invite(NearbyInvite {
            request_id: 9,
            sender_endpoint_id: "endpoint".into(),
            sender_peer_key: "0011223344556677".into(),
            sender_display_name: "Sender".into(),
            invite: "secret".into(),
            expires_at_unix_secs: u64::MAX,
        });
        assert_eq!(projected.request_id, 9);
        assert_eq!(projected.sender_endpoint_id, "endpoint");
        assert_eq!(projected.sender_peer_key, "0011223344556677");
        assert_eq!(projected.sender_display_name, "Sender");
        assert_eq!(projected.invite, "secret");
        assert_eq!(projected.expires_at_epoch_ms, u64::MAX);
    }

    #[test]
    fn endpoint_projection_preserves_explicit_routes() {
        let ffi = project_endpoint(NearbyInviteEndpoint {
            endpoint_id: "endpoint".into(),
            relay_url: Some("https://relay.example".into()),
            direct_addresses: vec!["192.0.2.1:4433".into(), "[2001:db8::1]:4433".into()],
        });
        assert_eq!(ffi.endpoint_id, "endpoint");
        assert_eq!(ffi.relay_url.as_deref(), Some("https://relay.example"));
        assert_eq!(
            ffi.direct_addresses,
            ["192.0.2.1:4433", "[2001:db8::1]:4433"]
        );
        assert_eq!(
            core_endpoint(ffi),
            NearbyInviteEndpoint {
                endpoint_id: "endpoint".into(),
                relay_url: Some("https://relay.example".into()),
                direct_addresses: vec!["192.0.2.1:4433".into(), "[2001:db8::1]:4433".into()],
            }
        );
    }

    #[test]
    fn core_info_advertises_nearby_invite_inbox_v1_ffi_v25() {
        let info = crate::envoix_core_info();
        assert_eq!(info.ffi_api_version, 25);
        assert!(
            info.capabilities
                .iter()
                .any(|capability| capability == "nearby_invite_inbox_v1")
        );
    }
}
