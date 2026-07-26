//! Directional invitation facade.
//!
//! The carrier-neutral grammar and cryptography live in `envoix-invite`; this
//! module maps its typed failures into the application-facing client error.

pub use envoix_invite::{
    BootstrapKind, Capabilities, CreatedInvitation, InvitationAuthContext, InvitationBootstrap,
    InvitationError, InvitationErrorCode, InvitationPublicContext, InvitationSide, InviteV2,
    RoomCode, TransferRole, ValidatedInvitation,
};

use super::TransferError;

pub fn create_invitation(
    broker: String,
    relay_urls: Vec<String>,
    creator_role: TransferRole,
    now: u64,
) -> Result<CreatedInvitation, TransferError> {
    InviteV2::create(
        broker,
        relay_urls,
        creator_role,
        Capabilities::current(),
        now,
    )
    .map_err(to_transfer_error)
}

pub fn parse_invitation_for_role(
    payload: &str,
    local_role: TransferRole,
    now: u64,
) -> Result<ValidatedInvitation, TransferError> {
    InviteV2::parse_for_role(payload, local_role, now).map_err(to_transfer_error)
}

pub fn parse_invitation_for_routing(
    payload: &str,
    now: u64,
) -> Result<ValidatedInvitation, TransferError> {
    InviteV2::parse(payload, now).map_err(to_transfer_error)
}

pub fn parse_room_code(
    input: &str,
    local_role: TransferRole,
) -> Result<InvitationBootstrap, TransferError> {
    RoomCode::parse(input)
        .map(|room_code| InvitationBootstrap::room_code_joiner(room_code, local_role))
        .map_err(to_transfer_error)
}

fn to_transfer_error(error: InvitationError) -> TransferError {
    TransferError::input(format!("invitation {}: {error}", error.code().as_str()))
}
