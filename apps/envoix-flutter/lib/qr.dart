import 'package:flutter/material.dart';

import 'bindings/envoix_read.dart';

/// UI06 — the invite as the square a camera reads.
///
/// This app does not ENCODE a QR: the invite grammar in Rust does, and publishes
/// the modules on the read contract. What is left here is drawing, which is the
/// half that is genuinely a frontend's — scale, quiet zone and colour are
/// decisions the contract deliberately does not make. That split is why showing
/// a code costs this app no dependency and would cost a SwiftUI app none either.
class InviteQr extends StatelessWidget {
  const InviteQr({required this.qr, super.key});

  /// The published square, or null when this invite has none.
  final QrView? qr;

  @override
  Widget build(BuildContext context) {
    final QrView? square = qr;
    if (square == null) {
      return const _NoSquare();
    }
    final _Modules? modules = _Modules.parse(square);
    if (modules == null) {
      // The contract's own bound makes a short module string unreachable, so
      // this is not a case that should occur — but drawing a wrong square is
      // worse than saying so, and a silent half-code is worst of all.
      return const _NoSquare(
        reason: 'This code did not arrive whole — share the link instead.',
      );
    }
    return Semantics(
      container: true,
      label: 'Invite QR code',
      value: '${modules.width} by ${modules.width} modules',
      child: ExcludeSemantics(
        child: Center(
          child: Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: DecoratedBox(
              // The quiet zone is part of a readable code, not decoration: a
              // scanner needs the light margin to find the square at all.
              decoration: const BoxDecoration(color: Color(0xFFFFFFFF)),
              child: Padding(
                padding: const EdgeInsets.all(12),
                child: SizedBox(
                  width: 220,
                  height: 220,
                  child: CustomPaint(painter: _QrPainter(modules)),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The drawn answer for an invite with no square.
///
/// A real frontier, not an error: our grammar can spell invites longer than any
/// QR version holds, so "too long to show" is a state the contract publishes and
/// this renders. A blank space where a code was expected would be the bug.
class _NoSquare extends StatelessWidget {
  const _NoSquare({this.reason});

  final String? reason;

  @override
  Widget build(BuildContext context) {
    final ThemeData theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: Text(
        reason ?? 'Too long to show as a code — share the link instead.',
        style: theme.textTheme.bodySmall?.copyWith(
          color: theme.colorScheme.onSurfaceVariant,
        ),
      ),
    );
  }
}

/// The published modules unpacked into one bool per module.
class _Modules {
  const _Modules(this.width, this._dark);

  /// Unpacks the row-major, MSB-first bitmap the contract carries, or null when
  /// it does not hold `width * width` bits.
  static _Modules? parse(QrView view) {
    final int width = view.width;
    if (width <= 0) {
      return null;
    }
    // Exposed because DRAWING it is the exposure the seal exists to permit;
    // every other path renders it redacted.
    final String hex = view.modules.expose();
    final int bits = width * width;
    if (hex.length != ((bits + 7) ~/ 8) * 2) {
      return null;
    }
    final List<bool> dark = List<bool>.filled(bits, false);
    for (int index = 0; index < bits; index++) {
      final int byte = int.parse(
        hex.substring((index ~/ 8) * 2, (index ~/ 8) * 2 + 2),
        radix: 16,
      );
      dark[index] = (byte & (0x80 >> (index % 8))) != 0;
    }
    return _Modules(width, dark);
  }

  final int width;
  final List<bool> _dark;

  bool isDark(int row, int column) => _dark[row * width + column];
}

class _QrPainter extends CustomPainter {
  const _QrPainter(this.modules);

  final _Modules modules;

  @override
  void paint(Canvas canvas, Size size) {
    final double module = size.width / modules.width;
    final Paint paint = Paint()..color = const Color(0xFF000000);
    for (int row = 0; row < modules.width; row++) {
      for (int column = 0; column < modules.width; column++) {
        if (!modules.isDark(row, column)) {
          continue;
        }
        // Drawn a hair wide so neighbouring dark modules meet: a seam of
        // background between them is what makes a rendered code unscannable.
        canvas.drawRect(
          Rect.fromLTWH(
            column * module,
            row * module,
            module + 0.5,
            module + 0.5,
          ),
          paint,
        );
      }
    }
  }

  @override
  bool shouldRepaint(_QrPainter oldDelegate) =>
      !identical(oldDelegate.modules, modules);
}
