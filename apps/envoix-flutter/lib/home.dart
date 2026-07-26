import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'attachment.dart';
import 'bindings/envoix_command.dart';
import 'bindings/envoix_read.dart';
import 'commands.dart';
import 'instrumentation.dart';
import 'labels.dart';
import 'lane.dart';

/// UI01 — every card this attachment can see, and how healthy the lane that
/// fed them is.
///
/// The order is the order this attachment observed the cards, most recent
/// first — not recency of creation, which the read contract carries no fact
/// about (see [Attachment.cards]). The screen makes no claim about age.
class HomeScreen extends StatelessWidget {
  const HomeScreen({
    required this.attachment,
    required this.commander,
    this.fault,
    super.key,
  });

  final Attachment attachment;

  /// Speaks for this attachment, at its epochs.
  final Commander commander;

  /// Why the lane is not delivering, when it is not.
  final Object? fault;

  @override
  Widget build(BuildContext context) {
    final List<CardRow> cards = attachment.cards;
    return ListView(
      padding: const EdgeInsets.all(12),
      children: <Widget>[
        if (fault != null) FaultBanner(fault: fault!),
        ActivitySummary(cards: cards),
        for (final MapEntry<String, SubscribeRejectionView> refusal
            in attachment.refusals.entries)
          RefusalTile(card: refusal.key, reason: refusal.value),
        if (cards.isEmpty && fault == null)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 24),
            child: Text('No transfers yet. Use New transfer to start one.'),
          ),
        for (final CardRow row in cards)
          CardTile(
            row: row,
            commander: commander,
            intents: attachment.commands.forCard(row.card),
          ),
        LaneHealth(attachment: attachment),
      ],
    );
  }
}

/// The aggregate: how many cards there are, and a tally of the states the
/// authority reported for them. A tally counts identical authoritative values;
/// it does not sort them into active and finished, which would be this app
/// deciding what the states mean.
class ActivitySummary extends StatelessWidget {
  const ActivitySummary({required this.cards, super.key});

  final List<CardRow> cards;

  @override
  Widget build(BuildContext context) {
    final Map<String, int> tally = <String, int>{};
    for (final CardRow row in cards) {
      final CardView? view = row.view;
      if (view != null) {
        final String label = stateLabel(view.state);
        tally[label] = (tally[label] ?? 0) + 1;
      }
    }
    final String summary = '${cards.length} '
        '${cards.length == 1 ? 'transfer' : 'transfers'}';
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text(summary, style: Theme.of(context).textTheme.titleMedium),
          if (tally.isNotEmpty)
            Text(
              <String>[
                for (final MapEntry<String, int> state in tally.entries)
                  '${state.value} ${state.key}',
              ].join(' · '),
              style: Theme.of(context).textTheme.bodySmall,
            ),
        ],
      ),
    );
  }
}

/// One card, as the host last described it — and what may be asked of it.
class CardTile extends StatelessWidget {
  const CardTile({
    required this.row,
    required this.commander,
    this.intents = const <CommandIntent>[],
    super.key,
  });

  final CardRow row;
  final Commander commander;

  /// This card's intents, newest first.
  final List<CommandIntent> intents;

  @override
  Widget build(BuildContext context) {
    final CardView? view = row.view;
    reportRendered(row);
    reportCard(row);
    final ThemeData theme = Theme.of(context);
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            Text(
              view?.offeredName ?? row.card,
              style: theme.textTheme.titleMedium,
            ),
            if (view != null) ...<Widget>[
              _Fact(
                label: 'Status',
                value: '${directionLabel(view.direction)}'
                    ' · ${stateLabel(view.state)}',
              ),
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 8),
                // The indicator's own `semanticsValue` must be a bare number,
                // which cannot say "unknown" — and a total of zero makes the
                // fraction genuinely unanswerable. So the bar is described the
                // same way every other fact on this card is.
                child: Semantics(
                  container: true,
                  label: 'Transfer progress',
                  value: percentLabel(view),
                  child: ExcludeSemantics(
                    child: LinearProgressIndicator(value: progressOf(view)),
                  ),
                ),
              ),
              Text(
                <String>[
                  '${view.bytes}/${view.total} bytes',
                  if (view.bytesResumed > 0)
                    'resumed from ${view.bytesResumed}',
                  phaseLabel(view.phase),
                  quiescenceLabel(view.quiescence),
                ].join(' · '),
                style: theme.textTheme.bodySmall,
              ),
              if (view.invite != null)
                _Invite(card: row.card, invite: view.invite!),
              if (view.outcome != null) _Outcome(outcome: view.outcome!),
              _Actions(
                row: row,
                view: view,
                commander: commander,
                intents: intents,
              ),
            ],
            for (final CommandIntent intent in intents)
              _Intent(row: row, intent: intent, commander: commander),
            if (row.status != StreamStatus.live)
              _Fact(label: 'Stream', value: streamLabel(row), warn: true),
            if (row.duty != null)
              Text(
                'The host asked the system to '
                '${capabilityActionLabel(row.duty!.action)} '
                '(${dutyKindLabel(row.duty!.duty.kind)})',
                style: theme.textTheme.bodySmall,
              ),
            Text(
              'card ${row.card} · epoch ${row.epoch}'
              '${view == null ? '' : ' · attempt ${view.generation}'}',
              style: theme.textTheme.bodySmall,
            ),
          ],
        ),
      ),
    );
  }
}

