import 'package:flutter/foundation.dart';

import 'attachment.dart';
import 'bindings/envoix_read.dart';

/// Every line already reported, so a rebuild does not repeat one.
final Set<String> rendered = <String>{};

/// What the on-device instrumentation reads: one line per distinct thing this
/// app has actually PUT ON SCREEN, emitted from the widget that drew it — a
/// claim about the screen rather than about the model behind it.
void reportRendered(CardRow row) {
  _report('envoix-f1b rendered card=${row.card} '
      'epoch=${row.epoch} status=${row.status.name}');
}

/// The same claim for the logs screen: this timeline was drawn, with this many
/// entries, and said this about its own completeness.
void reportTimeline(EvidenceTimelineView timeline) {
  final String status = switch (timeline.status) {
    DiagnosticsStatusViewComplete() => 'complete',
    DiagnosticsStatusViewDegraded(:final DegradedView value) =>
      'degraded:${value.droppedEvents}',
  };
  _report('envoix-f1c timeline card=${timeline.session.card} '
      'generation=${timeline.session.generation} '
      'entries=${timeline.entries.length} diagnostics=$status');
}

void _report(String line) {
  if (rendered.add(line)) {
    debugPrint(line);
  }
}
