import 'package:flutter/material.dart';

import 'bindings/envoix_capability.dart';
import 'bindings/envoix_command.dart';
import 'capability.dart';
import 'commands.dart';
import 'instrumentation.dart';
import 'labels.dart';
import 'lane.dart';
import 'theme.dart';

/// UI02 — the new-transfer sheet: send a document, or join an invite.
///
/// It is the only place in this app that asks for a card to exist, and it owns
/// none of what happens next. It does not parse the invite it is given, does
/// not decide which side of the transfer the user will be on, does not mint an
/// id, and does not claim a card was made — every one of those is the answer
/// the authority sends back, rendered here in its own words.
///
/// It no longer asks for a document either. A file used to be chosen HERE,
/// before the card existed — which meant the pick belonged to no acquisition and
/// could be consumed by whichever card asked next. The card is created first
/// now, and it publishes the `pick_source` action carrying the acquisition an
/// offer must name.
class NewTransferSheet extends StatefulWidget {
  const NewTransferSheet({
    required this.creator,
    required this.ask,
    super.key,
  });

  final Creator creator;

  /// Asks the platform for a capability. A platform whose adapter ANSWERS
  /// `unsupported` gets an absent button rather than a broken one; an adapter
  /// that could not be asked at all gets a fault, and keeps its button.
  final CapabilityAsk ask;

  @override
  State<NewTransferSheet> createState() => _NewTransferSheetState();
}

class _NewTransferSheetState extends State<NewTransferSheet> {
  final TextEditingController _invite = TextEditingController();

  /// The request in flight or the answer to the last one.
  CreateIntent? _request;

  bool _scanning = false;

  /// Why the last scan produced no text, or null when none has. It is drawn as
  /// three distinct answers, because cancelling, refusing and having no camera
  /// are three different things to tell a user.
  DeclinedView? _declined;

  /// Set once this platform has ANSWERED that it cannot scan at all. The offer
  /// is then withdrawn rather than repeatedly failed. Only an adapter's own
  /// `unsupported` sets this: a scanner we failed to reach has told us nothing
  /// about the device, and withdrawing on it would hide our own bug behind a
  /// hardware claim.
  bool _scanUnsupported = false;

  /// Set when the adapter could not be asked at all. Distinct from `_declined`
  /// because it is not the platform's answer — it is our own seam failing, so
  /// it reads as a fault and the offer stays.
  bool _scanUnreachable = false;

  String? _sendRequestId;
  String? _joinRequestId;

  @override
  void initState() {
    super.initState();
    // Editing forms a different join intent. The identity is minted lazily on
    // the first tap, then remains bound to these exact bytes for every retry.
    _invite.addListener(_joinChanged);
  }

  void _joinChanged() {
    _joinRequestId = null;
  }

  @override
  void dispose() {
    _invite.removeListener(_joinChanged);
    _invite.dispose();
    super.dispose();
  }

  Future<void> _ask(Future<CreateIntent> Function() request) async {
    setState(() => _request = null);
    final CreateIntent answered = await request();
    if (mounted) {
      setState(() => _request = answered);
    }
  }

  Future<void> _send() {
    final String id = _sendRequestId ??= mintCommandId();
    // The create frame carries NO document, and this screen no longer asks for
    // one first. The card is minted, and it then publishes the acquisition a
    // document must be offered against — so a pick belongs to one card by
    // construction instead of to whoever asked next.
    return _ask(() => widget.creator.mint(
          id: id,
          direction: LocalDirectionView.send,
        ));
  }