/// Exactly the commands the authority currently admits for this card, and
/// nothing else.
///
/// `allowedActions` is the product reducer's own `allowed_commands`, published
/// in the read contract. Rendering that list verbatim is what makes an illegal
/// affordance unrepresentable here: this widget has no rule about states,
/// because owning one would be the frontend owning transfer truth (R0).
class _Actions extends StatelessWidget {
  const _Actions({
    required this.row,
    required this.view,
    required this.commander,
    required this.intents,
  });

  final CardRow row;
  final CardView view;
  final Commander commander;
  final List<CommandIntent> intents;

  @override
  Widget build(BuildContext context) {
    if (view.allowedActions.isEmpty) {
      return Padding(
        padding: const EdgeInsets.only(top: 8),
        child: _Fact(
          label: 'Actions',
          value: 'Nothing can be asked of this card right now.',
        ),
      );
    }
    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: Wrap(
        spacing: 8,
        children: <Widget>[
          for (final CommandKindView kind in view.allowedActions)
            _Action(
              row: row,
              command: commandOf(kind),
              commander: commander,
              waiting: commandInFlight(intents, commandOf(kind)),
            ),
        ],
      ),
    );
  }
}

class _Action extends StatelessWidget {
  const _Action({
    required this.row,
    required this.command,
    required this.commander,
    required this.waiting,
  });

  final CardRow row;
  final CommandView command;
  final Commander commander;
  final bool waiting;

  @override
  Widget build(BuildContext context) {
    reportAffordance(context, row.card, command);
    return FilledButton.tonal(
      onPressed: waiting ? null : () => commander.issue(row, command),
      child: Text(commandLabel(command)),
    );
  }
}

/// One command this app issued, and every answer the authority gave it.
///
/// In-flight and committed are drawn differently on purpose: an accepted
/// command has not crossed the durability barrier, and a screen that showed the
/// two the same way would be claiming an effect nobody committed.
class _Intent extends StatelessWidget {
  const _Intent({
    required this.row,
    required this.intent,
    required this.commander,
  });

  final CardRow row;
  final CommandIntent intent;
  final Commander commander;

  @override
  Widget build(BuildContext context) {
    reportIntent(intent);
    final ColorScheme colors = Theme.of(context).colorScheme;
    final bool waiting = intent.unsettled;
    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: Container(
        width: double.infinity,
        padding: const EdgeInsets.all(8),
        decoration: BoxDecoration(
          color: waiting
              ? colors.secondaryContainer
              : colors.surfaceContainerHighest,
          borderRadius: BorderRadius.circular(8),
          // The in-flight border is the second carrier: colour alone must not
          // be what says "this has not committed".
          border: waiting ? Border.all(color: colors.secondary) : null,
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            _Fact(label: 'Command', value: intentLabel(intent)),
            if (intent.mayDisambiguate)
              // The disambiguation, and the only way to run it: the SAME
              // identity again. Duplicate means it had committed, a fresh
              // acceptance means it had not — the app never decides which.
              TextButton(
                onPressed: () => commander.reissue(row, intent),
                child: const Text('Ask again'),
              ),
          ],
        ),
      ),
    );
  }
}

/// The card's invite, exactly as the authority published it.
///
/// The app renders it and can put it on the clipboard; it does not build one,
/// parse one, or judge one. The link is sized by the invite grammar itself, so
/// an invite the authority holds always arrives whole; a card whose stored
/// channel no longer spells one still shows its code, because the code is what
/// a user reads out.
class _Invite extends StatelessWidget {
  const _Invite({required this.card, required this.invite});

  final String card;
  final InviteView invite;

