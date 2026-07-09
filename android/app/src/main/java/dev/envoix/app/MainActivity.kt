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
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import dev.envoix.app.ui.EnvoixTheme
import dev.envoix.app.ui.HomeScreen
import dev.envoix.app.ui.LogScreen
import dev.envoix.app.ui.SettingsScreen
import kotlinx.coroutines.launch

private enum class Screen { Home, Logs, Settings }

class MainActivity : ComponentActivity() {

    private val vm: TransferViewModel by viewModels()

    private val requestNotif =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            requestNotif.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
        setContent {
            EnvoixTheme {
                var screen by remember { mutableStateOf(Screen.Home) }
                if (screen != Screen.Home) BackHandler { screen = Screen.Home }
                when (screen) {
                    Screen.Logs -> LogScreen(onBack = { screen = Screen.Home })
                    Screen.Settings -> SettingsScreen(onBack = { screen = Screen.Home })
                    Screen.Home -> {
                        val transfers by vm.transfers.collectAsState()
                        HomeScreen(
                            transfers = transfers,
                            onReceive = { c, b, r, qr -> vm.startReceive(c, b, r, qr) },
                            // Staging (the content:// -> real-path copy) happens in
                            // the service, visibly, so the card appears instantly.
                            onSend = { c, b, r, uri, qr -> vm.startSend(c, uri.toString(), b, r, qr) },
                            onPauseResume = { vm.pauseResume(it) },
                            onCancel = { vm.cancel(it) },
                            onRemove = { vm.remove(it) },
                            onOpenLogs = { screen = Screen.Logs },
                            onOpenSettings = { screen = Screen.Settings },
                            onOpen = { openReceived(it) },
                        )
                    }
                }
            }
        }
    }

    /** Open a received file (a Downloads content Uri) in whatever app handles it. */
    private fun openReceived(t: Transfer) {
        val uri = t.savedUri?.let { Uri.parse(it) } ?: return
        val view = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, "*/*")
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        runCatching { startActivity(Intent.createChooser(view, "Open with")) }
    }

}
