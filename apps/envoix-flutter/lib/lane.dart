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

/// The one thing that crosses in the other direction: an intent frame out, the
/// authority's answer frame back. Not a transfer verb — the host decides what
/// the intent does, and says so.
typedef CommandSink = Future<List<int>?> Function(List<int> frame);

/// Asks the platform to open the document picker.
///
/// It answers with what the provider says the document is called and how big it
/// is — sanitized metadata (`SF09`) — or null when the user cancelled. It never
/// answers with a URI or a handle: the picked source stays on the platform side
/// (`XP01`), and this app has no type that could hold one.
typedef SourcePicker = Future<PickedSource?> Function();

/// What the platform will tell this app about a document the user picked.
class PickedSource {
  const PickedSource({required this.displayName, required this.sizeBytes});

  final String displayName;
  final int sizeBytes;
}

/// Mirrors the catalogued `android.frontend_lane_channel`.
const String laneChannel = 'app.envoix.host/frontend-lane';

/// Mirrors the catalogued `android.frontend_command_channel`.
const String commandChannel = 'app.envoix.host/frontend-commands';

/// The intent method that channel carries.
const String intentMethod = 'intent';

/// The platform-capability method: open the document picker.
const String pickSourceMethod = 'pickSource';

/// Platform error code for a frame the authority received and rejected.
const String hostRejected = 'host-rejected';

/// The real lane. The bytes are opaque here and on the platform side; only the
/// generated codec in `bindings/` ever looks inside one.
Stream<List<int>> platformLane() => const EventChannel(laneChannel)
    .receiveBroadcastStream()
    .cast<List<int>>();

/// The real command sink. The reply is the encoded answer frame; a committed
/// completion follows separately, on the frame lane.
Future<List<int>?> platformCommands(List<int> frame) =>
    const MethodChannel(commandChannel)
        .invokeMethod<Uint8List>(intentMethod, Uint8List.fromList(frame));

/// The real source picker. The reply carries two scalars and nothing else.
Future<PickedSource?> platformPickSource() async {
  final Map<Object?, Object?>? granted =
      await const MethodChannel(commandChannel)
          .invokeMethod<Map<Object?, Object?>>(pickSourceMethod);
  if (granted == null) {
    return null;
  }
  return PickedSource(
    displayName: granted['displayName'] as String? ?? '',
    sizeBytes: granted['sizeBytes'] as int? ?? 0,
  );
}

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
    void announce() {
      if (!events.isClosed) {
        events.add(attachment);
      }
    }

    return LaneAttachment._(
      attachment,
      events.stream,
      Commander(attachment: attachment, sink: commands, announce: announce),
      Creator(sink: commands),
    );
  }

  const LaneAttachment._(
    this.attachment,
    this.frames,
    this.commander,
    this.creator,
  );

  /// What the lane has said so far, and what the screen shows before the first
  /// frame arrives.
  final Attachment attachment;

  /// One event per frame — or per answered command — that changed what
  /// [attachment] shows.
  final Stream<Attachment> frames;

  /// Issues commands on this attachment's behalf, at its epochs.
  final Commander commander;

  /// Asks the authority for new cards. It carries no epoch and belongs to no
  /// card, because the card it asks for does not exist yet.
  final Creator creator;
}

/// Asks the authority to create a card, and reports exactly what it answered.
///
/// It transmits the identity the sheet bound to the user's form state, encodes
/// the create frame with the generated encoder, and records the answer. It
/// decides nothing at all: it does not look at the invite text it carries, does
/// not judge whether one is valid, and does not infer a direction — every one
/// of those is in the answer it gets back.
class Creator {
  Creator({required CommandSink sink}) : _sink = sink;

  final CommandSink _sink;

