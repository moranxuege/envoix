//! Thin command-line frontend over the generated Envoix contracts.
//!
//! This crate deliberately has no product or runtime dependency. Its input is
//! the generated read/command lane, its output is a generated intent frame,
//! and dropping [`Frontend`] drops only a transient attachment projection.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use envoix_bindings::command::{
    CommandBody, CommandError, CommandFrame, CommandView, CreateIntentView, CreateView,
    FrontendIntentView, JoinInviteView, LocalDirectionView, MintRoomView, SubmitView,
    decode_command_frame, encode_command_frame,
};
use envoix_bindings::read::{
    CardUpdateKindView, CardView, CommandKindView, EpochGate, GateDecision, LosslessKindView,
    ReadBody, ReadError, ReadFrame, decode_read_frame,
};
use envoix_types::Secret;

/// Why an inbound lane frame was not admitted by this attachment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrontendError {
    ReadContract(ReadError),
    CommandContract(CommandError),
    StaleEpoch,
    ContractBreach,
}

impl std::fmt::Display for FrontendError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReadContract(error) => write!(formatter, "read contract error: {error:?}"),
            Self::CommandContract(error) => {
                write!(formatter, "command contract error: {error:?}")
            }
            Self::StaleEpoch => formatter.write_str("the frame belongs to a stale attachment"),
            Self::ContractBreach => formatter.write_str("the lane broke the generated contract"),
        }
    }
}

impl std::error::Error for FrontendError {}

/// Why a requested frontend intent could not be encoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntentError {
    UnknownCard,
    NotOffered,
    Contract(CommandError),
}

impl std::fmt::Display for IntentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCard => formatter.write_str("the attachment has no such card"),
            Self::NotOffered => formatter.write_str("the authority did not offer that action"),
            Self::Contract(error) => write!(formatter, "command contract error: {error:?}"),
        }
    }
}

impl std::error::Error for IntentError {}

/// The last generated stream state published for a card.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamStatus {
    Live,
    Lagged(LosslessKindView),
    Closed,
}

/// One card as this attachment last observed it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardRow {
    pub epoch: u64,
    pub view: CardView,
    pub stream: StreamStatus,
}

/// A decoded frame from the multiplexed frontend lane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ingested {
    Read(Box<ReadFrame>),
    Command(CommandFrame),
}

/// One detachable frontend attachment.
///
/// The map is a projection of generated read frames, not durable transfer
/// state. A new process creates a new value and is re-seeded by the authority.
#[derive(Debug, Default)]
pub struct Frontend {
    gates: BTreeMap<String, EpochGate>,
    cards: BTreeMap<String, CardRow>,
}

impl Frontend {
    /// Decodes one multiplexed lane frame through the generated codecs.
    pub fn ingest(&mut self, bytes: &[u8]) -> Result<Ingested, FrontendError> {
        match decode_read_frame(bytes) {
            Ok(frame) => {
                self.admit_read(&frame)?;
                Ok(Ingested::Read(Box::new(frame)))
            }
            Err(ReadError::UnknownSchema) => {
                let frame = decode_command_frame(bytes).map_err(FrontendError::CommandContract)?;
                if matches!(frame.body, CommandBody::Intent(_)) {
                    return Err(FrontendError::ContractBreach);
                }
                Ok(Ingested::Command(frame))
            }
            Err(error) => Err(FrontendError::ReadContract(error)),
        }
    }

    /// Cards in authority-id order. This does not invent a creation order the
    /// read contract does not publish.
    pub fn cards(&self) -> impl Iterator<Item = (&str, &CardRow)> {
        self.cards.iter().map(|(card, row)| (card.as_str(), row))
    }

    /// The most recent projection for one authority-minted card id.
    pub fn card(&self, card: &str) -> Option<&CardRow> {
        self.cards.get(card)
    }

    /// Encodes a command only when the card's generated view offers it.
    pub fn command_frame(
        &self,
        card: &str,
        command_id: String,
        command: CommandKindView,
    ) -> Result<Vec<u8>, IntentError> {
        let row = self.cards.get(card).ok_or(IntentError::UnknownCard)?;
        if !row.view.allowed_actions.contains(&command) {
            return Err(IntentError::NotOffered);
        }
        encode_command_frame(&CommandFrame {
            body: CommandBody::Intent(FrontendIntentView::Command(SubmitView {
                card: card.to_owned(),
                epoch: row.epoch,
                command_id,
                command: command_view(command),
            })),
        })
        .map_err(IntentError::Contract)
    }

