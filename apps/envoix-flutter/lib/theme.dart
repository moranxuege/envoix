/// The design system, recovered from the source it was drawn in.
///
/// The palette, the type scale, the radii and the spacing rhythm are the demo
/// stylesheet's own custom properties, transposed. They are written out by hand
/// at both brightnesses because that is how the design was authored: a
/// generated scheme has no `muted`, no `line`, no soft tint and no
/// success/warning distinction, and every one of those carries meaning here.
///
/// `MaterialTheme` is DOWNSTREAM of the tokens, never upstream — [envoixTheme]
/// builds a [ColorScheme] out of [EnvoixTokens] so that a stock widget lands in
/// the same palette as a hand-drawn one, and so that no Material default ever
/// decides what a colour in this app is.
library;

import 'package:flutter/material.dart';

/// The eleven authored colours, plus the two the stylesheet expresses as
/// derivations rather than as tokens.
///
/// `danger` is not a stylesheet token: the demo has no failure state, and its
/// only negative signal is `warning`. The Compose port added `#e74c3c`, which
/// is a FILL weight — it works under white text on the swipe-to-remove panel
/// and fails WCAG AA as text on a light surface (3.8:1). Every authored light
/// token is an ink weight instead, sitting at HSL lightness 28-39% so it can be
/// read on white. So `danger` here is that same hue and saturation dropped into
/// the authored band (HSL 6/78/40), and it reads at 6.4:1. Dark keeps the
/// Compose value, which was already legible on `surface`.
///
/// The other Compose addition, `successSoft`, needs no token at all: the
/// stylesheet writes its tints as `color-mix(in srgb, <colour> 14%,
/// transparent)`, and running that rule over `success` reproduces the frozen
/// Compose hex to within a few points at both brightnesses. It is [soft] here.
@immutable
class EnvoixTokens {
  const EnvoixTokens({
    required this.bg,
    required this.surface,
    required this.surfaceRaised,
    required this.text,
    required this.muted,
    required this.line,
    required this.accent,
    required this.accentStrong,
    required this.accentSoft,
    required this.onAccent,
    required this.success,
    required this.warning,
    required this.danger,
    required this.shadowInk,
  });

  /// The recessed well: what a sunken block inside a card sits on.
  ///
  /// In the stylesheet this is the page BEHIND the phone frame, which a real
  /// phone does not have — the screen itself is `surface`. The Compose port
  /// already used it as the inset ground under the log box, and that is the job
  /// it keeps.
  final Color bg;

  /// The screen.
  final Color surface;

  /// A card, a row, a raised block. One step above [surface], and in light that
  /// step is almost invisible on purpose: the hairline [line] border is what
  /// separates them, and the fill only has to stop them being identical.
  final Color surfaceRaised;

  final Color text;

  /// Secondary prose, and every helper line.
  final Color muted;

  /// The hairline. It does the work of a border and a divider both.
  final Color line;

  final Color accent;

  /// Accent as TEXT. The plain accent is a fill; this is what survives on one.
  final Color accentStrong;

  /// The one authored tint, and not a 14% mix of [accent] — it is lighter than
  /// the rule would give in light and bluer in dark, so it stays a token.
  final Color accentSoft;

  /// What is legible ON [accent]. White in light, as the stylesheet says; the
  /// page ink in dark, where the stylesheet's own white-on-pale-blue would not
  /// have been readable and the Compose port left a Material default in place.
  final Color onAccent;

  final Color success;
  final Color warning;
  final Color danger;

  /// The ink of the elevation shadow, at the authored alpha.
  final Color shadowInk;

  static const EnvoixTokens light = EnvoixTokens(
    bg: Color(0xfff6f7f9),
    surface: Color(0xffffffff),
    surfaceRaised: Color(0xfffdfefe),
    text: Color(0xff17202a),
    muted: Color(0xff647181),
    line: Color(0xffd9e0e7),
    accent: Color(0xff0f6bff),
    accentStrong: Color(0xff084fbd),
    accentSoft: Color(0xffe7f0ff),
    onAccent: Color(0xffffffff),
    success: Color(0xff147a4b),
    warning: Color(0xffa05a00),
    danger: Color(0xffb62616),
    shadowInk: Color(0x1f1c2736),
  );

