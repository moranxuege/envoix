package dev.envoix.app

import android.Manifest
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.OpenableColumns
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
import androidx.lifecycle.lifecycleScope
import dev.envoix.app.ui.EnvoixTheme
import dev.envoix.app.ui.HomeScreen
import dev.envoix.app.ui.LogScreen
import dev.envoix.app.ui.SettingsScreen
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

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
                            onReceive = { c, b, r -> vm.startReceive(c, b, r) },
                            onSend = { c, b, r, uri ->
                                lifecycleScope.launch {
                                    val path = withContext(Dispatchers.IO) { copyToCache(uri) }
                                    if (path != null) vm.startSend(c, path, b, r)
                                }
                            },
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

    /** Copy a picked content Uri into a real cache path the core can read. */
    private fun copyToCache(uri: Uri): String? {
        val name = displayName(uri) ?: "upload.bin"
        val dir = File(cacheDir, "send").apply { mkdirs() }
        val out = File(dir, name)
        return runCatching {
            contentResolver.openInputStream(uri)!!.use { input ->
                out.outputStream().use { input.copyTo(it) }
            }
            out.absolutePath
        }.getOrNull()
    }

    private fun displayName(uri: Uri): String? =
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
}