    fn admit_read(&mut self, frame: &ReadFrame) -> Result<(), FrontendError> {
        match &frame.body {
            ReadBody::CardUpdate(update) => {
                if !self.gates.contains_key(&update.card) {
                    if !matches!(update.kind, CardUpdateKindView::Snapshot(_)) {
                        return Err(FrontendError::ContractBreach);
                    }
                    self.gates
                        .insert(update.card.clone(), EpochGate::attach(update.epoch));
                }
                let decision = self
                    .gates
                    .get_mut(&update.card)
                    .expect("the gate was inserted above")
                    .admit(frame);
                decide(decision)?;
                let view = match &update.kind {
                    CardUpdateKindView::Snapshot(view)
                    | CardUpdateKindView::Progress(view)
                    | CardUpdateKindView::State(view)
                    | CardUpdateKindView::Terminal(view) => Some(view.clone()),
                    CardUpdateKindView::CapabilityDuty(_) => None,
                };
                if let Some(view) = view {
                    self.cards.insert(
                        update.card.clone(),
                        CardRow {
                            epoch: update.epoch,
                            view,
                            stream: StreamStatus::Live,
                        },
                    );
                }
                Ok(())
            }
            ReadBody::Lag(lag) => {
                let gate = self
                    .gates
                    .get_mut(&lag.card)
                    .ok_or(FrontendError::StaleEpoch)?;
                decide(gate.admit(frame))?;
                if let Some(row) = self.cards.get_mut(&lag.card) {
                    row.stream = StreamStatus::Lagged(lag.missed);
                }
                Ok(())
            }
            ReadBody::Closed(closed) => {
                let gate = self
                    .gates
                    .get_mut(&closed.card)
                    .ok_or(FrontendError::StaleEpoch)?;
                decide(gate.admit(frame))?;
                if let Some(row) = self.cards.get_mut(&closed.card) {
                    row.stream = StreamStatus::Closed;
                }
                Ok(())
            }
            ReadBody::SubscribeRejected(_) | ReadBody::Evidence(_) | ReadBody::BuildManifest(_) => {
                Ok(())
            }
        }
    }
}

/// Encodes a request to mint a room and be on `local_direction` of it.
///
/// It carries no document: a source is acquired after the card exists, so this
/// is the same frame whichever side the caller will be on.
pub fn create_mint_frame(
    request_id: String,
    local_direction: LocalDirectionView,
) -> Result<Vec<u8>, IntentError> {
    create_frame(
        request_id,
        CreateIntentView::MintRoom(MintRoomView { local_direction }),
    )
}

/// Encodes a request carrying invite text unchanged and unexamined.
pub fn create_join_frame(request_id: String, invite: String) -> Result<Vec<u8>, IntentError> {
    create_frame(
        request_id,
        CreateIntentView::JoinRoom(JoinInviteView {
            invite: Secret::new(invite),
        }),
    )
}

fn create_frame(request_id: String, intent: CreateIntentView) -> Result<Vec<u8>, IntentError> {
    encode_command_frame(&CommandFrame {
        body: CommandBody::Intent(FrontendIntentView::Create(CreateView {
            intent,
            request_id,
        })),
    })
    .map_err(IntentError::Contract)
}

fn decide(decision: GateDecision) -> Result<(), FrontendError> {
    match decision {
        GateDecision::Deliver => Ok(()),
        GateDecision::DropStale => Err(FrontendError::StaleEpoch),
        GateDecision::ContractBreach => Err(FrontendError::ContractBreach),
    }
}

/// The exhaustive correspondence between the read offer and command intent.
///
/// This is not a legality table: `command_frame` consults the authority's
/// `allowed_actions` before reaching it.
pub const fn command_view(command: CommandKindView) -> CommandView {
    match command {
        CommandKindView::Pause => CommandView::Pause,
        CommandKindView::Cancel => CommandView::Cancel,
        CommandKindView::Resume => CommandView::Resume,
        CommandKindView::Remove => CommandView::Remove,
        CommandKindView::RePickSource => CommandView::RePickSource,
    }
}