  static const EnvoixTokens dark = EnvoixTokens(
    bg: Color(0xff121519),
    surface: Color(0xff1b2027),
    surfaceRaised: Color(0xff232a33),
    text: Color(0xffedf2f7),
    muted: Color(0xffaab5c2),
    line: Color(0xff343d49),
    accent: Color(0xff6bb6ff),
    accentStrong: Color(0xff9ed0ff),
    accentSoft: Color(0xff19334f),
    onAccent: Color(0xff121519),
    success: Color(0xff61d69a),
    warning: Color(0xffffc166),
    danger: Color(0xfff07167),
    shadowInk: Color(0x52000000),
  );

  /// The tokens behind the theme on this subtree. [envoixTheme] is the only
  /// theme this app installs, so the brightness it was built at is the whole
  /// answer and there is nothing to be absent.
  static EnvoixTokens of(BuildContext context) =>
      Theme.of(context).brightness == Brightness.dark ? dark : light;

  /// The stylesheet's tint rule: the colour at 14%, over the surface it lands
  /// on. It is what makes a pill read as a pill rather than as a chip.
  Color soft(Color colour) =>
      Color.alphaBlend(colour.withValues(alpha: 0.14), surface);

  /// The authored elevation, for the one thing in this app that floats over the
  /// whole screen. The stylesheet spends it on the phone frame, the desktop
  /// window and the toast — chrome, never a card. A card gets [line] and a
  /// one-pixel hard edge instead, which is why the list reads flat.
  List<BoxShadow> get shadow => <BoxShadow>[
        BoxShadow(color: shadowInk, offset: const Offset(0, 18), blurRadius: 38),
      ];
}

/// Radii. The stylesheet has essentially one: 8 for everything rectangular, a
/// full round for pills. The Compose port fanned that out into 16/14/12/10/9/8,
/// which is fussier and reads as Material rather than as this product.
abstract final class EnvoixShape {
  static const double radius = 8;
  static const BorderRadius corner = BorderRadius.all(Radius.circular(radius));
  static const BorderRadius pill = BorderRadius.all(Radius.circular(999));
  static const double hairline = 1;
}

/// The spacing rhythm, off the mobile view of the stylesheet.
abstract final class EnvoixSpace {
  /// `padding-inline` on every band of the phone screen.
  static const double gutter = 18;

  /// A card's own padding.
  static const double card = 14;

  /// Between blocks on a screen.
  static const double block = 16;

  /// Between cards, and between a card's own rows.
  static const double row = 12;

  /// Between a label and its control, and inside a row.
  static const double tight = 8;

  static const double hair = 4;

  /// The bottom of a sheet, clear of the gesture bar.
  static const double foot = 28;

  /// Clearance below the last card so the floating action never covers it.
  static const double aboveFloating = 96;
}

/// The type scale.
///
/// Hierarchy is carried by WEIGHT and size, not by colour: that is what makes
/// the app read as an engineering tool. The stylesheet uses variable-font
/// weights (650 on a field label, 750 on a mode pill) which a [FontWeight]
/// cannot express, so those round to 600 and 700 exactly as the Compose port
/// rounded them.
///
/// Letter spacing is pinned to zero throughout, because Material 3's defaults
/// track headings and labels apart and the stylesheet sets `letter-spacing: 0`
/// on every heading it styles. The two places that DO track are the ones the
/// design tracks on purpose: [section] and [eyebrow].
abstract final class EnvoixType {
  /// Machine-generated text: ids, codes, byte counts, paths, log lines.
  ///
  /// The rule is the whole point — anything the machine wrote is monospace,
  /// anything a human wrote is not. The family is the platform's, named the way
  /// the stylesheet names its own code stack: a preferred face, then whatever
  /// the device actually has.
  static const String mono = 'monospace';
  static const List<String> monoFallback = <String>[
    'SF Mono',
    'Menlo',
    'Consolas',
    'Liberation Mono',
    'Roboto Mono',
    'Droid Sans Mono',
  ];

  static const TextStyle wordmark =
      TextStyle(fontSize: 24, fontWeight: FontWeight.w800, letterSpacing: 0);
  static const TextStyle screen =
      TextStyle(fontSize: 26, fontWeight: FontWeight.w800, letterSpacing: 0);
  static const TextStyle sheet =
      TextStyle(fontSize: 21, fontWeight: FontWeight.w800, letterSpacing: 0);
  static const TextStyle panel =
      TextStyle(fontSize: 18, fontWeight: FontWeight.w700, letterSpacing: 0);

