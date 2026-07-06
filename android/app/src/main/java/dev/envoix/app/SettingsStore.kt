package dev.envoix.app

import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.io.File

data class Settings(
    val broker: String = Endpoints.BROKER,
    val relay: String = Endpoints.RELAY,
    /** Exclude VPN/Tailscale interfaces from the transfer (candidate deny list). */
    val blockVpn: Boolean = false,
)

/** App settings, persisted in SharedPreferences and observable as a StateFlow. */
object SettingsStore {
    private lateinit var prefs: SharedPreferences
    private val _settings = MutableStateFlow(Settings())
    val settings: StateFlow<Settings> = _settings.asStateFlow()

    fun init(context: Context) {
        prefs = context.getSharedPreferences("envoix.settings", Context.MODE_PRIVATE)
        _settings.value = Settings(
            broker = prefs.getString("broker", Endpoints.BROKER)!!,
            relay = prefs.getString("relay", Endpoints.RELAY)!!,
            blockVpn = prefs.getBoolean("blockVpn", false),
        )
    }

    fun update(transform: (Settings) -> Settings) {
        val s = transform(_settings.value)
        prefs.edit()
            .putString("broker", s.broker)
            .putString("relay", s.relay)
            .putBoolean("blockVpn", s.blockVpn)
            .apply()
        _settings.value = s
    }

    /**
     * Render a `config.toml` for the core when the candidate filter is on and
     * return its path; null when no config file is needed. Same format as the
     * CLI, so the app and CLI share one config schema.
     */
    fun renderConfig(context: Context): String? {
        if (!_settings.value.blockVpn) return null
        val toml = """
            [candidates]
            deny = ["100.64.0.0/10", "fd7a:115c:a1e0::/48"]
        """.trimIndent() + "\n"
        return File(context.filesDir, "config.toml").apply { writeText(toml) }.absolutePath
    }
}
