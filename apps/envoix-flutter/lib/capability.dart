import 'dart:convert';

import 'package:flutter/services.dart';

import 'bindings/envoix_capability.dart';

/// UI07 — asking the platform for a capability, in the contract every frontend
/// is handed rather than one this app invented.
///
/// The seam is deliberately card-less: a scan happens BEFORE a card exists, so
/// there is no provenance to name and no duty ledger to discharge. What crosses
/// is a generated frame, which is why a SwiftUI frontend implements this by
/// decoding the same contract with `EnvoixCapability.swift` and never reads a
/// line of the Android adapter.

/// The capability method on the command channel. A method name is a slot inside
/// one channel rather than a name in the process-wide channel namespace, so it
/// is pinned by the adapter beside it (`FrontendLane.CAPABILITY`) exactly as
/// `intent` is, not by the identifier catalog.
const String capabilityMethod = 'capability';

/// What a capability answered. Four outcomes, and three of them are answers
/// rather than failures — a frontend that collapsed them would be unable to say
/// whether to offer the scanner again, send the user to settings, or stop
/// offering it at all.
sealed class CapabilityAnswer {
  const CapabilityAnswer();
}

/// The scanner produced text. Opaque here: this app does not parse it, does
/// not validate it and does not decide a role — it hands it to the SAME
/// create-join call the paste field fills, and the grammar in Rust judges it.
final class CapabilityProvided extends CapabilityAnswer {
  const CapabilityProvided(this.text);

  final String text;
}

/// The picker produced a document, described. No handle, no path, no URI: those
/// stay in the adapter, registered under the acquisition this answer names.
final class SourcePicked extends CapabilityAnswer {
  const SourcePicked({required this.displayName, required this.reportedSize});

  final String displayName;

  /// Null when the provider did not say. NOT zero — an empty file reports
  /// zero, and a frontend that could not tell them apart would offer a size it
  /// had invented.
  final int? reportedSize;
}

/// The picker itself could not run. Distinct from `declined`, which is a person
/// answering: showing "you cancelled" for a picker that was never installed
/// blames the user for the platform.
final class SourcePickFailed extends CapabilityAnswer {
  const SourcePickFailed(this.reason);

  final PickSourceFailureView reason;
}

/// The capability was not exercised, and why.
final class CapabilityDeclined extends CapabilityAnswer {
  const CapabilityDeclined(this.reason);

  final DeclinedView reason;
}

/// The adapter could not be asked at all — the platform channel is missing or
/// answered something that was not this contract. Distinct from `declined`,
/// which is an adapter answering properly.
final class CapabilityUnavailable extends CapabilityAnswer {
  const CapabilityUnavailable(this.error);

  final Object error;
}

/// Asks the platform adapter for one capability and reports what it answered.
typedef CapabilityAsk = Future<CapabilityAnswer> Function(
  CapabilityExchangeView request,
);

/// Asks for an invite to be scanned.
Future<CapabilityAnswer> askToScan(CapabilityAsk ask) =>
    ask(const CapabilityExchangeViewScanInvite(
      ScanInviteExchangeView(step: ScanInviteStepViewRequested()),
    ));

/// Asks for a document to be picked FOR one acquisition.
///
/// The key travels because the answer must name it: the authority accepts an
/// offer only for the acquisition it published, so a picker answer that could
/// not say which one it belonged to would be unusable — or, worse, usable for
/// the wrong one.
Future<CapabilityAnswer> askToPickSource(
  CapabilityAsk ask,
  SourceAcquisitionKeyView acquisition,
) =>
    ask(CapabilityExchangeViewPickSource(
      PickSourceExchangeView(
        acquisition: acquisition,
        step: const PickSourceStepViewRequested(),
      ),
    ));

