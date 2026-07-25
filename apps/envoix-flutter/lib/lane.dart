import 'package:flutter/services.dart';

import 'attachment.dart';

/// A source of encoded contract frames. Listening to it IS attaching: the
/// platform side opens a fresh attachment on the host, and cancelling — or
/// dying — only stops the delivery (Pillar 7: the frontend owns no lifetime).
typedef LaneSource = Stream<List<int>> Function();

/// Mirrors the catalogued `android.frontend_lane_channel`.
const String laneChannel = 'app.envoix.host/frontend-lane';

/// The real lane. The bytes are opaque here and on the platform side; only the
/// generated codec in `bindings/` ever looks inside one.
Stream<List<int>> platformLane() => const EventChannel(laneChannel)
    .receiveBroadcastStream()
    .cast<List<int>>();

/// One frontend attachment: the [Attachment] and the frames that feed it,
/// created together and reachable only through each other.
///
/// Nothing can hand this an existing attachment, so re-attaching cannot reuse
/// the one it supersedes — the host has already restarted every card at a new
/// epoch, which the old gates would reject forever. And [frames] carries the
/// attachment itself, so a frame that changed what it shows arrives as a stream
/// event rather than as a rebuild someone has to remember to ask for.
class LaneAttachment {
  factory LaneAttachment.open(LaneSource lane) {
    final Attachment attachment = Attachment();
    return LaneAttachment._(
      attachment,
      lane().where(attachment.ingest).map((List<int> frame) => attachment),
    );
  }

  const LaneAttachment._(this.attachment, this.frames);

  /// What the lane has said so far, and what the screen shows before the first
  /// frame arrives.
  final Attachment attachment;

  /// One event per frame that changed what [attachment] shows.
  final Stream<Attachment> frames;
}
