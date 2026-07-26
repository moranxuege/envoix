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
import 'package:envoix/bindings/envoix_read.dart';
import 'package:envoix/home.dart';
import 'package:envoix/instrumentation.dart';
import 'package:envoix/logs.dart';
import 'package:envoix/main.dart';
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
}) =>
    CardView(
      identity: IdentityView(
        card: card,
        transfer: 'ab' * 16,
        artifact: 'cd' * 16,
      ),
      direction: direction,
      offeredName: name,
      total: total,
      state: state,
      quiescence: quiescence,
      generation: 1,
      phase: phase,
      bytes: bytes,
      bytesResumed: bytesResumed,
      outcome: outcome,
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

Future<void> pumpTile(WidgetTester tester, CardRow row) => tester.pumpWidget(
      MaterialApp(
        theme: envoixTheme(Brightness.light),
        home: Scaffold(body: CardTile(row: row)),
      ),
    );

Future<void> pumpHome(WidgetTester tester, Attachment attachment) =>
    tester.pumpWidget(
      MaterialApp(
        theme: envoixTheme(Brightness.light),
        home: Scaffold(body: HomeScreen(attachment: attachment)),
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
        rendered.single,
        'envoix-f1b rendered card=$card epoch=7 status=live',
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
      expect(find.text('No transfers yet.'), findsOneWidget);
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
      expect(find.text('No transfers yet.'), findsOneWidget);

      await tester.tap(find.text('Logs'));
      await tester.pumpAndSettle();
      expect(find.text('No evidence yet.'), findsOneWidget);

      await tester.tap(find.text('Transfers'));
      await tester.pumpAndSettle();
      expect(find.text('No transfers yet.'), findsOneWidget);
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
      expect(find.text('No transfers yet.'), findsOneWidget);
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

    testWidgets('the build the host states is shown, trust root and all', (
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
                  evidenceRustAbiId: 'evidence/abi/1',
                  evidenceTimelineSchemaId: 'evidence/timeline/1',
                  mailboxReceiptSchemaId: 'receipt/1',
                  operationEnvelopeSchemaId: 'operation/1',
                ),
                trustRoot: TrustRootViewUnprovisioned(),
              ),
            ),
          ),
        );
      await pumpLogs(tester, attachment);
      expect(find.text('version 0.2.0'), findsOneWidget);
      expect(find.text('trust root not provisioned'), findsOneWidget);
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
        expect(find.text('No transfers yet.'), findsOneWidget);
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
}

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
