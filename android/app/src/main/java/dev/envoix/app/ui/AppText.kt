package dev.envoix.app.ui

import androidx.annotation.PluralsRes
import androidx.annotation.StringRes
import androidx.compose.runtime.Composable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.platform.LocalContext
import dev.envoix.app.localizedResources
import dev.envoix.app.localizedString

object AppLanguage {
    const val ENGLISH = "en"
    const val SIMPLIFIED_CHINESE = "zh-Hans"
}

val LocalAppLanguage = staticCompositionLocalOf { AppLanguage.ENGLISH }

@Composable
fun appString(
    @StringRes id: Int,
    vararg formatArgs: Any,
): String =
    LocalContext.current.localizedString(
        id,
        LocalAppLanguage.current,
        *formatArgs,
    )

@Composable
fun appQuantityString(
    @PluralsRes id: Int,
    quantity: Int,
    vararg formatArgs: Any,
): String =
    LocalContext.current
        .localizedResources(LocalAppLanguage.current)
        .getQuantityString(id, quantity, *formatArgs)

internal sealed interface UiMessage {
    data class Dynamic(
        val value: String,
    ) : UiMessage

    data class Resource(
        @StringRes val id: Int,
    ) : UiMessage
}

@Composable
internal fun UiMessage.resolve(): String =
    when (this) {
        is UiMessage.Dynamic -> value
        is UiMessage.Resource -> appString(id)
    }
