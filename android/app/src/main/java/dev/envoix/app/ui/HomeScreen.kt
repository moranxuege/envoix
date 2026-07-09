package dev.envoix.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.OpenInNew
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.Diagnostics
import dev.envoix.app.Direction
import dev.envoix.app.humanBytes
import dev.envoix.app.smoothedBps
import dev.envoix.app.LogUpload
import dev.envoix.app.Room
import dev.envoix.app.SettingsStore
import dev.envoix.app.Status
import dev.envoix.app.Transfer
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(
    transfers: List<Transfer>,
    onReceive: (code: String, broker: String, relay: String, qrPayload: String?) -> Unit,
    onSend: (code: String, broker: String, relay: String, file: android.net.Uri, qrPayload: String?) -> Unit,
    onPauseResume: (Long) -> Unit,
    onCancel: (Long) -> Unit,
    onRemove: (Long) -> Unit,
    onOpenLogs: () -> Unit,
    onOpenSettings: () -> Unit,
    onOpen: (Transfer) -> Unit,
) {
    val colors = Envoix.colors
    var sheetOpen by remember { mutableStateOf(false) }
    val expanded = remember { mutableStateListOf<Long>() }
    val listState = rememberLazyListState()
    // A just-created transfer lands at the top (newest-first sort); bring it
    // into view instead of leaving it above the fold.
    val newestId = transfers.maxOfOrNull { it.id } ?: -1L
    LaunchedEffect(newestId) {
        if (newestId >= 0) listState.animateScrollToItem(0)
    }
    val active = transfers.count { it.status == Status.Connecting || it.status == Status.Transferring }

    Scaffold(
        containerColor = colors.bg,
        floatingActionButton = {
            ExtendedFloatingActionButton(
                onClick = { sheetOpen = true },
                containerColor = colors.accent,
                contentColor = androidx.compose.ui.graphics.Color.White,
            ) {
                Icon(Icons.Default.Add, contentDescription = null)
                Spacer(Modifier.width(8.dp))
                Text("New transfer", fontWeight = FontWeight.SemiBold)
            }
        },
    ) { inner ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(inner)
                .padding(horizontal = 20.dp),
        ) {
            Header(active, onOpenLogs, onOpenSettings)
            Spacer(Modifier.height(12.dp))
            if (transfers.isEmpty()) {
                EmptyState()
            } else {
                LazyColumn(
                    state = listState,
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                    contentPadding = PaddingValues(top = 4.dp, bottom = 96.dp),
                ) {
                    items(transfers.sortedByDescending { it.id }, key = { it.id }) { t ->
                        TransferCard(
                            t = t,
                            expanded = t.id in expanded,
                            onToggleDetail = { if (it in expanded) expanded.remove(it) else expanded.add(it) },
                            onPauseResume = onPauseResume,
                            onCancel = onCancel,
                            onRemove = onRemove,
                            onOpen = onOpen,
                        )
                    }
                }
            }
        }
    }

    if (sheetOpen) {
        ModalBottomSheet(
            onDismissRequest = { sheetOpen = false },
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            containerColor = colors.surface,
        ) {
            NewTransferSheet(
                onReceive = { c, b, r, qr -> sheetOpen = false; onReceive(c, b, r, qr) },
                onSend = { c, b, r, uri, qr -> sheetOpen = false; onSend(c, b, r, uri, qr) },
            )
        }
    }
}

@Composable
private fun Header(active: Int, onOpenLogs: () -> Unit, onOpenSettings: () -> Unit) {
    val colors = Envoix.colors
    Row(
        Modifier
            .fillMaxWidth()
            .padding(top = 16.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("Envoix", color = colors.text, fontSize = 32.sp, fontWeight = FontWeight.ExtraBold)
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (active > 0) {
                Pill(text = "$active active", fg = colors.success, bg = colors.successSoft)
                Spacer(Modifier.width(8.dp))
            }
            Text(
                "Logs",
                color = colors.accent,
                fontWeight = FontWeight.Bold,
                fontSize = 14.sp,
                modifier = Modifier
                    .clip(RoundedCornerShape(10.dp))
                    .clickable(onClick = onOpenLogs)
                    .padding(horizontal = 10.dp, vertical = 6.dp),
            )
            Icon(
                Icons.Default.Settings,
                contentDescription = "Settings",
                tint = colors.accent,
                modifier = Modifier
                    .clip(CircleShape)
                    .clickable(onClick = onOpenSettings)
                    .padding(6.dp)
                    .size(22.dp),
            )
        }
    }
}

