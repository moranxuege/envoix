package dev.envoix.app.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color

/** The demo's design tokens (see envoix-demo CSS: --accent, --bg, --muted, ...). */
@Immutable
data class EnvoixColors(
    val bg: Color,
    val surface: Color,
    val surfaceRaised: Color,
    val text: Color,
    val muted: Color,
    val line: Color,
    val accent: Color,
    val accentStrong: Color,
    val accentSoft: Color,
    val success: Color,
    val successSoft: Color,
    val warning: Color,
    val danger: Color,
)

private val LightColors = EnvoixColors(
    bg = Color(0xFFF6F7F9),
    surface = Color(0xFFFFFFFF),
    surfaceRaised = Color(0xFFFDFEFE),
    text = Color(0xFF17202A),
    muted = Color(0xFF647181),
    line = Color(0xFFD9E0E7),
    accent = Color(0xFF0F6BFF),
    accentStrong = Color(0xFF084FBD),
    accentSoft = Color(0xFFE7F0FF),
    success = Color(0xFF147A4B),
    successSoft = Color(0xFFDDF3E7),
    warning = Color(0xFFA05A00),
    danger = Color(0xFFE74C3C),
)

private val DarkColors = EnvoixColors(
    bg = Color(0xFF121519),
    surface = Color(0xFF1B2027),
    surfaceRaised = Color(0xFF232A33),
    text = Color(0xFFEDF2F7),
    muted = Color(0xFFAAB5C2),
    line = Color(0xFF343D49),
    accent = Color(0xFF6BB6FF),
    accentStrong = Color(0xFF9ED0FF),
    accentSoft = Color(0xFF19334F),
    success = Color(0xFF61D69A),
    successSoft = Color(0xFF16362A),
    warning = Color(0xFFFFC166),
    danger = Color(0xFFF07167),
)

val LocalEnvoixColors = staticCompositionLocalOf { LightColors }

/** Convenience accessor: `Envoix.colors.accent`. */
object Envoix {
    val colors: EnvoixColors
        @Composable get() = LocalEnvoixColors.current
}

@Composable
fun EnvoixTheme(
    dark: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    val colors = if (dark) DarkColors else LightColors
    val scheme = if (dark) {
        darkColorScheme(
            primary = colors.accent,
            background = colors.bg,
            surface = colors.surface,
            onBackground = colors.text,
            onSurface = colors.text,
            error = colors.danger,
        )
    } else {
        lightColorScheme(
            primary = colors.accent,
            background = colors.bg,
            surface = colors.surface,
            onBackground = colors.text,
            onSurface = colors.text,
            error = colors.danger,
        )
    }
    CompositionLocalProvider(LocalEnvoixColors provides colors) {
        MaterialTheme(colorScheme = scheme, content = content)
    }
}