  @override
  Widget build(BuildContext context) {
    reportInvite(card, invite);
    final ColorScheme colors = Theme.of(context).colorScheme;
    // Exposed because sharing it IS the feature. Every other path renders it
    // redacted, which is what keeps it out of a log or a stack trace.
    final String? link = invite.link?.expose();
    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: Container(
        width: double.infinity,
        padding: const EdgeInsets.all(8),
        decoration: BoxDecoration(
          color: colors.surfaceContainerHighest,
          borderRadius: BorderRadius.circular(8),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            _Fact(label: 'Room code', value: invite.code.expose()),
            if (link != null)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: TextButton(
                  onPressed: () =>
                      Clipboard.setData(ClipboardData(text: link)),
                  child: const Text('Copy invite'),
                ),
              )
            else
              const _Fact(
                label: 'Invite link',
                value: 'This card has no shareable link — read out the code.',
                warn: true,
              ),
          ],
        ),
      ),
    );
  }
}

/// The typed outcome, with the recovery the authority suggested. It is shown as
/// a fact, never as a button: acting on it is a command whose legality the
/// authority publishes in `allowedActions`.
class _Outcome extends StatelessWidget {
  const _Outcome({required this.outcome});

  final OutcomeView outcome;

  @override
  Widget build(BuildContext context) {
    final ColorScheme colors = Theme.of(context).colorScheme;
    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: Container(
        width: double.infinity,
        padding: const EdgeInsets.all(8),
        decoration: BoxDecoration(
          color: colors.surfaceContainerHighest,
          borderRadius: BorderRadius.circular(8),
        ),
        child: _Fact(
          label: 'Outcome',
          value: <String>[
            '${outcomeCodeLabel(outcome.code)} while ${phaseLabel(outcome.phase)}',
            outcome.display,
            retryabilityLabel(outcome.retry),
            if (outcome.recovery != null) recoveryLabel(outcome.recovery!),
          ].join(' — '),
        ),
      ),
    );
  }
}

/// One labelled fact. The label is what a screen reader announces before the
/// value, and the text itself carries no semantics of its own so the pair is
/// read once rather than twice.
class _Fact extends StatelessWidget {
  const _Fact({required this.label, required this.value, this.warn = false});

  final String label;
  final String value;
  final bool warn;

  @override
  Widget build(BuildContext context) {
    final ColorScheme colors = Theme.of(context).colorScheme;
    return Semantics(
      container: true,
      label: label,
      value: value,
      child: ExcludeSemantics(
        child: Text(
          value,
          style: TextStyle(color: warn ? colors.error : colors.onSurface),
        ),
      ),
    );
  }
}

/// A card the runtime refused to stream. Typed truth, not an error.
class RefusalTile extends StatelessWidget {
  const RefusalTile({required this.card, required this.reason, super.key});

  final String card;
  final SubscribeRejectionView reason;

  @override
  Widget build(BuildContext context) => Card(
        color: Theme.of(context).colorScheme.errorContainer,
        child: ListTile(
          title: Text('card $card is not observable'),
          subtitle: Text(refusalLabel(reason)),
        ),
      );
}

/// The lane is not delivering at all — the host may not be running.
///
/// A live region, unlike every other fact here: this one appears while the
/// reader is already on the screen, and a delivery error that only a sighted
/// user notices is a list that has silently stopped updating.
class FaultBanner extends StatelessWidget {
  const FaultBanner({required this.fault, super.key});

  final Object fault;

  @override
  Widget build(BuildContext context) => Semantics(
        container: true,
        liveRegion: true,
        label: 'The lane is not delivering',
        value: '$fault',
        child: ExcludeSemantics(
          child: Card(
            color: Theme.of(context).colorScheme.errorContainer,
            child: ListTile(
              title: const Text('The lane is not delivering'),
              subtitle: Text('$fault'),
            ),
          ),
        ),
      );
}

/// What the lane dropped. Frames the app could not use are content, not
/// silence: a list that may be missing updates says so where the list is.
class LaneHealth extends StatelessWidget {
  const LaneHealth({required this.attachment, super.key});

  final Attachment attachment;

  @override
  Widget build(BuildContext context) {
    final int dropped = FrameRejection.values
        .map(attachment.rejected)
        .fold(0, (int total, int count) => total + count);
    final ThemeData theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          if (dropped > 0)
            Text(
              'This list may be missing updates.',
              style: TextStyle(color: theme.colorScheme.error),
            ),
          Semantics(
            container: true,
            label: 'Frames the app could not use',
            value: '$dropped',
            child: ExcludeSemantics(
              child: Text(
                <String>[
                  for (final FrameRejection kind in FrameRejection.values)
                    '${rejectionLabel(kind)} ${attachment.rejected(kind)}',
                  // An answer to a command a PREVIOUS attachment issued: the
                  // host resolves it whenever its barrier does, and this
                  // attachment has no intent to put it against.
                  'unaddressed answers ${attachment.commands.unaddressed}',
                ].join(' · '),
                style: theme.textTheme.bodySmall,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
