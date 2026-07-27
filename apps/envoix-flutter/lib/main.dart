import 'package:flutter/material.dart';

import 'attachment.dart';
import 'capability.dart';
import 'create.dart';
import 'home.dart';
import 'instrumentation.dart';
import 'lane.dart';
import 'logs.dart';
import 'theme.dart';

void main() => runApp(const EnvoixApp());

/// The Envoix frontend: it observes the host, shows what it says, and asks it
/// for exactly the commands it publishes as admissible.
class EnvoixApp extends StatelessWidget {
  const EnvoixApp({
    super.key,
    this.lane = platformLane,
    this.commands = platformCommands,
    this.picker = platformPickSource,
    this.ask = platformCapability,
  });

  final LaneSource lane;
  final CommandSink commands;
  final SourcePicker picker;

  /// How this app asks the platform for a capability. Injectable so a test
  /// drives every answer without a camera.
  final CapabilityAsk ask;

  @override
  Widget build(BuildContext context) => MaterialApp(
        title: 'Envoix',
        theme: envoixTheme(Brightness.light),
        darkTheme: envoixTheme(Brightness.dark),
        home: Shell(
          lane: lane,
          commands: commands,
          picker: picker,
          ask: ask,
        ),
      );
}

/// UI06 — the shell: one attachment, two destinations onto it.
///
/// Both destinations read the same immutable [Attachment], so there is exactly
/// one subscription to the lane and exactly one thing that rebuilds when a
/// frame arrives.
///
/// Home is the shell's own root, and Logs is a destination within it rather
/// than a pushed route — a second route would need a second listener on a lane
/// that is single-subscription, and a second listener is a second attachment
/// waiting to happen. That makes the system Back button this widget's problem:
/// without [PopScope] it would pop the only route there is and leave the app,
/// which is not "return from a secondary screen".
class Shell extends StatefulWidget {
  const Shell({
    required this.lane,
    required this.commands,
    required this.picker,
    required this.ask,
    super.key,
  });

  final LaneSource lane;
  final CommandSink commands;
  final SourcePicker picker;
  final CapabilityAsk ask;

  @override
  State<Shell> createState() => _ShellState();
}

class _ShellState extends State<Shell> {
  late LaneAttachment _lane = _open();

  LaneAttachment _open() =>
      LaneAttachment.open(widget.lane, commands: widget.commands);

  /// Which destination is on screen. It belongs to the state rather than to the
  /// build, so a frame arriving cannot move the reader off what they are
  /// reading — the whole shell rebuilds on every accepted frame.
  int _destination = 0;

  /// Opens a fresh attachment. Every card's stream restarts at a new epoch, so
  /// the gates of the attachment being replaced can never admit anything again
  /// — which is why [LaneAttachment.open] builds its own rather than taking
  /// one. Replacing the stream is also what cancels the old subscription, so
  /// the platform side hears exactly one detach per re-attach.
  void _attach() => setState(() => _lane = _open());

  void _showHome() => _show(0);

  /// Opens the new-transfer sheet. Asking for a card is not a lifetime verb:
  /// the authority makes one or refuses, and this attachment finds out the same
  /// way it finds out about every other card — on the lane.
  void _newTransfer() {
    showModalBottomSheet<void>(
      context: context,
      isScrollControlled: true,
      // The sheet is the one thing in this app that floats over the whole
      // screen, so it is where the design spends its elevation. Material's own
      // elevation shadow is a different drawing, so the sheet paints the
      // authored one itself and Material's is switched off in the theme.
      backgroundColor: Colors.transparent,
      builder: (BuildContext context) {
        final EnvoixTokens tokens = EnvoixTokens.of(context);
        return DecoratedBox(
          decoration: BoxDecoration(
            color: tokens.surface,
            borderRadius: const BorderRadius.vertical(
              top: Radius.circular(EnvoixShape.radius),
            ),
            boxShadow: tokens.shadow,
          ),
          child: NewTransferSheet(
            creator: _lane.creator,
            picker: widget.picker,
            ask: widget.ask,
          ),
        );
      },
    );
  }

  /// Moving to a destination re-renders what is there, so the report ledger
  /// clears with the move.
  void _show(int destination) {
    forgetRendered();
    setState(() => _destination = destination);
  }

  /// The screen is a function of the lane: [StreamBuilder] rebuilds on every
  /// frame the attachment accepted, so "ingested but never rendered" is not a
  /// state this can be in.
  @override
  Widget build(BuildContext context) => StreamBuilder<Attachment>(
        stream: _lane.frames,
        builder: (BuildContext context, AsyncSnapshot<Attachment> frames) =>
            _shell(frames.error),
      );

