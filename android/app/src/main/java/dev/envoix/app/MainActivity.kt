package dev.envoix.app

import android.Manifest
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.core.content.IntentCompat
import dev.envoix.app.ui.AppText
import dev.envoix.app.ui.DiscoveryScreen
import dev.envoix.app.ui.EnvoixTheme
import dev.envoix.app.ui.HomeScreen
import dev.envoix.app.ui.LocalAppLanguage
import dev.envoix.app.ui.LogScreen
import dev.envoix.app.ui.SettingsScreen
import kotlinx.coroutines.flow.MutableStateFlow

private enum class Screen { Home, Discovery, Logs, Settings }

class MainActivity : ComponentActivity() {
    private val vm: TransferViewModel by viewModels()
    private val sharedUris = MutableStateFlow<List<Uri>>(emptyList())
    private var inboundInvite by androidx.compose.runtime.mutableStateOf<String?>(null)

    private val requestNotif =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            requestNotif.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
        TransferService.restoreAll(this)
        captureSharedUris(intent)
        captureInvite(intent)
        setContent {
            val settings by SettingsStore.settings.collectAsState()
            CompositionLocalProvider(LocalAppLanguage provides settings.language) {
                EnvoixTheme {
                    var screen by androidx.compose.runtime.remember {
                        androidx.compose.runtime.mutableStateOf(Screen.Home)
                    }
                    if (screen != Screen.Home) BackHandler { screen = Screen.Home }
                    when (screen) {
                        Screen.Discovery ->
                            DiscoveryScreen(
                                onBack = { screen = Screen.Home },
                                onReceive = { c, b, r, qr, copyApproved ->
                                    screen = Screen.Home
                                    vm.startReceive(c, b, r, qr, copyApproved)
                                },
                                onSend = { c, b, r, jobId, qr ->
                                    screen = Screen.Home
                                    vm.startSend(c, jobId, b, r, qr)
                                },
                            )
                        Screen.Logs -> LogScreen(onBack = { screen = Screen.Home })
                        Screen.Settings -> SettingsScreen(onBack = { screen = Screen.Home })
                        Screen.Home -> {
                            val transfers by vm.transfers.collectAsState()
                            val incomingShares by sharedUris.collectAsState()
                            HomeScreen(
                                transfers = transfers,
                                initialSharedUris = incomingShares,
                                onSharedUrisConsumed = { sharedUris.value = emptyList() },
                                onReceive = { c, b, r, qr, copyApproved ->
                                    inboundInvite = null
                                    vm.startReceive(c, b, r, qr, copyApproved)
                                },
                                onSend = { c, b, r, jobId, qr ->
                                    inboundInvite = null
                                    vm.startSend(c, jobId, b, r, qr)
                                },
                                onPauseResume = { vm.pauseResume(it) },
                                onApproveReceive = { vm.approveReceive(it) },
                                onCancel = { vm.cancel(it) },
                                onRemove = { vm.remove(it) },
                                onOpenDiscovery = { screen = Screen.Discovery },
                                onOpenLogs = { screen = Screen.Logs },
                                onOpenSettings = { screen = Screen.Settings },
                                onOpen = { openReceived(it) },
                                onShare = { shareReceived(it) },
                                initialPairingInput = inboundInvite,
                            )
                        }
                    }
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        captureSharedUris(intent)
        captureInvite(intent)
    }

    private fun captureInvite(intent: Intent?) {
        val value = intent?.dataString ?: return
        if (value.startsWith("envoix://invite/v2/") &&
            InviteCodec.parseForRouting(value) != null
        ) {
            inboundInvite = value
        }
    }

    private fun captureSharedUris(intent: Intent?) {
        val action = intent?.action ?: return
        val uris =
            when (action) {
                Intent.ACTION_SEND ->
                    listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
                Intent.ACTION_SEND_MULTIPLE ->
                    IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
                else -> emptyList()
            }
        if (uris.isNotEmpty()) sharedUris.value = uris.distinct()
    }

    /** Open a received file (a Downloads content Uri) in whatever app handles it. */
    private fun openReceived(t: Transfer) {
        val uri = t.savedUri?.let { Uri.parse(it) } ?: return
        val view =
            Intent(Intent.ACTION_VIEW).apply {
                setDataAndType(uri, "*/*")
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
        runCatching {
            startActivity(
                Intent.createChooser(
                    view,
                    AppText.value("Open with", "打开方式", SettingsStore.settings.value.language),
                ),
            )
        }
    }

    private fun shareReceived(t: Transfer) {
        val uris = t.savedUris.map(Uri::parse)
        if (uris.isEmpty()) return
        val share =
            if (uris.size == 1) {
                Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, uris[0])
            } else {
                Intent(Intent.ACTION_SEND_MULTIPLE).putParcelableArrayListExtra(
                    Intent.EXTRA_STREAM,
                    ArrayList(uris),
                )
            }
        share.type = "*/*"
        share.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        runCatching {
            startActivity(
                Intent.createChooser(
                    share,
                    AppText.value("Share received items", "分享已接收项目", SettingsStore.settings.value.language),
                ),
            )
        }
    }
}
