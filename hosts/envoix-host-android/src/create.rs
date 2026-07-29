//! Turning a frontend create intent into a [`NewTransfer`] the authority will
//! admit — or into the authority's own typed refusal.
//!
//! Every judgement about an invite is made here by `envoix-invite` (C4), never
//! by the frontend: the grammar, the bounds, the version, the dialect and the
//! declared role are all its answers (`XI02`). The frontend carries opaque text
//! and renders what comes back (`XI03`).

use envoix_bindings::bridge::{CreateIntent, CreateSpec};
use envoix_bindings::command::CreateRefusalView;
use envoix_deployment::BUILD_TARGET;
use envoix_invite::{
    EntropyError, EntropySource, Invite, InviteError, RecognizedInvalid, Role, generate_room_code,
    route_invite,
};
use envoix_product::{NewTransfer, PairingChannel, SourceDecision};
use envoix_types::{ByteCount, Direction, OfferedName};

/// Longer than the invite grammar's own input bound, so the refusal for an
/// over-long paste is the GRAMMAR's rather than the encoder's. The command
/// contract admits twice this, which is what makes that reachable.
#[cfg(test)]
const OVER_LONG_INVITE: usize = envoix_invite::MAX_INVITE_INPUT_LENGTH + 1;

/// The platform entropy the room code is drawn from.
struct SystemEntropy;

impl EntropySource for SystemEntropy {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), EntropyError> {
        getrandom::fill(destination).map_err(|_| EntropyError::Unavailable)
    }
}

/// The transfer a create intent asks for, or why the authority will not make
/// one.
pub fn plan(spec: &CreateSpec) -> Result<NewTransfer, CreateRefusalView> {
    match &spec.intent {
        CreateIntent::MintRoom { local_direction } => plan_mint(*local_direction),
        CreateIntent::JoinRoom { invite } => plan_join(invite),
    }
}

/// Mints a room this endpoint will be on `direction` of.
///
/// BOTH directions are mintable, which is the whole point of the 2x2: a
/// receiver showing its own code and waiting for a sender is an ordinary thing
/// to want, and it was unreachable while `send` meant "mint" and `join` meant
/// "receive".
///
/// No document is named. A sender acquires one afterwards under an identity
/// this create mints, so the card is born with no offered name and no total —
/// the same shape a joined receiver has always had.
fn plan_mint(direction: Direction) -> Result<NewTransfer, CreateRefusalView> {
    let code = generate_room_code(&mut SystemEntropy).map_err(refusal)?;
    // The invite declares OUR role, so whoever joins takes the opposite one.
    //
    // The broker and the relay are THIS BUILD'S deployment, resolved by
    // `envoix-deployment`'s build script out of `deploy/environments.toml` and
    // spelled by that file's own derivation templates. There is no constant
    // here to drift from the catalogue, and no way to compile an app for an
    // environment the catalogue will not deploy — which is why the endpoint
    // that gets frozen into every durable record this build writes is safe to
    // be a real one, where until D1 it had to be `.invalid`.
    // The invite declares OUR role, so whoever joins takes the opposite one.
    let invite = Invite::new(
        code.as_str(),
        BUILD_TARGET.rendezvous_endpoint.as_ref(),
        BUILD_TARGET.relay_url.as_ref(),
        match direction {
            Direction::Send => Role::Send,
            Direction::Receive => Role::Receive,
        },
    )
    .map_err(refusal)?;
    Ok(NewTransfer {
        direction,
        // Unknown until a source is acquired and staged (sender) or the peer
        // states it (receiver). The authority's own fallback rather than an
        // invented name — which is what a joined card has always carried.
        offered_name: OfferedName::from_untrusted("").expect("the fallback name is bounded"),
        total: ByteCount::new(0),
        source: match direction {
            // A minting sender needs source work, and it is not recoverable:
            // this build holds a granted source only in process memory, so a
            // process death really does mean re-picking.
            Direction::Send => SourceDecision::Stage { recoverable: false },
            Direction::Receive => SourceDecision::Ready,
        },
        pairing: Some(Box::new(PairingChannel::from_invite(&invite))),
    })
}