/// The real capability client, over the channel the intent lane already uses.
///
/// A platform with no adapter registered for this method is `unavailable`, NOT
/// `declined(unsupported)`. The two look alike from here and mean opposite
/// things: `unsupported` is an adapter answering that this device has nothing to
/// scan with, while a missing handler is our own build failing to register one.
/// Collapsing them lets a registration bug present itself as a hardware fact —
/// permanently, since a frontend that believes `unsupported` stops asking.
Future<CapabilityAnswer> platformCapability(
  CapabilityExchangeView requested,
) async {
  final String request = encodeCapabilityFrame(requested);
  final Uint8List? reply;
  try {
    reply = await const MethodChannel(commandChannelName)
        .invokeMethod<Uint8List>(
      capabilityMethod,
      Uint8List.fromList(utf8.encode(request)),
    );
  } on MissingPluginException catch (error) {
    return CapabilityUnavailable(error);
  } on PlatformException catch (error) {
    return CapabilityUnavailable(error);
  }
  if (reply == null) {
    return const CapabilityUnavailable('the adapter answered nothing');
  }
  final CapabilityFrame answer;
  try {
    answer = decodeCapabilityFrame(utf8.decode(reply));
  } on FormatException catch (error) {
    return CapabilityUnavailable(error);
  } on CapabilityContractException catch (error) {
    return CapabilityUnavailable(error);
  }
  final CapabilityExchangeView exchange = switch (answer.body) {
    CapabilityBodyExchange(value: final CapabilityExchangeView value) => value,
  };
  // An adapter that answered a different capability answered a question nobody
  // asked. Say so rather than believe it.
  return switch ((requested, exchange)) {
    (
      CapabilityExchangeViewScanInvite(),
      CapabilityExchangeViewScanInvite(value: final ScanInviteExchangeView it)
    ) =>
      switch (it.step) {
        ScanInviteStepViewProvided(value: final ScannedTextView text) =>
          CapabilityProvided(text.text.expose()),
        ScanInviteStepViewDeclined(value: final DeclinedReasonView declined) =>
          CapabilityDeclined(declined.reason),
        // The frontend's own half echoed back is not an answer to it.
        ScanInviteStepViewRequested() => const CapabilityUnavailable(
            'the adapter echoed the request instead of answering it',
          ),
      },
    (
      CapabilityExchangeViewPickSource(value: final PickSourceExchangeView ask),
      CapabilityExchangeViewPickSource(value: final PickSourceExchangeView it)
    ) =>
      // The acquisition is checked before the payload is believed. An answer
      // for a different one would otherwise become an offer for whichever ask
      // happened to be outstanding — the exact ownership defect the key exists
      // to close.
      !_sameAcquisition(ask.acquisition, it.acquisition)
          ? const CapabilityUnavailable(
              'the adapter answered a different acquisition',
            )
          : switch (it.step) {
              PickSourceStepViewProvided(value: final PickedSourceView picked) =>
                SourcePicked(
                  displayName: picked.displayName,
                  reportedSize: picked.reportedSize,
                ),
              PickSourceStepViewDeclined(
                value: final DeclinedReasonView declined
              ) =>
                CapabilityDeclined(declined.reason),
              PickSourceStepViewFailed(
                value: final PickSourceFailureReasonView failed
              ) =>
                SourcePickFailed(failed.reason),
              PickSourceStepViewRequested() => const CapabilityUnavailable(
                  'the adapter echoed the request instead of answering it',
                ),
            },
    _ => const CapabilityUnavailable(
        'the adapter answered a capability nobody asked for',
      ),
  };
}

bool _sameAcquisition(
  SourceAcquisitionKeyView asked,
  SourceAcquisitionKeyView answered,
) =>
    asked.card == answered.card &&
    asked.generation == answered.generation &&
    asked.request == answered.request;

/// The channel the capability method shares with the intent lane. Declared here
/// rather than imported from `lane.dart` so this file states the whole seam;
/// `lane.dart` mirrors the catalogued `android.frontend_command_channel`, and
/// `theCapabilitySeamRidesTheCataloguedChannel` pins the two spellings equal.
const String commandChannelName = 'app.envoix.host/frontend-commands';
