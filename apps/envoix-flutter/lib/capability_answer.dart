import 'bindings/envoix_capability.dart';
import 'bindings/envoix_command.dart' show SourceOfferOutcomeView;

/// The capability vocabulary, with no transport in it.
///
/// Split from `capability.dart` because that file owns the platform channel and
/// therefore imports Flutter — and the generated-contract replays that prove
/// this app's decoding run under plain `dart`, with no Flutter to import. A
/// vocabulary a headless harness cannot name is a vocabulary those harnesses
/// cannot check.

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

/// The picker produced a SELECTION, described. No handle, no path, no URI:
/// those stay in the adapter, registered under the acquisition this answer names
/// and each item's position in this list.
///
/// A list rather than one document, because what a card sends may be produced
/// from what was picked rather than be one of them. One document is the
/// one-element case, and the order is the person's — it decides how an archive
/// is written.
///
/// Each item's `reportedSize` is null when the provider did not say. NOT zero —
/// an empty file reports zero, and a frontend that could not tell them apart
/// would offer a size it had invented.
final class SourcePicked extends CapabilityAnswer {
  const SourcePicked({required this.items});

  final List<PickedItemView> items;
}

/// The picker itself could not run. Distinct from `declined`, which is a person
/// answering: showing "you cancelled" for a picker that was never installed
/// blames the user for the platform.
final class SourcePickFailed extends CapabilityAnswer {
  const SourcePickFailed(this.reason);

  final PickSourceFailureView reason;
}

/// A picked document reached the AUTHORITY, and this is what it said.
///
/// The last step of the exchange rather than a capability answer of its own,
/// but it belongs in this vocabulary: the caller asked one question — "put a
/// document on this card" — and every way that can end is one of these.
final class SourceOffered extends CapabilityAnswer {
  const SourceOffered(this.outcome);

  final SourceOfferOutcomeView outcome;
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

