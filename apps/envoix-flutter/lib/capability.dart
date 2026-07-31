import 'dart:convert';

import 'package:flutter/services.dart';

import 'bindings/envoix_capability.dart';
import 'capability_answer.dart';

export 'capability_answer.dart';

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
                SourcePicked(items: picked.items),
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
