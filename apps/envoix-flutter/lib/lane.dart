import 'dart:async';
import 'dart:convert';

import 'package:flutter/services.dart';

import 'attachment.dart';
import 'bindings/envoix_command.dart';
import 'commands.dart';

/// A source of encoded contract frames. Listening to it IS attaching: the
/// platform side opens a fresh attachment on the host, and cancelling — or
/// dying — only stops the delivery (Pillar 7: the frontend owns no lifetime).
typedef LaneSource = Stream<List<int>> Function();

/// The one thing that crosses in the other direction: a submit frame out, the
/// authority's acceptance frame back. Not a transfer verb — the host decides
/// what the command does, and says so.
typedef CommandSink = Future<List<int>?> Function(List<int> frame);

/// Mirrors the catalogued `android.frontend_lane_channel`.
const String laneChannel = 'app.envoix.host/frontend-lane';

/// Mirrors the catalogued `android.frontend_command_channel`.
const String commandChannel = 'app.envoix.host/frontend-commands';

/// The one method that channel carries.
const String submitMethod = 'submit';

/// The real lane. The bytes are opaque here and on the platform side; only the
/// generated codec in `bindings/` ever looks inside one.
Stream<List<int>> platformLane() => const EventChannel(laneChannel)
    .receiveBroadcastStream()
    .cast<List<int>>();

/// The real command sink. The reply is the encoded acceptance frame; the
/// committed completion follows separately, on the frame lane.
Future<List<int>?> platformCommands(List<int> frame) =>
    const MethodChannel(commandChannel)
        .invokeMethod<Uint8List>(submitMethod, Uint8List.fromList(frame));

/// One frontend attachment: the [Attachment], the frames that feed it, and the
/// commander that speaks for it — created together and reachable only through
/// each other.
///
/// Nothing can hand this an existing attachment, so re-attaching cannot reuse
/// the one it supersedes — the host has already restarted every card at a new
/// epoch, which the old gates would reject forever. And [frames] carries the
/// attachment itself, so anything that changed what it shows arrives as a
/// stream event rather than as a rebuild someone has to remember to ask for.
class LaneAttachment {
  factory LaneAttachment.open(
    LaneSource lane, {
    CommandSink commands = platformCommands,
  }) {
    final Attachment attachment = Attachment();
    late final StreamController<Attachment> events;
    StreamSubscription<List<int>>? source;
    events = StreamController<Attachment>(
      // Subscribing to the lane is what attaches, so it happens when the screen
      // listens and stops when it stops — never on construction.
      onListen: () {
        source = lane().listen(
          (List<int> bytes) {
            if (attachment.ingest(bytes)) {
              events.add(attachment);
            }
          },
          onError: events.addError,
          onDone: events.close,
        );
      },
      // Teardown detaches a subscriber, never a transfer (Pillar 7): there is
      // no native verb here at all, only a subscription being dropped.
      onCancel: () async {
        await source?.cancel();
        source = null;
      },
    );
    return LaneAttachment._(
      attachment,
      events.stream,
      Commander(
        attachment: attachment,
        sink: commands,
        announce: () {
          if (!events.isClosed) {
            events.add(attachment);
          }
        },
      ),
    );
  }

  const LaneAttachment._(this.attachment, this.frames, this.commander);

  /// What the lane has said so far, and what the screen shows before the first
  /// frame arrives.
  final Attachment attachment;

  /// One event per frame — or per answered command — that changed what
  /// [attachment] shows.
  final Stream<Attachment> frames;

  /// Issues commands on this attachment's behalf, at its epochs.
  final Commander commander;
}

/// Turns a tap into a durable effect, or into an honest account of why not.
///
/// It mints one identity per intent, encodes the submit frame with the
/// generated encoder, and records what came back. It decides nothing: every
/// verdict, every completion and every refusal is the authority's, and an
/// answer that does not arrive stays an answer that did not arrive.
class Commander {
  Commander({
    required Attachment attachment,
    required CommandSink sink,
    required void Function() announce,
  })  : _attachment = attachment,
        _sink = sink,
        _announce = announce;

  final Attachment _attachment;
  final CommandSink _sink;
  final void Function() _announce;

  /// Issues `command` for `row` under a fresh identity.
  Future<void> issue(CardRow row, CommandView command) =>
      _send(row, command, null);

  /// Re-presents an intent's identity — the documented `Interrupted`
  /// disambiguation. A `Duplicate` answer means the first submission had
  /// committed; a fresh acceptance means it had not. The UI never guesses which.
  Future<void> reissue(CardRow row, CommandIntent intent) =>
      _send(row, intent.command, intent.id);

  Future<void> _send(CardRow row, CommandView command, String? id) async {
    final CommandIntent intent =
        _attachment.commands.open(row.card, command, id: id);
    _announce();
    final List<int> frame;
    try {
      frame = submitFrame(
        card: row.card,
        epoch: row.epoch,
        id: intent.id,
        command: command,
      );
    } on CommandContractException catch (error) {
      // The encoder enforces every bound its decoder checks, so a frame that
      // does not encode never leaves the process — and never becomes a command
      // whose fate is unknown.
      return _fault(intent, error);
    }
    final List<int>? reply;
    try {
      reply = await _sink(frame);
    } on PlatformException catch (error) {
      return _fault(intent, error);
    } on MissingPluginException catch (error) {
      return _fault(intent, error);
    }
    if (reply == null) {
      return _fault(intent, 'the host answered nothing');
    }
    final CommandFrame answer;
    try {
      answer = decodeCommandFrame(utf8.decode(reply));
    } on FormatException catch (error) {
      return _fault(intent, error);
    } on CommandContractException catch (error) {
      return _fault(intent, error);
    }
    if (_attachment.admitCommand(answer) != CommandAdmission.answered) {
      return _fault(intent, 'the host answered a command nobody issued');
    }
    _announce();
  }

  void _fault(CommandIntent intent, Object error) {
    _attachment.commands.faulted(intent.id, error);
    _announce();
  }
}
