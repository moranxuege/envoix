/// The command half of the frontend: one user intent, one identity, and
/// whatever the authority has said about it so far.
///
/// The frontend owns no transfer truth (R0). It does not decide what a command
/// did, does not retry one on a timer, and keeps nothing durable: this journal
/// lives inside one [Attachment] and dies with it, because the completions it
/// is waiting for were addressed to that attachment and are discarded when the
/// next one opens. What survives is the card's own committed truth, which
/// arrives as a snapshot on the fresh attachment.
///
/// Nothing here writes JSON. `encodeCommandFrame` is the generated encoder for
/// the ONE body a frontend may originate; acceptance and completion have no
/// encoder in any native artifact, so this file could not fabricate one.
library;

import 'dart:convert';
import 'dart:math';

import 'bindings/envoix_command.dart';
import 'bindings/envoix_read.dart';

/// The command a published affordance sends.
///
/// The read contract offers a *kind*; the command contract is what a frontend
/// may originate. They are two schemas because the generator has no cross-schema
/// reference, and one vocabulary because
/// `the_read_contract_publishes_every_command_a_frontend_can_send` says so.
/// This exhaustive switch is the only place they meet.
CommandView commandOf(CommandKindView kind) => switch (kind) {
      CommandKindView.pause => CommandView.pause,
      CommandKindView.cancel => CommandView.cancel,
      CommandKindView.resume => CommandView.resume,
      CommandKindView.remove => CommandView.remove,
      CommandKindView.rePickSource => CommandView.rePickSource,
    };

/// Where an intent's account of itself ran out.
///
/// These are OPPOSITE facts and the frontend must never print one as the other:
/// "if it appears in the list, it was created" is honest about a request that
/// left this process and false about one that never did. The lane already draws
/// the line — the encoder enforces every bound its decoder checks, so a request
/// that does not encode is not sent — and this is what carries that distinction
/// past the function that drew it.
enum FaultOrigin {
  /// The encoder refused it, so no frame left this process. Nothing was asked
  /// of the authority, so there is nothing anywhere to go looking for.
  unsent,

  /// It was sent and no verdict came back. This says nothing about whether the
  /// command applied; only the card's own truth does.
  unanswered,
}

/// Why an intent has no verdict. Not a verdict itself.
class IntentFault {
  const IntentFault(this.origin, this.error);

  final FaultOrigin origin;

  /// What refused or interrupted it, in its own words.
  final Object error;

  @override
  String toString() => '$error';
}

/// How far one intent has got. Derived from what the authority has said, never
/// stored, so it cannot disagree with the answers it is derived from.
enum CommandPhase {
  /// Submitted; nothing has answered yet.
  submitted,

  /// Accepted, and NOT committed. Acceptance is not completion (BN2): the
  /// effect has not crossed the durability barrier and may still fail.
  accepted,

  /// A terminal answer exists — a verdict at intake or a completion.
  settled,

  /// The encoder refused it, so nothing was sent. Not "undelivered": there was
  /// no delivery to fail, and nothing on the authority's side to reconcile.
  refused,

  /// The lane failed, so there is no verdict at all. This says nothing about
  /// whether the command applied; only the card's own truth does.
  undelivered,
}

/// One user intent: a minted identity, the command it carries, and every answer
/// this attachment has heard about it.
class CommandIntent {
  CommandIntent({
    required this.id,
    required this.card,
    required this.command,
  });

  /// The caller-minted 128-bit identity, 32 lowercase hex digits. It is the
  /// host's dedup key, and it is never written down anywhere durable.
  final String id;
  final String card;
  final CommandView command;

  /// How many times this identity has been submitted. The second and later
  /// submissions are the documented `Interrupted` disambiguation: the same
  /// identity re-presented, whose answer decides what the first one did.
  int attempts = 0;

  AcceptanceView? acceptance;
  CompletionView? completion;

  /// Why the frame never produced a verdict. Not a verdict itself.
  IntentFault? fault;