  /// Asks for a room this endpoint will be on `direction` of.
  ///
  /// It carries no document. A sender acquires one AFTER the card exists,
  /// under an identity the authority mints — so this is the same request
  /// whichever side the user will be on, and a receiver minting its own room
  /// is finally expressible.
  Future<CreateIntent> mint({
    required String id,
    required LocalDirectionView direction,
  }) =>
      _ask(
        CreateIntent(id: id, kind: CreateKind.mint),
        CreateIntentViewMintRoom(MintRoomView(localDirection: direction)),
      );

  /// Asks to join whatever `invite` turns out to be. The text is passed
  /// through untouched — not trimmed, not sniffed, not measured.
  Future<CreateIntent> join({required String id, required String invite}) => _ask(
        CreateIntent(id: id, kind: CreateKind.join),
        CreateIntentViewJoinRoom(JoinInviteView(invite: CommandSecretString(invite))),
      );

  Future<CreateIntent> _ask(
    CreateIntent request,
    CreateIntentView intent,
  ) async {
    final List<int> frame;
    try {
      frame = createFrame(id: request.id, intent: intent);
    } on CommandContractException catch (error) {
      // The encoder enforces every bound its decoder checks, so a request that
      // does not encode never leaves the process — and is never reported as
      // one whose answer got lost.
      request.fault = IntentFault(FaultOrigin.unsent, error);
      return request;
    }
    final List<int>? reply;
    try {
      reply = await _sink(frame);
    } on PlatformException catch (error) {
      request.fault = IntentFault(
        error.code == hostRejected
            ? FaultOrigin.authorityRefused
            : FaultOrigin.unanswered,
        error,
      );
      return request;
    } on MissingPluginException catch (error) {
      request.fault = IntentFault(FaultOrigin.unanswered, error);
      return request;
    }
    if (reply == null) {
      request.fault =
          const IntentFault(FaultOrigin.unanswered, 'the host answered nothing');
      return request;
    }
    final CommandFrame answer;
    try {
      answer = decodeCommandFrame(utf8.decode(reply));
    } on FormatException catch (error) {
      request.fault = IntentFault(FaultOrigin.unanswered, error);
      return request;
    } on CommandContractException catch (error) {
      request.fault = IntentFault(FaultOrigin.unanswered, error);
      return request;
    }
    // The answer has to be this request's. A create result for another
    // identity, or a body that is not a create result at all, is not an answer
    // to what was asked — and is never dressed as one.
    if (answer.body case final CommandBodyCreateResult body
        when body.value.requestId == request.id) {
      request.outcome = body.value.outcome;
    } else {
      request.fault = const IntentFault(
        FaultOrigin.unanswered,
        'the host answered a request nobody made',
      );
    }
    return request;
  }
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
      return _fault(intent, FaultOrigin.unsent, error);
    }
    final List<int>? reply;
    try {
      reply = await _sink(frame);
    } on PlatformException catch (error) {
      return _fault(
        intent,
        error.code == hostRejected
            ? FaultOrigin.authorityRefused
            : FaultOrigin.unanswered,
        error,
      );
    } on MissingPluginException catch (error) {
      return _fault(intent, FaultOrigin.unanswered, error);
    }
    if (reply == null) {
      return _fault(
        intent,
        FaultOrigin.unanswered,
        'the host answered nothing',
      );
    }
    final CommandFrame answer;
    try {
      answer = decodeCommandFrame(utf8.decode(reply));
    } on FormatException catch (error) {
      return _fault(intent, FaultOrigin.unanswered, error);
    } on CommandContractException catch (error) {
      return _fault(intent, FaultOrigin.unanswered, error);
    }
    if (_attachment.admitCommand(answer) != CommandAdmission.answered) {
      return _fault(
        intent,
        FaultOrigin.unanswered,
        'the host answered a command nobody issued',
      );
    }
    _announce();
  }

  void _fault(CommandIntent intent, FaultOrigin origin, Object error) {
    _attachment.commands.faulted(intent.id, IntentFault(origin, error));
    _announce();
  }
}
