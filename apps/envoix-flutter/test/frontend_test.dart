// The widget half of the F1b/F1c proof: a frame becomes a view model becomes
// text on screen, and a frame that is not this attachment's becomes nothing.
//
// Frames are built from the generated view types, whose constructors are
// public — no JSON is written here, and no decoder is reimplemented. The bytes
// half (decoding what a real host actually emits, including hostile input) is
// `flutter_attaches_and_decodes_live_frames`, which runs this same view model
// under `cargo test` over frames a running host produced.
//
// The expected words are written out here rather than taken from `labels.dart`:
// a test that asks the code under test what it should say proves only that it
// is consistent with itself.

import 'dart:async';
import 'dart:convert';
import 'dart:math' as math;

import 'package:envoix/attachment.dart';
// Three contracts declare `SourceAcquisitionKeyView`: the generator has no
// cross-schema reference, so the acquisition key is spelled once per schema
// that carries it (EH-20). They are structurally identical by gate, not by
// type, so this file names the READ one and hides the other two.
import 'package:envoix/bindings/envoix_capability.dart'
    hide SourceAcquisitionKeyView;
import 'package:envoix/bindings/envoix_command.dart'
    hide SourceAcquisitionKeyView;
import 'package:envoix/bindings/envoix_read.dart';
import 'package:envoix/capability.dart';
import 'package:envoix/commands.dart';
import 'package:envoix/home.dart';
import 'package:envoix/instrumentation.dart';
import 'package:envoix/labels.dart';
import 'package:envoix/lane.dart';
import 'package:envoix/logs.dart';
import 'package:envoix/main.dart';
import 'package:envoix/qr.dart';
// The theme moved out of `main.dart` when it grew from one seed colour into an
// authored token set; the assertions on it are unchanged.
import 'package:envoix/theme.dart';
import 'package:flutter/foundation.dart' show DebugPrintCallback, debugPrint;
import 'package:flutter/material.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

const String card = '00112233445566aa';
const String other = 'ffeeddccbbaa9988';

CardView cardView({
  int bytes = 1024,
  String name = 'photo.jpg',
  int total = 4096,
  int bytesResumed = 0,
  DirectionView direction = DirectionView.send,
  ProductStateView state = const ProductStateViewTransferring(),
  PhaseView phase = PhaseView.transferring,
  QuiescenceView quiescence = const QuiescenceViewRunning(
    RunningView(worker: WorkerKindView.attempt),
  ),
  OutcomeView? outcome,
  InviteView? invite,
  List<CommandKindView> allowedActions = const <CommandKindView>[
    CommandKindView.pause,
    CommandKindView.cancel,
    CommandKindView.remove,
  ],
  SourceLifecycleView? source,
}) =>
    CardView(
      allowedActions: allowedActions
          .map(CardActionViewCommand.new)
          .toList(growable: false),
      identity: IdentityView(
        card: card,
        transfer: 'ab' * 16,
        artifact: 'cd' * 16,
      ),
      participation: RoomParticipationView.minted,
      direction: direction,
      // The name and total live in the lifecycle now. A sender that has staged
      // its document is the default here because that is the card these tests
      // are mostly about; a card with none passes its own `source`.
      source: source ??
          SourceLifecycleViewReady(SourceReadyView(
            offer: AcceptedSourceOfferView(
              acquisition: SourceAcquisitionKeyView(
                card: card,
                generation: 1,
                request: 'ef' * 16,
              ),
              displayName: name,
              reportedSize: total,
            ),
            content: TransferContentView(offeredName: name, total: total),
          )),
      state: state,
      quiescence: quiescence,
      generation: 1,
      phase: phase,
      bytes: bytes,
      bytesResumed: bytesResumed,
      outcome: outcome,
      invite: invite,
    );

ReadFrame update(int epoch, CardUpdateKindView kind, {String id = card}) =>
    ReadFrame(
      body: ReadBodyCardUpdate(
        CardUpdateView(epoch: epoch, card: id, kind: kind),
      ),
    );

Attachment attachedAt(int epoch) => Attachment()
  ..admit(update(epoch, CardUpdateKindViewSnapshot(cardView())));

/// One card as this attachment last saw it, ready to hand to a tile.
CardRow rowOf(CardView view) =>
    (Attachment()..admit(update(1, CardUpdateKindViewSnapshot(view))))
        .cards
        .single;

/// One typed outcome. Everything but the code has a default, so a sweep over
/// the codes reads as a sweep over the codes.
OutcomeView outcomeOf(
  OutcomeCodeView code, {
  RetryabilityView retry = RetryabilityView.terminal,
  RecoveryView? recovery,
}) =>
    OutcomeView(
      code: code,
      phase: PhaseView.confirming,
      retry: retry,
      recovery: recovery,
      display: 'what the authority said',
    );

EvidenceTimelineView timelineOf({
  DiagnosticsStatusView status = const DiagnosticsStatusViewComplete(),
  List<TimelineEntryView> entries = const <TimelineEntryView>[
    TimelineEntryView(sequence: 1, value: EvidenceValueViewPhase(PhaseView.pairing)),
  ],
}) =>
    EvidenceTimelineView(
      session: const SessionKeyView(card: card, generation: 9),
      status: status,
      entries: entries,
    );

/// A commander whose host refuses everything. Enough to draw the affordances a
/// card publishes; the tests that care what the authority said drive the sink.
Commander commanderOf(
  Attachment attachment, {
  CommandSink sink = _noHost,
}) =>
    Commander(attachment: attachment, sink: sink, announce: () {});

Future<List<int>?> _noHost(List<int> frame) async => null;

/// A sink that records what it was handed and answers with the frame given.
class RecordingSink {
  RecordingSink(this.reply);

  /// The acceptance the fake authority answers with, built from the generated
  /// view types — no JSON is written in this file.
  CommandFrame? Function(SubmitView submit) reply;
  final List<SubmitView> submitted = <SubmitView>[];

  Future<List<int>?> call(List<int> frame) async {
    final CommandFrame decoded = decodeCommandFrame(utf8.decode(frame));
    final CommandBodyIntent body = decoded.body as CommandBodyIntent;
    final FrontendIntentViewCommand intent =
        body.value as FrontendIntentViewCommand;
    submitted.add(intent.value);
    final CommandFrame? answer = reply(intent.value);
    return answer == null ? null : utf8.encode(_encoded(answer));
  }
}

/// A sink for the CREATE half: it records the request the app originated and
/// answers with whatever verdict the test wants the authority to have given.
class RecordingCreateSink {
  RecordingCreateSink(this.reply);

  CommandFrame? Function(CreateView create) reply;
  final List<CreateView> requested = <CreateView>[];

  Future<List<int>?> call(List<int> frame) async {
    final CommandFrame decoded = decodeCommandFrame(utf8.decode(frame));
    final CommandBodyIntent body = decoded.body as CommandBodyIntent;
    final FrontendIntentViewCreate intent =
        body.value as FrontendIntentViewCreate;
    requested.add(intent.value);
    final CommandFrame? answer = reply(intent.value);
    return answer == null ? null : utf8.encode(_encoded(answer));
  }
}

Future<List<int>?> _authorityRefuses(List<int> frame) async {
  throw PlatformException(
    code: hostRejected,
    message: 'the authority refused the intent frame',
  );
}

CommandFrame createdOf(String id, String createdCard) => CommandFrame(
      body: CommandBodyCreateResult(
        CreateResultView(
          outcome: CreateOutcomeViewCreated(CardCreatedView(card: createdCard)),
          requestId: id,
        ),
      ),
    );

CommandFrame refusedOf(String id, CreateRefusalView refusal) => CommandFrame(
      body: CommandBodyCreateResult(
        CreateResultView(
          outcome: CreateOutcomeViewRefused(refusal),
          requestId: id,
        ),
      ),
    );

/// The one place this suite writes contract text, and it has to.
///
/// A frontend cannot encode an acceptance or a completion: BN3b emits no such
/// encoder in any native artifact, which is the guarantee F2a must not weaken.
/// A test that needs one must therefore spell it — and the generated DECODER is
/// what judges the spelling, because every byte written here goes back through
/// `decodeCommandFrame` before anything asserts about it
/// (`the fake authority's frames are the contract's`). Real host-produced
/// acceptance and completion bytes are replayed against this same app model by
/// `flutter_mutating_hot_restart_preserves_cards` under `cargo test`.
String _encoded(CommandFrame frame) {
  final CommandBody body = frame.body;
  final String kind = switch (body) {
    CommandBodyIntent() => 'intent',
    CommandBodyAcceptance() => 'acceptance',
    CommandBodyCompletion() => 'completion',
    CommandBodyCreateResult() => 'create_result',
  };
  return '{"body":{"kind":"$kind","value":${_body(body)}},'
      '"schema":"$commandSchemaId"}';
}

String _body(CommandBody body) => switch (body) {
      CommandBodyIntent(:final FrontendIntentView value) => _intent(value),
      CommandBodyAcceptance(:final CommandAcceptanceView value) =>
        '{"acceptance":${_acceptance(value.acceptance)},'
            '"command_id":"${value.commandId}"}',
      CommandBodyCompletion(:final CommandCompletionView value) =>
        '{"command_id":"${value.commandId}",'
            '"completion":${_completion(value.completion)}}',
      CommandBodyCreateResult(:final CreateResultView value) =>
        '{"outcome":${_createOutcome(value.outcome)},'
            '"request_id":"${value.requestId}"}',
    };

String _intent(FrontendIntentView intent) => switch (intent) {
      FrontendIntentViewCommand(:final SubmitView value) =>
        '{"kind":"command","value":'
            '{"card":"${value.card}","command":"${_command(value.command)}",'
            '"command_id":"${value.commandId}","epoch":${value.epoch}}}',
      FrontendIntentViewCreate(:final CreateView value) =>
        '{"kind":"create","value":'
            '{"intent":${_createIntent(value.intent)},'
            '"request_id":"${value.requestId}"}}',
      FrontendIntentViewSourceOffer(:final SourceOfferView value) =>
        '{"kind":"source_offer","value":'
            '{"display_name":${jsonEncode(value.displayName)},'
            '"key":{"card":"${value.key.card}",'
            '"generation":${value.key.generation},'
            '"request":"${value.key.request}"},'
            '"reported_size":${value.reportedSize}}}',
    };

String _createIntent(CreateIntentView intent) => switch (intent) {
      CreateIntentViewMintRoom(:final MintRoomView value) =>
        '{"kind":"mint_room","value":'
            '{"local_direction":"${value.localDirection.name}"}}',
      CreateIntentViewJoinRoom(:final JoinInviteView value) =>
        '{"kind":"join_room","value":{"invite":${jsonEncode(value.invite.expose())}}}',
    };

String _createOutcome(CreateOutcomeView outcome) => switch (outcome) {
      CreateOutcomeViewCreated(:final CardCreatedView value) =>
        '{"kind":"created","value":{"card":"${value.card}"}}',
      CreateOutcomeViewRefused(:final CreateRefusalView value) =>
        '{"kind":"refused","value":"${_refusal(value)}"}',
    };

String _refusal(CreateRefusalView refusal) => switch (refusal) {
      CreateRefusalView.inviteNotRecognized => 'invite_not_recognized',
      CreateRefusalView.inviteBareRoomCode => 'invite_bare_room_code',
      CreateRefusalView.inviteMalformed => 'invite_malformed',
      CreateRefusalView.inviteTooLong => 'invite_too_long',
      CreateRefusalView.inviteUnsupported => 'invite_unsupported',
      CreateRefusalView.inviteRoleUnsupported => 'invite_role_unsupported',
      CreateRefusalView.nameTooLong => 'name_too_long',
      CreateRefusalView.storageFault => 'storage_fault',
      CreateRefusalView.internal => 'internal',
    };

String _command(CommandView command) => switch (command) {
      CommandView.pause => 'pause',
      CommandView.cancel => 'cancel',
      CommandView.resume => 'resume',
      CommandView.remove => 'remove',
      CommandView.rePickSource => 're_pick_source',
    };