@Composable
private fun EmptyState() {
    val colors = Envoix.colors
    Box(Modifier.fillMaxWidth().padding(top = 80.dp), contentAlignment = Alignment.Center) {
        Text(
            "No transfers yet.\nTap “New transfer” to send or receive.",
            color = colors.muted,
            fontSize = 15.sp,
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalFoundationApi::class)
@Composable
private fun TransferCard(
    t: Transfer,
    expanded: Boolean,
    onToggleDetail: (Long) -> Unit,
    onPauseResume: (Long) -> Unit,
    onCancel: (Long) -> Unit,
    onRemove: (Long) -> Unit,
    onOpen: (Transfer) -> Unit,
) {
    val colors = Envoix.colors
    val failed = t.status == Status.Failed
    val cancelled = t.status == Status.Cancelled
    val dismissState = rememberSwipeToDismissBoxState(
        confirmValueChange = {
            if (it == SwipeToDismissBoxValue.EndToStart) { onRemove(t.id); true } else false
        },
    )
    SwipeToDismissBox(
        state = dismissState,
        enableDismissFromStartToEnd = false,
        enableDismissFromEndToStart = true,
        backgroundContent = {
            Row(
                Modifier.fillMaxSize().clip(RoundedCornerShape(16.dp)).background(colors.danger)
                    .padding(horizontal = 22.dp),
                horizontalArrangement = Arrangement.End,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(Icons.Default.Delete, null, tint = Color.White, modifier = Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                Text("Remove", color = Color.White, fontWeight = FontWeight.Bold, fontSize = 14.sp)
            }
        },
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(16.dp))
                .background(if (cancelled) colors.line else colors.surface)
                .border(1.dp, colors.line, RoundedCornerShape(16.dp))
                .combinedClickable(
                    interactionSource = remember { MutableInteractionSource() },
                    indication = null, // the drawer expanding is feedback enough; the
                    // default ripple fills the whole card, which looks heavy
                    onClick = { if (t.status == Status.Completed && t.savedUri != null) onOpen(t) },
                    onLongClick = { onToggleDetail(t.id) },
                ),
        ) {
            if ((t.status == Status.Waiting || t.status == Status.Connecting) && t.qrPayload != null) {
                WaitingBody(t, onCancel)
            } else {
            Row(Modifier.fillMaxWidth().padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            title(t),
                            color = if (cancelled) colors.muted else colors.text,
                            fontSize = 16.sp,
                            fontWeight = FontWeight.Bold,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f, fill = false),
                        )
                        Spacer(Modifier.width(8.dp))
                        PathBadge(t)
                    }
                    Text(
                        subtitle(t),
                        color = colors.muted,
                        fontSize = 13.sp,
                        fontFamily = FontFamily.Monospace,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Spacer(Modifier.height(10.dp))
                    LinearProgressIndicator(
                        progress = { fraction(t) },
                        modifier = Modifier.fillMaxWidth().height(8.dp).clip(CircleShape),
                        color = when {
                            failed -> colors.danger
                            t.status == Status.Paused || t.status == Status.Unconfirmed -> colors.warning
                            cancelled -> colors.muted
                            else -> colors.accent
                        },
                        trackColor = colors.line.copy(alpha = 0.6f),
                    )
                    Spacer(Modifier.height(10.dp))
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                        Stat(speedText(t)); Stat(etaText(t)); Stat(sizeText(t))
                    }
                    if (failed && t.error != null) {
                        Spacer(Modifier.height(6.dp))
                        Text(t.error, color = colors.danger, fontSize = 12.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    }
                    if (t.status == Status.Unconfirmed) {
                        Spacer(Modifier.height(6.dp))
                        Text(
                            "All bytes sent — peer didn't confirm receipt. It likely arrived; tap ↻ to re-confirm.",
                            color = colors.warning, fontSize = 12.sp, maxLines = 2, overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
                Spacer(Modifier.width(10.dp))
                CardControls(t, onPauseResume, onCancel, onOpen)
            }
            }
            if (expanded) DetailDrawer(t)
        }
    }
}

@Composable
private fun CardControls(
    t: Transfer,
    onPauseResume: (Long) -> Unit,
    onCancel: (Long) -> Unit,
    onOpen: (Transfer) -> Unit,
) {
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        when (t.status) {
            Status.Waiting, Status.Connecting, Status.Verifying,
            Status.Transferring, Status.Confirming -> {
                CircleBtn(Icons.Default.Pause, filled = true) { onPauseResume(t.id) }
                CircleBtn(Icons.Default.Close, filled = false) { onCancel(t.id) }
            }
            Status.Paused -> {
                CircleBtn(Icons.Default.PlayArrow, filled = true) { onPauseResume(t.id) }
                CircleBtn(Icons.Default.Close, filled = false) { onCancel(t.id) }
            }
            Status.Failed, Status.Unconfirmed, Status.Cancelled ->
                CircleBtn(Icons.Default.Refresh, filled = true) { onPauseResume(t.id) }
            Status.Completed -> {
                if (t.savedUri != null) CircleBtn(Icons.Default.OpenInNew, filled = false) { onOpen(t) }
                // RECEIVER, and ONLY while the confirmation duty is open (the
                // receipt has not reached the rdz): the manual fallback for
                // serving the peer's re-verify. Once delivered - retired, no ↻.
                if (t.direction == Direction.Receive && !t.proofDelivered)
                    CircleBtn(Icons.Default.Refresh, filled = false) { onPauseResume(t.id) }
            }
        }
    }
}

