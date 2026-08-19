package dev.envoix.app.ui

import android.content.res.Configuration
import android.os.LocaleList
import androidx.annotation.PluralsRes
import androidx.annotation.StringRes
import androidx.compose.runtime.Composable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.platform.LocalContext

object AppText {
    fun value(
        english: String,
        simplifiedChinese: String,
        language: String,
    ): String = if (language == SIMPLIFIED_CHINESE) simplifiedChinese else english

    const val ENGLISH = "en"
    const val SIMPLIFIED_CHINESE = "zh-Hans"
}

val LocalAppLanguage = staticCompositionLocalOf { AppText.ENGLISH }

@Composable
fun appText(
    english: String,
    simplifiedChinese: String,
): String = AppText.value(english, simplifiedChinese, LocalAppLanguage.current)

@Composable
fun appString(
    @StringRes id: Int,
    vararg formatArgs: Any,
): String {
    val context = LocalContext.current
    val configuration = Configuration(context.resources.configuration)
    configuration.setLocales(LocaleList.forLanguageTags(LocalAppLanguage.current))
    val resources = context.createConfigurationContext(configuration).resources
    return if (formatArgs.isEmpty()) {
        resources.getString(id)
    } else {
        resources.getString(id, *formatArgs)
    }
}

@Composable
fun appQuantityString(
    @PluralsRes id: Int,
    quantity: Int,
    vararg formatArgs: Any,
): String {
    val context = LocalContext.current
    val configuration = Configuration(context.resources.configuration)
    configuration.setLocales(LocaleList.forLanguageTags(LocalAppLanguage.current))
    return context
        .createConfigurationContext(configuration)
        .resources
        .getQuantityString(id, quantity, *formatArgs)
}