String _acceptance(AcceptanceView acceptance) => switch (acceptance) {
      AcceptanceViewAccepted() => '{"kind":"accepted"}',
      AcceptanceViewDuplicate(:final DispositionView value) =>
        '{"kind":"duplicate","value":${_disposition(value)}}',
      AcceptanceViewConflict(:final CommandView value) =>
        '{"kind":"conflict","value":"${_command(value)}"}',
      AcceptanceViewRejected(:final RejectionView value) =>
        '{"kind":"rejected","value":"${_rejection(value)}"}',
    };

String _completion(CompletionView completion) => switch (completion) {
      CompletionViewCommitted(:final DispositionView value) =>
        '{"kind":"committed","value":${_disposition(value)}}',
      CompletionViewCommitFailed(:final DispositionView value) =>
        '{"kind":"commit_failed","value":${_disposition(value)}}',
      CompletionViewInterrupted() => '{"kind":"interrupted"}',
      CompletionViewInternal() => '{"kind":"internal"}',
    };

String _disposition(DispositionView state) => switch (state) {
      DispositionViewPreparing() => '{"kind":"preparing"}',
      DispositionViewWaiting() => '{"kind":"waiting"}',
      DispositionViewConnecting() => '{"kind":"connecting"}',
      DispositionViewVerifying() => '{"kind":"verifying"}',
      DispositionViewTransferring() => '{"kind":"transferring"}',
      DispositionViewConfirming() => '{"kind":"confirming"}',
      DispositionViewPaused(:final PausedStateView value) =>
        '{"kind":"paused","value":{"origin":"${value.origin.name}"}}',
      DispositionViewUnconfirmed() => '{"kind":"unconfirmed"}',
      DispositionViewCompleted() => '{"kind":"completed"}',
      DispositionViewFailed() => '{"kind":"failed"}',
      DispositionViewCancelled() => '{"kind":"cancelled"}',
    };

String _rejection(RejectionView reason) => switch (reason) {
      RejectionView.unknownCard => 'unknown_card',
      RejectionView.staleEpoch => 'stale_epoch',
      RejectionView.superseded => 'superseded',
      RejectionView.atCapacity => 'at_capacity',
      RejectionView.runtimeStopped => 'runtime_stopped',
      RejectionView.interrupted => 'interrupted',
      RejectionView.internal => 'internal',
    };

CommandFrame acceptanceOf(String id, AcceptanceView acceptance) => CommandFrame(
      body: CommandBodyAcceptance(
        CommandAcceptanceView(commandId: id, acceptance: acceptance),
      ),
    );

CommandFrame completionOf(String id, CompletionView completion) => CommandFrame(
      body: CommandBodyCompletion(
        CommandCompletionView(commandId: id, completion: completion),
      ),
    );

Future<void> pumpTile(
  WidgetTester tester,
  CardRow row, {
  Commander? commander,
  List<CommandIntent> intents = const <CommandIntent>[],
}) =>
    tester.pumpWidget(
      MaterialApp(
        theme: envoixTheme(Brightness.light),
        home: Scaffold(
          body: CardTile(
            row: row,
            commander: commander ?? commanderOf(Attachment()),
            intents: intents,
          ),
        ),
      ),
    );

Future<void> pumpHome(
  WidgetTester tester,
  Attachment attachment, {
  Commander? commander,
}) =>
    tester.pumpWidget(
      MaterialApp(
        theme: envoixTheme(Brightness.light),
        home: Scaffold(
          body: HomeScreen(
            attachment: attachment,
            commander: commander ?? commanderOf(attachment),
          ),
        ),
      ),
    );

Future<void> pumpLogs(WidgetTester tester, Attachment attachment) =>
    tester.pumpWidget(
      MaterialApp(
        theme: envoixTheme(Brightness.light),
        home: Scaffold(body: LogsScreen(attachment: attachment)),
      ),
    );

/// WCAG relative luminance, used to check that the two themes are readable
/// rather than merely different.
double _luminance(Color color) {
  double channel(double value) =>
      value <= 0.03928 ? value / 12.92 : math.pow((value + 0.055) / 1.055, 2.4).toDouble();
  return 0.2126 * channel(color.r) +
      0.7152 * channel(color.g) +
      0.0722 * channel(color.b);
}

double _contrast(Color foreground, Color background) {
  final double first = _luminance(foreground);
  final double second = _luminance(background);
  return (math.max(first, second) + 0.05) / (math.min(first, second) + 0.05);
}