  /// Asks the platform to scan an invite and puts whatever it read into the
  /// SAME field a paste fills. This screen does not look at the text, does not
  /// judge it and does not join on its behalf: a scan is another way to fill
  /// the box, never a second way to create a card.
  Future<void> _scan() async {
    setState(() {
      _scanning = true;
      _declined = null;
    });
    final CapabilityAnswer answer = await askToScan(widget.ask);
    if (!mounted) {
      return;
    }
    setState(() {
      _scanning = false;
      switch (answer) {
        case CapabilityProvided(text: final String text):
          _invite.text = text;
          _scanUnreachable = false;
        case CapabilityDeclined(reason: final DeclinedView reason):
          _declined = reason;
          _scanUnreachable = false;
          _scanUnsupported = reason == DeclinedView.unsupported;
        case CapabilityUnavailable():
          // The adapter could not be asked, so nothing answered — which is a
          // fault of ours, not a fact about the device. The offer stays: a
          // withdrawn scanner would make a registration bug look permanent and
          // unfixable to the one person who could report it.
          _scanUnreachable = true;
        // A picker answer to a scan request. The client already refuses a
        // mismatched capability, so reaching here would mean it stopped doing
        // that; treat it the same as no answer at all.
        case SourcePicked():
        case SourcePickFailed():
        case SourceOffered():
          _scanUnreachable = true;
      }
    });
  }

  /// The text goes to the core exactly as typed. Trimming it here would be this
  /// app deciding what an invite is allowed to look like, which is the grammar's
  /// job (`XI02`), and guessing readiness from its shape is the `XI03` bug.
  Future<void> _join() {
    final String id = _joinRequestId ??= mintCommandId();
    return _ask(() => widget.creator.join(id: id, invite: _invite.text));
  }

  @override
  Widget build(BuildContext context) {
    final EnvoixTokens tokens = EnvoixTokens.of(context);
    final CreateIntent? request = _request;
    return Padding(
      padding: EdgeInsets.only(
        left: EnvoixSpace.gutter,
        right: EnvoixSpace.gutter,
        top: EnvoixSpace.gutter,
        bottom: MediaQuery.of(context).viewInsets.bottom + EnvoixSpace.foot,
      ),
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            Text(
              'New transfer',
              style: EnvoixType.sheet.copyWith(color: tokens.text),
            ),
            const SizedBox(height: EnvoixSpace.block),
            const _Section(label: 'Send a file'),
            const SizedBox(height: EnvoixSpace.tight),
            Text(
              'Envoix will ask which file after the transfer is created.',
              style: EnvoixType.subtitle.copyWith(color: tokens.muted),
            ),
            const SizedBox(height: EnvoixSpace.row),
            Builder(
              builder: (BuildContext context) {
                reportSheetControl(context, 'send');
                return SizedBox(
                  // The design's primary action is full width and taller than
                  // the controls around it. That is the whole of its rank.
                  width: double.infinity,
                  height: 46,
                  child: FilledButton(
                    onPressed: _send,
                    child: const Text('Start sending'),
                  ),
                );
              },
            ),
            const Divider(),
            const _Section(label: 'Join an invite'),
            const SizedBox(height: EnvoixSpace.tight),
            Builder(
              builder: (BuildContext context) {
                reportSheetControl(context, 'invite');
                return TextField(
                  controller: _invite,
                  minLines: 1,
                  maxLines: 3,
                  // An invite is machine-written and read back character by
                  // character when it goes wrong, so the field is set in the
                  // face that makes that possible.
                  style: EnvoixType.monoValue.copyWith(color: tokens.text),
                  decoration: const InputDecoration(
                    labelText: 'Invite',
                    helperText: 'Paste or type the invite you were given.',
                  ),
                );
              },
            ),
            if (!_scanUnsupported) ...<Widget>[
              const SizedBox(height: EnvoixSpace.tight),
              Builder(
                builder: (BuildContext context) {
                  reportSheetControl(context, 'scan');
                  return OutlinedButton(
                    onPressed: _scanning ? null : _scan,
                    child: const Text('Scan a code'),
                  );
                },
              ),
            ],
            if (_declined != null) ...<Widget>[
              const SizedBox(height: EnvoixSpace.tight),
              Semantics(
                container: true,
                liveRegion: true,
                label: 'The scanner answered',
                value: scanDeclinedLabel(_declined!),
                child: ExcludeSemantics(
                  child: Text(
                    scanDeclinedLabel(_declined!),
                    style: EnvoixType.subtitle.copyWith(color: tokens.warning),
                  ),
                ),
              ),
            ],
            if (_scanUnreachable) ...<Widget>[
              const SizedBox(height: EnvoixSpace.tight),
              Semantics(
                container: true,
                liveRegion: true,
                // Not "the scanner answered": it did not.
                label: 'The scanner could not be reached',
                value: scanUnreachableLabel,
                child: ExcludeSemantics(
                  child: Text(
                    scanUnreachableLabel,
                    // Danger, not warning — a decline is a normal answer and
                    // this is a defect.
                    style: EnvoixType.subtitle.copyWith(color: tokens.danger),
                  ),
                ),
              ),
            ],
            const SizedBox(height: EnvoixSpace.row),
            // Always enabled. Whether the text is an invite is the core's
            // answer, and a button that greys itself out has already decided.
            Builder(
              builder: (BuildContext context) {
                reportSheetControl(context, 'join');
                return SizedBox(
                  width: double.infinity,
                  height: 46,
                  child: FilledButton(
                    onPressed: _join,
                    child: const Text('Join'),
                  ),
                );
              },
            ),
            if (request != null) ...<Widget>[
              const SizedBox(height: EnvoixSpace.block),
              _Answer(request: request),
            ],
          ],
        ),
      ),
    );
  }
}

