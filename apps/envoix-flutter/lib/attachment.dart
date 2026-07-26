import 'dart:convert';

import 'bindings/envoix_command.dart';
import 'bindings/envoix_read.dart';
import 'commands.dart';

/// Why a frame the lane delivered changed nothing.
enum FrameRejection {
  /// It belongs to an attachment this one superseded.
  staleEpoch,

  /// The stream broke its own contract: no opening snapshot, a second one, or
  /// a body only a frontend may originate arriving AT one.
  contractBreach,

  /// The bytes are not a frame of either contract this app speaks.
  undecodable,
}

/// What the lane last said about one card's stream.
enum StreamStatus {
  /// The epoch is open and delivering.
  live,

  /// The epoch dropped a lossless update and ended. A fresh attachment
  /// re-seeds it from current truth; nothing durable was lost.
  lagged,

  /// The runtime ended the epoch (the card retired, or the host stopped).
  closed,
}

/// One card as this attachment last saw it.
class CardRow {
  CardRow(this.card);

  /// The card's 16-hex-digit id, which is also its identity on the lane.
  final String card;

  /// The epoch that delivered [view]. It changes only across attachments.
  int epoch = 0;
  CardView? view;
  StreamStatus status = StreamStatus.live;

  /// Which lossless class the lag dropped, when [status] is
  /// [StreamStatus.lagged].
  LosslessKindView? missed;

  /// The last platform duty the host issued for this card. It is the service's
  /// work order — shown here as something observed, never as something to do.
  DutyFrameView? duty;
}

/// One frontend attachment: the epoch gates it opened and the rows they fed.
///
/// A new instance belongs to every lane subscription. Opening the lane makes
/// the host restart each card's stream at a new epoch, so this attachment's
/// gates only ever admit its own — every earlier epoch is stale by
/// construction, and [EpochGate] is what says so.
///
/// Nothing here parses JSON. The generated codec decodes the bytes and the
/// generated gate decides admission; this type only routes the outcome.
class Attachment {
  final Map<String, EpochGate> _gates = <String, EpochGate>{};
  final Map<String, CardRow> _rows = <String, CardRow>{};
  final Map<String, SubscribeRejectionView> _refusals =
      <String, SubscribeRejectionView>{};

  /// The commands this attachment issued and what the authority answered. It
  /// belongs to the attachment because the answers do: `open_lane` discards the
  /// frames the superseded attachment never drained, so an intent carried
  /// across would wait for a completion that can no longer arrive.
  final CommandJournal commands = CommandJournal();
  final Map<(String, int), EvidenceTimelineView> _timelines =
      <(String, int), EvidenceTimelineView>{};
  BuildManifestView? _build;
  final Map<FrameRejection, int> _rejected = <FrameRejection, int>{
    for (final FrameRejection kind in FrameRejection.values) kind: 0,
  };

  /// Every card this attachment has seen, most recently observed first.
  ///
  /// This is **observation order, not creation order**. Within one attachment
  /// it is a real arrival order — the row this attachment learned about last is
  /// first, and a later frame for a card it already knows does not move it.
  /// Across attachments it is not: `open_lane` re-subscribes every known card
  /// from a `BTreeSet<RecordId>`, so a re-attached frontend observes them in id
  /// order, and `RecordId` is minted at random.
  ///
  /// UI01's newest-first therefore needs a fact the read contract does not
  /// carry — a monotonic creation ordinal on the record — and inferring one
  /// here would be the frontend minting transfer truth. Named as a gap rather
  /// than guessed at.
  List<CardRow> get cards =>
      _rows.values.toList(growable: false).reversed.toList(growable: false);

  /// Cards the runtime refused to attach, and why. Typed truth, not an error.
  Map<String, SubscribeRejectionView> get refusals =>
      Map<String, SubscribeRejectionView>.unmodifiable(_refusals);

  /// The session timelines this attachment has been told, newest generation of
  /// each card last. One per session key, because a re-attached frontend is
  /// re-told a timeline it already has rather than sent the difference.
  List<EvidenceTimelineView> get timelines {
    final List<(String, int)> keys = _timelines.keys.toList()
      ..sort(((String, int) left, (String, int) right) {
        final int card = left.$1.compareTo(right.$1);
        return card != 0 ? card : left.$2.compareTo(right.$2);
      });
    return <EvidenceTimelineView>[
      for (final (String, int) key in keys) _timelines[key]!,
    ];
  }

  /// What the core says it is: protocol, schemas and trust root.
  BuildManifestView? get build => _build;

  int rejected(FrameRejection kind) => _rejected[kind]!;

