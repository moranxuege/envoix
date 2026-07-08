package dev.envoix.app.ui

import android.content.Context
import android.content.Intent
import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.DeleteOutline
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.ClipboardManager
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.LogStore
import dev.envoix.app.LogUpload
import dev.envoix.app.SettingsStore
import kotlinx.coroutines.launch

// The clipboard/share go over a Binder transaction (~1 MB hard cap), and the rdz
// log endpoint rejects bodies over MAX_BODY (512 KB). A -vvv session log is several
// MB, so both must be bounded to the tail — a crash always lives at the end.
private const val CLIP_MAX = 256 * 1024
private const val UPLOAD_MAX = 480 * 1024

@Composable
fun LogScreen(onBack: () -> Unit) {
    val colors = Envoix.colors
    val lines by LogStore.lines.collectAsState()
    val settings by SettingsStore.settings.collectAsState()
    val listState = rememberLazyListState()
    val clipboard = LocalClipboardManager.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var showSessions by remember { mutableStateOf(false) }

    LaunchedEffect(lines.size) {
        if (lines.isNotEmpty()) listState.scrollToItem(lines.size - 1)
    }

    Column(
        Modifier
            .fillMaxSize()
            .background(colors.bg)
            .statusBarsPadding(),
    ) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onBack) {
                Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back", tint = colors.text)
            }
            Text(
                "Logs · ${lines.size}",
                color = colors.text,
                fontWeight = FontWeight.Bold,
                fontSize = 18.sp,
                modifier = Modifier.weight(1f).padding(start = 4.dp),
            )
            // Dev-mode: reach the retained previous-session logs (survive relaunches),
            // for copy / upload — a native crash lives there, not in the live buffer.
            if (settings.devMode) {
                IconButton(onClick = { showSessions = true }) {
                    Icon(Icons.Default.History, "Session logs", tint = colors.accent)
                }
            }
            IconButton(onClick = {
                copyToClipboard(clipboard, context, LogStore.dump(), "logs")
            }) { Icon(Icons.Default.ContentCopy, "Copy", tint = colors.accent) }
            IconButton(onClick = {
                runCatching {
                    val intent = Intent(Intent.ACTION_SEND).apply {
                        type = "text/plain"
                        putExtra(Intent.EXTRA_TEXT, tail(LogStore.dump(), CLIP_MAX))
                    }
                    context.startActivity(Intent.createChooser(intent, "Share logs"))
                }
            }) { Icon(Icons.Default.Share, "Share", tint = colors.accent) }
            IconButton(onClick = { LogStore.clear() }) {
                Icon(Icons.Default.DeleteOutline, "Clear", tint = colors.muted)
            }
        }

        LazyColumn(
            state = listState,
            modifier = Modifier.fillMaxSize().padding(horizontal = 12.dp),
            contentPadding = PaddingValues(bottom = 24.dp),
        ) {
            items(lines) { line ->
                Text(
                    line,
                    color = lineColor(line),
                    fontFamily = FontFamily.Monospace,
                    fontSize = 11.sp,
                    lineHeight = 15.sp,
                    modifier = Modifier.padding(vertical = 1.dp),
                )
            }
        }
    }

    if (showSessions) {
        // Snapshot the retained on-disk session logs when the dialog opens.
        val sessions = remember { LogStore.sessions() }
        val canUpload = settings.devMode && settings.logServer.isNotBlank()
        AlertDialog(
            onDismissRequest = { showSessions = false },
            confirmButton = {
                TextButton(onClick = { showSessions = false }) {
                    Text("Close", color = colors.accent)
                }
            },
            title = { Text("Session logs", color = colors.text) },
            text = {
                Column {
                    if (sessions.isEmpty()) {
                        Text("No logs on disk yet.", color = colors.muted, fontSize = 13.sp)
                    }
                    sessions.forEach { s ->
                        Row(
                            Modifier.fillMaxWidth().padding(vertical = 4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f)) {
                                Text(s.label, color = colors.text, fontSize = 13.sp)
                                Text(
                                    "${(s.bytes + 1023) / 1024} KB",
                                    color = colors.muted,
                                    fontSize = 11.sp,
                                )
                            }
                            TextButton(onClick = {
                                copyToClipboard(clipboard, context, LogStore.readSession(s.file), s.label)
                            }) { Text("Copy", color = colors.accent, fontSize = 13.sp) }
                            if (canUpload) {
                                TextButton(onClick = {
                                    val key = "app-" + java.text.SimpleDateFormat(
                                        "MMddHHmmss", java.util.Locale.US,
                                    ).format(java.util.Date())
                                    val body = tail(LogStore.readSession(s.file), UPLOAD_MAX)
                                    scope.launch {
                                        val ok = LogUpload.upload(settings.logServer, key, "app", body)
                                        Toast.makeText(
                                            context,
                                            if (ok) "Uploaded → $key" else "Upload failed",
                                            Toast.LENGTH_LONG,
                                        ).show()
                                    }
                                }) { Text("Upload", color = colors.accent, fontSize = 13.sp) }
                            }
                        }
                    }
                }
            },
            containerColor = colors.surface,
        )
    }
}

/** The last [maxBytes] UTF-8 bytes of [text], prefixed with a note when clipped.
 *  A crash lives at the end of a log, so the tail is what matters — and both the
 *  clipboard (Binder) and the rdz POST reject bodies over ~0.5-1 MB. */
private fun tail(text: String, maxBytes: Int): String {
    val bytes = text.toByteArray(Charsets.UTF_8)
    if (bytes.size <= maxBytes) return text
    val note = "[… truncated — last ${maxBytes / 1024} KB of ${bytes.size / 1024} KB]\n"
    return note + String(bytes, bytes.size - maxBytes, maxBytes, Charsets.UTF_8)
}

/** Copy the log's tail to the clipboard, guarded so a multi-MB log can never throw
 *  TransactionTooLargeException (the crash the naive copy caused). */
private fun copyToClipboard(clipboard: ClipboardManager, context: Context, text: String, label: String) {
    val body = tail(text, CLIP_MAX)
    val ok = runCatching { clipboard.setText(AnnotatedString(body)) }.isSuccess
    val msg = when {
        !ok -> "Copy failed"
        body.length != text.length -> "Copied last ${CLIP_MAX / 1024} KB of $label"
        else -> "Copied $label"
    }
    Toast.makeText(context, msg, Toast.LENGTH_SHORT).show()
}

@Composable
private fun lineColor(line: String) = when {
    line.contains(" ERROR ") || line.startsWith("FATAL") -> Envoix.colors.danger
    line.contains(" WARN ") -> Envoix.colors.warning
    line.contains(" INFO ") -> Envoix.colors.text
    else -> Envoix.colors.muted
}
