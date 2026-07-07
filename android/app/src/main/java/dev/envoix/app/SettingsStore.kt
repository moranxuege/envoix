package dev.envoix.app

import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.io.File

/**
 * Two layers, one flat holder:
 *  - core config — mirrors the CLI's `config.toml` schema (chunk_size, candidates)
 *    plus the connection defaults (broker/relay). Rendered by [SettingsStore.renderConfig].
 *  - native prefs — platform-only defaults that seed a transfer request (save
 *    folder, default role); never sent to the core.
 */
data class Settings(
    // core connection defaults
    val broker: String = Endpoints.BROKER,
    val relay: String = Endpoints.RELAY,
    // core config.toml (RuntimeConfig)
    val chunkSize: String = "",
    val candidatesAllow: List<String> = emptyList(),
    val candidatesDeny: List<String> = emptyList(),
    // native app prefs (seed per-transfer choices)
    val saveFolder: String = "Envoix",
    val defaultRole: String = "receive",
    // rendezvous modes to attempt, in order Room → mDNS (fall back on failure)
    val useRoom: Boolean = true,
    val useMdns: Boolean = true,
    // developer / diagnostics
    val devMode: Boolean = false,
    val verboseLog: Boolean = false,
)

/** App settings, persisted in SharedPreferences and observable as a StateFlow. */
object SettingsStore {
    /** Tailscale's CGNAT ranges — the "Avoid Tailscale" toggle is just these in `deny`. */
    val TAILSCALE_CIDRS = listOf("100.64.0.0/10", "fd7a:115c:a1e0::/48")

    private lateinit var prefs: SharedPreferences
    private val _settings = MutableStateFlow(Settings())
    val settings: StateFlow<Settings> = _settings.asStateFlow()

    fun init(context: Context) {
        prefs = context.getSharedPreferences("envoix.settings", Context.MODE_PRIVATE)
        _settings.value = Settings(
            broker = prefs.getString("broker", Endpoints.BROKER)!!,
            relay = prefs.getString("relay", Endpoints.RELAY)!!,
            chunkSize = prefs.getString("chunkSize", "")!!,
            candidatesAllow = readList("candidatesAllow"),
            candidatesDeny = readList("candidatesDeny"),
            saveFolder = prefs.getString("saveFolder", "Envoix")!!,
            defaultRole = prefs.getString("defaultRole", "receive")!!,
            useRoom = prefs.getBoolean("useRoom", true),
            useMdns = prefs.getBoolean("useMdns", true),
            devMode = prefs.getBoolean("devMode", false),
            verboseLog = prefs.getBoolean("verboseLog", false),
        )
    }

    private fun readList(key: String): List<String> =
        prefs.getString(key, "").orEmpty().lines().map { it.trim() }.filter { it.isNotEmpty() }

    fun update(transform: (Settings) -> Settings) {
        val s = transform(_settings.value)
        prefs.edit()
            .putString("broker", s.broker)
            .putString("relay", s.relay)
            .putString("chunkSize", s.chunkSize)
            .putString("candidatesAllow", s.candidatesAllow.joinToString("\n"))
            .putString("candidatesDeny", s.candidatesDeny.joinToString("\n"))
            .putString("saveFolder", s.saveFolder)
            .putString("defaultRole", s.defaultRole)
            .putBoolean("useRoom", s.useRoom)
            .putBoolean("useMdns", s.useMdns)
            .putBoolean("devMode", s.devMode)
            .putBoolean("verboseLog", s.verboseLog)
            .apply()
        _settings.value = s
    }

    private const val LOG_BASELINE = "envoix=debug,iroh=info,warn"
    private const val LOG_VERBOSE = "envoix=trace,iroh=debug,warn"

    /** Push the current verbosity down to the native reloadable filter. */
    fun applyLogLevel() =
        Native.setLogLevel(if (_settings.value.verboseLog) LOG_VERBOSE else LOG_BASELINE)

    /** The "Avoid Tailscale" toggle is a *view* over `deny`: on iff the ranges are present. */
    fun avoidsTailscale(s: Settings): Boolean = s.candidatesDeny.containsAll(TAILSCALE_CIDRS)

    /** Add/remove the Tailscale ranges in `deny` — the same source the Advanced editor edits. */
    fun setAvoidTailscale(on: Boolean) = update { s ->
        val deny = if (on) (s.candidatesDeny + TAILSCALE_CIDRS).distinct()
        else s.candidatesDeny.filterNot { it in TAILSCALE_CIDRS }
        s.copy(candidatesDeny = deny)
    }

    /**
     * Render a `config.toml` for the core from the config-tier fields, or null
     * when none are set. Same schema as the CLI's `RuntimeConfig`.
     */
    fun renderConfig(context: Context): String? {
        val s = _settings.value
        val lines = mutableListOf<String>()
        if (s.chunkSize.isNotBlank()) lines += "chunk_size = ${tomlStr(s.chunkSize.trim())}"
        if (s.candidatesAllow.isNotEmpty() || s.candidatesDeny.isNotEmpty()) {
            lines += "[candidates]"
            if (s.candidatesAllow.isNotEmpty()) lines += "allow = ${tomlArr(s.candidatesAllow)}"
            if (s.candidatesDeny.isNotEmpty()) lines += "deny = ${tomlArr(s.candidatesDeny)}"
        }
        if (lines.isEmpty()) return null
        return File(context.filesDir, "config.toml")
            .apply { writeText(lines.joinToString("\n") + "\n") }
            .absolutePath
    }

    private fun tomlStr(s: String) = "\"" + s.replace("\"", "") + "\""
    private fun tomlArr(items: List<String>) = "[" + items.joinToString(", ") { tomlStr(it) } + "]"
}