/// Human-facing text for one already-decoded event.
///
/// Secrets are intentionally omitted. Labels are presentation; all values and
/// every offered action remain the authority's generated vocabulary.
pub fn render(event: &Ingested) -> String {
    match event {
        Ingested::Read(frame) => match &frame.body {
            ReadBody::CardUpdate(update) => match &update.kind {
                CardUpdateKindView::Snapshot(view)
                | CardUpdateKindView::Progress(view)
                | CardUpdateKindView::State(view)
                | CardUpdateKindView::Terminal(view) => format!(
                    "card={} epoch={} direction={:?} name={:?} total={} state={:?} \
                     quiescence={:?} bytes={} allowed={:?}",
                    update.card,
                    update.epoch,
                    view.direction,
                    view.offered_name,
                    view.total,
                    view.state,
                    view.quiescence,
                    view.bytes,
                    view.allowed_actions
                ),
                CardUpdateKindView::CapabilityDuty(duty) => format!(
                    "card={} epoch={} duty={:?} action={:?}",
                    update.card, update.epoch, duty.duty.kind, duty.action
                ),
            },
            ReadBody::Lag(lag) => {
                format!(
                    "card={} epoch={} lagged={:?}",
                    lag.card, lag.epoch, lag.missed
                )
            }
            ReadBody::Closed(closed) => {
                format!("card={} epoch={} closed", closed.card, closed.epoch)
            }
            ReadBody::SubscribeRejected(refused) => {
                format!(
                    "card={} subscribe_refused={:?}",
                    refused.card, refused.reason
                )
            }
            ReadBody::Evidence(evidence) => format!(
                "card={} generation={} evidence={:?} entries={}",
                evidence.session.card,
                evidence.session.generation,
                evidence.status,
                evidence.entries.len()
            ),
            ReadBody::BuildManifest(manifest) => format!(
                "build={} protocol={} read={} command={} capability={}",
                manifest.package_version,
                manifest.protocol.set_id,
                manifest.abi_schema.read_binding_schema_id,
                manifest.abi_schema.command_binding_schema_id,
                manifest.abi_schema.capability_binding_schema_id
            ),
        },
        Ingested::Command(frame) => match &frame.body {
            CommandBody::Acceptance(answer) => format!(
                "command={} acceptance={:?}",
                answer.command_id, answer.acceptance
            ),
            CommandBody::Completion(answer) => format!(
                "command={} completion={:?}",
                answer.command_id, answer.completion
            ),
            CommandBody::CreateResult(answer) => {
                format!("request={} create={:?}", answer.request_id, answer.outcome)
            }
            CommandBody::Intent(_) => "contract breach: inbound intent".to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use envoix_bindings::command::{
        CommandBody, CommandFrame, CreateOutcomeView, CreateRefusalView, CreateResultView,
        decode_command_frame, encode_command_frame,
    };
    use envoix_bindings::read::{
        CardUpdateKindView, CardUpdateView, CardView, CommandKindView, DirectionView, IdentityView,
        PhaseView, ProductStateView, QuiescenceView, ReadBody, ReadFrame, encode_read_frame,
    };

    use super::{
        Frontend, FrontendError, Ingested, IntentError, LocalDirectionView, create_join_frame,
        create_mint_frame, render,
    };

    fn view(allowed_actions: Vec<CommandKindView>) -> CardView {
        CardView {
            identity: IdentityView {
                card: "0000000000000001".to_owned(),
                transfer: "00000000000000000000000000000002".to_owned(),
                artifact: "00000000000000000000000000000003".to_owned(),
            },
            direction: DirectionView::Send,
            offered_name: "contract.bin".to_owned(),
            total: 4096,
            state: ProductStateView::Preparing,
            quiescence: QuiescenceView::Quiescent,
            generation: 0,
            phase: PhaseView::Preparing,
            bytes: 0,
            bytes_resumed: 0,
            outcome: None,
            allowed_actions,
            invite: None,
        }
    }

    fn update(epoch: u64, kind: CardUpdateKindView) -> Vec<u8> {
        encode_read_frame(&ReadFrame {
            body: ReadBody::CardUpdate(CardUpdateView {
                epoch,
                card: "0000000000000001".to_owned(),
                kind,
            }),
        })
        .expect("the generated read frame encodes")
    }

    #[test]
    fn commands_are_generated_from_the_authoritys_offer() {
        let mut frontend = Frontend::default();
        frontend
            .ingest(&update(
                7,
                CardUpdateKindView::Snapshot(view(vec![CommandKindView::Pause])),
            ))
            .expect("the generated snapshot is admitted");
        let frame = frontend
            .command_frame(
                "0000000000000001",
                "0123456789abcdeffedcba9876543210".to_owned(),
                CommandKindView::Pause,
            )
            .expect("the offered command encodes");
        assert!(matches!(
            decode_command_frame(&frame)
                .expect("the generated decoder accepts it")
                .body,
            CommandBody::Intent(_)
        ));
        assert_eq!(
            frontend.command_frame(
                "0000000000000001",
                "0123456789abcdeffedcba9876543211".to_owned(),
                CommandKindView::Resume,
            ),
            Err(IntentError::NotOffered)
        );
    }

    #[test]
    fn a_new_frontend_has_no_truth_until_the_authority_reseeds_it() {
        let mut attached = Frontend::default();
        let snapshot = update(
            7,
            CardUpdateKindView::Snapshot(view(vec![CommandKindView::Pause])),
        );
        attached
            .ingest(&snapshot)
            .expect("the snapshot is admitted");
        assert_eq!(attached.cards().count(), 1);

        drop(attached);
        let mut reattached = Frontend::default();
        assert_eq!(reattached.cards().count(), 0);
        assert_eq!(
            reattached.ingest(&update(
                8,
                CardUpdateKindView::Snapshot(view(vec![CommandKindView::Pause])),
            )),
            Ok(Ingested::Read(Box::new(
                envoix_bindings::read::decode_read_frame(&update(
                    8,
                    CardUpdateKindView::Snapshot(view(vec![CommandKindView::Pause])),
                ))
                .expect("the generated frame decodes")
            )))
        );
    }

    #[test]
    fn stale_and_directionally_wrong_frames_are_refused() {
        let mut frontend = Frontend::default();
        assert_eq!(
            frontend.ingest(&update(1, CardUpdateKindView::Progress(view(Vec::new())))),
            Err(FrontendError::ContractBreach)
        );
        let inbound = create_mint_frame(
            "0123456789abcdeffedcba9876543210".to_owned(),
            LocalDirectionView::Send,
        )
        .expect("the create encodes");
        assert_eq!(
            frontend.ingest(&inbound),
            Err(FrontendError::ContractBreach)
        );
    }

    #[test]
    fn create_inputs_cross_unchanged_through_the_generated_contract() {
        let send = create_mint_frame(
            "0123456789abcdeffedcba9876543210".to_owned(),
            LocalDirectionView::Send,
        )
        .expect("the bounded name encodes");
        assert!(matches!(
            decode_command_frame(&send).expect("the send decodes").body,
            CommandBody::Intent(_)
        ));
        let invite = "  opaque invite text  \n";
        let join = create_join_frame(
            "0123456789abcdeffedcba9876543211".to_owned(),
            invite.to_owned(),
        )
        .expect("the opaque invite encodes");
        let decoded = decode_command_frame(&join).expect("the join decodes");
        let CommandBody::Intent(envoix_bindings::command::FrontendIntentView::Create(create)) =
            decoded.body
        else {
            panic!("the generated body is a create");
        };
        let envoix_bindings::command::CreateIntentView::JoinRoom(join) = create.intent else {
            panic!("the generated intent is a join");
        };
        assert_eq!(join.invite.expose(), invite);
    }

    #[test]
    fn rendering_omits_invite_secrets() {
        let answer = encode_command_frame(&CommandFrame {
            body: CommandBody::CreateResult(CreateResultView {
                outcome: CreateOutcomeView::Refused(CreateRefusalView::InviteMalformed),
                request_id: "0123456789abcdeffedcba9876543210".to_owned(),
            }),
        })
        .expect("the result encodes");
        let mut frontend = Frontend::default();
        let event = frontend.ingest(&answer).expect("the result is admitted");
        assert!(render(&event).contains("InviteMalformed"));
    }

    #[test]
    fn production_dependencies_stop_at_the_generated_and_platform_contracts() {
        let manifest = include_str!("../Cargo.toml");
        let dependencies = manifest
            .split_once("[dependencies]")
            .expect("the CLI declares production dependencies")
            .1
            .split_once("[dev-dependencies]")
            .expect("the CLI keeps test composition separate")
            .0;
        let names: Vec<&str> = dependencies
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name.trim()))
            .filter(|name| !name.is_empty())
            .collect();
        assert_eq!(
            names,
            ["envoix-bindings", "envoix-platform-local", "envoix-types"],
            "a frontend dependency must be justified at the contract boundary"
        );
    }
}