  /// Decodes one frame off the lane and admits it. Returns whether the view
  /// changed.
  ///
  /// The lane carries both contracts, so this splits them — on the schema
  /// envelope the codec itself reports, never on a guess about the bytes. Only
  /// `unknownSchema` is worth a second decoder: any other read failure means
  /// the frame claimed to be a read frame and was not one.
  bool ingest(List<int> bytes) {
    final String text;
    try {
      text = utf8.decode(bytes);
    } on FormatException {
      return _reject(FrameRejection.undecodable);
    }
    try {
      return admit(decodeReadFrame(text));
    } on FormatException {
      return _reject(FrameRejection.undecodable);
    } on ReadContractException catch (error) {
      if (error.kind != ReadErrorKind.unknownSchema) {
        return _reject(FrameRejection.undecodable);
      }
    }
    final CommandFrame command;
    try {
      command = decodeCommandFrame(text);
    } on FormatException {
      return _reject(FrameRejection.undecodable);
    } on CommandContractException {
      return _reject(FrameRejection.undecodable);
    }
    admitCommand(command);
    // Whatever it was, something on screen changed: an intent's answer, the
    // unaddressed count, or the out-of-contract count.
    return true;
  }

  /// Admits one decoded command frame — an acceptance or a completion the
  /// authority addressed to an intent this attachment issued. The caller is
  /// told which, because a submitter and an observer act on it differently.
  CommandAdmission admitCommand(CommandFrame frame) {
    final CommandAdmission admission = commands.admit(frame);
    if (admission == CommandAdmission.notAnAnswer) {
      // A `submit` body has exactly one legitimate direction, and it is not
      // this one.
      _reject(FrameRejection.contractBreach);
    }
    return admission;
  }

  /// Admits one decoded frame. Split from [ingest] because decoding and
  /// admission are separate questions: the codec decides whether the bytes are
  /// a frame at all, and this decides whether the frame is THIS attachment's.
  bool admit(ReadFrame frame) {
    switch (frame.body) {
      case final ReadBodyCardUpdate body:
        return _update(body.value, frame);
      case final ReadBodyLag body:
        return _end(body.value.card, frame, StreamStatus.lagged,
            body.value.missed);
      case final ReadBodyClosed body:
        return _end(body.value.card, frame, StreamStatus.closed, null);
      case final ReadBodySubscribeRejected body:
        _refusals[body.value.card] = body.value.reason;
        return true;
      case final ReadBodyEvidence body:
        // Evidence is downstream truth about a session, not a frame of a card's
        // stream: it carries no epoch, so the gates have no verdict to give and
        // it is kept exactly as the authority stated it.
        final SessionKeyView session = body.value.session;
        _timelines[(session.card, session.generation)] = body.value;
        return true;
      case final ReadBodyBuildManifest body:
        _build = body.value;
        return true;
    }
  }

  bool _update(CardUpdateView update, ReadFrame frame) {
    EpochGate? gate = _gates[update.card];
    if (gate == null) {
      // Every epoch opens with its snapshot, so anything else arrived from an
      // epoch this attachment never opened.
      if (update.kind is! CardUpdateKindViewSnapshot) {
        return _reject(FrameRejection.staleEpoch);
      }
      gate = EpochGate.attach(update.epoch);
      _gates[update.card] = gate;
    }
    return _decide(gate.admit(frame), () => _apply(update));
  }

  void _apply(CardUpdateView update) {
    final CardRow row = _rows.putIfAbsent(
      update.card,
      () => CardRow(update.card),
    );
    row.epoch = update.epoch;
    switch (update.kind) {
      case CardUpdateKindViewSnapshot(:final CardView value):
      case CardUpdateKindViewProgress(:final CardView value):
      case CardUpdateKindViewState(:final CardView value):
      case CardUpdateKindViewTerminal(:final CardView value):
        row.view = value;
      case CardUpdateKindViewCapabilityDuty(:final DutyFrameView value):
        // A duty is the service's work order. An observer may say that the
        // host asked for it; it must not act on it, and it is not card truth,
        // so it never touches [CardRow.view].
        row.duty = value;
    }
  }

  /// A lag or a close ends the epoch. The gate stays in place holding that
  /// verdict, so everything after it is stale until a fresh attachment.
  bool _end(
    String card,
    ReadFrame frame,
    StreamStatus status,
    LosslessKindView? missed,
  ) {
    final EpochGate? gate = _gates[card];
    if (gate == null) {
      return _reject(FrameRejection.staleEpoch);
    }
    return _decide(gate.admit(frame), () {
      final CardRow? row = _rows[card];
      if (row == null) {
        return;
      }
      row.status = status;
      row.missed = missed;
    });
  }

  bool _decide(GateDecision decision, void Function() apply) {
    switch (decision) {
      case GateDecision.deliver:
        apply();
        return true;
      case GateDecision.dropStale:
        return _reject(FrameRejection.staleEpoch);
      case GateDecision.contractBreach:
        return _reject(FrameRejection.contractBreach);
    }
  }

  bool _reject(FrameRejection kind) {
    _rejected[kind] = _rejected[kind]! + 1;
    return true;
  }
}
