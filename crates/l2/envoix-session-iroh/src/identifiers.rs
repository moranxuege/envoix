/// Explicit mDNS service label; never inherit the iroh dependency's default.
pub const MDNS_SERVICE_LABEL: &str = "envoix";
pub const CLIENT_PEER_KEY_ALIAS: &str = "envoix.iroh.peer-key.v2";

pub fn mdns_service_fqdn() -> String {
    format!("_{MDNS_SERVICE_LABEL}._udp.local.")
}
