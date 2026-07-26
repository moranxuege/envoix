import 'package:flutter/material.dart';

import 'bindings/envoix_capability.dart';
import 'bindings/envoix_command.dart';
import 'capability.dart';
import 'commands.dart';
import 'instrumentation.dart';
import 'labels.dart';
import 'lane.dart';

/// UI02 — the new-transfer sheet: send a document, or join an invite.
///
/// It is the only place in this app that asks for a card to exist, and it owns
/// none of what happens next. It does not parse the invite it is given, does
/// not decide which side of the transfer the user will be on, does not mint an
/// id, and does not claim a card was made — every one of those is the answer
/// the authority sends back, rendered here in its own words.
///
/// The one thing this screen does decide is whether the platform has granted a
/// source yet, and that is not a judgement about a transfer: there is no card
/// to have a rule about, only a picker that has or has not been used.
class NewTransferSheet extends StatefulWidget {
  const NewTransferSheet({
    required this.creator,
    required this.picker,
    required this.ask,
    super.key,
  });

  final Creator creator;
  final SourcePicker picker;

  /// Asks the platform for a capability. A platform with no adapter for one
  /// answers `unsupported`, which this screen draws as an absent button rather
  /// than a broken one.
  final CapabilityAsk ask;

  @override
  State<NewTransferSheet> createState() => _NewTransferSheetState();
}

class _NewTransferSheetState extends State<NewTransferSheet> {
  final TextEditingController _invite = TextEditingController();

  /// What the platform said about the document the user picked. Metadata, and
  /// the app holds nothing else about it — no URI, no handle, no stream.
  PickedSource? _source;

  /// The request in flight or the answer to the last one.
  CreateIntent? _request;

  bool _picking = false;
  bool _scanning = false;

  /// Why the last scan produced no text, or null when none has. It is drawn as
  /// three distinct answers, because cancelling, refusing and having no camera
  /// are three different things to tell a user.
  DeclinedView? _declined;

  /// Set once this platform has said it cannot scan at all. The offer is then
  /// withdrawn rather than repeatedly failed.
  bool _scanUnsupported = false;

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

  Future<void> _pick() async {
    setState(() => _picking = true);
    PickedSource? granted;
    try {
      granted = await widget.picker();
    } finally {
      if (mounted) {
        setState(() {
          _picking = false;
          if (granted != null) {
            _source = granted;
            // A newly chosen source is a new user intent, even when its
            // provider metadata happens to equal the previous pick.
            _sendRequestId = null;
          }
        });
      }
    }
  }

  Future<void> _ask(Future<CreateIntent> Function() request) async {
    setState(() => _request = null);
    final CreateIntent answered = await request();
    if (mounted) {
      setState(() => _request = answered);
    }
  }

  Future<void> _send() {
    final PickedSource? source = _source;
    if (source == null) {
      return Future<void>.value();
    }
    final String id = _sendRequestId ??= mintCommandId();
    return _ask(() => widget.creator.send(
          id: id,
          displayName: source.displayName,
          sizeBytes: source.sizeBytes,
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
    final CapabilityAnswer answer =
        await widget.ask(CapabilityRequestView.scanInvite);
    if (!mounted) {
      return;
    }
    setState(() {
      _scanning = false;
      switch (answer) {
        case CapabilityProvided(text: final String text):
          _invite.text = text;
        case CapabilityDeclined(reason: final DeclinedView reason):
          _declined = reason;
          _scanUnsupported = reason == DeclinedView.unsupported;
        case CapabilityUnavailable():
          // The adapter could not be asked. Not a decline — nothing answered.
          _scanUnsupported = true;
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
    final ThemeData theme = Theme.of(context);
    final PickedSource? source = _source;
    final CreateIntent? request = _request;
    return Padding(
      padding: EdgeInsets.only(
        left: 16,
        right: 16,
        top: 16,
        bottom: MediaQuery.of(context).viewInsets.bottom + 16,
      ),
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            Text('New transfer', style: theme.textTheme.titleLarge),
            const SizedBox(height: 16),
            Text('Send a file', style: theme.textTheme.titleMedium),
            const SizedBox(height: 8),
            Row(
              children: <Widget>[
                Builder(
                  builder: (BuildContext context) {
                    reportSheetControl(context, 'choose');
                    return OutlinedButton(
                      onPressed: _picking ? null : _pick,
                      child: Text(
                        source == null ? 'Choose a file' : 'Choose another',
                      ),
                    );
                  },
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Semantics(
                    container: true,
                    label: 'Chosen file',
                    value: source == null
                        ? 'nothing chosen yet'
                        : '${source.displayName}, ${source.sizeBytes} bytes',
                    child: ExcludeSemantics(
                      child: Text(
                        source == null
                            ? 'No file chosen yet.'
                            : '${source.displayName} · ${source.sizeBytes} bytes',
                        style: theme.textTheme.bodySmall,
                      ),
                    ),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 8),
            Builder(
              builder: (BuildContext context) {
                reportSheetControl(context, 'send');
                return FilledButton(
                  onPressed: source == null ? null : _send,
                  child: const Text('Start sending'),
                );
              },
            ),
            const Divider(height: 32),
            Text('Join an invite', style: theme.textTheme.titleMedium),
            const SizedBox(height: 8),
            Builder(
              builder: (BuildContext context) {
                reportSheetControl(context, 'invite');
                return TextField(
                  controller: _invite,
                  minLines: 1,
                  maxLines: 3,
                  decoration: const InputDecoration(
                    labelText: 'Invite',
                    helperText: 'Paste or type the invite you were given.',
                    border: OutlineInputBorder(),
                  ),
                );
              },
            ),
            if (!_scanUnsupported) ...<Widget>[
              const SizedBox(height: 8),
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
              const SizedBox(height: 8),
              Semantics(
                container: true,
                liveRegion: true,
                label: 'The scanner answered',
                value: scanDeclinedLabel(_declined!),
                child: ExcludeSemantics(
                  child: Text(
                    scanDeclinedLabel(_declined!),
                    style: theme.textTheme.bodySmall,
                  ),
                ),
              ),
            ],
            const SizedBox(height: 8),
            // Always enabled. Whether the text is an invite is the core's
            // answer, and a button that greys itself out has already decided.
            Builder(
              builder: (BuildContext context) {
                reportSheetControl(context, 'join');
                return FilledButton(
                  onPressed: _join,
                  child: const Text('Join'),
                );
              },
            ),
            if (request != null) ...<Widget>[
              const SizedBox(height: 16),
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
    final ColorScheme colors = Theme.of(context).colorScheme;
    final bool refused = switch (request.outcome) {
      CreateOutcomeViewRefused() => true,
      _ => false,
    };
    // Either fault — refused before sending, or sent with no answer — is a
    // failure to show as one.
    final bool faulted = request.fault != null;
    return Semantics(
      container: true,
      liveRegion: true,
      label: 'The authority answered',
      value: createAnswerLabel(request),
      child: ExcludeSemantics(
        child: Container(
          width: double.infinity,
          padding: const EdgeInsets.all(12),
          decoration: BoxDecoration(
            color: refused || faulted
                ? colors.errorContainer
                : colors.surfaceContainerHighest,
            borderRadius: BorderRadius.circular(8),
          ),
          child: Text(createAnswerLabel(request)),
        ),
      ),
    );
  }
}