@Composable
private fun CircleBtn(icon: ImageVector, filled: Boolean, onClick: () -> Unit) {
    val colors = Envoix.colors
    Box(
        Modifier
            .size(38.dp)
            .clip(CircleShape)
            .then(
                if (filled) Modifier.background(colors.accent)
                else Modifier.border(1.5.dp, colors.line, CircleShape)
            )
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, null, tint = if (filled) Color.White else colors.muted, modifier = Modifier.size(18.dp))
    }
}

/** The waiting-to-pair variant of a card: shows the QR + code for a peer to scan
 *  or type, with only a Cancel action. Used for initiated sessions until they pair,
 *  then the card becomes the normal progress variant. */
@Composable
private fun WaitingBody(t: Transfer, onCancel: (Long) -> Unit) {
    val colors = Envoix.colors
    val settings by SettingsStore.settings.collectAsState()
    Column(Modifier.fillMaxWidth().padding(16.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
            Column(Modifier.weight(1f)) {
                Text(
                    if (t.direction == Direction.Send) "Waiting to send" else "Waiting to receive",
                    color = colors.text, fontSize = 16.sp, fontWeight = FontWeight.Bold,
                )
                Text(
                    if (t.direction == Direction.Send) "Sending ${t.fileName ?: "a file"}"
                    else "Saving to Downloads/${settings.saveFolder}",
                    color = colors.muted, fontSize = 13.sp,
                    maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
            }
            CircleBtn(Icons.Default.Close, filled = false) { onCancel(t.id) }
        }
        Spacer(Modifier.height(14.dp))
        Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
            QrCode(t.qrPayload!!, 172.dp)
        }
        Spacer(Modifier.height(10.dp))
        val clip = LocalClipboardManager.current
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.Center,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                t.room,
                color = colors.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold,
                fontFamily = FontFamily.Monospace,
            )
            Spacer(Modifier.width(8.dp))
            Icon(
                Icons.Default.ContentCopy, "Copy code",
                tint = colors.muted,
                modifier = Modifier.clip(CircleShape)
                    .clickable { clip.setText(AnnotatedString(t.room)) }
                    .padding(6.dp).size(18.dp),
            )
        }
        Spacer(Modifier.height(2.dp))
        Text(
            "Scan or enter this code",
            color = colors.muted, fontSize = 12.sp,
            modifier = Modifier.fillMaxWidth(),
            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
        )
    }
}

/** Expanded on long-press: speed history, key details, and this transfer's log. */
@Composable
private fun DetailDrawer(t: Transfer) {
    val colors = Envoix.colors
    Column(Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, bottom = 14.dp)) {
        HorizontalDivider(color = colors.line)
        if (t.speedHistory.size >= 2) {
            val peak = t.speedHistory.maxOrNull() ?: 0.0
            val avg = t.avgBps
            Row(
                Modifier.fillMaxWidth().padding(top = 10.dp, bottom = 4.dp),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    "SPEED",
                    color = colors.muted, fontSize = 11.sp,
                    fontWeight = FontWeight.Bold, letterSpacing = 1.sp,
                )
                Text(
                    "avg ${humanBps(avg)} · peak ${humanBps(peak)}",
                    color = colors.muted, fontSize = 11.sp, fontFamily = FontFamily.Monospace,
                )
            }
            SpeedChart(t.speedHistory, t.avgBps)
        }
        Spacer(Modifier.height(4.dp))
        DetailRow("Room", t.room)
        if (t.pathAddr != null) DetailRow("Path", "${t.pathType ?: "—"} · ${t.pathAddr}")
        DetailRow("Transferred", "${humanBytes(t.bytes)} / ${humanBytes(t.total)}")
        if (t.log.isNotEmpty()) {
            val clip = LocalClipboardManager.current
            var copied by remember(t.id) { mutableStateOf(false) }
            val settings by SettingsStore.settings.collectAsState()
            val scope = rememberCoroutineScope()
            var upload by remember(t.id) { mutableStateOf("") }
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                DrawerLabel("This transfer's log")
                Row(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    if (settings.devMode && settings.logServer.isNotBlank()) {
                        PillButton(upload.ifEmpty { "Upload" }) {
                            upload = "Uploading…"
                            scope.launch {
                                val ok = LogUpload.upload(
                                    settings.logServer,
                                    Room(t.room).id,
                                    if (t.direction == Direction.Send) "send" else "receive",
                                    Diagnostics.build(Diagnostics.Kind.Transfer, t.id),
                                )
                                upload = if (ok) "Uploaded ✓" else "Failed"
                            }
                        }
                    }
                    PillButton(if (copied) "Copied ✓" else "Copy") {
                        // The full durable log via the one assembler (clip-capped).
                        runCatching {
                            clip.setText(AnnotatedString(
                                Diagnostics.build(Diagnostics.Kind.Transfer, t.id, Diagnostics.CLIP_MAX)
                            ))
                        }
                        copied = true
                    }
                }
            }
            Spacer(Modifier.height(6.dp))
            LogBox(t.log)
        }
    }
}