fn plan_join(text: &str) -> Result<NewTransfer, CreateRefusalView> {
    let invite = route_invite(text).map_err(refusal)?;
    // The invite names its creator's role; the joiner takes the other one.
    // Both are now reachable. A joiner that takes the SENDING side acquires its
    // document afterwards, exactly as a minting sender does — which is what
    // made the old typed refusal here unnecessary rather than principled.
    let direction = match invite.role().opposite() {
        Role::Receive => Direction::Receive,
        Role::Send => Direction::Send,
    };
    Ok(NewTransfer {
        direction,
        // The offered name is the sender's to state; until the peer does, the
        // record carries the authority's own fallback rather than an invented
        // name.
        offered_name: OfferedName::from_untrusted("").expect("the fallback name is bounded"),
        total: ByteCount::new(0),
        source: match direction {
            Direction::Send => SourceDecision::Stage { recoverable: false },
            Direction::Receive => SourceDecision::Ready,
        },
        pairing: Some(Box::new(PairingChannel::from_invite(&invite))),
    })
}

/// The invite grammar's answer, in the vocabulary the contract publishes.
fn refusal(error: InviteError) -> CreateRefusalView {
    match error {
        InviteError::NotEnvoixInvite => CreateRefusalView::InviteNotRecognized,
        InviteError::RecognizedInvalid(RecognizedInvalid::BareRoomCode) => {
            CreateRefusalView::InviteBareRoomCode
        }
        InviteError::RecognizedInvalid(
            RecognizedInvalid::UnsupportedPayloadVersion { .. }
            | RecognizedInvalid::LegacyPairDeepLink
            | RecognizedInvalid::UnsupportedEnvoixDialect
            | RecognizedInvalid::NonCanonicalOuterForm,
        ) => CreateRefusalView::InviteUnsupported,
        InviteError::InputTooLong { .. }
        | InviteError::EncodedPayloadTooLong { .. }
        | InviteError::DecodedPayloadTooLong { .. } => CreateRefusalView::InviteTooLong,
        InviteError::MalformedBase64
        | InviteError::MalformedPayload
        | InviteError::InvalidField(_) => CreateRefusalView::InviteMalformed,
        InviteError::EntropyUnavailable
        | InviteError::UnusableEntropy
        | InviteError::EncodingFailed => CreateRefusalView::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite_text(role: Role) -> String {
        let invite = Invite::new(
            "000123-amber-brass",
            "broker.example",
            "relay.example",
            role,
        )
        .expect("a well-formed invite");
        envoix_invite::encode_deep_link(&invite).expect("the invite encodes")
    }

    fn join(text: &str) -> Result<NewTransfer, CreateRefusalView> {
        plan(&CreateSpec {
            request_id: envoix_types::CommandId::from_bytes([1; 16]),
            intent: CreateIntent::JoinRoom {
                invite: text.to_owned(),
            },
        })
    }

    /// The invite declares its CREATOR's role and the joiner takes the other
    /// one, so the direction is the invite's answer and never the frontend's.
    ///
    /// BOTH are now joinable. This test used to assert that a receive-invite
    /// was refused `InviteRoleUnsupported`, which froze a limitation as though
    /// it were policy: the refusal existed only because the old `join` intent
    /// carried no source, and a joining sender now acquires one afterwards
    /// exactly as a minting sender does.
    #[test]
    fn the_invite_chooses_the_joiners_side() {
        let sending = join(&invite_text(Role::Send)).expect("a send invite is joinable");
        assert_eq!(sending.direction, Direction::Receive);
        assert_eq!(sending.source, SourceDecision::Ready);

        let receiving = join(&invite_text(Role::Receive)).expect("a receive invite is joinable");
        assert_eq!(receiving.direction, Direction::Send);
        assert_eq!(
            receiving.source,
            SourceDecision::Stage { recoverable: false },
            "a joining sender still has to acquire a document"
        );
    }

    /// Every family the invite grammar can answer with reaches the frontend as
    /// its own refusal. The grammar owns the judgement; this owns the wording,
    /// and a family that collapsed into another would tell a user the wrong
    /// thing about text they can see.
    #[test]
    fn every_invite_verdict_crosses_as_its_own_refusal() {
        let cases = [
            ("", CreateRefusalView::InviteNotRecognized),
            (
                "not an invite at all",
                CreateRefusalView::InviteNotRecognized,
            ),
            ("000123", CreateRefusalView::InviteBareRoomCode),
            ("000123-amber-brass", CreateRefusalView::InviteBareRoomCode),
            ("envoix://pair/legacy", CreateRefusalView::InviteUnsupported),
            (
                "envoix://invite/v1/AAAA",
                CreateRefusalView::InviteUnsupported,
            ),
            ("ENVOIX:something", CreateRefusalView::InviteUnsupported),
            (
                "envoix:!!!not-base64!!!",
                CreateRefusalView::InviteMalformed,
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(join(text), Err(expected), "{text:?}");
        }
        // Over-long input is the grammar's own bound, not the encoder's: the
        // contract admits twice what the grammar does, so this refusal is
        // reachable rather than decorative.
        let long = "e".repeat(OVER_LONG_INVITE);
        assert_eq!(join(&long), Err(CreateRefusalView::InviteTooLong));

        // Distinct wording is only worth having if the mapping is injective.
        let refusals: std::collections::BTreeSet<_> = cases
            .iter()
            .map(|(_, refusal)| format!("{refusal:?}"))
            .collect();
        assert_eq!(refusals.len(), 4, "the sweep covers four distinct families");
    }

    /// A send is created needing source work, which is what raises the duty,
    /// and its channel is minted fresh every time — two sends must never share
    /// a room code.
    #[test]
    fn a_send_mints_its_own_channel_and_needs_a_source() {
        let plan_send = || {
            plan(&CreateSpec {
                request_id: envoix_types::CommandId::from_bytes([2; 16]),
                intent: CreateIntent::MintRoom {
                    local_direction: Direction::Send,
                },
            })
            .expect("a send always plans")
        };
        let first = plan_send();
        assert_eq!(first.direction, Direction::Send);
        assert_eq!(first.source, SourceDecision::Stage { recoverable: false });
        // Born nameless: the name arrives with the source, not with the card.
        assert_eq!(first.offered_name.as_str(), "unnamed");
        let channel = first.pairing.expect("a send publishes a channel");
        assert_eq!(channel.role(), Role::Send);

        let second = plan_send();
        assert_ne!(
            second.pairing.expect("a channel").code(),
            channel.code(),
            "two sends shared a room code"
        );
    }

    /// Every card this build mints is frozen to the deployment this build is
    /// FOR — the endpoints the catalogue derives, not a constant beside them.
    ///
    /// This is the durable half of the deployment identity: the manifest says
    /// which environment the artifact is for, and this is where that answer
    /// becomes a value written into records that outlive the process.
    #[test]
    fn a_card_is_frozen_to_the_deployment_this_build_is_for() {
        let planned = plan(&CreateSpec {
            request_id: envoix_types::CommandId::from_bytes([5; 16]),
            intent: CreateIntent::MintRoom {
                local_direction: Direction::Send,
            },
        })
        .expect("a send always plans");
        let channel = planned.pairing.expect("a send publishes a channel");
        let invite = channel.invite().expect("the stored fields spell an invite");
        assert_eq!(invite.broker(), BUILD_TARGET.rendezvous_endpoint.as_ref());
        assert_eq!(invite.relay(), BUILD_TARGET.relay_url.as_ref());

        // Not a tautology about a placeholder: the endpoint is the catalogue's
        // own derivation for a deployable environment, so it names a real
        // rendezvous with a real key rather than something `.invalid`.
        let catalogue =
            envoix_deployment::DeploymentCatalogue::compiled().expect("the catalogue parses");
        assert_eq!(
            catalogue
                .identity(BUILD_TARGET.environment.as_ref())
                .expect("a build exists, so its environment is deployable"),
            BUILD_TARGET,
        );
    }

    /// Both mint directions produce a card, and a minting RECEIVER never asks
    /// for a document. This is the half of the 2x2 that was previously
    /// unreachable — a receiver could not mint a room at all.
    #[test]
    fn a_receiver_can_mint_a_room_and_needs_no_source() {
        let planned = plan(&CreateSpec {
            request_id: envoix_types::CommandId::from_bytes([3; 16]),
            intent: CreateIntent::MintRoom {
                local_direction: Direction::Receive,
            },
        })
        .expect("a receiving mint plans");
        assert_eq!(planned.direction, Direction::Receive);
        assert_eq!(planned.source, SourceDecision::Ready);
        // The invite declares OUR role, so a joiner takes the sending side.
        let channel = planned.pairing.expect("a mint publishes a channel");
        assert_eq!(channel.role(), Role::Receive);
    }
}
