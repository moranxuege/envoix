import 'dart:convert';

import 'bindings/envoix_read.dart';

/// Why a frame the lane delivered changed nothing.
enum FrameRejection {
  /// It belongs to an attachment this one superseded.
  staleEpoch,

  /// The stream broke its own contract: no opening snapshot, or a second one.
  contractBreach,

  /// The bytes are not a frame of the read contract this app speaks.
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
  final Map<FrameRejection, int> _rejected = <FrameRejection, int>{
    for (final FrameRejection kind in FrameRejection.values) kind: 0,
  };

  /// Every card this attachment has seen, in id order.
  List<CardRow> get cards {
    final List<CardRow> rows = _rows.values.toList();
    rows.sort((CardRow left, CardRow right) => left.card.compareTo(right.card));
    return rows;
  }

  /// Cards the runtime refused to attach, and why. Typed truth, not an error.
  Map<String, SubscribeRejectionView> get refusals =>
      Map<String, SubscribeRejectionView>.unmodifiable(_refusals);

  int rejected(FrameRejection kind) => _rejected[kind]!;

  /// Decodes one frame off the lane and admits it. Returns whether the view
  /// changed.
  bool ingest(List<int> bytes) {
    final ReadFrame frame;
    try {
      frame = decodeReadFrame(utf8.decode(bytes));
    } on FormatException {
      return _reject(FrameRejection.undecodable);
    } on ReadContractException {
      // Includes a frame of the COMMAND contract, which shares this lane and
      // belongs to F2's conversation, not to an observer.
      return _reject(FrameRejection.undecodable);
    }
    return admit(frame);
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
      case ReadBodyEvidence():
      case ReadBodyBuildManifest():
        // Neither is published on the card lane; F1c asks for them by name.
        return false;
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
    switch (update.kind) {
      case CardUpdateKindViewSnapshot(:final CardView value):
      case CardUpdateKindViewProgress(:final CardView value):
      case CardUpdateKindViewState(:final CardView value):
      case CardUpdateKindViewTerminal(:final CardView value):
        final CardRow row = _rows.putIfAbsent(
          update.card,
          () => CardRow(update.card),
        );
        row.epoch = update.epoch;
        row.view = value;
      case CardUpdateKindViewCapabilityDuty():
        // A duty is the service's work order, never an observer's business.
        break;
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
