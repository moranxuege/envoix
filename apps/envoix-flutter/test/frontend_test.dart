// The widget half of the F1b proof: a frame becomes a view model becomes text
// on screen, and a frame that is not this attachment's becomes nothing.
//
// Frames are built from the generated view types, whose constructors are
// public — no JSON is written here, and no decoder is reimplemented. The bytes
// half (decoding what a real host actually emits, including hostile input) is
// `flutter_attaches_and_decodes_live_frames`, which runs this same view model
// under `cargo test` over frames a running host produced.

import 'dart:async';
import 'dart:convert';

import 'package:envoix/attachment.dart';
import 'package:envoix/bindings/envoix_read.dart';
import 'package:envoix/main.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

const String card = '00112233445566aa';
const String other = 'ffeeddccbbaa9988';

CardView cardView({int bytes = 1024, String name = 'photo.jpg'}) => CardView(
      identity: IdentityView(
        card: card,
        transfer: 'ab' * 16,
        artifact: 'cd' * 16,
      ),
      direction: DirectionView.send,
      offeredName: name,
      total: 4096,
      state: const ProductStateViewTransferring(),
      quiescence: const QuiescenceViewRunning(
        RunningView(worker: WorkerKindView.attempt),
      ),
      generation: 1,
      phase: PhaseView.transferring,
      bytes: bytes,
      bytesResumed: 0,
      outcome: null,
    );

ReadFrame update(int epoch, CardUpdateKindView kind, {String id = card}) =>
    ReadFrame(
      body: ReadBodyCardUpdate(
        CardUpdateView(epoch: epoch, card: id, kind: kind),
      ),
    );

Attachment attachedAt(int epoch) => Attachment()
  ..admit(update(epoch, CardUpdateKindViewSnapshot(cardView())));

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

    test('a duty is the service\'s work, not the observer\'s', () {
      final Attachment attachment = attachedAt(7);
      attachment.admit(
        update(
          7,
          CardUpdateKindViewCapabilityDuty(
            DutyFrameView(
              duty: DutyView(
                provenance: DutyProvenanceView(
                  card: card,
                  generation: 1,
                  request: 'ef' * 16,
                ),
                kind: DutyKindView.notification,
              ),
              action: CapabilityActionView.postReceipt,
            ),
          ),
        ),
      );
      expect(attachment.cards.single.view?.bytes, 1024);
      expect(attachment.rejected(FrameRejection.contractBreach), 0);
    });
  });

  group('screen', () {
    testWidgets('a decoded card becomes text on screen', (
      WidgetTester tester,
    ) async {
      final CardRow row = attachedAt(7).cards.single;
      await tester.pumpWidget(MaterialApp(home: CardTile(row: row)));

      expect(find.text('photo.jpg'), findsOneWidget);
      expect(
        find.textContaining('card $card'),
        findsOneWidget,
        reason: 'the instrumentation asserts on the id the tile shows',
      );
      expect(find.textContaining('epoch 7'), findsOneWidget);
      expect(find.textContaining('transferring'), findsOneWidget);
      expect(find.textContaining('1024/4096 bytes'), findsOneWidget);
      expect(
        rendered.single,
        'envoix-f1b rendered card=$card epoch=7 status=live',
      );
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
      await tester.pumpWidget(
        MaterialApp(home: CardTile(row: attachment.cards.single)),
      );
      expect(find.textContaining('lagged (capabilityDuty)'), findsOneWidget);
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

      // Bytes the read contract does not accept are the one frame this test
      // can put on the wire without writing JSON by hand; what matters here is
      // that ingesting one CHANGES THE SCREEN.
      lane.add(utf8.encode('not a frame of this contract'));
      await tester.pumpAndSettle();
      expect(find.textContaining('undecodable 1'), findsOneWidget);
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
    });
  });
}