  CommandPhase get phase {
    final IntentFault? fault = this.fault;
    if (fault != null) {
      return switch (fault.origin) {
        FaultOrigin.unsent => CommandPhase.refused,
        FaultOrigin.unanswered => CommandPhase.undelivered,
      };
    }
    if (completion != null) {
      return CommandPhase.settled;
    }
    return switch (acceptance) {
      null => CommandPhase.submitted,
      AcceptanceViewAccepted() => CommandPhase.accepted,
      AcceptanceViewConflict() ||
      AcceptanceViewDuplicate() ||
      AcceptanceViewRejected() =>
        CommandPhase.settled,
    };
  }

  /// Whether the user may still be waiting for this one. A settled intent has
  /// its answer; an undelivered one has none and never will.
  bool get unsettled =>
      phase == CommandPhase.submitted || phase == CommandPhase.accepted;

  /// Whether the authority left this intent's fate UNKNOWN, so re-presenting
  /// the same identity is the documented way to find out (BN2): a `Duplicate`
  /// proves it had committed, a fresh acceptance proves it had not.
  ///
  /// `Interrupted` arrives on either arm — the actor can die before it answers
  /// the acceptance as well as after it — and neither arm may be guessed at.
  bool get mayDisambiguate => switch ((acceptance, completion)) {
        (_, CompletionViewInterrupted()) => true,
        (AcceptanceViewRejected(value: RejectionView.interrupted), _) => true,
        _ => false,
      };
}

/// What the journal did with a command frame off the lane.
enum CommandAdmission {
  /// It answered an intent this attachment holds.
  answered,

  /// It answered an identity this attachment never issued. The completion of a
  /// command submitted by the attachment this one replaced can still arrive
  /// here — the host resolves it whenever the barrier does. Nothing is invented
  /// for it: an intent has a user behind it.
  unaddressed,

  /// A `submit` body arriving AT a frontend. Only a frontend originates one, so
  /// this is the contract's own direction being broken.
  notAnAnswer,
}

/// Every intent this attachment issued, and what came back.
class CommandJournal {
  final Map<String, CommandIntent> _intents = <String, CommandIntent>{};

  /// Answers addressed to an identity this attachment never issued.
  int unaddressed = 0;

  /// Newest first, so the answer a user is waiting for is the one they see.
  List<CommandIntent> forCard(String card) => <CommandIntent>[
        for (final CommandIntent intent in _intents.values.toList().reversed)
          if (intent.card == card) intent,
      ];

  /// Opens an intent. Passing `id` re-presents an identity already issued —
  /// the `Interrupted` disambiguation — which clears the answers it is asking
  /// again about, because the new answer is the answer.
  CommandIntent open(String card, CommandView command, {String? id}) {
    final CommandIntent intent = id == null
        ? CommandIntent(id: mintCommandId(), card: card, command: command)
        : _intents[id] ??
            CommandIntent(id: id, card: card, command: command);
    intent
      ..attempts += 1
      ..acceptance = null
      ..completion = null
      ..fault = null;
    _intents[intent.id] = intent;
    return intent;
  }

  /// No verdict exists for `id`, and none is invented. The origin travels with
  /// it because "never sent" and "sent, no answer" are different facts.
  void faulted(String id, IntentFault fault) {
    _intents[id]?.fault = fault;
  }

  CommandAdmission admit(CommandFrame frame) {
    switch (frame.body) {
      // Both a command submission and a create request travel as `intent`, and
      // neither is something a frontend may be SENT.
      case final CommandBodyIntent _:
        return CommandAdmission.notAnAnswer;
      // A create is answered on its own submission, not on this lane; one
      // arriving here belongs to no intent this journal holds, and is counted
      // exactly like any other answer nobody asked for.
      case final CommandBodyCreateResult _:
        unaddressed += 1;
        return CommandAdmission.unaddressed;
      case final CommandBodyAcceptance body:
        return _answer(
          body.value.commandId,
          (CommandIntent intent) => intent.acceptance = body.value.acceptance,
        );
      case final CommandBodyCompletion body:
        return _answer(
          body.value.commandId,
          (CommandIntent intent) => intent.completion = body.value.completion,
        );
    }
  }

