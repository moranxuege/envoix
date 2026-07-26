//! Turning a frontend create intent into a [`NewTransfer`] the authority will
//! admit — or into the authority's own typed refusal.
//!
//! Every judgement about an invite is made here by `envoix-invite` (C4), never
//! by the frontend: the grammar, the bounds, the version, the dialect and the
//! declared role are all its answers (`XI02`). The frontend carries opaque text
//! and renders what comes back (`XI03`).

use envoix_bindings::bridge::{CreateIntent, CreateSpec};
use envoix_bindings::command::CreateRefusalView;
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

/// The rendezvous broker a card minted by this build is frozen to.
///
/// A deliberately unroutable placeholder: F2b creates cards and publishes
/// shareable invites, and D1 is the step that points them at a deployment.
/// Naming a real endpoint here would freeze it into every durable record this
/// build writes, and `.invalid` can never be mistaken for one that works.
const DEFAULT_BROKER: &str = "rendezvous.envoix.invalid";
/// The relay a card minted by this build is frozen to. Same reasoning.
const DEFAULT_RELAY: &str = "relay.envoix.invalid";

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
        CreateIntent::Send {
            display_name,
            total,
        } => plan_send(display_name, *total),
        CreateIntent::Join { invite } => plan_join(invite),
    }
}

fn plan_send(display_name: &str, total: u64) -> Result<NewTransfer, CreateRefusalView> {
    let offered_name =
        OfferedName::from_untrusted(display_name).map_err(|_| CreateRefusalView::NameTooLong)?;
    let code = generate_room_code(&mut SystemEntropy).map_err(refusal)?;
    // The invite declares OUR role, so whoever joins takes the opposite one.
    let invite =
        Invite::new(code.as_str(), DEFAULT_BROKER, DEFAULT_RELAY, Role::Send).map_err(refusal)?;
    Ok(NewTransfer {
        direction: Direction::Send,
        // Untrusted provider metadata, sanitized by the authority rather than
        // trusted as the frontend spelled it (`SF09`).
        offered_name,
        total: ByteCount::new(total),
        // The document is picked but not yet staged, so the card is created
        // needing source work — which is what mints the `SourceHandle` duty.
        // It is not recoverable: this build holds the granted source only in
        // process memory, so a process death really does mean re-picking, and
        // saying otherwise would be a promise the platform cannot keep.
        source: SourceDecision::Stage { recoverable: false },
        pairing: Some(Box::new(PairingChannel::from_invite(&invite))),
    })
}

fn plan_join(text: &str) -> Result<NewTransfer, CreateRefusalView> {
    let invite = route_invite(text).map_err(refusal)?;
    // The invite names its creator's role; the joiner takes the other one.
    let direction = match invite.role().opposite() {
        Role::Receive => Direction::Receive,
        // An invite asking us to SEND needs a source this intent does not
        // carry. Refusing it typed is the authority answering; guessing a
        // direction would be the frontend owning the decision (`SF06`).
        Role::Send => return Err(CreateRefusalView::InviteRoleUnsupported),
    };
    Ok(NewTransfer {
        direction,
        // The offered name is the sender's to state; until the peer does, the
        // record carries the authority's own fallback rather than an invented
        // name.
        offered_name: OfferedName::from_untrusted("").expect("the fallback name is bounded"),
        total: ByteCount::new(0),
        source: SourceDecision::Ready,
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
            intent: CreateIntent::Join {
                invite: text.to_owned(),
            },
        })
    }

    /// The invite declares its CREATOR's role and the joiner takes the other
    /// one, so the direction is the invite's answer and never the frontend's.
    /// An invite asking us to send carries no source, so it is refused typed
    /// rather than turned into a card that can never do anything.
    #[test]
    fn the_invite_chooses_the_joiners_side() {
        let sending = join(&invite_text(Role::Send)).expect("a send invite is joinable");
        assert_eq!(sending.direction, Direction::Receive);
        assert_eq!(sending.source, SourceDecision::Ready);

        assert_eq!(
            join(&invite_text(Role::Receive)),
            Err(CreateRefusalView::InviteRoleUnsupported)
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
                intent: CreateIntent::Send {
                    display_name: "report.pdf".to_owned(),
                    total: 4096,
                },
            })
            .expect("a send always plans")
        };
        let first = plan_send();
        assert_eq!(first.direction, Direction::Send);
        assert_eq!(first.source, SourceDecision::Stage { recoverable: false });
        assert_eq!(first.offered_name.as_str(), "report.pdf");
        let channel = first.pairing.expect("a send publishes a channel");
        assert_eq!(channel.role(), Role::Send);

        let second = plan_send();
        assert_ne!(
            second.pairing.expect("a channel").code(),
            channel.code(),
            "two sends shared a room code"
        );
    }

    /// The provider name is untrusted metadata, sanitized by the authority
    /// rather than trusted as the frontend spelled it (`SF09`).
    #[test]
    fn a_provider_name_is_sanitized_not_trusted() {
        let planned = plan(&CreateSpec {
            request_id: envoix_types::CommandId::from_bytes([3; 16]),
            intent: CreateIntent::Send {
                display_name: "../../etc/passwd".to_owned(),
                total: 1,
            },
        })
        .expect("a send always plans");
        assert_eq!(planned.offered_name.as_str(), "passwd");
    }

    /// UTF-8 bytes, not user-perceived characters, are the portable leaf-name
    /// limit. The wider command contract deliberately lets the ordinary CJK
    /// filename reach this authority boundary so the answer is typed rather
    /// than a frontend codec exception.
    #[test]
    fn an_over_byte_cjk_name_is_refused_before_a_room_is_minted() {
        let cjk = "界".repeat(86);
        assert_eq!(cjk.len(), 258);
        assert_eq!(
            plan(&CreateSpec {
                request_id: envoix_types::CommandId::from_bytes([4; 16]),
                intent: CreateIntent::Send {
                    display_name: cjk,
                    total: 1,
                },
            }),
            Err(CreateRefusalView::NameTooLong)
        );
    }
}