void main() {
  setUp(rendered.clear);

  group('attachment', () {
    test('an epoch opens with its snapshot and then delivers', () {
      final Attachment attachment = attachedAt(7);
      expect(attachment.cards.single.card, card);
      expect(attachment.cards.single.epoch, 7);

      attachment
          .admit(update(7, CardUpdateKindViewProgress(cardView(bytes: 2048))));
      expect(attachment.cards.single.view?.bytes, 2048);
      expect(attachment.rejected(FrameRejection.staleEpoch), 0);
    });

    test('rows are in observation order, and stay put once observed', () {
      // The two ids disagree with the order they arrive in: `other` sorts last
      // and is seen last, so a sort by id and a sort by observation cannot both
      // produce this list.
      final Attachment attachment = Attachment()
        ..admit(update(3, CardUpdateKindViewSnapshot(cardView())))
        ..admit(
          update(
            4,
            CardUpdateKindViewSnapshot(cardView(name: 'later.bin')),
            id: other,
          ),
        );
      expect(
        attachment.cards.map((CardRow row) => row.card),
        <String>[other, card],
      );

      // Within one attachment the order a reader learned is the order it keeps:
      // a card that keeps moving bytes does not jump to the front of the list
      // under their finger.
      attachment
          .admit(update(3, CardUpdateKindViewProgress(cardView(bytes: 2048))));
      expect(
        attachment.cards.map((CardRow row) => row.card),
        <String>[other, card],
      );
    });

    test('a frame from a superseded attachment is dropped, not shown', () {
      final Attachment attachment = attachedAt(7);
      attachment
          .admit(update(8, CardUpdateKindViewProgress(cardView(bytes: 9999))));
      expect(attachment.cards.single.view?.bytes, 1024);
      expect(attachment.rejected(FrameRejection.staleEpoch), 1);
    });

    test('a lag ends the epoch and says what it dropped', () {
      final Attachment attachment = attachedAt(7);
      attachment.admit(
        ReadFrame(
          body: ReadBodyLag(
            LagView(epoch: 7, card: card, missed: LosslessKindView.terminal),
          ),
        ),
      );
      expect(attachment.cards.single.status, StreamStatus.lagged);
      expect(attachment.cards.single.missed, LosslessKindView.terminal);

      // Everything after the lag belongs to an epoch that is over.
      attachment
          .admit(update(7, CardUpdateKindViewProgress(cardView(bytes: 4096))));
      expect(attachment.cards.single.view?.bytes, 1024);
      expect(attachment.rejected(FrameRejection.staleEpoch), 1);
    });

    test('a close ends the epoch', () {
      final Attachment attachment = attachedAt(7);
      attachment.admit(
        ReadFrame(body: ReadBodyClosed(ClosedView(epoch: 7, card: card))),
      );
      expect(attachment.cards.single.status, StreamStatus.closed);
      expect(attachment.cards.single.missed, isNull);
    });

    test('a refused attach is typed truth, not an error', () {
      final Attachment attachment = Attachment();
      attachment.admit(
        ReadFrame(
          body: ReadBodySubscribeRejected(
            SubscribeRejectedView(
              card: other,
              reason: SubscribeRejectionView.runtimeStopped,
            ),
          ),
        ),
      );
      expect(
        attachment.refusals[other],
        SubscribeRejectionView.runtimeStopped,
      );
      expect(attachment.cards, isEmpty);
    });

    test('a second snapshot in one epoch breaks the stream contract', () {
      final Attachment attachment = attachedAt(7);
      attachment
          .admit(update(7, CardUpdateKindViewSnapshot(cardView(bytes: 8888))));
      expect(attachment.rejected(FrameRejection.contractBreach), 1);
      // Counting a breach and then applying it anyway would put the frame on
      // screen exactly as if the stream had kept its contract.
      expect(attachment.cards.single.view?.bytes, 1024);
    });

    test('a duty is observed, and is never card truth', () {
      final Attachment attachment = attachedAt(7);
      attachment.admit(update(7, dutyOf(DutyKindView.notification)));
      expect(attachment.cards.single.view?.bytes, 1024);
      expect(attachment.cards.single.duty?.action,
          CapabilityActionView.postReceipt);
      expect(attachment.rejected(FrameRejection.contractBreach), 0);
    });

    test('an evidence timeline is kept by session, not by epoch', () {
      final Attachment attachment = Attachment();
      // No card stream was ever opened here: evidence carries no epoch, so it
      // is not the gates' business and cannot be dropped as stale.
      expect(attachment.admit(ReadFrame(body: ReadBodyEvidence(timelineOf()))),
          isTrue);
      expect(attachment.timelines.single.session.generation, 9);
      expect(attachment.rejected(FrameRejection.staleEpoch), 0);
    });

    test('a re-stated timeline replaces the one it repeats', () {
      final Attachment attachment = Attachment()
        ..admit(ReadFrame(body: ReadBodyEvidence(timelineOf())))
        ..admit(
          ReadFrame(
            body: ReadBodyEvidence(
              timelineOf(
                entries: const <TimelineEntryView>[
                  TimelineEntryView(
                    sequence: 1,
                    value: EvidenceValueViewPhase(PhaseView.pairing),
                  ),
                  TimelineEntryView(
                    sequence: 2,
                    value: EvidenceValueViewPhase(PhaseView.transferring),
                  ),
                ],
              ),
            ),
          ),
        );
      expect(attachment.timelines, hasLength(1));
      expect(attachment.timelines.single.entries, hasLength(2));
    });
  });

  group('screen', () {
    testWidgets('flutter_readonly_snapshot_render', (
      WidgetTester tester,
    ) async {
      // A realistic sequence: the epoch opens, bytes move, the state changes,
      // and the card ends with a typed outcome.
      final Attachment attachment = Attachment()
        ..admit(update(
          7,
          CardUpdateKindViewSnapshot(
            cardView(
              bytes: 0,
              state: const ProductStateViewPreparing(),
              phase: PhaseView.preparing,
            ),
          ),
        ))
        ..admit(update(7, CardUpdateKindViewProgress(cardView(bytes: 2048))))
        ..admit(update(
          7,
          CardUpdateKindViewState(
            cardView(
              bytes: 4096,
              state: const ProductStateViewConfirming(),
              phase: PhaseView.confirming,
            ),
          ),
        ))
        ..admit(update(
          7,
          CardUpdateKindViewTerminal(
            cardView(
              bytes: 4096,
              state: const ProductStateViewCompleted(),
              phase: PhaseView.confirming,
              quiescence: const QuiescenceViewQuiescent(),
              outcome: const OutcomeView(
                code: OutcomeCodeView.completed,
                phase: PhaseView.confirming,
                retry: RetryabilityView.terminal,
                recovery: null,
                display: 'transfer verified',
              ),
            ),
          ),
        ));

      final SemanticsHandle semantics = tester.ensureSemantics();
      await pumpHome(tester, attachment);

      expect(find.text('photo.jpg'), findsOneWidget);
      expect(find.text('1 transfer'), findsOneWidget);
      expect(find.textContaining('1 Completed'), findsOneWidget);
      expect(find.textContaining('Sending · Completed'), findsOneWidget);
      expect(find.textContaining('4096/4096 bytes'), findsOneWidget);
      expect(find.textContaining('no work in flight'), findsOneWidget);
      expect(find.textContaining('transfer verified'), findsOneWidget);
      expect(
        find.textContaining('card $card'),
        findsOneWidget,
        reason: 'the instrumentation asserts on the id the tile shows',
      );
      expect(find.textContaining('epoch 7'), findsOneWidget);
      // The screen says what it drew, and the last frame is what it drew.
      expect(
        rendered,
        contains('envoix-f1b rendered card=$card epoch=7 status=live'),
      );
      // …including the offer the authority publishes for it, which is the
      // on-device proof that legality is the host's answer and not a rule here.
      expect(
        rendered,
        contains(
          'envoix-f2a card=$card actions=pause,cancel,remove state=Completed',
        ),
      );
      expect(
        tester.getSemantics(find.bySemanticsLabel('Transfer progress')).value,
        '100 percent',
      );
      semantics.dispose();
    });

    testWidgets('every product state the contract can express renders', (
      WidgetTester tester,
    ) async {
      const List<(ProductStateView, String)> states =
          <(ProductStateView, String)>[
        (ProductStateViewPreparing(), 'Preparing'),
        (ProductStateViewWaiting(), 'Waiting for a peer'),
        (ProductStateViewConnecting(), 'Connecting'),
        (ProductStateViewVerifying(), 'Verifying'),
        (ProductStateViewTransferring(), 'Transferring'),
        (ProductStateViewConfirming(), 'Confirming'),
        (
          ProductStateViewPaused(PausedView(origin: PauseOriginView.local)),
          'Paused by you'
        ),
        (
          ProductStateViewPaused(PausedView(origin: PauseOriginView.peer)),
          'Paused by the peer'
        ),
        (
          ProductStateViewPaused(PausedView(origin: PauseOriginView.lost)),
          'Paused after losing the connection'
        ),
        (ProductStateViewUnconfirmed(), 'Delivery unconfirmed'),
        (ProductStateViewCompleted(), 'Completed'),
        (ProductStateViewFailed(), 'Failed'),
        (ProductStateViewCancelled(), 'Cancelled'),
      ];
      expect(
        states.map(((ProductStateView, String) state) => state.$1.runtimeType)
            .toSet(),
        hasLength(11),
        reason: 'all eleven states, with all three pause origins',
      );

      for (final (ProductStateView, String) state in states) {
        await pumpTile(tester, rowOf(cardView(state: state.$1)));
        expect(
          find.textContaining('Sending · ${state.$2}'),
          findsOneWidget,
          reason: '${state.$1.runtimeType} rendered nothing recognisable',
        );
      }
    });

    testWidgets('every direction, phase and quiescence renders', (
      WidgetTester tester,
    ) async {
      const List<(DirectionView, String)> directions =
          <(DirectionView, String)>[
        (DirectionView.send, 'Sending'),
        (DirectionView.receive, 'Receiving'),
      ];
      expect(directions, hasLength(DirectionView.values.length));
      for (final (DirectionView, String) direction in directions) {
        await pumpTile(tester, rowOf(cardView(direction: direction.$1)));
        expect(
          find.textContaining('${direction.$2} · Transferring'),
          findsOneWidget,
        );
      }

      const List<(PhaseView, String)> phases = <(PhaseView, String)>[
        (PhaseView.preparing, 'preparing the source'),
        (PhaseView.pairing, 'pairing with the peer'),
        (PhaseView.authenticating, 'authenticating'),
        (PhaseView.transferring, 'moving bytes'),
        (PhaseView.confirming, 'confirming delivery'),
        (PhaseView.publishing, 'publishing the file'),
        (PhaseView.restoring, 'restoring'),
      ];
      expect(phases, hasLength(PhaseView.values.length));
      for (final (PhaseView, String) phase in phases) {
        await pumpTile(tester, rowOf(cardView(phase: phase.$1)));
        expect(find.textContaining(phase.$2), findsOneWidget);
      }

      const List<(QuiescenceView, String)> resting =
          <(QuiescenceView, String)>[
        (
          QuiescenceViewRunning(RunningView(worker: WorkerKindView.staging)),
          'staging running'
        ),
        (
          QuiescenceViewRetiring(
            RetiringView(
              worker: WorkerKindView.attempt,
              intent: RetirementIntentView.cancel,
            ),
          ),
          'transfer stopping to cancel'
        ),
        (QuiescenceViewQuiescent(), 'no work in flight'),
      ];
      for (final (QuiescenceView, String) rest in resting) {
        await pumpTile(tester, rowOf(cardView(quiescence: rest.$1)));
        expect(find.textContaining(rest.$2), findsOneWidget);
      }
    });

    testWidgets('an outcome is shown with its recovery hint, and only when '
        'the authority gave one', (WidgetTester tester) async {
      final SemanticsHandle semantics = tester.ensureSemantics();
      await pumpTile(tester, rowOf(cardView()));
      expect(find.bySemanticsLabel('Outcome'), findsNothing);

      await pumpTile(
        tester,
        rowOf(
          cardView(
            state: const ProductStateViewFailed(),
            outcome: const OutcomeView(
              code: OutcomeCodeView.sourceUnreadable,
              phase: PhaseView.preparing,
              retry: RetryabilityView.needsUser,
              recovery: RecoveryView.rePickSource,
              display: 'the source went away',
            ),
          ),
        ),
      );
      expect(
        tester.getSemantics(find.bySemanticsLabel('Outcome')).value,
        'The source could not be read while preparing the source — '
        'the source went away — needs you — Pick the source file again',
      );
      semantics.dispose();
    });

    testWidgets('every outcome code, retryability and recovery renders', (
      WidgetTester tester,
    ) async {
      // Exhaustiveness makes an unhandled variant a compile error; it says
      // nothing about a variant handled with the wrong words. Only rendering
      // each one does.
      const List<(OutcomeCodeView, String)> codes = <(OutcomeCodeView, String)>[
        (OutcomeCodeView.completed, 'Completed'),
        (OutcomeCodeView.cancelled, 'Cancelled'),
        (OutcomeCodeView.paused, 'Paused'),
        (OutcomeCodeView.peerLost, 'The peer went away'),
        (OutcomeCodeView.timeout, 'Timed out'),
        (
          OutcomeCodeView.unauthenticated,
          'The peer could not be authenticated'
        ),
        (
          OutcomeCodeView.versionMismatch,
          'The two sides speak different versions'
        ),
        (OutcomeCodeView.storageFault, 'Storage fault'),
        (OutcomeCodeView.publishFailed, 'The file could not be published'),
        (OutcomeCodeView.sourceUnreadable, 'The source could not be read'),
        (OutcomeCodeView.networkUnreachable, 'The network was unreachable'),
        (OutcomeCodeView.internal, 'Internal fault'),
      ];
      expect(codes, hasLength(OutcomeCodeView.values.length));
      for (final (OutcomeCodeView, String) code in codes) {
        await pumpTile(tester, rowOf(cardView(outcome: outcomeOf(code.$1))));
        expect(
          find.textContaining('${code.$2} while confirming delivery'),
          findsOneWidget,
          reason: '${code.$1} rendered nothing recognisable',
        );
      }

      const List<(RetryabilityView, String)> retries =
          <(RetryabilityView, String)>[
        (RetryabilityView.retryable, 'can be retried'),
        (RetryabilityView.terminal, 'final'),
        (RetryabilityView.needsUser, 'needs you'),
      ];
      expect(retries, hasLength(RetryabilityView.values.length));
      for (final (RetryabilityView, String) retry in retries) {
        await pumpTile(
          tester,
          rowOf(cardView(outcome: outcomeOf(
            OutcomeCodeView.timeout,
            retry: retry.$1,
          ))),
        );
        expect(find.textContaining('— ${retry.$2}'), findsOneWidget);
      }

      const List<(RecoveryView, String)> recoveries = <(RecoveryView, String)>[
        (RecoveryView.rePickSource, 'Pick the source file again'),
        (RecoveryView.retryLater, 'Try again later'),
        (RecoveryView.reconnectPeer, 'Reconnect to the peer'),
      ];
      expect(recoveries, hasLength(RecoveryView.values.length));
      for (final (RecoveryView, String) recovery in recoveries) {
        await pumpTile(
          tester,
          rowOf(cardView(outcome: outcomeOf(
            OutcomeCodeView.timeout,
            recovery: recovery.$1,
          ))),
        );
        expect(find.textContaining('— ${recovery.$2}'), findsOneWidget);
      }
    });

    testWidgets('every duty the host can issue is shown as the system\'s work',
        (WidgetTester tester) async {
      const List<(DutyKindView, String)> duties = <(DutyKindView, String)>[
        (DutyKindView.sourceHandle, 'open the source'),
        (DutyKindView.grant, 'hold a permission grant'),
        (DutyKindView.staging, 'stage the file'),
        (DutyKindView.publication, 'publish the file'),
        (DutyKindView.courier, 'carry a receipt'),
        (DutyKindView.foreground, 'keep the service in the foreground'),
        (DutyKindView.notification, 'show a notification'),
        (DutyKindView.lock, 'hold a network lock'),
        (DutyKindView.openShare, 'open or share the file'),
      ];
      expect(duties, hasLength(DutyKindView.values.length));
      for (final (DutyKindView, String) duty in duties) {
        final Attachment attachment = attachedAt(7)
          ..admit(update(7, dutyOf(duty.$1)));
        await pumpTile(tester, attachment.cards.single);
        expect(
          find.textContaining(
            'The host asked the system to post the receipt (${duty.$2})',
          ),
          findsOneWidget,
          reason: '${duty.$1} rendered nothing recognisable',
        );
      }
    });

    testWidgets('a lagged stream says so on the card', (
      WidgetTester tester,
    ) async {
      final Attachment attachment = attachedAt(7);
      attachment.admit(
        ReadFrame(
          body: ReadBodyLag(
            LagView(
              epoch: 7,
              card: card,
              missed: LosslessKindView.capabilityDuty,
            ),
          ),
        ),
      );
      await pumpTile(tester, attachment.cards.single);
      expect(
        find.textContaining('Stopped after dropping a system duty'),
        findsOneWidget,
      );
    });

    testWidgets('a closed stream says so on the card', (
      WidgetTester tester,
    ) async {
      final Attachment attachment = attachedAt(7)
        ..admit(
          ReadFrame(body: ReadBodyClosed(ClosedView(epoch: 7, card: card))),
        );
      await pumpTile(tester, attachment.cards.single);
      expect(find.textContaining('Closed by the host'), findsOneWidget);
    });

    testWidgets('a refused card is on screen with its typed reason', (
      WidgetTester tester,
    ) async {
      final Attachment attachment = Attachment()
        ..admit(
          ReadFrame(
            body: ReadBodySubscribeRejected(
              SubscribeRejectedView(
                card: other,
                reason: SubscribeRejectionView.epochExhausted,
              ),
            ),
          ),
        );
      await pumpHome(tester, attachment);
      expect(find.text('card $other is not observable'), findsOneWidget);
      expect(
        find.text('the card ran out of stream epochs'),
        findsOneWidget,
      );
    });

    testWidgets('an empty lane says there is nothing, not that it broke', (
      WidgetTester tester,
    ) async {
      await tester.pumpWidget(
        EnvoixApp(lane: () => const Stream<List<int>>.empty()),
      );
      await tester.pump();
      expect(find.text('No transfers yet. Use New transfer to start one.'), findsOneWidget);
      expect(find.text('Re-attach'), findsOneWidget);
    });

    testWidgets('a frame off the lane reaches the screen', (
      WidgetTester tester,
    ) async {
      final StreamController<List<int>> lane =
          StreamController<List<int>>.broadcast();
      addTearDown(lane.close);
      await tester.pumpWidget(EnvoixApp(lane: () => lane.stream));
      await tester.pump();
      expect(find.textContaining('undecodable 0'), findsOneWidget);
      expect(find.text('This list may be missing updates.'), findsNothing);

      // Bytes the read contract does not accept are the one frame this test
      // can put on the wire without writing JSON by hand; what matters here is
      // that ingesting one CHANGES THE SCREEN.
      lane.add(utf8.encode('not a frame of this contract'));
      await tester.pumpAndSettle();
      expect(find.textContaining('undecodable 1'), findsOneWidget);
      expect(find.text('This list may be missing updates.'), findsOneWidget);
    });

    testWidgets('re-attaching opens a fresh attachment', (
      WidgetTester tester,
    ) async {
      final StreamController<List<int>> lane =
          StreamController<List<int>>.broadcast();
      addTearDown(lane.close);
      await tester.pumpWidget(EnvoixApp(lane: () => lane.stream));
      await tester.pump();
      lane.add(utf8.encode('not a frame of this contract'));
      await tester.pumpAndSettle();
      expect(find.textContaining('undecodable 1'), findsOneWidget);

      // The host restarts every card at a new epoch, so an attachment that
      // survived the re-attach would be gating the new epoch's frames against
      // the old one's and showing the counters of a lane that is over.
      await tester.tap(find.text('Re-attach'));
      await tester.pump();
      expect(find.textContaining('undecodable 0'), findsOneWidget);
      lane.add(utf8.encode('still not a frame'));
      await tester.pumpAndSettle();
      expect(find.textContaining('undecodable 1'), findsOneWidget);
    });

    testWidgets('a lane that cannot attach shows why', (
      WidgetTester tester,
    ) async {
      await tester.pumpWidget(
        EnvoixApp(
          lane: () => Stream<List<int>>.error(
            Exception('the transfer host is not running'),
          ),
        ),
      );
      await tester.pump();
      expect(find.text('The lane is not delivering'), findsOneWidget);
      expect(
        find.textContaining('the transfer host is not running'),
        findsOneWidget,
      );

      // It arrives while the reader is already on the screen, so it is
      // announced rather than merely drawn.
      final SemanticsHandle semantics = tester.ensureSemantics();
      final SemanticsNode banner =
          tester.getSemantics(find.bySemanticsLabel('The lane is not delivering'));
      expect(banner.value, contains('the transfer host is not running'));
      expect(banner.flagsCollection.isLiveRegion, isTrue);
      semantics.dispose();
    });

    testWidgets('the shell navigates transfers to logs and back', (
      WidgetTester tester,
    ) async {
      await tester.pumpWidget(
        EnvoixApp(lane: () => const Stream<List<int>>.empty()),
      );
      await tester.pump();
      expect(find.text('No transfers yet. Use New transfer to start one.'), findsOneWidget);

      await tester.tap(find.text('Logs'));
      await tester.pumpAndSettle();
      expect(find.text('No evidence yet.'), findsOneWidget);

      await tester.tap(find.text('Transfers'));
      await tester.pumpAndSettle();
      expect(find.text('No transfers yet. Use New transfer to start one.'), findsOneWidget);
    });

    testWidgets('the system back button leaves the logs, not the app', (
      WidgetTester tester,
    ) async {
      // What "leaving the app" looks like from inside a test: the framework
      // finding nothing that wanted the back press and asking the platform to
      // close the activity.
      final List<String> platform = <String>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (MethodCall call) async {
          platform.add(call.method);
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger
            .setMockMethodCallHandler(SystemChannels.platform, null),
      );

      await tester.pumpWidget(
        EnvoixApp(lane: () => const Stream<List<int>>.empty()),
      );
      await tester.pump();
      await tester.tap(find.text('Logs'));
      await tester.pumpAndSettle();
      expect(find.text('No evidence yet.'), findsOneWidget);

      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.text('No transfers yet. Use New transfer to start one.'), findsOneWidget);
      expect(
        platform,
        isNot(contains('SystemNavigator.pop')),
        reason: 'UI06: back out of a secondary screen returns Home',
      );

      // And back from Home is still the system's own, so blocking the press is
      // a redirection rather than a trap.
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(platform, contains('SystemNavigator.pop'));
    });

    testWidgets('a frame arriving does not drag the reader off the logs', (
      WidgetTester tester,
    ) async {
      final StreamController<List<int>> lane =
          StreamController<List<int>>.broadcast();
      addTearDown(lane.close);
      await tester.pumpWidget(EnvoixApp(lane: () => lane.stream));
      await tester.pump();
      await tester.tap(find.text('Logs'));
      await tester.pumpAndSettle();
      expect(find.text('No evidence yet.'), findsOneWidget);

      // The whole shell rebuilds on every accepted frame; if that rebuild threw
      // the tab state away, reading the log during a live transfer would be
      // impossible.
      lane.add(utf8.encode('not a frame of this contract'));
      await tester.pumpAndSettle();
      expect(find.text('No evidence yet.'), findsOneWidget);
    });
  });

  group('logs', () {
    testWidgets('a timeline renders its session and every entry kind', (
      WidgetTester tester,
    ) async {
      final Attachment attachment = Attachment()
        ..admit(
          ReadFrame(
            body: ReadBodyEvidence(
              timelineOf(
                entries: const <TimelineEntryView>[
                  TimelineEntryView(
                    sequence: 4,
                    value: EvidenceValueViewPhase(PhaseView.publishing),
                  ),
                  TimelineEntryView(
                    sequence: 5,
                    value: EvidenceValueViewProgress(
                      EvidenceProgressView(transferred: 512, total: 4096),
                    ),
                  ),
                  TimelineEntryView(
                    sequence: 6,
                    value: EvidenceValueViewOutcome(
                      OutcomeView(
                        code: OutcomeCodeView.timeout,
                        phase: PhaseView.pairing,
                        retry: RetryabilityView.retryable,
                        recovery: RecoveryView.retryLater,
                        display: 'no peer arrived',
                      ),
                    ),
                  ),
                  TimelineEntryView(
                    sequence: 7,
                    value: EvidenceValueViewIdentifier(
                      RedactedIdView(kind: RedactedIdKindView.artifact),
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
      await pumpLogs(tester, attachment);

      expect(find.text('card $card · attempt 9'), findsOneWidget);
      expect(find.text('4. Reached publishing the file'), findsOneWidget);
      expect(find.text('5. Transferred 512 of 4096 bytes'), findsOneWidget);
      expect(
        find.text('6. Timed out while pairing with the peer — no peer arrived'),
        findsOneWidget,
      );
      expect(find.text('7. Recorded a artifact identifier'), findsOneWidget);
      expect(
        rendered.single,
        'envoix-f1c timeline card=$card generation=9 entries=4 '
        'diagnostics=complete',
      );
    });

    testWidgets('every redacted identifier is named by kind, never by value', (
      WidgetTester tester,
    ) async {
      const List<(RedactedIdKindView, String)> kinds =
          <(RedactedIdKindView, String)>[
        (RedactedIdKindView.record, 'card'),
        (RedactedIdKindView.transfer, 'transfer'),
        (RedactedIdKindView.artifact, 'artifact'),
        (RedactedIdKindView.request, 'request'),
      ];
      expect(kinds, hasLength(RedactedIdKindView.values.length));
      for (final (RedactedIdKindView, String) kind in kinds) {
        await pumpLogs(
          tester,
          Attachment()
            ..admit(
              ReadFrame(
                body: ReadBodyEvidence(
                  timelineOf(
                    entries: <TimelineEntryView>[
                      TimelineEntryView(
                        sequence: 1,
                        value: EvidenceValueViewIdentifier(
                          RedactedIdView(kind: kind.$1),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
        );
        expect(
          find.text('1. Recorded a ${kind.$2} identifier'),
          findsOneWidget,
          reason: '${kind.$1} rendered nothing recognisable',
        );
      }
    });

    testWidgets('a degraded timeline is never shown as complete', (
      WidgetTester tester,
    ) async {
      final SemanticsHandle semantics = tester.ensureSemantics();
      final Attachment attachment = Attachment()
        ..admit(
          ReadFrame(
            body: ReadBodyEvidence(
              timelineOf(
                status: const DiagnosticsStatusViewDegraded(
                  DegradedView(droppedEvents: 3),
                ),
              ),
            ),
          ),
        );
      await pumpLogs(tester, attachment);

      const String degraded = 'Incomplete — 3 entries were dropped';
      expect(find.text(degraded), findsOneWidget);
      expect(find.textContaining('Complete'), findsNothing);
      expect(
        tester.getSemantics(find.bySemanticsLabel('Diagnostics')).value,
        degraded,
      );
      expect(
        tester.widget<Text>(find.text(degraded)).style?.color,
        envoixTheme(Brightness.light).colorScheme.error,
        reason: 'a timeline that lost entries must not read like a healthy one',
      );
      expect(
        rendered.single,
        'envoix-f1c timeline card=$card generation=9 entries=1 '
        'diagnostics=degraded:3',
      );
      semantics.dispose();
    });

    testWidgets('the build the host states is shown, every identity of it', (
      WidgetTester tester,
    ) async {
      final Attachment attachment = Attachment()
        ..admit(
          const ReadFrame(
            body: ReadBodyBuildManifest(
              BuildManifestView(
                packageVersion: '0.2.0',
                protocol: ProtocolManifestView(
                  setId: 'envoix/protocol/1',
                  dataAlpn: 'abcd',
                  dataMagic: 'ef01',
                  dataWireVersion: 2,
                ),
                abiSchema: AbiSchemaManifestView(
                  readBindingSchemaId: 'envoix/binding/read/2',
                  commandBindingSchemaId: 'envoix/binding/command/1',
                  capabilityBindingSchemaId: 'envoix/binding/capability/1',
                  evidenceRustAbiId: 'evidence/abi/1',
                  evidenceTimelineSchemaId: 'evidence/timeline/1',
                  mailboxReceiptSchemaId: 'receipt/1',
                  operationEnvelopeSchemaId: 'operation/1',
                ),
                deployment: DeploymentManifestView(
                  environment: 'dev',
                  rendezvousEndpoint: 'abcd@rdz.dev.example:9645',
                  relayUrl: 'https://relay.dev.example:9644',
                ),
              ),
            ),
          ),
        );
      await pumpLogs(tester, attachment);
      expect(find.text('version 0.2.0'), findsOneWidget);
      // The destination is part of the build the app states: an artifact that
      // named no deployment could not have been produced, so the screen that
      // reports what this core IS must say where it talks to.
      expect(find.text('deployment dev'), findsOneWidget);
      expect(
        find.text('rendezvous abcd@rdz.dev.example:9645'),
        findsOneWidget,
      );
      expect(
        find.text('relay https://relay.dev.example:9644'),
        findsOneWidget,
      );
      expect(
        find.text('receipt receipt/1 · operation operation/1'),
        findsOneWidget,
      );
      expect(
        find.text('read envoix/binding/read/2 · command envoix/binding/command/1'),
        findsOneWidget,
      );
    });
  });

  group('accessibility', () {
    testWidgets('progress and status are labelled for a screen reader', (
      WidgetTester tester,
    ) async {
      final SemanticsHandle semantics = tester.ensureSemantics();
      await pumpTile(tester, rowOf(cardView(bytes: 1024)));

      expect(
        tester.getSemantics(find.bySemanticsLabel('Transfer progress')).value,
        '25 percent',
      );
      expect(
        tester.getSemantics(find.bySemanticsLabel('Status')).value,
        'Sending · Transferring',
      );

      // A total of zero makes the fraction unanswerable, and an unanswerable
      // fraction is said so rather than shown as nothing.
      await pumpTile(tester, rowOf(cardView(bytes: 0, total: 0)));
      expect(
        tester.getSemantics(find.bySemanticsLabel('Transfer progress')).value,
        'unknown',
      );
      semantics.dispose();
    });

    testWidgets('a dead stream and the lane\'s own losses are announced', (
      WidgetTester tester,
    ) async {
      final SemanticsHandle semantics = tester.ensureSemantics();
      final Attachment attachment = attachedAt(7)
        ..admit(
          ReadFrame(body: ReadBodyClosed(ClosedView(epoch: 7, card: card))),
        )
        ..ingest(utf8.encode('not a frame'));
      await pumpHome(tester, attachment);

      expect(
        tester.getSemantics(find.bySemanticsLabel('Stream')).value,
        'Closed by the host',
      );
      expect(
        tester
            .getSemantics(
                find.bySemanticsLabel('Frames the app could not use'))
            .value,
        '1',
      );
      semantics.dispose();
    });
  });

  group('theme', () {
    testWidgets('light and dark both render the same screen', (
      WidgetTester tester,
    ) async {
      addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
      for (final Brightness brightness in Brightness.values) {
        tester.platformDispatcher.platformBrightnessTestValue = brightness;
        // A cold start at this brightness, so what is asserted is the app
        // reading the system setting rather than a tree that happened to
        // survive the change.
        await tester.pumpWidget(const SizedBox.shrink());
        await tester.pumpWidget(
          EnvoixApp(lane: () => const Stream<List<int>>.empty()),
        );
        await tester.pump();
        expect(find.text('No transfers yet. Use New transfer to start one.'), findsOneWidget);
        expect(
          Theme.of(tester.element(find.byType(HomeScreen))).colorScheme
              .brightness,
          brightness,
        );
      }
    });

    test('both themes keep body text readable', () {
      for (final Brightness brightness in Brightness.values) {
        final ColorScheme colors = envoixTheme(brightness).colorScheme;
        expect(
          _contrast(colors.onSurface, colors.surface),
          greaterThanOrEqualTo(4.5),
          reason: '$brightness body text fails WCAG AA',
        );
        expect(
          _contrast(colors.error, colors.surface),
          greaterThanOrEqualTo(4.5),
          reason: '$brightness error text fails WCAG AA',
        );
      }
    });
  });

  group('commands', () {
    test('the fake authority\'s frames are the contract\'s', () {
      // Anti-vacuity for every other test in this group: the text this file
      // writes is judged by the generated decoder, arm by arm, so a frame this
      // suite invents cannot be one the app would never really be sent.
      final List<CommandFrame> frames = <CommandFrame>[
        acceptanceOf(id, const AcceptanceViewAccepted()),
        for (final DispositionView state in dispositions)
          acceptanceOf(id, AcceptanceViewDuplicate(state)),
        for (final CommandView command in CommandView.values)
          acceptanceOf(id, AcceptanceViewConflict(command)),
        for (final RejectionView reason in RejectionView.values)
          acceptanceOf(id, AcceptanceViewRejected(reason)),
        for (final DispositionView state in dispositions)
          completionOf(id, CompletionViewCommitted(state)),
        for (final DispositionView state in dispositions)
          completionOf(id, CompletionViewCommitFailed(state)),
        completionOf(id, const CompletionViewInterrupted()),
        completionOf(id, const CompletionViewInternal()),
        CommandFrame(
          body: CommandBodyIntent(
            FrontendIntentViewCommand(
              SubmitView(
                card: card,
                epoch: 7,
                commandId: id,
                command: CommandView.pause,
              ),
            ),
          ),
        ),
        CommandFrame(
          body: CommandBodyIntent(
            FrontendIntentViewCreate(
              CreateView(
                intent: const CreateIntentViewMintRoom(
                  MintRoomView(localDirection: LocalDirectionView.send),
                ),
                requestId: id,
              ),
            ),
          ),
        ),
        CommandFrame(
          body: CommandBodyIntent(
            FrontendIntentViewCreate(
              CreateView(
                intent: const CreateIntentViewJoinRoom(
                  JoinInviteView(invite: CommandSecretString('envoix://invite/v3/AAAA')),
                ),
                requestId: id,
              ),
            ),
          ),
        ),
        createdOf(id, other),
        for (final CreateRefusalView refusal in CreateRefusalView.values)
          refusedOf(id, refusal),
      ];
      expect(frames.length, 67);
      for (final CommandFrame frame in frames) {
        final CommandFrame decoded = decodeCommandFrame(_encoded(frame));
        // Re-spelling what the decoder produced must give the same text: the
        // writer and the contract agree on every field, not just on parsing.
        expect(_encoded(decoded), _encoded(frame));
      }
    });

    test('every command variant round-trips from tap to host', () async {
      for (final CommandKindView kind in CommandKindView.values) {
        final Attachment attachment = attachedAt(11);
        final RecordingSink sink =
            RecordingSink((SubmitView submit) => null);
        final Commander commander =
            commanderOf(attachment, sink: sink.call);
        await commander.issue(attachment.cards.single, commandOf(kind));

        // The frame the host would receive, read back with the generated
        // decoder — the encoder is the generated one, so this is the whole
        // path from a tap to bytes a host decodes.
        expect(sink.submitted.single.command, commandOf(kind));
        expect(sink.submitted.single.card, card);
        expect(sink.submitted.single.epoch, 11);
        expect(sink.submitted.single.commandId, hasLength(32));
        expect(sink.submitted.single.commandId, matches(RegExp(r'^[0-9a-f]+$')));
      }
    });

    test('one identity per intent, and it is never reused', () async {
      final Attachment attachment = attachedAt(11);
      final RecordingSink sink = RecordingSink((SubmitView submit) => null);
      final Commander commander = commanderOf(attachment, sink: sink.call);
      final CardRow row = attachment.cards.single;
      await commander.issue(row, CommandView.pause);
      await commander.issue(row, CommandView.pause);
      expect(
        sink.submitted[0].commandId,
        isNot(sink.submitted[1].commandId),
        reason: 'a second intent is a second command',
      );

      // Asking again about ONE intent re-presents its identity: that is the
      // whole disambiguation, and a fresh id would ask a different question.
      final CommandIntent intent =
          attachment.commands.forCard(card).last;
      await commander.reissue(row, intent);
      expect(sink.submitted[2].commandId, intent.id);
      expect(intent.attempts, 2);
    });

    test('acceptance is never shown as a committed effect', () async {
      final Attachment attachment = attachedAt(11);
      final RecordingSink sink = RecordingSink(
        (SubmitView submit) =>
            acceptanceOf(submit.commandId, const AcceptanceViewAccepted()),
      );
      final Commander commander = commanderOf(attachment, sink: sink.call);
      await commander.issue(attachment.cards.single, CommandView.pause);

      final CommandIntent intent = attachment.commands.forCard(card).single;
      expect(intent.phase, CommandPhase.accepted);
      expect(intentLabel(intent), 'Pause — Accepted — not committed yet');

      // The committed completion arrives separately, on the frame lane.
      attachment.admitCommand(
        completionOf(
          intent.id,
          const CompletionViewCommitted(
            DispositionViewPaused(PausedStateView(origin: PauseCauseView.local)),
          ),
        ),
      );
      expect(intent.phase, CommandPhase.settled);
      expect(
        intentLabel(intent),
        'Pause — Committed — the card is paused by you',
      );
    });

    test('every acceptance verdict and completion outcome is rendered', () {
      final Map<AcceptanceView, String> verdicts = <AcceptanceView, String>{
        const AcceptanceViewAccepted(): 'Accepted — not committed yet',
        const AcceptanceViewDuplicate(DispositionViewCancelled()):
            'Already applied — the card was cancelled',
        // A conflict names the command that owns the identity: the diagnostic
        // detail IS the value of the arm.
        const AcceptanceViewConflict(CommandView.pause):
            'Refused — that request id already belongs to Pause',
        const AcceptanceViewConflict(CommandView.rePickSource):
            'Refused — that request id already belongs to Pick the source '
                'again',
        const AcceptanceViewRejected(RejectionView.staleEpoch):
            'Refused — a newer view of this app is in charge — re-attach and '
                'try again',
        const AcceptanceViewRejected(RejectionView.superseded):
            'Refused — a newer view of this app took over while it was queued',
        const AcceptanceViewRejected(RejectionView.unknownCard):
            'Refused — the host holds no such card',
        const AcceptanceViewRejected(RejectionView.atCapacity):
            'Refused — the host has no room to run this card',
        const AcceptanceViewRejected(RejectionView.runtimeStopped):
            'Refused — the transfer runtime has stopped',
        // Not "Refused": the authority does not know whether it applied, and
        // the words are the completion arm's because it is the same question.
        const AcceptanceViewRejected(RejectionView.interrupted):
            'Unknown — the host died before it could say. Ask again with the '
                'same request to find out.',
        const AcceptanceViewRejected(RejectionView.internal):
            'Refused — an internal fault',
      };
      for (final MapEntry<AcceptanceView, String> verdict in verdicts.entries) {
        expect(acceptanceLabel(verdict.key), verdict.value);
      }
      // Every rejection reason the contract has, so a new one cannot arrive
      // unworded.
      expect(verdicts.length, 4 + RejectionView.values.length);

      final Map<CompletionView, String> outcomes = <CompletionView, String>{
        const CompletionViewCommitted(DispositionViewCompleted()):
            'Committed — the card is completed',
        const CompletionViewCommitFailed(DispositionViewTransferring()):
            'Not durable — it was rolled back and the card stays transferring',
        const CompletionViewInterrupted():
            'Unknown — the host died before it could say. Ask again with the '
                'same request to find out.',
        const CompletionViewInternal(): 'An internal fault ended it',
      };
      for (final MapEntry<CompletionView, String> outcome in outcomes.entries) {
        expect(completionLabel(outcome.key), outcome.value);
      }

      // And every disposition a verdict can carry.
      final List<String> said = <String>[
        for (final DispositionView state in dispositions)
          dispositionLabel(state),
      ];
      expect(said, <String>[
        'preparing',
        'waiting for a peer',
        'connecting',
        'verifying',
        'transferring',
        'confirming',
        'paused by you',
        'paused by the peer',
        'paused after losing the connection',
        'delivery unconfirmed',
        'completed',
        'failed',
        'cancelled',
      ]);
    });

    test('a command from a superseded attachment is refused, and says so',
        () async {
      // The submit carries the epoch that delivered the card's last update, so
      // an attachment the host has replaced sends an epoch the host refuses.
      final Attachment attachment = attachedAt(4);
      final RecordingSink sink = RecordingSink(
        (SubmitView submit) => acceptanceOf(
          submit.commandId,
          const AcceptanceViewRejected(RejectionView.staleEpoch),
        ),
      );
      final Commander commander = commanderOf(attachment, sink: sink.call);
      await commander.issue(attachment.cards.single, CommandView.pause);

      expect(sink.submitted.single.epoch, 4);
      final CommandIntent intent = attachment.commands.forCard(card).single;
      expect(intent.phase, CommandPhase.settled);
      expect(
        intentLabel(intent),
        contains('a newer view of this app is in charge'),
      );
      expect(intent.completion, isNull, reason: 'it never reached a barrier');
    });

    test('interrupted is not guessed at: the same identity asks again',
        () async {
      final Attachment attachment = attachedAt(11);
      final RecordingSink sink = RecordingSink(
        (SubmitView submit) =>
            acceptanceOf(submit.commandId, const AcceptanceViewAccepted()),
      );
      final Commander commander = commanderOf(attachment, sink: sink.call);
      final CardRow row = attachment.cards.single;
      await commander.issue(row, CommandView.cancel);
      final CommandIntent intent = attachment.commands.forCard(card).single;
      attachment.admitCommand(
        completionOf(intent.id, const CompletionViewInterrupted()),
      );
      expect(intentLabel(intent), contains('Unknown'));

      // Duplicate on the re-issue ⇒ it HAD committed.
      sink.reply = (SubmitView submit) => acceptanceOf(
            submit.commandId,
            const AcceptanceViewDuplicate(DispositionViewCancelled()),
          );
      await commander.reissue(row, intent);
      expect(attachment.commands.forCard(card).single.id, intent.id);
      expect(
        intentLabel(intent),
        'Cancel (asked again) — Already applied — the card was cancelled',
      );

      // A fresh acceptance on the re-issue ⇒ it had NOT.
      final Attachment second = attachedAt(11);
      final RecordingSink other = RecordingSink(
        (SubmitView submit) =>
            acceptanceOf(submit.commandId, const AcceptanceViewAccepted()),
      );
      final Commander again = commanderOf(second, sink: other.call);
      await again.issue(second.cards.single, CommandView.cancel);
      final CommandIntent lost = second.commands.forCard(card).single;
      second.admitCommand(
        completionOf(lost.id, const CompletionViewInterrupted()),
      );
      await again.reissue(second.cards.single, lost);
      expect(lost.phase, CommandPhase.accepted);
      expect(
        intentLabel(lost),
        'Cancel (asked again) — Accepted — not committed yet',
      );
    });

    test('an interrupted ACCEPTANCE is unknown too, not a refusal', () async {
      // The actor can die before it answers the acceptance as well as after
      // it. The contract calls both `interrupted` and means the same thing by
      // it: nobody knows whether the command applied. An app that worded this
      // arm as a refusal would be deciding, and would then have no way back.
      final Attachment attachment = attachedAt(11);
      final RecordingSink sink = RecordingSink(
        (SubmitView submit) => acceptanceOf(
          submit.commandId,
          const AcceptanceViewRejected(RejectionView.interrupted),
        ),
      );
      final Commander commander = commanderOf(attachment, sink: sink.call);
      final CardRow row = attachment.cards.single;
      await commander.issue(row, CommandView.cancel);
      final CommandIntent intent = attachment.commands.forCard(card).single;
      expect(intent.phase, CommandPhase.settled);
      expect(intentLabel(intent), contains('Unknown'));
      expect(intentLabel(intent), isNot(contains('Refused')));
      expect(intent.mayDisambiguate, isTrue);

      // And the way out is the documented one: the SAME identity again.
      sink.reply = (SubmitView submit) => acceptanceOf(
            submit.commandId,
            const AcceptanceViewDuplicate(DispositionViewCancelled()),
          );
      await commander.reissue(row, intent);
      expect(attachment.commands.forCard(card).single.id, intent.id);
      expect(
        intentLabel(intent),
        'Cancel (asked again) — Already applied — the card was cancelled',
      );
      expect(intent.mayDisambiguate, isFalse);
    });

    test('a lane that never answers leaves no verdict at all', () async {
      final Attachment attachment = attachedAt(11);
      final Commander commander = commanderOf(attachment);
      await commander.issue(attachment.cards.single, CommandView.pause);
      final CommandIntent intent = attachment.commands.forCard(card).single;
      expect(intent.phase, CommandPhase.undelivered);
      expect(intent.acceptance, isNull);
      expect(intent.completion, isNull);
      expect(intent.fault?.origin, FaultOrigin.unanswered);
      expect(intentLabel(intent), contains('never reached the host'));
    });

    // The two are opposite facts. An intent the encoder refused reached
    // nothing, so "it never reached the host" — true of a lane failure — would
    // be the only thing the user is told about a request that was never made.
    test('an intent the encoder refused is not one whose fate is unknown', () {
      final Attachment attachment = attachedAt(11);
      final CommandIntent intent =
          attachment.commands.open(card, CommandView.pause);
      attachment.commands.faulted(
        intent.id,
        const IntentFault(FaultOrigin.unsent, 'bound at SubmitView.card'),
      );
      expect(intent.phase, CommandPhase.refused);
      expect(intent.unsettled, isFalse);
      expect(intentLabel(intent), contains('not sent'));
      expect(intentLabel(intent), isNot(contains('never reached the host')));
    });

    test('an answer to a command nobody issued is counted, not invented', () {
      final Attachment attachment = attachedAt(11);
      attachment.admitCommand(
        completionOf(id, const CompletionViewCommitted(DispositionViewFailed())),
      );
      expect(attachment.commands.forCard(card), isEmpty);
      expect(attachment.commands.unaddressed, 1);

      // And it is not quietly attached to whatever intent happens to be open:
      // the completion of a command the PREVIOUS attachment issued can still
      // arrive here, and putting it against this one's would be an answer the
      // authority never gave about it.
      final CommandIntent mine =
          attachment.commands.open(card, CommandView.pause);
      attachment.admitCommand(
        completionOf(
          'ffffffffffffffffffffffffffffffff',
          const CompletionViewCommitted(DispositionViewCancelled()),
        ),
      );
      expect(attachment.commands.unaddressed, 2);
      expect(mine.completion, isNull);
      expect(mine.phase, CommandPhase.submitted);

      // Acceptances are addressed exactly the same way, and a gate that swept
      // only one arm of that would pass on the other drifting.
      attachment.admitCommand(
        acceptanceOf(
          'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
          const AcceptanceViewAccepted(),
        ),
      );
      expect(attachment.commands.unaddressed, 3);
      expect(mine.acceptance, isNull);
      expect(mine.phase, CommandPhase.submitted);
    });

    test('an intent body arriving AT the frontend breaks the contract', () {
      final Attachment attachment = attachedAt(11);
      attachment.admitCommand(
        CommandFrame(
          body: CommandBodyIntent(
            FrontendIntentViewCommand(
              SubmitView(
                card: card,
                epoch: 11,
                commandId: id,
                command: CommandView.pause,
              ),
            ),
          ),
        ),
      );
      expect(attachment.rejected(FrameRejection.contractBreach), 1);
      expect(attachment.commands.unaddressed, 0);
    });

    test('a create result arriving on the frame lane answers nobody', () {
      final Attachment attachment = attachedAt(11);
      expect(
        attachment.admitCommand(createdOf(id, other)),
        CommandAdmission.unaddressed,
      );
      expect(attachment.commands.unaddressed, 1);
      expect(attachment.rejected(FrameRejection.contractBreach), 0);
    });

    test('the lane carries both contracts and splits them by schema', () {
      final Attachment attachment = attachedAt(11);
      final String frame = _encoded(
        acceptanceOf(id, const AcceptanceViewAccepted()),
      );
      // Bytes, exactly as they arrive on the frame lane: the read decoder sees
      // a schema it does not speak and hands them to the command decoder.
      attachment.ingest(utf8.encode(frame));
      expect(attachment.commands.unaddressed, 1);
      expect(attachment.rejected(FrameRejection.undecodable), 0);

      // Bytes of neither contract stay undecodable.
      attachment.ingest(utf8.encode('{"schema":"envoix/binding/other/1"}'));
      expect(attachment.rejected(FrameRejection.undecodable), 1);
    });
  });

  group('affordances', () {
    testWidgets('a card offers exactly what the authority admits', (
      WidgetTester tester,
    ) async {
      await pumpTile(
        tester,
        rowOf(
          cardView(
            allowedActions: const <CommandKindView>[
              CommandKindView.resume,
              CommandKindView.remove,
            ],
          ),
        ),
      );
      expect(find.widgetWithText(FilledButton, 'Resume'), findsOneWidget);
      expect(find.widgetWithText(FilledButton, 'Remove'), findsOneWidget);
      expect(find.widgetWithText(FilledButton, 'Pause'), findsNothing);
      expect(find.widgetWithText(FilledButton, 'Cancel'), findsNothing);
    });

    testWidgets('a card the authority admits nothing for offers nothing', (
      WidgetTester tester,
    ) async {
      await pumpTile(
        tester,
        rowOf(cardView(allowedActions: const <CommandKindView>[])),
      );
      expect(find.byType(FilledButton), findsNothing);
      expect(
        find.text('Nothing can be asked of this card right now.'),
        findsOneWidget,
      );
    });

    testWidgets('every command the contract has is offerable and labelled', (
      WidgetTester tester,
    ) async {
      await pumpTile(
        tester,
        rowOf(cardView(allowedActions: CommandKindView.values)),
      );
      for (final String label in <String>[
        'Pause',
        'Cancel',
        'Resume',
        'Remove',
        'Pick the source again',
      ]) {
        expect(find.widgetWithText(FilledButton, label), findsOneWidget);
      }
      expect(find.byType(FilledButton), findsNWidgets(5));
    });

    testWidgets('tapping an affordance submits it, and the screen says so', (
      WidgetTester tester,
    ) async {
      final Attachment attachment = attachedAt(11);
      final RecordingSink sink = RecordingSink(
        (SubmitView submit) =>
            acceptanceOf(submit.commandId, const AcceptanceViewAccepted()),
      );
      final Commander commander = commanderOf(attachment, sink: sink.call);
      await pumpHome(tester, attachment, commander: commander);

      await tester.tap(find.widgetWithText(FilledButton, 'Pause'));
      await tester.pumpAndSettle();
      await pumpHome(tester, attachment, commander: commander);

      expect(sink.submitted.single.command, CommandView.pause);
      expect(find.text('Pause — Accepted — not committed yet'), findsOneWidget);
      // A second tap would be a second command; the affordance waits for the
      // answer to the first.
      expect(
        tester.widget<FilledButton>(find.widgetWithText(FilledButton, 'Pause'))
            .onPressed,
        isNull,
      );
      expect(
        tester.widget<FilledButton>(find.widgetWithText(FilledButton, 'Cancel'))
            .onPressed,
        isNotNull,
        reason: 'the debounce is per command, not a lock on the card',
      );
    });

    testWidgets('an in-flight command does not read like a committed one', (
      WidgetTester tester,
    ) async {
      final Attachment attachment = attachedAt(11);
      final CommandIntent intent =
          attachment.commands.open(card, CommandView.pause);
      attachment.admitCommand(
        acceptanceOf(intent.id, const AcceptanceViewAccepted()),
      );
      await pumpHome(tester, attachment);
      final Container waiting = tester.widget<Container>(
        find
            .ancestor(
              of: find.text('Pause — Accepted — not committed yet'),
              matching: find.byType(Container),
            )
            .first,
      );
      final BoxDecoration inFlight = waiting.decoration! as BoxDecoration;
      expect(inFlight.border, isNotNull, reason: 'colour is not the only cue');

      attachment.admitCommand(
        completionOf(
          intent.id,
          const CompletionViewCommitted(DispositionViewPaused(
            PausedStateView(origin: PauseCauseView.local),
          )),
        ),
      );
      await pumpHome(tester, attachment);
      final Container settled = tester.widget<Container>(
        find
            .ancestor(
              of: find.text('Pause — Committed — the card is paused by you'),
              matching: find.byType(Container),
            )
            .first,
      );
      expect((settled.decoration! as BoxDecoration).border, isNull);
      expect(
        (settled.decoration! as BoxDecoration).color,
        isNot(inFlight.color),
      );
    });

    testWidgets('an interrupted command offers the disambiguation, and runs it',
        (WidgetTester tester) async {
      final Attachment attachment = attachedAt(11);
      final RecordingSink sink = RecordingSink(
        (SubmitView submit) =>
            acceptanceOf(submit.commandId, const AcceptanceViewAccepted()),
      );
      final Commander commander = commanderOf(attachment, sink: sink.call);
      final CommandIntent intent =
          attachment.commands.open(card, CommandView.cancel);
      attachment.admitCommand(
        completionOf(intent.id, const CompletionViewInterrupted()),
      );
      await pumpHome(tester, attachment, commander: commander);
      expect(find.text('Ask again'), findsOneWidget);

      sink.reply = (SubmitView submit) => acceptanceOf(
            submit.commandId,
            const AcceptanceViewDuplicate(DispositionViewCancelled()),
          );
      await tester.tap(find.text('Ask again'));
      await tester.pumpAndSettle();
      await pumpHome(tester, attachment, commander: commander);
      expect(sink.submitted.single.commandId, intent.id);
      expect(
        find.text(
          'Cancel (asked again) — Already applied — the card was cancelled',
        ),
        findsOneWidget,
      );
      expect(find.text('Ask again'), findsNothing);

      // The acceptance arm carries the same unknown, so it must carry the same
      // way out — an intent stranded without one is the app's guess by default.
      final Attachment stranded = attachedAt(11);
      final CommandIntent early =
          stranded.commands.open(card, CommandView.pause);
      stranded.admitCommand(
        acceptanceOf(
          early.id,
          const AcceptanceViewRejected(RejectionView.interrupted),
        ),
      );
      await pumpHome(tester, stranded, commander: commanderOf(stranded));
      expect(find.text('Ask again'), findsOneWidget);
    });

    testWidgets('the command line is instrumented by the widget that drew it', (
      WidgetTester tester,
    ) async {
      final Attachment attachment = attachedAt(11);
      final CommandIntent intent =
          attachment.commands.open(card, CommandView.remove);
      await pumpHome(tester, attachment);
      expect(
        rendered,
        contains(
          'envoix-f2a command card=$card id=${intent.id} '
          'command=remove attempts=1 phase=submitted answer=none',
        ),
      );

      // `settled` alone cannot tell a committed command from a refused one, so
      // the line carries the authority's own answer beside it.
      attachment.admitCommand(
        acceptanceOf(intent.id, const AcceptanceViewAccepted()),
      );
      attachment.admitCommand(
        completionOf(
          intent.id,
          const CompletionViewCommitted(
            DispositionViewPaused(PausedStateView(origin: PauseCauseView.local)),
          ),
        ),
      );
      await pumpHome(tester, attachment);
      expect(
        rendered,
        contains(
          'envoix-f2a command card=$card id=${intent.id} '
          'command=remove attempts=1 phase=settled answer=committed:paused:local',
        ),
      );

      final CommandIntent refused =
          attachment.commands.open(card, CommandView.cancel);
      attachment.admitCommand(
        acceptanceOf(
          refused.id,
          const AcceptanceViewRejected(RejectionView.staleEpoch),
        ),
      );
      await pumpHome(tester, attachment);
      expect(
        rendered,
        contains(
          'envoix-f2a command card=$card id=${refused.id} '
          'command=cancel attempts=1 phase=settled answer=rejected:staleEpoch',
        ),
      );
    });

    testWidgets('a restart forgets every command and keeps the card\'s truth', (
      WidgetTester tester,
    ) async {
      // A hot restart throws the Dart isolate away and runs `main` again; the
      // host, its runtime and the card's durable truth are untouched. What
      // comes back is a NEW attachment at a NEW epoch, re-seeded with the card
      // as the authority now holds it and knowing nothing about any command —
      // because the frontend keeps nothing. (The whole-app version of this,
      // over bytes a real host emitted, is
      // `flutter_mutating_hot_restart_preserves_cards` under `cargo test`.)
      final Attachment before = attachedAt(3);
      final RecordingSink sink = RecordingSink(
        (SubmitView submit) =>
            acceptanceOf(submit.commandId, const AcceptanceViewAccepted()),
      );
      final Commander commander = commanderOf(before, sink: sink.call);
      await pumpHome(tester, before, commander: commander);
      await tester.tap(find.widgetWithText(FilledButton, 'Pause'));
      await tester.pumpAndSettle();
      await pumpHome(tester, before, commander: commander);
      expect(find.text('Pause — Accepted — not committed yet'), findsOneWidget);
      final String issued = sink.submitted.single.commandId;

      final Attachment after = Attachment()
        ..admit(
          update(
            4,
            CardUpdateKindViewSnapshot(
              cardView(
                state: const ProductStateViewPaused(
                  PausedView(origin: PauseOriginView.local),
                ),
                quiescence: const QuiescenceViewQuiescent(),
                allowedActions: const <CommandKindView>[
                  CommandKindView.resume,
                  CommandKindView.cancel,
                  CommandKindView.remove,
                ],
              ),
            ),
          ),
        );
      final Commander restarted = commanderOf(after, sink: sink.call);
      await pumpHome(tester, after, commander: restarted);

      expect(find.text('Pause — Accepted — not committed yet'), findsNothing);
      expect(after.commands.forCard(card), isEmpty);
      expect(find.textContaining('Sending · Paused by you'), findsOneWidget);
      expect(find.widgetWithText(FilledButton, 'Resume'), findsOneWidget);
      expect(find.widgetWithText(FilledButton, 'Pause'), findsNothing);

      // The identity died with the isolate: a new tap is a new intent at the
      // new epoch, and the card's own committed truth is what decided what the
      // old one did.
      await tester.tap(find.widgetWithText(FilledButton, 'Resume'));
      await tester.pumpAndSettle();
      expect(sink.submitted.last.commandId, isNot(issued));
      expect(sink.submitted.last.epoch, 4);
    });
  });

  group('create', () {
    const String invite = 'envoix://invite/v3/eyJ2ZXJzaW9uIjozfQ';

    test('a secret passed to shipped instrumentation cannot reach its log', () {
      const String password = '481920-thistle-zephyr';
      const String fingerprint = '0123456789abcdef';
      final List<String> lines = <String>[];
      final DebugPrintCallback original = debugPrint;
      debugPrint = (String? message, {int? wrapWidth}) {
        lines.add(message ?? '');
      };
      addTearDown(() => debugPrint = original);
      forgetRendered();

      const ReadSecretString secret = ReadSecretString(password);
      expect(secret.toString(), isNot(contains(password)));
      reportInvite(
        card,
        const InviteView(
          code: secret,
          codeFingerprint: fingerprint,
          link: null,
          qr: null,
        ),
      );

      expect(lines.join('\n'), contains('fingerprint=$fingerprint'));
      expect(lines.join('\n'), isNot(contains(password)));
    });

    Future<void> openSheet(
      WidgetTester tester,
      RecordingCreateSink sink, {
      CapabilityAsk ask = _noScanner,
    }) async {
      await tester.pumpWidget(EnvoixApp(
        lane: () => const Stream<List<int>>.empty(),
        commands: sink.call,
        ask: ask,
      ));
      await tester.pumpAndSettle();
      await tester.tap(find.text('New transfer'));
      await tester.pumpAndSettle();
    }

    test('a join carries the text VERBATIM, whatever it looks like', () async {
      final RecordingCreateSink sink =
          RecordingCreateSink((CreateView create) => null);
      final Creator creator = Creator(sink: sink.call);
      // Text with no dash at all, and text that is only a room code: the old
      // app guessed readiness with `contains("-")` (XI03). Both must reach the
      // authority untouched, leading and trailing whitespace included — an app
      // that trimmed would be deciding what an invite may look like.
      const List<String> texts = <String>[
        '',
        'nonsense',
        '000123-amber-brass',
        '   $invite   ',
        'envoix://pair/legacy',
      ];
      for (final String text in texts) {
        await creator.join(id: mintCommandId(), invite: text);
      }
      expect(sink.requested.length, texts.length);
      for (int index = 0; index < texts.length; index += 1) {
        final CreateIntentView intent = sink.requested[index].intent;
        expect(intent, isA<CreateIntentViewJoinRoom>());
        expect(
          (intent as CreateIntentViewJoinRoom).value.invite.expose(),
          texts[index],
          reason: 'the app must not interpret, trim or judge invite text',
        );
      }
    });

    test('a mint carries only which side of the room this endpoint is on',
        () async {
      // Both directions, because a receiver minting its own room is the half of
      // the 2x2 that used to be unreachable — and neither carries a document.
      for (final LocalDirectionView direction in LocalDirectionView.values) {
        final RecordingCreateSink sink =
            RecordingCreateSink((CreateView create) => null);
        await Creator(sink: sink.call)
            .mint(id: mintCommandId(), direction: direction);
        final CreateIntentView intent = sink.requested.single.intent;
        expect(intent, isA<CreateIntentViewMintRoom>());
        expect(
          (intent as CreateIntentViewMintRoom).value.localDirection,
          direction,
        );
      }
    });

    test('the identity bound to each user intent is preserved', () async {
      final RecordingCreateSink sink =
          RecordingCreateSink((CreateView create) => null);
      final Creator creator = Creator(sink: sink.call);
      final List<String> ids =
          List<String>.generate(3, (_) => mintCommandId());
      await creator.join(id: ids[0], invite: invite);
      await creator.join(id: ids[1], invite: invite);
      await creator.mint(id: ids[2], direction: LocalDirectionView.send);
      expect(
        sink.requested.map((CreateView create) => create.requestId),
        ids,
      );
    });

    test('retrying a formed intent reuses its identity', () async {
      final RecordingCreateSink sink =
          RecordingCreateSink((CreateView create) => null);
      final Creator creator = Creator(sink: sink.call);
      final String requestId = mintCommandId();
      await creator.join(id: requestId, invite: invite);
      await creator.join(id: requestId, invite: invite);
      expect(
        sink.requested.map((CreateView create) => create.requestId),
        <String>[requestId, requestId],
      );
    });

    test('the authority decides, and its refusal is what is shown', () async {
      for (final CreateRefusalView refusal in CreateRefusalView.values) {
        final RecordingCreateSink sink = RecordingCreateSink(
          (CreateView create) => refusedOf(create.requestId, refusal),
        );
        // The text is a perfectly well-formed invite; the answer is still the
        // authority's, because the app never looked at it.
        final CreateIntent request = await Creator(sink: sink.call)
            .join(id: mintCommandId(), invite: invite);
        expect(request.outcome, isA<CreateOutcomeViewRefused>());
        expect(request.card, isNull);
        expect(createAnswerLabel(request), startsWith('Refused'));
      }
      // Every refusal has its own words: a shared string would leave a user
      // told "no" with no idea which "no" it was.
      final Set<String> words = <String>{
        for (final CreateRefusalView refusal in CreateRefusalView.values)
          createRefusalLabel(refusal),
      };
      expect(words.length, CreateRefusalView.values.length);
    });

    test('a created answer names the card the authority made', () async {
      final RecordingCreateSink sink = RecordingCreateSink(
        (CreateView create) => createdOf(create.requestId, other),
      );
      final CreateIntent request = await Creator(sink: sink.call)
          .join(id: mintCommandId(), invite: invite);
      expect(request.card, other);
      expect(createAnswerLabel(request), contains(other));
    });

    test('an answer for a different request is not this one\'s', () async {
      final RecordingCreateSink sink = RecordingCreateSink(
        (CreateView create) => createdOf('9' * 32, other),
      );
      final CreateIntent request = await Creator(sink: sink.call)
          .join(id: mintCommandId(), invite: invite);
      expect(request.outcome, isNull);
      expect(request.card, isNull);
      expect(request.fault, isNotNull);
    });

    test('a lane that never answers is not a verdict', () async {
      final RecordingCreateSink sink =
          RecordingCreateSink((CreateView create) => null);
      final CreateIntent request = await Creator(sink: sink.call)
          .join(id: mintCommandId(), invite: invite);
      expect(request.outcome, isNull);
      expect(request.pending, isFalse);
      expect(request.fault?.origin, FaultOrigin.unanswered);
      // Not "it failed": the request may or may not have made a card, and only
      // the card list can say.
      expect(createAnswerLabel(request), contains('If a transfer appears'));
    });

    test('an authority refusal is neither unsent nor an unanswered create',
        () async {
      final CreateIntent request = await Creator(sink: _authorityRefuses)
          .join(id: mintCommandId(), invite: invite);
      expect(request.outcome, isNull);
      expect(request.fault?.origin, FaultOrigin.authorityRefused);
      expect(createAnswerLabel(request), contains('transfer authority'));
      expect(createAnswerLabel(request), contains('Nothing was created'));
      expect(createAnswerLabel(request), isNot(contains('If a transfer appears')));
    });

    test('an ordinary over-byte CJK name reaches the authority and is typed',
        () async {
      final RecordingCreateSink sink = RecordingCreateSink(
        (CreateView create) =>
            refusedOf(create.requestId, CreateRefusalView.nameTooLong),
      );
      final CreateIntent request = await Creator(sink: sink.call).mint(
        id: mintCommandId(),
        direction: LocalDirectionView.send,
      );
      expect(sink.requested, hasLength(1));
      expect(request.outcome, isA<CreateOutcomeViewRefused>());
      expect(
        (request.outcome! as CreateOutcomeViewRefused).value,
        CreateRefusalView.nameTooLong,
      );
      expect(request.fault, isNull);
      expect(createAnswerLabel(request), contains('Rename it'));
    });

    testWidgets('a send is created BEFORE a document is chosen',
        (WidgetTester tester) async {
      final RecordingCreateSink sink = RecordingCreateSink(
        (CreateView create) => createdOf(create.requestId, other),
      );
      await openSheet(tester, sink);
      // No picker on this sheet at all any more. A file used to be chosen here,
      // before the card existed, which meant the pick belonged to no
      // acquisition — the card publishes `pick_source` now, carrying the one an
      // offer must name.
      expect(find.widgetWithText(OutlinedButton, 'Choose a file'), findsNothing);
      final Finder start = find.widgetWithText(FilledButton, 'Start sending');
      expect(
        tester.widget<FilledButton>(start).onPressed,
        isNotNull,
        reason: 'a send no longer waits on a pick that could not be delivered',
      );

      await tester.tap(start);
      await tester.pumpAndSettle();
      final CreateIntentView intent = sink.requested.single.intent;
      expect(
        (intent as CreateIntentViewMintRoom).value.localDirection,
        LocalDirectionView.send,
      );
      expect(find.textContaining('Created'), findsOneWidget);
      expect(find.textContaining(other), findsOneWidget);

      // Re-tapping retries the same formed user intent. The sheet, rather than
      // each transmission, owns the id.
      await tester.tap(start);
      await tester.pumpAndSettle();
      expect(sink.requested.length, 2);
      expect(
        sink.requested[1].requestId,
        sink.requested[0].requestId,
      );
    });

    testWidgets('join is always offered; the answer is the authority\'s',
        (WidgetTester tester) async {
      final RecordingCreateSink sink = RecordingCreateSink(
        (CreateView create) =>
            refusedOf(create.requestId, CreateRefusalView.inviteBareRoomCode),
      );
      await openSheet(tester, sink);
      // Deliberately something the old app would have called "ready" — six
      // digits and two words, with a dash — and padded, so the WHOLE path from
      // the field to the encoder is held to the verbatim rule rather than only
      // the `Creator` the unit test drives.
      await tester.enterText(find.byType(TextField), '  000123-amber-brass  ');
      final Finder join = find.widgetWithText(FilledButton, 'Join');
      expect(tester.widget<FilledButton>(join).onPressed, isNotNull);
      await tester.tap(join);
      await tester.pumpAndSettle();
      expect(
        (sink.requested.single.intent as CreateIntentViewJoinRoom).value.invite.expose(),
        '  000123-amber-brass  ',
      );
      expect(
        find.text('Refused — That is only the room code. '
            'Paste the whole invite.'),
        findsOneWidget,
      );
    });

    test('the source duty is rendered as an observation, not a task', () {
      // The read contract can carry it, so the words for it exist. Nothing in
      // this app acts on a duty; it says the host asked for one. The words
      // changed with the duty: it is taking hold of a document ALREADY chosen,
      // never opening the picker — that is the card's `pick_source` action.
      expect(dutyKindLabel(DutyKindView.sourceHandle), 'open the source');
      expect(
        capabilityActionLabel(CapabilityActionView.acquireSource),
        'take hold of the file you chose',
      );
    });

    testWidgets('a card shows the invite the authority published',
        (WidgetTester tester) async {
      final Attachment attachment = Attachment()
        ..admit(update(
          1,
          CardUpdateKindViewSnapshot(cardView(
            invite: const InviteView(
              code: const ReadSecretString('000123-amber-brass'),
              codeFingerprint: '0123456789abcdef',
              link: ReadSecretString(invite),
              qr: null,
            ),
          )),
        ));
      await pumpHome(tester, attachment);
      expect(find.text('000123-amber-brass'), findsOneWidget);
      expect(find.widgetWithText(TextButton, 'Copy invite'), findsOneWidget);
    });


    testWidgets('a published square is drawn, not described',
        (WidgetTester tester) async {
      // A 3x3 checker: 9 bits, packed MSB-first into 2 bytes.
      final Attachment attachment = Attachment()
        ..admit(update(
          1,
          CardUpdateKindViewSnapshot(cardView(
            invite: const InviteView(
              code: ReadSecretString('000123-amber-brass'),
              codeFingerprint: '0123456789abcdef',
              link: ReadSecretString(invite),
              qr: QrView(width: 3, modules: ReadSecretString('aa80')),
            ),
          )),
        ));
      await pumpHome(tester, attachment);
      expect(find.byType(InviteQr), findsOneWidget);
      expect(find.byType(CustomPaint), findsWidgets);
      expect(
        find.text('Too long to show as a code — share the link instead.'),
        findsNothing,
      );
    });

    testWidgets('an invite past the QR frontier draws an answer, never a blank',
        (WidgetTester tester) async {
      // The frontier is real: this grammar reaches 5481 bytes and QR stops
      // around 2.3 kB, so a card CAN hold a link with no square. The absence
      // must be words a user can act on.
      final Attachment attachment = Attachment()
        ..admit(update(
          1,
          CardUpdateKindViewSnapshot(cardView(
            invite: const InviteView(
              code: ReadSecretString('000123-amber-brass'),
              codeFingerprint: '0123456789abcdef',
              link: ReadSecretString(invite),
              qr: null,
            ),
          )),
        ));
      await pumpHome(tester, attachment);
      expect(
        find.text('Too long to show as a code — share the link instead.'),
        findsOneWidget,
      );
      // And the link is still offered: the fallback the message names exists.
      expect(find.widgetWithText(TextButton, 'Copy invite'), findsOneWidget);
    });

    testWidgets('a QR whose modules do not fill its width is refused, not drawn',
        (WidgetTester tester) async {
      final Attachment attachment = Attachment()
        ..admit(update(
          1,
          CardUpdateKindViewSnapshot(cardView(
            invite: const InviteView(
              code: ReadSecretString('000123-amber-brass'),
              codeFingerprint: '0123456789abcdef',
              link: ReadSecretString(invite),
              // 3x3 needs 9 bits = 2 bytes = 4 hex characters; this is 2.
              qr: QrView(width: 3, modules: ReadSecretString('aa')),
            ),
          )),
        ));
      await pumpHome(tester, attachment);
      expect(
        find.text('This code did not arrive whole — share the link instead.'),
        findsOneWidget,
      );
    });

    testWidgets('a scanned invite fills the SAME field a paste fills',
        (WidgetTester tester) async {
      final RecordingCreateSink sink = RecordingCreateSink(
        (CreateView create) => createdOf(create.requestId, other),
      );
      await openSheet(
        tester,
        sink,
        ask: (CapabilityExchangeView request) async {
          expect(request, isA<CapabilityExchangeViewScanInvite>());
          return const CapabilityProvided(invite);
        },
      );
      await tester.tap(find.widgetWithText(OutlinedButton, 'Scan a code'));
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(FilledButton, 'Join'));
      await tester.pumpAndSettle();

      // The scan produced a JOIN on the ordinary create path, carrying the text
      // verbatim. A scanner that had its own creation route would show up here
      // as a different intent — or as none at all.
      expect(sink.requested, hasLength(1));
      final CreateView create = sink.requested.single;
      final CreateIntentView intent = create.intent;
      expect(intent, isA<CreateIntentViewJoinRoom>());
      expect(
        (intent as CreateIntentViewJoinRoom).value.invite.expose(),
        invite,
      );
    });

    // Cancelling, refusing and having no camera are three ANSWERS. One test
    // per answer, and the set below is asserted distinct: a single "scan
    // failed" would tell a user which of three situations they are in — none.
    const Map<DeclinedView, String> declineAnswers = <DeclinedView, String>{
      DeclinedView.cancelled: 'Scan cancelled.',
      DeclinedView.refused:
          'Envoix needs camera permission to scan. You can still paste the '
              'invite.',
      DeclinedView.unsupported:
          'This device has no camera to scan with. Paste the invite instead.',
    };

    test('the capability seam rides the catalogued channel', () {
      // capability.dart states the whole seam locally so it reads on its own;
      // this is what stops the two spellings from drifting apart.
      expect(commandChannelName, commandChannel);
    });

    test('every decline reads as itself', () {
      expect(declineAnswers.values.toSet(), hasLength(3));
      for (final DeclinedView reason in DeclinedView.values) {
        expect(declineAnswers.containsKey(reason), isTrue,
            reason: '$reason has no words');
        expect(scanDeclinedLabel(reason), declineAnswers[reason]);
      }
    });

    for (final MapEntry<DeclinedView, String> answer
        in declineAnswers.entries) {
      testWidgets('a scan declined as ${answer.key.name} says so',
          (WidgetTester tester) async {
        final RecordingCreateSink sink =
            RecordingCreateSink((CreateView create) => null);
        await openSheet(
          tester,
          sink,
          ask: (CapabilityExchangeView request) async =>
              CapabilityDeclined(answer.key),
        );
        await tester.tap(find.widgetWithText(OutlinedButton, 'Scan a code'));
        await tester.pumpAndSettle();
        expect(find.text(answer.value), findsOneWidget);
        // Nothing was created: a decline is an answer about the scanner, never
        // about the transfer.
        expect(sink.requested, isEmpty);
      });
    }

    testWidgets('a platform with no scanner withdraws the offer',
        (WidgetTester tester) async {
      final RecordingCreateSink sink =
          RecordingCreateSink((CreateView create) => null);
      // The desktop/CLI answer. Declining is first-class: the button is offered
      // until the platform says it cannot, then withdrawn rather than left to
      // fail again.
      await openSheet(tester, sink, ask: _noScanner);
      expect(find.widgetWithText(OutlinedButton, 'Scan a code'), findsOneWidget);
      await tester.tap(find.widgetWithText(OutlinedButton, 'Scan a code'));
      await tester.pumpAndSettle();
      expect(find.widgetWithText(OutlinedButton, 'Scan a code'), findsNothing);
      // Paste still works, which is the whole point of declining well.
      expect(find.widgetWithText(FilledButton, 'Join'), findsOneWidget);
    });

    testWidgets('a scanner that cannot be reached keeps the offer',
        (WidgetTester tester) async {
      final RecordingCreateSink sink =
          RecordingCreateSink((CreateView create) => null);
      // `unavailable` is NOT the desktop answer above. Nothing answered at all,
      // which means our own seam failed — a missing handler, a malformed reply,
      // an adapter answering a different capability. Withdrawing the button on
      // it would dress a bug of ours as a fact about the user's hardware, and
      // do it permanently, since a withdrawn offer is never retried.
      await openSheet(
        tester,
        sink,
        ask: (CapabilityExchangeView request) async =>
            const CapabilityUnavailable('no handler is registered'),
      );
      await tester.tap(find.widgetWithText(OutlinedButton, 'Scan a code'));
      await tester.pumpAndSettle();
      expect(find.widgetWithText(OutlinedButton, 'Scan a code'), findsOneWidget);
      expect(find.text(scanUnreachableLabel), findsOneWidget);
      // And it must never borrow one of the platform's three answers.
      for (final String declined in declineAnswers.values) {
        expect(find.text(declined), findsNothing);
      }
      expect(sink.requested, isEmpty);
    });

    test('a platform with no handler registered is unavailable, not unsupported',
        () async {
      // The seam's own failure mode, at the level that produces it. With no
      // mock handler the channel raises `MissingPluginException`, which is
      // exactly what a build that forgot to register the adapter does.
      TestWidgetsFlutterBinding.ensureInitialized();
      final CapabilityAnswer answer =
          await askToScan(platformCapability);
      expect(answer, isA<CapabilityUnavailable>());
      expect(answer, isNot(isA<CapabilityDeclined>()));
    });

    testWidgets('a channel that no longer spells an invite still shows its code',
        (WidgetTester tester) async {
      final Attachment attachment = Attachment()
        ..admit(update(
          1,
          CardUpdateKindViewSnapshot(cardView(
            invite: const InviteView(
              code: const ReadSecretString('000999-cedar-onyx'),
              codeFingerprint: 'fedcba9876543210',
              link: null,
              qr: null,
            ),
          )),
        ));
      await pumpHome(tester, attachment);
      expect(find.text('000999-cedar-onyx'), findsOneWidget);
      expect(find.widgetWithText(TextButton, 'Copy invite'), findsNothing);
      expect(
        find.text('This card has no shareable link — read out the code.'),
        findsOneWidget,
      );
    });
  });
}

/// A platform with no scanner adapter — the desktop/CLI case. It answers, and
/// what it answers is a first-class part of the contract.
Future<CapabilityAnswer> _noScanner(CapabilityExchangeView request) async =>
    const CapabilityDeclined(DeclinedView.unsupported);

/// The 13 shapes a `DispositionView` can take (11 variants, 3 pause causes).
const List<DispositionView> dispositions = <DispositionView>[
  DispositionViewPreparing(),
  DispositionViewWaiting(),
  DispositionViewConnecting(),
  DispositionViewVerifying(),
  DispositionViewTransferring(),
  DispositionViewConfirming(),
  DispositionViewPaused(PausedStateView(origin: PauseCauseView.local)),
  DispositionViewPaused(PausedStateView(origin: PauseCauseView.peer)),
  DispositionViewPaused(PausedStateView(origin: PauseCauseView.lost)),
  DispositionViewUnconfirmed(),
  DispositionViewCompleted(),
  DispositionViewFailed(),
  DispositionViewCancelled(),
];

/// A command identity of the shape the contract accepts.
const String id = 'a1b2c3d4e5f60718293a4b5c6d7e8f90';

/// A duty frame: the host telling the service to do platform work.
CardUpdateKindView dutyOf(DutyKindView kind) => CardUpdateKindViewCapabilityDuty(
      DutyFrameView(
        duty: DutyView(
          provenance: const DutyProvenanceView(
            card: card,
            generation: 1,
            request: 'efefefefefefefefefefefefefefefef',
          ),
          kind: kind,
        ),
        action: CapabilityActionView.postReceipt,
      ),
    );
