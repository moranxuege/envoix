pub const URI_SCHEME: &str = "envoix";
pub const QR_OUTER_PREFIX: &str = "envoix:";
pub const DEEP_LINK_OUTER_PREFIX: &str = "envoix://";
pub const INVITE_PAYLOAD_VERSION: u32 = 3;

/// Prefix for the server's internal lookup key. It is never rendered in a human code.
pub const ROOM_CODE_NAMESPACE_PREFIX: &str = "v2:";

pub struct InviteDialect;

impl InviteDialect {
    pub fn canonical_identifier() -> String {
        format!(
            "qr-prefix={QR_OUTER_PREFIX};deep-link-prefix={DEEP_LINK_OUTER_PREFIX};payload-version={INVITE_PAYLOAD_VERSION}"
        )
    }

    /// Legacy forms that a future parser must classify as recognized-but-unsupported.
    pub const fn legacy_rejection_identifiers() -> &'static [&'static str] {
        &[
            "qr-prefix=envoix:;payload-version=2",
            "deep-link-prefix=envoix://pair/",
            "bare-room-code=^[0-9]{6}$",
            "bare-room-code=^[0-9]{6}-<legacy-word>-<legacy-word>$",
        ]
    }
}