/// What the authority said about the last request. A refusal is rendered in its
/// own words, and it is always the authority's — this app never decides that an
/// invite is bad.
class _Answer extends StatelessWidget {
  const _Answer({required this.request});

  final CreateIntent request;

  @override
  Widget build(BuildContext context) {
    // Reported from the widget that DRAWS it, like every other claim this app
    // makes to instrumentation: a harness asserting on an answer the model
    // holds would pass on one the user never saw.
    reportCreate(request);
    final EnvoixTokens tokens = EnvoixTokens.of(context);
    final bool refused = switch (request.outcome) {
      CreateOutcomeViewRefused() => true,
      _ => false,
    };
    // Either fault — refused before sending, or sent with no answer — is a
    // failure to show as one.
    final bool faulted = request.fault != null;
    final bool bad = refused || faulted;
    // Three answers, three grounds. "Asked — no answer yet" is neither good nor
    // bad and must not be dressed as either, so it keeps the neutral well.
    final bool made = !bad &&
        switch (request.outcome) {
          CreateOutcomeViewCreated() => true,
          _ => false,
        };
    final Color? signal = bad
        ? tokens.danger
        : made
            ? tokens.success
            : null;
    return Semantics(
      container: true,
      liveRegion: true,
      label: 'The authority answered',
      value: createAnswerLabel(request),
      child: ExcludeSemantics(
        child: Container(
          width: double.infinity,
          padding: const EdgeInsets.all(EnvoixSpace.card),
          decoration: BoxDecoration(
            color: signal == null ? tokens.bg : tokens.soft(signal),
            borderRadius: EnvoixShape.corner,
            border: Border.all(
              color: signal?.withValues(alpha: 0.35) ?? tokens.line,
            ),
          ),
          child: Text(
            createAnswerLabel(request),
            style: EnvoixType.value.copyWith(color: signal ?? tokens.muted),
          ),
        ),
      ),
    );
  }
}

/// A section's name, in the design's own caps label: tracked out, accent-strong
/// and small, so that it reads as navigation rather than as a heading competing
/// with the sheet's own title.
class _Section extends StatelessWidget {
  const _Section({required this.label});

  final String label;

  @override
  Widget build(BuildContext context) => Text(
        label.toUpperCase(),
        style: EnvoixType.eyebrow.copyWith(
          color: EnvoixTokens.of(context).accentStrong,
        ),
      );
}
