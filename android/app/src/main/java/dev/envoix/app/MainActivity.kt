package dev.envoix.app

import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import androidx.activity.ComponentActivity
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
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

class MainActivity : ComponentActivity() {

    private val vm: TransferViewModel by viewModels()
    private var pendingSendRoom: String? = null

    private val pickFile =
        registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            val room = pendingSendRoom ?: return@registerForActivityResult
            pendingSendRoom = null
            if (uri == null) return@registerForActivityResult
            lifecycleScope.launch {
                val path = withContext(Dispatchers.IO) { copyToCache(uri) }
                if (path != null) vm.startSend(room, path)
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            EnvoixTheme {
                var showLogs by remember { mutableStateOf(false) }
                if (showLogs) {
                    LogScreen(onBack = { showLogs = false })
                } else {
                    val transfers by vm.transfers.collectAsState()
                    HomeScreen(
                        transfers = transfers,
                        onReceive = { room -> vm.startReceive(room) },
                        onSend = { room ->
                            pendingSendRoom = room
                            pickFile.launch(arrayOf("*/*"))
                        },
                        onCancel = { vm.cancel(it) },
                        onDismiss = { vm.dismiss(it) },
                        onOpenLogs = { showLogs = true },
                    )
                }
            }
        }
    }

    /** Copy a picked content Uri into a real cache path the CLI can read. */
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