  /// A card's own title, and a settings row's.
  static const TextStyle title =
      TextStyle(fontSize: 16, fontWeight: FontWeight.w700, letterSpacing: 0);

  /// A fact worth reading before its neighbours.
  static const TextStyle value =
      TextStyle(fontSize: 15, fontWeight: FontWeight.w600, letterSpacing: 0);

  static const TextStyle body =
      TextStyle(fontSize: 15, fontWeight: FontWeight.w400, letterSpacing: 0);

  /// A subtitle or a helper line. Muted by the theme, not by the caller.
  static const TextStyle subtitle =
      TextStyle(fontSize: 13, fontWeight: FontWeight.w400, letterSpacing: 0);

  /// A control's label.
  static const TextStyle action =
      TextStyle(fontSize: 14, fontWeight: FontWeight.w600, letterSpacing: 0);

  static const TextStyle pillLabel =
      TextStyle(fontSize: 12, fontWeight: FontWeight.w700, letterSpacing: 0);

  /// The caps section label. Tracked out, because a run of small capitals set
  /// solid is unreadable.
  static const TextStyle section =
      TextStyle(fontSize: 11, fontWeight: FontWeight.w700, letterSpacing: 1);

  /// The eyebrow over a title, tracked the way the stylesheet tracks it
  /// (`0.08em` at `0.77rem`).
  static const TextStyle eyebrow =
      TextStyle(fontSize: 12, fontWeight: FontWeight.w700, letterSpacing: 0.96);

  /// A machine-generated value read alongside prose.
  static const TextStyle monoValue = TextStyle(
    fontFamily: mono,
    fontFamilyFallback: monoFallback,
    fontSize: 14,
    fontWeight: FontWeight.w600,
    letterSpacing: 0,
  );

  /// A machine-generated line in a run of them: log entries, manifests.
  static const TextStyle monoLine = TextStyle(
    fontFamily: mono,
    fontFamilyFallback: monoFallback,
    fontSize: 12,
    fontWeight: FontWeight.w400,
    letterSpacing: 0,
    height: 1.4,
  );
}

