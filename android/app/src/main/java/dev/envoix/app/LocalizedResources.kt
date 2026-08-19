package dev.envoix.app

import android.content.Context
import android.content.res.Configuration
import android.content.res.Resources
import android.os.LocaleList
import androidx.annotation.StringRes

internal fun Context.localizedResources(language: String): Resources {
    val configuration = Configuration(resources.configuration)
    configuration.setLocales(LocaleList.forLanguageTags(language))
    return createConfigurationContext(configuration).resources
}

internal fun Context.localizedString(
    @StringRes id: Int,
    language: String,
    vararg formatArgs: Any,
): String {
    val localized = localizedResources(language)
    return if (formatArgs.isEmpty()) {
        localized.getString(id)
    } else {
        localized.getString(id, *formatArgs)
    }
}
