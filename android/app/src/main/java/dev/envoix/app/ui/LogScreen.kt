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
import dev.envoix.app.Diagnostics
import dev.envoix.app.LogStore
import dev.envoix.app.LogUpload
import dev.envoix.app.OpLog
import dev.envoix.app.SettingsStore
import kotlinx.coroutines.launch

// The clipboard/share go over a Binder transaction (~1 MB hard cap), and the rdz
// log endpoint rejects bodies over MAX_BODY (512 KB). A -vvv session log is several
// MB, so both must be bounded to the tail — a crash always lives at the end.
private const val CLIP_MAX = Diagnostics.CLIP_MAX
private const val UPLOAD_MAX = Diagnostics.UPLOAD_MAX

@Composable
fun LogScreen(onBack: () -> Unit) {
    val colors = Envoix.colors
    val lines by LogStore.lines.collectAsState()
    val settings by SettingsStore.settings.collectAsState()
    val listState = rememberLazyListState()
    val clipboard = LocalClipboardManager.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val language = LocalAppLanguage.current
    val uiText =
        remember(language) {
            { english: String, simplifiedChinese: String ->
                AppText.value(english, simplifiedChinese, language)
            }
        }
    var showSessions by remember { mutableStateOf(false) }
    var crashPending by remember { mutableStateOf(Diagnostics.pendingCrash()) }

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
                Icon(Icons.AutoMirrored.Filled.ArrowBack, uiText("Back", "返回"), tint = colors.text)
            }
            Text(
                uiText("Logs · ${lines.size}", "日志 · ${lines.size}"),
                color = colors.text,
                fontWeight = FontWeight.Bold,
                fontSize = 18.sp,
                modifier = Modifier.weight(1f).padding(start = 4.dp),
            )
            TextButton(onClick = {
                val server = settings.logServer.trimEnd('/')
                scope.launch {
                    val key =
                        "app-" +
                            java.text
                                .SimpleDateFormat("MMddHHmmss", java.util.Locale.US)
                                .format(java.util.Date())
                    val ok =
                        server.isNotEmpty() &&
                            LogUpload.upload(
                                server,
                                key,
                                "app",
                                Diagnostics.build(Diagnostics.Kind.App),
                            )
                    Toast
                        .makeText(
                            context,
                            if (ok) uiText("Report sent → $key", "报告已发送 → $key") else uiText("Upload failed", "上传失败"),
                            Toast.LENGTH_LONG,
                        ).show()
                }
            }) { Text(uiText("Report", "报告"), color = colors.accent, fontSize = 13.sp) }
            // Dev-mode: reach the retained previous-session logs (survive relaunches),
            // for copy / upload — a native crash lives there, not in the live buffer.
            if (settings.devMode) {
                IconButton(onClick = { showSessions = true }) {
                    Icon(Icons.Default.History, uiText("Session logs", "会话日志"), tint = colors.accent)
                }
            }
            IconButton(onClick = {
                copyToClipboard(clipboard, context, LogStore.dump(), uiText("logs", "日志"), language)
            }) { Icon(Icons.Default.ContentCopy, uiText("Copy", "复制"), tint = colors.accent) }
            IconButton(onClick = {
                runCatching {
                    val intent =
                        Intent(Intent.ACTION_SEND).apply {
                            type = "text/plain"
                            putExtra(Intent.EXTRA_TEXT, tail(LogStore.dump(), CLIP_MAX))
                        }
                    context.startActivity(Intent.createChooser(intent, uiText("Share logs", "分享日志")))
                }
            }) { Icon(Icons.Default.Share, uiText("Share", "分享"), tint = colors.accent) }
            IconButton(onClick = { LogStore.clear() }) {
                Icon(Icons.Default.DeleteOutline, uiText("Clear", "清空"), tint = colors.muted)
            }
        }

        if (crashPending) {
            Row(
                Modifier
                    .fillMaxWidth()
                    .background(colors.warning.copy(alpha = 0.15f))
                    .padding(horizontal = 12.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    uiText("Previous session crashed", "上次会话发生崩溃"),
                    color = colors.warning,
                    fontSize = 13.sp,
                    fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = {
                    val server = settings.logServer.trimEnd('/')
                    scope.launch {
                        val key =
                            "crash-" +
                                java.text
                                    .SimpleDateFormat("MMddHHmmss", java.util.Locale.US)
                                    .format(java.util.Date())
                        val ok =
                            server.isNotEmpty() &&
                                LogUpload.upload(
                                    server,
                                    key,
                                    "crash",
                                    Diagnostics.build(Diagnostics.Kind.Crash),
                                )
                        Toast
                            .makeText(
                                context,
                                if (ok) uiText("Uploaded → $key", "已上传 → $key") else uiText("Upload failed", "上传失败"),
                                Toast.LENGTH_LONG,
                            ).show()
                        if (ok) {
                            Diagnostics.ackCrash()
                            crashPending = false
                        }
                    }
                }) { Text(uiText("Upload report", "上传报告"), color = colors.warning, fontSize = 13.sp) }
                TextButton(onClick = {
                    Diagnostics.ackCrash()
                    crashPending = false
                }) {
                    Text(uiText("Dismiss", "忽略"), color = colors.muted, fontSize = 13.sp)
                }
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
                    Text(uiText("Close", "关闭"), color = colors.accent)
                }
            },
            title = { Text(uiText("Session logs", "会话日志"), color = colors.text) },
            text = {
                Column {
                    // Operations breadcrumbs — the user-action trail, kept separate
                    // from the core transfer trace (the session logs below).
                    Row(
                        Modifier.fillMaxWidth().padding(vertical = 4.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Column(Modifier.weight(1f)) {
                            Text(uiText("Operations", "操作记录"), color = colors.text, fontSize = 13.sp)
                            Text(
                                uiText("what you did · recent launches", "近期启动与用户操作"),
                                color = colors.muted,
                                fontSize = 11.sp,
                            )
                        }
                        TextButton(onClick = {
                            copyToClipboard(
                                clipboard,
                                context,
                                OpLog.report(),
                                uiText("operations", "操作记录"),
                                language,
                            )
                        }) { Text(uiText("Copy", "复制"), color = colors.accent, fontSize = 13.sp) }
                        if (canUpload) {
                            TextButton(onClick = {
                                val key =
                                    "ops-" +
                                        java.text
                                            .SimpleDateFormat(
                                                "MMddHHmmss",
                                                java.util.Locale.US,
                                            ).format(java.util.Date())
                                val body = tail(OpLog.report(), UPLOAD_MAX)
                                scope.launch {
                                    val ok = LogUpload.upload(settings.logServer, key, "ops", body)
                                    Toast
                                        .makeText(
                                            context,
                                            if (ok) uiText("Uploaded → $key", "已上传 → $key") else uiText("Upload failed", "上传失败"),
                                            Toast.LENGTH_LONG,
                                        ).show()
                                }
                            }) { Text(uiText("Upload", "上传"), color = colors.accent, fontSize = 13.sp) }
                        }
                    }
                    if (sessions.isEmpty()) {
                        Text(uiText("No logs on disk yet.", "磁盘上尚无日志。"), color = colors.muted, fontSize = 13.sp)
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
                                copyToClipboard(clipboard, context, LogStore.readSession(s.file), s.label, language)
                            }) { Text(uiText("Copy", "复制"), color = colors.accent, fontSize = 13.sp) }
                            if (canUpload) {
                                TextButton(onClick = {
                                    val key =
                                        "app-" +
                                            java.text
                                                .SimpleDateFormat(
                                                    "MMddHHmmss",
                                                    java.util.Locale.US,
                                                ).format(java.util.Date())
                                    val body = tail(LogStore.readSession(s.file), UPLOAD_MAX)
                                    scope.launch {
                                        val ok = LogUpload.upload(settings.logServer, key, "app", body)
                                        Toast
                                            .makeText(
                                                context,
                                                if (ok) uiText("Uploaded → $key", "已上传 → $key") else uiText("Upload failed", "上传失败"),
                                                Toast.LENGTH_LONG,
                                            ).show()
                                    }
                                }) { Text(uiText("Upload", "上传"), color = colors.accent, fontSize = 13.sp) }
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
private fun tail(
    text: String,
    maxBytes: Int,
): String {
    val bytes = text.toByteArray(Charsets.UTF_8)
    if (bytes.size <= maxBytes) return text
    val note = "[… truncated — last ${maxBytes / 1024} KB of ${bytes.size / 1024} KB]\n"
    return note + String(bytes, bytes.size - maxBytes, maxBytes, Charsets.UTF_8)
}

/** Copy the log's tail to the clipboard, guarded so a multi-MB log can never throw
 *  TransactionTooLargeException (the crash the naive copy caused). */
private fun copyToClipboard(
    clipboard: ClipboardManager,
    context: Context,
    text: String,
    label: String,
    language: String,
) {
    val body = tail(text, CLIP_MAX)
    val ok = runCatching { clipboard.setText(AnnotatedString(body)) }.isSuccess
    val msg =
        when {
            !ok -> AppText.value("Copy failed", "复制失败", language)
            body.length != text.length ->
                AppText.value(
                    "Copied last ${CLIP_MAX / 1024} KB of $label",
                    "已复制 $label 的最后 ${CLIP_MAX / 1024} KB",
                    language,
                )
            else -> AppText.value("Copied $label", "已复制$label", language)
        }
    Toast.makeText(context, msg, Toast.LENGTH_SHORT).show()
}

@Composable
private fun lineColor(line: String) =
    when {
        line.contains(" ERROR ") || line.startsWith("FATAL") -> Envoix.colors.danger
        line.contains(" WARN ") -> Envoix.colors.warning
        line.contains(" INFO ") -> Envoix.colors.text
        else -> Envoix.colors.muted
    }