@Composable
private fun DrawerLabel(text: String) {
    Text(
        text,
        color = Envoix.colors.muted,
        fontSize = 11.sp,
        fontWeight = FontWeight.Bold,
        letterSpacing = 1.sp,
        modifier = Modifier.padding(top = 10.dp, bottom = 4.dp),
    )
}

@Composable
private fun PillButton(text: String, onClick: () -> Unit) {
    val colors = Envoix.colors
    Text(
        text,
        color = colors.accent, fontSize = 12.sp, fontWeight = FontWeight.Bold,
        modifier = Modifier.clip(RoundedCornerShape(8.dp)).clickable(onClick = onClick)
            .background(colors.accentSoft).padding(horizontal = 10.dp, vertical = 5.dp),
    )
}

@Composable
private fun SpeedChart(history: List<Double>, avgBps: Double) {
    val accent = Envoix.colors.accent
    val muted = Envoix.colors.muted
    Canvas(Modifier.fillMaxWidth().height(50.dp)) {
        val n = history.size
        if (n < 2) return@Canvas
        val max = (history.maxOrNull() ?: 1.0).coerceAtLeast(1.0)
        val avg = avgBps.coerceIn(0.0, max)
        val w = size.width
        val h = size.height
        fun px(i: Int) = w * i / (n - 1)
        fun py(v: Double) = (h - h * (v / max)).toFloat()
        val line = Path()
        val area = Path().apply { moveTo(0f, h) }
        history.forEachIndexed { i, v ->
            val x = px(i)
            val y = py(v)
            if (i == 0) line.moveTo(x, y) else line.lineTo(x, y)
            area.lineTo(x, y)
        }
        area.lineTo(w, h)
        area.close()
        drawPath(area, accent.copy(alpha = 0.14f))
        // dashed avg reference line — the top edge of the chart is the peak
        drawLine(
            muted.copy(alpha = 0.55f),
            Offset(0f, py(avg)), Offset(w, py(avg)),
            strokeWidth = 1.dp.toPx(),
            pathEffect = PathEffect.dashPathEffect(floatArrayOf(6f, 6f)),
        )
        drawPath(line, accent, style = Stroke(width = 2.5.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round))
        drawCircle(accent, radius = 3.dp.toPx(), center = Offset(px(n - 1), py(history.last())))
    }
}

@Composable
private fun LogBox(log: List<String>) {
    val colors = Envoix.colors
    Column(
        Modifier
            .fillMaxWidth()
            .heightIn(max = 132.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(colors.bg)
            .border(1.dp, colors.line, RoundedCornerShape(10.dp))
            .verticalScroll(rememberScrollState())
            .padding(10.dp),
    ) {
        Text(
            log.joinToString("\n"),
            color = colors.muted,
            fontSize = 11.sp,
            fontFamily = FontFamily.Monospace,
            lineHeight = 17.sp,
        )
    }
}

@Composable
private fun DetailRow(label: String, value: String) {
    val colors = Envoix.colors
    Row(Modifier.fillMaxWidth().padding(vertical = 3.dp), horizontalArrangement = Arrangement.SpaceBetween) {
        Text(label, color = colors.muted, fontSize = 12.sp)
        Text(
            value, color = colors.text, fontSize = 12.sp, fontFamily = FontFamily.Monospace,
            maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false).padding(start = 12.dp),
        )
    }
}

