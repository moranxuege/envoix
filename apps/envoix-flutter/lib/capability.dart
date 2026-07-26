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
/// `intent` and `pickSource` are, not by the identifier catalog.
const String capabilityMethod = 'capability';

/// What a capability answered. Four outcomes, and three of them are answers
/// rather than failures — a frontend that collapsed them would be unable to say
/// whether to offer the scanner again, send the user to settings, or stop
/// offering it at all.
sealed class CapabilityAnswer {
  const CapabilityAnswer();
}

/// The capability produced text. Opaque here: this app does not parse it, does
/// not validate it and does not decide a role — it hands it to the SAME
/// create-join call the paste field fills, and the grammar in Rust judges it.
final class CapabilityProvided extends CapabilityAnswer {
  const CapabilityProvided(this.text);

  final String text;
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
  CapabilityRequestView capability,
);

/// The real capability client, over the channel the intent lane already uses.
///
/// A platform with no adapter for this method is not an error to show as a
/// crash: `MissingPluginException` means the same thing `unsupported` does, and
/// a desktop build that never registers a handler falls back to paste for free.
Future<CapabilityAnswer> platformCapability(
  CapabilityRequestView capability,
) async {
  final String request = encodeCapabilityFrame(
    CapabilityExchangeView(
      capability: capability,
      step: const CapabilityStepViewRequested(),
    ),
  );
  final Uint8List? reply;
  try {
    reply = await const MethodChannel(commandChannelName)
        .invokeMethod<Uint8List>(
      capabilityMethod,
      Uint8List.fromList(utf8.encode(request)),
    );
  } on MissingPluginException {
    return const CapabilityDeclined(DeclinedView.unsupported);
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
  if (exchange.capability != capability) {
    return const CapabilityUnavailable(
      'the adapter answered a capability nobody asked for',
    );
  }
  return switch (exchange.step) {
    CapabilityStepViewProvided(value: final ScannedTextView text) =>
      CapabilityProvided(text.text.expose()),
    CapabilityStepViewDeclined(value: final DeclinedReasonView declined) =>
      CapabilityDeclined(declined.reason),
    // The frontend's own half echoed back is not an answer to it.
    CapabilityStepViewRequested() => const CapabilityUnavailable(
        'the adapter echoed the request instead of answering it',
      ),
  };
}

/// The channel the capability method shares with the intent lane. Declared here
/// rather than imported from `lane.dart` so this file states the whole seam;
/// `lane.dart` mirrors the catalogued `android.frontend_command_channel`, and
/// `theCapabilitySeamRidesTheCataloguedChannel` pins the two spellings equal.
const String commandChannelName = 'app.envoix.host/frontend-commands';
