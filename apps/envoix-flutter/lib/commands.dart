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
  Object? fault;

  CommandPhase get phase {
    if (fault != null) {
      return CommandPhase.undelivered;
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

  /// The lane failed for `id`: no verdict exists, and none is invented.
  void faulted(String id, Object error) {
    _intents[id]?.fault = error;
  }

  CommandAdmission admit(CommandFrame frame) {
    switch (frame.body) {
      case final CommandBodySubmit _:
        return CommandAdmission.notAnAnswer;
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

/// The one frame a frontend may originate, through the generated encoder.
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
    utf8.encode(
      encodeCommandFrame(
        SubmitView(
          card: card,
          epoch: epoch,
          commandId: id,
          command: command,
        ),
      ),
    );
