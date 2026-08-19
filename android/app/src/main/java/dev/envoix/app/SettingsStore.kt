package dev.envoix.app

import android.content.Context
import android.content.SharedPreferences
import android.os.Build
import dev.envoix.app.discovery.DiscoveryPeerRegistry
import dev.envoix.app.ffi.setLogLevel
import dev.envoix.app.ui.AppText
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.util.Locale

/**
 * Two layers, one flat holder:
 *  - core config — mirrors the CLI's transport-only `config.toml` schema
 *    plus the connection defaults (broker/relay). Rendered by [SettingsStore.paramsJson (TransferService.Spec)].
 *  - native prefs — platform-only defaults that seed a transfer request (save
 *    folder, default role); never sent to the core.
 */
data class Settings(
    val language: String = AppText.ENGLISH,
    // core connection defaults
    val broker: String = Endpoints.BROKER,
    val relay: String = Endpoints.RELAY,
    /** Core config.toml stream window (e.g. `32MB`); empty = transport default (16MB). */
    val dataStreamWindow: String = "",
    val candidatesAllow: List<String> = emptyList(),
    val candidatesDeny: List<String> = emptyList(),
    // native app prefs (seed per-transfer choices)
    val saveFolder: String = "Envoix",
    /** A user-picked SAF folder for received files; empty = default Downloads/[saveFolder]. */
    val saveTreeUri: String = "",
    val defaultRole: String = "receive",
    /** Canonical Manifest-v2 compression policy: never, always, or smart. */
    val compressionPolicy: String = "smart",
    /** Public nearby label. This is presentation only, never authenticated identity. */
    val nearbyDisplayName: String = Build.MODEL,
    /** hidden, everyone_10m, or foreground. */
    val nearbyVisibility: String = "hidden",
    /** Wall-clock deadline used only by everyone_10m. */
    val nearbyVisibilityExpiresAtEpochMs: Long = 0L,
    // rendezvous routes: receivers listen concurrently; senders prefer Room then mDNS
    val useRoom: Boolean = true,
    val useMdns: Boolean = true,
    // developer / diagnostics
    val devMode: Boolean = false,
    val verboseLog: Boolean = false,
    /** -vvv: trace-level iroh internals (path/QUIC state machine). Very high volume. */
    val traceIroh: Boolean = false,
    /** Base URL of the rdz log-collection endpoint. Empty = uploads off. */
    val logServer: String = Endpoints.LOG_SERVER,
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
        _settings.value =
            Settings(
                language =
                    prefs.getString("language", null)
                        ?: if (Locale.getDefault().language == "zh") {
                            AppText.SIMPLIFIED_CHINESE
                        } else {
                            AppText.ENGLISH
                        },
                broker = prefs.getString("broker", Endpoints.BROKER)!!,
                relay = prefs.getString("relay", Endpoints.RELAY)!!,
                dataStreamWindow = prefs.getString("dataStreamWindow", "")!!,
                candidatesAllow = readList("candidatesAllow"),
                candidatesDeny = readList("candidatesDeny"),
                saveFolder = prefs.getString("saveFolder", "Envoix")!!,
                saveTreeUri = prefs.getString("saveTreeUri", "")!!,
                defaultRole = prefs.getString("defaultRole", "receive")!!,
                compressionPolicy = prefs.getString("compressionPolicy", "smart")!!,
                nearbyDisplayName =
                    DiscoveryPeerRegistry.sanitizeDisplayName(
                        prefs.getString("nearbyDisplayName", Build.MODEL),
                    ) ?: "Android device",
                nearbyVisibility = prefs.getString("nearbyVisibility", "hidden")!!,
                nearbyVisibilityExpiresAtEpochMs =
                    prefs.getLong("nearbyVisibilityExpiresAtEpochMs", 0L),
                useRoom = prefs.getBoolean("useRoom", true),
                useMdns = prefs.getBoolean("useMdns", true),
                devMode = prefs.getBoolean("devMode", false),
                verboseLog = prefs.getBoolean("verboseLog", false),
                traceIroh = prefs.getBoolean("traceIroh", false),
                logServer = prefs.getString("logServer", Endpoints.LOG_SERVER)!!,
            )
    }

    private fun readList(key: String): List<String> =
        prefs
            .getString(key, "")
            .orEmpty()
            .lines()
            .map { it.trim() }
            .filter { it.isNotEmpty() }

    fun update(transform: (Settings) -> Settings) {
        val s = transform(_settings.value)
        prefs
            .edit()
            .putString("language", s.language)
            .putString("broker", s.broker)
            .putString("relay", s.relay)
            .putString("dataStreamWindow", s.dataStreamWindow)
            .putString("candidatesAllow", s.candidatesAllow.joinToString("\n"))
            .putString("candidatesDeny", s.candidatesDeny.joinToString("\n"))
            .putString("saveFolder", s.saveFolder)
            .putString("saveTreeUri", s.saveTreeUri)
            .putString("defaultRole", s.defaultRole)
            .putString("compressionPolicy", s.compressionPolicy)
            .putString("nearbyDisplayName", s.nearbyDisplayName)
            .putString("nearbyVisibility", s.nearbyVisibility)
            .putLong("nearbyVisibilityExpiresAtEpochMs", s.nearbyVisibilityExpiresAtEpochMs)
            .putBoolean("useRoom", s.useRoom)
            .putBoolean("useMdns", s.useMdns)
            .putBoolean("devMode", s.devMode)
            .putBoolean("verboseLog", s.verboseLog)
            .putBoolean("traceIroh", s.traceIroh)
            .putString("logServer", s.logServer)
            .apply()
        _settings.value = s
    }

    fun setNearbyDisplayName(value: String): Boolean {
        val normalized = DiscoveryPeerRegistry.sanitizeDisplayName(value) ?: return false
        update { it.copy(nearbyDisplayName = normalized) }
        return true
    }

    fun setNearbyVisibility(
        mode: String,
        nowEpochMs: Long = System.currentTimeMillis(),
    ) {
        require(mode == "hidden" || mode == "everyone_10m" || mode == "foreground")
        update {
            it.copy(
                nearbyVisibility = mode,
                nearbyVisibilityExpiresAtEpochMs =
                    if (mode == "everyone_10m") nowEpochMs + TEN_MINUTES_MS else 0L,
            )
        }
    }

    private const val LOG_BASELINE = "envoix=debug,iroh=info,warn"
    private const val LOG_VERBOSE = "envoix=trace,iroh=debug,warn"
    private const val LOG_TRACE_IROH = "envoix=trace,iroh=trace,iroh_relay=debug,netwatch=debug,warn"

    /** Push the current verbosity down to the native reloadable filter. -vvv (trace
     *  iroh internals) wins over -vv (verbose) wins over the baseline. */
    fun applyLogLevel() =
        setLogLevel(
            when {
                _settings.value.traceIroh -> LOG_TRACE_IROH
                _settings.value.verboseLog -> LOG_VERBOSE
                else -> LOG_BASELINE
            },
        )

    /** Where received files go, for display: the picked SAF folder's name, else Downloads/<folder>. */
    fun saveLabel(context: Context): String {
        val s = _settings.value
        if (s.saveTreeUri.isNotBlank()) {
            val name =
                runCatching {
                    androidx.documentfile.provider.DocumentFile
                        .fromTreeUri(context, android.net.Uri.parse(s.saveTreeUri))
                        ?.name
                }.getOrNull()
            if (!name.isNullOrBlank()) return name
        }
        return "Downloads / ${s.saveFolder}"
    }

    /** Where the folder picker should open: the current custom folder if set, else
     *  the Downloads folder — so after "Reset to Downloads" it doesn't reopen at
     *  the previously picked place. */
    fun savePickerInitialUri(): android.net.Uri {
        val tree = _settings.value.saveTreeUri
        return if (tree.isNotBlank()) {
            android.net.Uri.parse(tree)
        } else {
            android.provider.DocumentsContract.buildDocumentUri(
                "com.android.externalstorage.documents",
                "primary:Download",
            )
        }
    }

    /** Persist a SAF folder pick with a durable permission grant; null clears it. */
    fun setSaveTree(
        context: Context,
        uri: android.net.Uri?,
    ) {
        if (uri == null) {
            update { it.copy(saveTreeUri = "") }
            return
        }
        val granted =
            runCatching {
                context.contentResolver.takePersistableUriPermission(
                    uri,
                    android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION or
                        android.content.Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
                )
            }.isSuccess
        // Never persist a tree we cannot durably write: a URI without the
        // grant would fail every future publish while the setting claims it
        // works. On failure the previous (working) choice stays.
        if (granted) {
            update { it.copy(saveTreeUri = uri.toString()) }
        } else {
            LogStore.append("app: SAF permission grant failed for $uri; keeping previous folder")
        }
    }

    /** The "Avoid Tailscale" toggle is a *view* over `deny`: on iff the ranges are present. */
    fun avoidsTailscale(s: Settings): Boolean = s.candidatesDeny.containsAll(TAILSCALE_CIDRS)

    /** Add/remove the Tailscale ranges in `deny` — the same source the Advanced editor edits. */
    fun setAvoidTailscale(on: Boolean) =
        update { s ->
            val deny =
                if (on) {
                    (s.candidatesDeny + TAILSCALE_CIDRS).distinct()
                } else {
                    s.candidatesDeny.filterNot { it in TAILSCALE_CIDRS }
                }
            s.copy(candidatesDeny = deny)
        }

    private const val TEN_MINUTES_MS = 10L * 60L * 1_000L
}