  Widget _shell(Object? fault) {
    final bool home = _destination == 0;
    return PopScope(
      // Back out of Logs returns Home; back out of Home is the system's own,
      // so the user is never trapped in the app either.
      canPop: home,
      onPopInvokedWithResult: (bool didPop, Object? result) {
        if (!didPop) {
          _showHome();
        }
      },
      child: Scaffold(
        appBar: AppBar(
          // A gesture-navigation device has no on-screen Back button, so the
          // way out of Logs is also on the screen it is a way out of.
          leading: home
              ? null
              : TextButton(onPressed: _showHome, child: const Text('Back')),
          leadingWidth: home ? null : 72,
          title: Text(home ? 'Envoix' : 'Logs'),
          actions: <Widget>[
            // Re-attaching is an observer action, not a command: it opens a
            // new epoch and touches nothing the host is doing.
            TextButton(onPressed: _attach, child: const Text('Re-attach')),
          ],
        ),
        body: home
            ? HomeScreen(
                attachment: _lane.attachment,
                commander: _lane.commander,
                fault: fault,
              )
            : LogsScreen(attachment: _lane.attachment),
        floatingActionButton: home
            ? Builder(
                builder: (BuildContext context) {
                  reportSheetControl(context, 'new-transfer');
                  return FloatingActionButton.extended(
                    onPressed: _newTransfer,
                    label: const Text('New transfer'),
                    icon: const _Plus(),
                  );
                },
              )
            : null,
        // The tab band is separated from the content by a hairline, the way
        // every other edge in this design is. `NavigationBar` has no border of
        // its own, so the border is drawn around it.
        bottomNavigationBar: DecoratedBox(
          decoration: BoxDecoration(
            border: Border(
              top: BorderSide(
                color: EnvoixTokens.of(context).line,
                width: EnvoixShape.hairline,
              ),
            ),
          ),
          child: NavigationBar(
            selectedIndex: _destination,
            onDestinationSelected: _show,
            destinations: <NavigationDestination>[
              const NavigationDestination(
                icon: _Dot(filled: false),
                selectedIcon: _Dot(filled: true),
                label: 'Transfers',
              ),
              NavigationDestination(
                // Reported rather than computed: the Logs destination is where
                // the build this app is states which deployment it is FOR, and
                // a harness that guessed its coordinate would be tapping
                // arithmetic.
                icon: Builder(
                  builder: (BuildContext context) {
                    reportDestination(context, 'logs');
                    return const _Dot(filled: false);
                  },
                ),
                selectedIcon: const _Dot(filled: true),
                label: 'Logs',
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// The new-transfer affordance's icon, drawn rather than set in a font — the
/// same reason as [_Dot]: no icon font is packaged.
class _Plus extends StatelessWidget {
  const _Plus();

  @override
  Widget build(BuildContext context) => SizedBox(
        width: 16,
        height: 16,
        // The colour the button hands its icons, so the glyph and the label
        // beside it can never disagree about which fill they are sitting on.
        child: CustomPaint(
          painter: _PlusPainter(
            IconTheme.of(context).color ?? EnvoixTokens.of(context).onAccent,
          ),
        ),
      );
}

class _PlusPainter extends CustomPainter {
  const _PlusPainter(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final Paint stroke = Paint()
      ..color = color
      ..strokeWidth = 2;
    canvas.drawLine(
      Offset(size.width / 2, 0),
      Offset(size.width / 2, size.height),
      stroke,
    );
    canvas.drawLine(
      Offset(0, size.height / 2),
      Offset(size.width, size.height / 2),
      stroke,
    );
  }

  @override
  bool shouldRepaint(_PlusPainter old) => old.color != color;
}

/// A destination's indicator, drawn rather than set in a font.
///
/// No icon font is packaged (the release gate's package inventory is an
/// allow-list and a 1.6 MB font is not on it), and a glyph borrowed from the
/// device's own fonts is a coverage gamble on hardware this step cannot test.
/// A drawn dot needs neither. The label beside it is what a screen reader
/// announces.
class _Dot extends StatelessWidget {
  const _Dot({required this.filled});

  final bool filled;

  @override
  Widget build(BuildContext context) {
    // The tab band tints its own icons — accent-strong on the selected tab,
    // muted on the rest — so the dot takes the colour it is handed.
    final Color color =
        IconTheme.of(context).color ?? EnvoixTokens.of(context).muted;
    return Container(
      width: 12,
      height: 12,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: filled ? color : null,
        border: Border.all(color: color, width: 2),
      ),
    );
  }
}