@Composable
private fun PathBadge(t: Transfer) {
    val colors = Envoix.colors
    val (label, fg, bg) = when {
        t.status == Status.Completed -> Triple("Done", colors.success, colors.successSoft)
        t.status == Status.Unconfirmed ->
            Triple("Sent · unconfirmed", colors.warning, colors.warning.copy(alpha = 0.14f))
        t.status == Status.Failed -> Triple("Failed", colors.danger, colors.danger.copy(alpha = 0.12f))
        t.status == Status.Cancelled -> Triple("Cancelled", colors.muted, colors.line.copy(alpha = 0.5f))
        t.status == Status.Paused -> Triple("Paused", colors.warning, colors.warning.copy(alpha = 0.14f))
        t.status == Status.Waiting -> Triple("Waiting", colors.accent, colors.accentSoft)
        t.status == Status.Verifying -> Triple("Verifying", colors.accent, colors.accentSoft)
        t.status == Status.Confirming -> Triple("Confirming", colors.accent, colors.accentSoft)
        t.pathType == "relay" -> Triple("Relay", colors.accent, colors.accentSoft)
        t.pathType == "direct" -> Triple("Direct", colors.accent, colors.accentSoft)
        // pre-connection, path unknown: say what is HAPPENING, never "…"
        else -> Triple("Pairing", colors.accent, colors.accentSoft)
    }
    Pill(label, fg, bg)
}

@Composable
private fun Pill(text: String, fg: androidx.compose.ui.graphics.Color, bg: androidx.compose.ui.graphics.Color) {
    Text(
        text,
        color = fg,
        fontSize = 12.sp,
        fontWeight = FontWeight.Bold,
        modifier = Modifier
            .clip(CircleShape)
            .background(bg)
            .padding(horizontal = 10.dp, vertical = 4.dp),
    )
}

@Composable
private fun Stat(text: String) {
    Text(text, color = Envoix.colors.text, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
}

/* ---- formatting helpers ---- */

private fun title(t: Transfer): String {
    val arrow = if (t.direction == Direction.Send) "↑" else "↓"
    val name = t.fileName ?: if (t.direction == Direction.Send) "file" else "incoming"
    return "$arrow $name"
}

private fun subtitle(t: Transfer): String = when {
    t.status == Status.Completed && t.savedUri != null -> "Saved to Downloads · tap to open"
    t.pathAddr != null -> t.pathAddr
    else -> "room ${t.room}"
}

private fun fraction(t: Transfer): Float {
    if (t.status == Status.Completed) return 1f
    if (t.total <= 0) return 0f
    return (t.bytes.toFloat() / t.total.toFloat()).coerceIn(0f, 1f)
}

private fun speedText(t: Transfer): String {
    if (t.status != Status.Transferring || t.speedBps <= 0) return when (t.status) {
        Status.Waiting -> "waiting for peer"
        Status.Connecting -> "connecting"
        Status.Verifying -> "verifying"
        Status.Confirming -> "confirming"
        Status.Completed -> "complete"
        Status.Paused -> "paused"
        Status.Failed -> "failed"
        Status.Unconfirmed -> "unconfirmed"
        Status.Cancelled -> "cancelled"
        else -> "—"
    }
    val bps = smoothedBps(t)
    val mbps = bps / 1_000_000.0
    return if (mbps >= 1) "${(mbps * 10).roundToInt() / 10.0} MB/s"
    else "${(bps / 1000).roundToInt()} KB/s"
}

private fun etaText(t: Transfer): String {
    val bps = smoothedBps(t)
    if (t.status != Status.Transferring || bps <= 0 || t.total <= 0) return "—"
    val remain = (t.total - t.bytes).coerceAtLeast(0)
    val secs = (remain / bps).roundToInt()
    val m = secs / 60
    val s = secs % 60
    return "%02d:%02d ETA".format(m, s)
}

private fun sizeText(t: Transfer): String {
    val bytes = if (t.total > 0) t.total else t.bytes
    return humanBytes(bytes)
}

private fun humanBytes(b: Long): String {
    if (b <= 0) return "0 B"
    val units = listOf("B", "KB", "MB", "GB")
    var v = b.toDouble(); var i = 0
    while (v >= 1024 && i < units.size - 1) { v /= 1024; i++ }
    return if (i == 0) "${b} B" else "${(v * 10).roundToInt() / 10.0} ${units[i]}"
}

private fun humanBps(bps: Double): String {
    if (bps <= 0) return "—"
    val mbps = bps / 1_000_000.0
    return if (mbps >= 1) "${(mbps * 10).roundToInt() / 10.0} MB/s"
    else "${(bps / 1000).roundToInt()} KB/s"
}
