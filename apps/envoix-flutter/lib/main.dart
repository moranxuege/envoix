import 'package:flutter/material.dart';

import 'attachment.dart';
import 'bindings/envoix_read.dart';
import 'lane.dart';

void main() => runApp(const EnvoixApp());

/// The read-only Envoix frontend: it observes the host and shows what it says.
class EnvoixApp extends StatelessWidget {
  const EnvoixApp({super.key, this.lane = platformLane});

  final LaneSource lane;

  @override
  Widget build(BuildContext context) => MaterialApp(
        title: 'Envoix',
        theme: ThemeData.dark(useMaterial3: true),
        home: CardsScreen(lane: lane),
      );
}

/// One screen: every card this attachment can see.
class CardsScreen extends StatefulWidget {
  const CardsScreen({required this.lane, super.key});

  final LaneSource lane;

  @override
  State<CardsScreen> createState() => _CardsScreenState();
}

class _CardsScreenState extends State<CardsScreen> {
  late LaneAttachment _lane = LaneAttachment.open(widget.lane);

  /// Opens a fresh attachment. Every card's stream restarts at a new epoch, so
  /// the gates of the attachment being replaced can never admit anything again
  /// — which is why [LaneAttachment.open] builds its own rather than taking
  /// one. Replacing the stream is also what cancels the old subscription, so
  /// the platform side hears exactly one detach per re-attach.
  void _attach() => setState(() => _lane = LaneAttachment.open(widget.lane));

  /// The screen is a function of the lane: [StreamBuilder] rebuilds on every
  /// frame the attachment accepted, so "ingested but never rendered" is not a
  /// state this can be in.
  @override
  Widget build(BuildContext context) => StreamBuilder<Attachment>(
        stream: _lane.frames,
        builder: (BuildContext context, AsyncSnapshot<Attachment> frames) =>
            _screen(context, frames.error),
      );

  Widget _screen(BuildContext context, Object? fault) {
    final List<CardRow> cards = _lane.attachment.cards;
    return Scaffold(
      appBar: AppBar(
        title: const Text('Envoix'),
        actions: <Widget>[
          // Re-attaching is an observer action, not a command: it opens a new
          // epoch and touches nothing the host is doing. The icon font is
          // deliberately not packaged, so this is text.
          TextButton(onPressed: _attach, child: const Text('Re-attach')),
        ],
      ),
      body: ListView(
        padding: const EdgeInsets.all(12),
        children: <Widget>[
          if (fault != null) _FaultBanner(fault: fault),
          for (final MapEntry<String, SubscribeRejectionView> refusal
              in _lane.attachment.refusals.entries)
            _RefusalTile(card: refusal.key, reason: refusal.value),
          if (cards.isEmpty && fault == null)
            const Padding(
              padding: EdgeInsets.symmetric(vertical: 24),
              child: Text('No transfers yet.'),
            ),
          for (final CardRow row in cards) CardTile(row: row),
          _Counters(attachment: _lane.attachment),
        ],
      ),
    );
  }
}

/// One card, as the host last described it.
class CardTile extends StatelessWidget {
  const CardTile({required this.row, super.key});

  final CardRow row;

  @override
  Widget build(BuildContext context) {
    final CardView? view = row.view;
    reportRendered(row);
    return Card(
      child: ListTile(
        title: Text(view?.offeredName ?? row.card),
        subtitle: Text(
          <String>[
            'card ${row.card}',
            'epoch ${row.epoch}',
            if (view != null) describeState(view.state),
            if (view != null) '${view.bytes}/${view.total} bytes',
            if (view != null) view.direction.name,
            if (row.status != StreamStatus.live)
              '${row.status.name}${row.missed == null ? '' : ' (${row.missed!.name})'}',
          ].join(' · '),
        ),
      ),
    );
  }
}

class _RefusalTile extends StatelessWidget {
  const _RefusalTile({required this.card, required this.reason});

  final String card;
  final SubscribeRejectionView reason;

  @override
  Widget build(BuildContext context) => Card(
        color: Theme.of(context).colorScheme.errorContainer,
        child: ListTile(
          title: Text('card $card is not observable'),
          subtitle: Text(reason.name),
        ),
      );
}

class _FaultBanner extends StatelessWidget {
  const _FaultBanner({required this.fault});

  final Object fault;

  @override
  Widget build(BuildContext context) => Card(
        color: Theme.of(context).colorScheme.errorContainer,
        child: ListTile(
          title: const Text('The lane is not delivering'),
          subtitle: Text('$fault'),
        ),
      );
}

class _Counters extends StatelessWidget {
  const _Counters({required this.attachment});

  final Attachment attachment;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.only(top: 16),
        child: Text(
          <String>[
            for (final FrameRejection kind in FrameRejection.values)
              '${kind.name} ${attachment.rejected(kind)}',
          ].join(' · '),
          style: Theme.of(context).textTheme.bodySmall,
        ),
      );
}

/// The closed product state, spelled for a human.
String describeState(ProductStateView state) => switch (state) {
      ProductStateViewPreparing() => 'preparing',
      ProductStateViewWaiting() => 'waiting',
      ProductStateViewConnecting() => 'connecting',
      ProductStateViewVerifying() => 'verifying',
      ProductStateViewTransferring() => 'transferring',
      ProductStateViewConfirming() => 'confirming',
      ProductStateViewPaused(:final PausedView value) =>
        'paused (${value.origin.name})',
      ProductStateViewUnconfirmed() => 'unconfirmed',
      ProductStateViewCompleted() => 'completed',
      ProductStateViewFailed() => 'failed',
      ProductStateViewCancelled() => 'cancelled',
    };

/// Every line already reported, so a rebuild does not repeat one.
final Set<String> rendered = <String>{};

/// What the on-device instrumentation reads: one line per distinct thing this
/// app has actually PUT ON SCREEN, emitted from the tile that drew it — a
/// claim about the screen rather than about the model behind it.
void reportRendered(CardRow row) {
  final String line = 'envoix-f1b rendered card=${row.card} '
      'epoch=${row.epoch} status=${row.status.name}';
  if (rendered.add(line)) {
    debugPrint(line);
  }
}