/// Light and dark are one design at two brightnesses, written out at both:
/// every pair below is authored, so neither brightness is a derivation of the
/// other and neither can drift into a colour nobody chose.
ThemeData envoixTheme(Brightness brightness) {
  final EnvoixTokens t =
      brightness == Brightness.dark ? EnvoixTokens.dark : EnvoixTokens.light;
  final bool dark = brightness == Brightness.dark;
  // Material's container ladder runs from least to most elevated, and which
  // direction that is on screen depends on the brightness: whiter in light,
  // lighter in dark. Mapped so a stock widget reaching for one still lands on
  // an authored colour.
  final Color containerLowest = dark ? t.bg : t.surface;
  final Color containerLow = dark ? t.surface : t.surfaceRaised;
  final Color container = dark ? t.surfaceRaised : t.bg;

  final ColorScheme scheme = ColorScheme(
    brightness: brightness,
    primary: t.accent,
    onPrimary: t.onAccent,
    primaryContainer: t.accentSoft,
    onPrimaryContainer: t.accentStrong,
    secondary: t.accentStrong,
    onSecondary: t.onAccent,
    // Tonal controls land here, which is exactly the stylesheet's active-tab
    // and active-rail treatment: accent-strong on accent-soft.
    secondaryContainer: t.accentSoft,
    onSecondaryContainer: t.accentStrong,
    tertiary: t.success,
    onTertiary: t.onAccent,
    tertiaryContainer: t.soft(t.success),
    onTertiaryContainer: t.success,
    error: t.danger,
    onError: t.onAccent,
    errorContainer: t.soft(t.danger),
    onErrorContainer: t.danger,
    surface: t.surface,
    onSurface: t.text,
    surfaceDim: t.bg,
    surfaceBright: dark ? t.surfaceRaised : t.surface,
    surfaceContainerLowest: containerLowest,
    surfaceContainerLow: containerLow,
    surfaceContainer: container,
    surfaceContainerHigh: container,
    surfaceContainerHighest: container,
    onSurfaceVariant: t.muted,
    outline: t.line,
    outlineVariant: t.line,
    shadow: t.shadowInk.withValues(alpha: 1),
    inverseSurface: t.text,
    onInverseSurface: t.surface,
    inversePrimary: t.accentStrong,
    // Material tints a surface by its elevation using this. Left transparent so
    // an authored colour is the colour that reaches the screen.
    surfaceTint: Colors.transparent,
  );

  final TextTheme type = TextTheme(
    displayLarge: EnvoixType.screen.copyWith(color: t.text),
    displayMedium: EnvoixType.screen.copyWith(color: t.text),
    displaySmall: EnvoixType.screen.copyWith(color: t.text),
    headlineLarge: EnvoixType.screen.copyWith(color: t.text),
    headlineMedium: EnvoixType.screen.copyWith(color: t.text),
    headlineSmall: EnvoixType.panel.copyWith(color: t.text),
    titleLarge: EnvoixType.sheet.copyWith(color: t.text),
    titleMedium: EnvoixType.title.copyWith(color: t.text),
    titleSmall: EnvoixType.value.copyWith(color: t.text),
    bodyLarge: EnvoixType.body.copyWith(color: t.text),
    bodyMedium: EnvoixType.body.copyWith(color: t.text),
    bodySmall: EnvoixType.subtitle.copyWith(color: t.muted),
    labelLarge: EnvoixType.action.copyWith(color: t.text),
    labelMedium: EnvoixType.pillLabel.copyWith(color: t.text),
    labelSmall: EnvoixType.section.copyWith(color: t.muted),
  );

  final OutlineInputBorder field = OutlineInputBorder(
    borderRadius: EnvoixShape.corner,
    borderSide: BorderSide(color: t.line),
  );

  return ThemeData(
    useMaterial3: true,
    colorScheme: scheme,
    textTheme: type,
    // The phone screen is `surface`; cards on it are `surface-raised` with a
    // hairline. `bg` is the recessed well, not the page.
    scaffoldBackgroundColor: t.surface,
    canvasColor: t.surface,
    dividerColor: t.line,
    dividerTheme: DividerThemeData(color: t.line, thickness: 1, space: 32),
    appBarTheme: AppBarTheme(
      backgroundColor: t.surface,
      foregroundColor: t.text,
      surfaceTintColor: Colors.transparent,
      shadowColor: Colors.transparent,
      elevation: 0,
      scrolledUnderElevation: 0,
      centerTitle: false,
      titleSpacing: EnvoixSpace.gutter,
      titleTextStyle: EnvoixType.wordmark.copyWith(color: t.text),
    ),
    cardTheme: CardThemeData(
      color: t.surfaceRaised,
      surfaceTintColor: Colors.transparent,
      shadowColor: Colors.transparent,
      elevation: 0,
      margin: const EdgeInsets.only(bottom: EnvoixSpace.row),
      shape: RoundedRectangleBorder(
        borderRadius: EnvoixShape.corner,
        side: BorderSide(color: t.line, width: EnvoixShape.hairline),
      ),
    ),
    listTileTheme: ListTileThemeData(
      contentPadding: const EdgeInsets.symmetric(
        horizontal: EnvoixSpace.card,
        vertical: EnvoixSpace.hair,
      ),
      titleTextStyle: EnvoixType.title.copyWith(color: t.text),
      subtitleTextStyle: EnvoixType.subtitle.copyWith(color: t.muted),
    ),
    bottomSheetTheme: BottomSheetThemeData(
      backgroundColor: t.surface,
      modalBackgroundColor: t.surface,
      surfaceTintColor: Colors.transparent,
      elevation: 0,
      modalElevation: 0,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(
          top: Radius.circular(EnvoixShape.radius),
        ),
      ),
    ),
    navigationBarTheme: NavigationBarThemeData(
      backgroundColor: t.surface,
      surfaceTintColor: Colors.transparent,
      shadowColor: Colors.transparent,
      elevation: 0,
      height: 68,
      indicatorColor: t.accentSoft,
      indicatorShape: const RoundedRectangleBorder(
        borderRadius: EnvoixShape.corner,
      ),
      labelBehavior: NavigationDestinationLabelBehavior.alwaysShow,
      labelTextStyle: WidgetStateProperty.resolveWith(
        (Set<WidgetState> states) => states.contains(WidgetState.selected)
            ? EnvoixType.pillLabel.copyWith(color: t.accentStrong)
            : EnvoixType.pillLabel.copyWith(
                color: t.muted,
                fontWeight: FontWeight.w600,
              ),
      ),
      iconTheme: WidgetStateProperty.resolveWith(
        (Set<WidgetState> states) => IconThemeData(
          color: states.contains(WidgetState.selected)
              ? t.accentStrong
              : t.muted,
        ),
      ),
    ),
    floatingActionButtonTheme: FloatingActionButtonThemeData(
      backgroundColor: t.accent,
      foregroundColor: t.onAccent,
      splashColor: t.accentStrong,
      elevation: 3,
      focusElevation: 3,
      hoverElevation: 3,
      highlightElevation: 3,
      extendedTextStyle: EnvoixType.value.copyWith(color: t.onAccent),
      shape: const RoundedRectangleBorder(borderRadius: EnvoixShape.corner),
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: ButtonStyle(
        textStyle: WidgetStatePropertyAll<TextStyle>(EnvoixType.action),
        minimumSize: const WidgetStatePropertyAll<Size>(Size(0, 42)),
        padding: const WidgetStatePropertyAll<EdgeInsetsGeometry>(
          EdgeInsets.symmetric(horizontal: EnvoixSpace.card),
        ),
        shape: const WidgetStatePropertyAll<OutlinedBorder>(
          RoundedRectangleBorder(borderRadius: EnvoixShape.corner),
        ),
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: ButtonStyle(
        // The stylesheet's secondary control: raised fill, hairline border,
        // ordinary text colour — quieter than an accent button without being
        // a bare label.
        backgroundColor: WidgetStatePropertyAll<Color>(t.surfaceRaised),
        foregroundColor: WidgetStatePropertyAll<Color>(t.text),
        side: WidgetStatePropertyAll<BorderSide>(BorderSide(color: t.line)),
        textStyle: WidgetStatePropertyAll<TextStyle>(EnvoixType.action),
        minimumSize: const WidgetStatePropertyAll<Size>(Size(0, 42)),
        shape: const WidgetStatePropertyAll<OutlinedBorder>(
          RoundedRectangleBorder(borderRadius: EnvoixShape.corner),
        ),
      ),
    ),
    textButtonTheme: TextButtonThemeData(
      style: ButtonStyle(
        foregroundColor: WidgetStatePropertyAll<Color>(t.accentStrong),
        textStyle: WidgetStatePropertyAll<TextStyle>(
          EnvoixType.action.copyWith(fontWeight: FontWeight.w700),
        ),
        minimumSize: const WidgetStatePropertyAll<Size>(Size(0, 38)),
        shape: const WidgetStatePropertyAll<OutlinedBorder>(
          RoundedRectangleBorder(borderRadius: EnvoixShape.corner),
        ),
      ),
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: t.surface,
      border: field,
      enabledBorder: field,
      focusedBorder: OutlineInputBorder(
        borderRadius: EnvoixShape.corner,
        borderSide: BorderSide(color: t.accent, width: 2),
      ),
      contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 12),
      labelStyle: EnvoixType.subtitle.copyWith(
        color: t.muted,
        fontWeight: FontWeight.w600,
      ),
      floatingLabelStyle: EnvoixType.subtitle.copyWith(
        color: t.accentStrong,
        fontWeight: FontWeight.w600,
      ),
      helperStyle: EnvoixType.subtitle.copyWith(color: t.muted),
    ),
    progressIndicatorTheme: ProgressIndicatorThemeData(
      color: t.accent,
      linearTrackColor: t.line.withValues(alpha: 0.7),
      // The authored bar: eight tall, fully rounded, one unbroken track.
      linearMinHeight: 8,
      borderRadius: EnvoixShape.pill,
      // The 2024 Material indicator breaks itself with a gap and ends in a stop
      // dot — a different drawing of the same number. Flutter still draws the
      // older, unbroken one, and these two are inert until that default flips;
      // setting them is the documented way to keep the bar whole when it does,
      // without touching the deprecated `year2023` flag.
      trackGap: 0,
      stopIndicatorRadius: 0,
    ),
    textSelectionTheme: TextSelectionThemeData(
      cursorColor: t.accent,
      selectionColor: t.accentSoft,
      selectionHandleColor: t.accent,
    ),
  );
}