  /// Routes one answer to the intent that OWNS its identity, or to nobody.
  ///
  /// Acceptances and completions are addressed the same way, so the rule is
  /// stated once: two copies of it could disagree, and the arm that drifted
  /// would attach an answer the authority never gave about that intent.
  CommandAdmission _answer(String id, void Function(CommandIntent) record) {
    final CommandIntent? intent = _intents[id];
    if (intent == null) {
      unaddressed += 1;
      return CommandAdmission.unaddressed;
    }
    record(intent);
    return CommandAdmission.answered;
  }
}

/// Whether `intents` still holds an unanswered intent for `command`.
///
/// This debounces the user's own finger — a second tap would be a second
/// command with a second identity, which the host would apply a second time.
/// It is NOT a legality judgement: legality is `CardView.allowedActions`, and
/// it is what put the affordance there. Deliberately per command, so Cancel
/// stays live while a Pause is in flight.
bool commandInFlight(List<CommandIntent> intents, CommandView command) =>
    intents.any((CommandIntent intent) =>
        intent.command == command && intent.unsettled);

/// A fresh command identity: 128 bits from the platform's secure source, as the
/// 32 lowercase hex digits the contract's `hex32` accepts.
///
/// One per user intent. It is used only for that intent's in-flight retries and
/// is never persisted — the host's ledger (`retryHorizonCompletions`
/// completions per card) is what makes a re-issue exactly-once, and that
/// horizon is the host's memory, not this app's.
String mintCommandId() {
  final Random entropy = Random.secure();
  final StringBuffer id = StringBuffer();
  for (int byte = 0; byte < 16; byte += 1) {
    id.write(entropy.nextInt(256).toRadixString(16).padLeft(2, '0'));
  }
  return id.toString();
}

/// A command on a card that exists, through the generated encoder.
///
/// `epoch` is the epoch that delivered the card's last update: only the newest
/// attachment commands (BN2), so a frame carrying a superseded epoch is refused
/// `stale_epoch` by the authority rather than applied.
List<int> submitFrame({
  required String card,
  required int epoch,
  required String id,
  required CommandView command,
}) =>
    _intentFrame(
      FrontendIntentViewCommand(
        SubmitView(
          card: card,
          epoch: epoch,
          commandId: id,
          command: command,
        ),
      ),
    );

/// A request that a card be created, through the same encoder and the same
/// lane: the two things a frontend may originate are one contract.
///
/// There is no epoch, because there is no card to be the newest attachment of
/// — creation is the request that makes one.
List<int> createFrame({required String id, required CreateIntentView intent}) =>
    _intentFrame(FrontendIntentViewCreate(
      CreateView(intent: intent, requestId: id),
    ));

List<int> _intentFrame(FrontendIntentView intent) =>
    utf8.encode(encodeCommandFrame(intent));

/// Which kind of card is being asked for. It is the frontend's INTENT, not a
/// decision about the transfer: a send says the platform has granted a source,
/// a join carries text the app has not looked at.
enum CreateKind { send, join }

/// One request that a card be created, and the authority's answer to it.
///
/// The answer is a single durable verdict rather than an acceptance followed by
/// a completion, because creation has no in-flight window: the record write IS
/// the creation. Nothing here decides whether an invite is good — the whole
/// point is that the text crosses untouched and the authority answers.
class CreateIntent {
  CreateIntent({required this.id, required this.kind, this.displayName});

  /// The caller-minted 128-bit identity, 32 lowercase hex digits. It correlates
  /// the answer with the request and is written down nowhere.
  final String id;
  final CreateKind kind;

  /// What the platform said the picked document is called, for a send. Shown
  /// so the user can see what they are about to send; it is metadata, and the
  /// authority sanitizes it again on arrival.
  final String? displayName;

  CreateOutcomeView? outcome;

  /// Why no answer exists. Not an answer itself: a create whose frame was SENT
  /// may or may not have made a card, and only the card list says — while one
  /// the encoder refused made nothing at all.
  IntentFault? fault;

  /// Whether the user may still be waiting.
  bool get pending => outcome == null && fault == null;

  /// The card this request made, if it made one.
  String? get card => switch (outcome) {
        CreateOutcomeViewCreated(:final CardCreatedView value) => value.card,
        _ => null,
      };
}
