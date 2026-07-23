package dev.envoix.app.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.staticCompositionLocalOf

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
