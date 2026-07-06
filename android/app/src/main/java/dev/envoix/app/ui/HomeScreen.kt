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
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Close
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.Direction
import dev.envoix.app.Status
import dev.envoix.app.Transfer
import kotlin.math.roundToInt

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(
    transfers: List<Transfer>,
    onReceive: (String) -> Unit,
    onSend: (String) -> Unit,
    onCancel: (Long) -> Unit,
    onDismiss: (Long) -> Unit,
    onOpenLogs: () -> Unit,
) {
    val colors = Envoix.colors
    var sheetOpen by remember { mutableStateOf(false) }
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
            Header(active, onOpenLogs)
            Spacer(Modifier.height(12.dp))
            if (transfers.isEmpty()) {
                EmptyState()
            } else {
                LazyColumn(
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                    contentPadding = PaddingValues(top = 4.dp, bottom = 96.dp),
                ) {
                    items(transfers.sortedByDescending { it.id }, key = { it.id }) { t ->
                        TransferCard(t, onCancel, onDismiss)
                    }
                }
            }
        }
    }

    if (sheetOpen) {
        ModalBottomSheet(
            onDismissRequest = { sheetOpen = false },
            sheetState = rememberModalBottomSheetState(),
            containerColor = colors.surface,
        ) {
            NewTransferSheet(
                onReceive = { room -> sheetOpen = false; onReceive(room) },
                onSend = { room -> sheetOpen = false; onSend(room) },
            )
        }
    }
}

@Composable
private fun Header(active: Int, onOpenLogs: () -> Unit) {
    val colors = Envoix.colors
    Row(
        Modifier
            .fillMaxWidth()
            .padding(top = 16.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column {
            Text(
                "ANDROID PAIRING",
                color = colors.accent,
                fontSize = 12.sp,
                fontWeight = FontWeight.Bold,
                letterSpacing = 1.2.sp,
            )
            Text("Envoix", color = colors.text, fontSize = 32.sp, fontWeight = FontWeight.ExtraBold)
        }
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

@Composable
private fun TransferCard(t: Transfer, onCancel: (Long) -> Unit, onDismiss: (Long) -> Unit) {
    val colors = Envoix.colors
    val done = t.status == Status.Completed
    val failed = t.status == Status.Failed || t.status == Status.Cancelled
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(colors.surface)
            .border(1.dp, colors.line, RoundedCornerShape(16.dp))
            .padding(14.dp),
        verticalAlignment = Alignment.Top,
    ) {
        // cancel / dismiss circle
        Box(
            Modifier
                .size(26.dp)
                .clip(CircleShape)
                .background(colors.muted.copy(alpha = 0.85f))
                .clickable { if (done || failed) onDismiss(t.id) else onCancel(t.id) },
            contentAlignment = Alignment.Center,
        ) {
            Icon(Icons.Default.Close, null, tint = androidx.compose.ui.graphics.Color.White, modifier = Modifier.size(16.dp))
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    title(t),
                    color = colors.text,
                    fontSize = 17.sp,
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
                modifier = Modifier
                    .fillMaxWidth()
                    .height(8.dp)
                    .clip(CircleShape),
                color = if (failed) colors.danger else colors.accent,
                trackColor = colors.line.copy(alpha = 0.6f),
            )
            Spacer(Modifier.height(10.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                Stat(speedText(t))
                Stat(etaText(t))
                Stat(sizeText(t))
            }
            if (failed && t.error != null) {
                Spacer(Modifier.height(6.dp))
                Text(t.error, color = colors.danger, fontSize = 12.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

@Composable
private fun PathBadge(t: Transfer) {
    val colors = Envoix.colors
    val (label, fg, bg) = when {
        t.status == Status.Completed -> Triple("Done", colors.success, colors.successSoft)
        t.status == Status.Failed -> Triple("Failed", colors.danger, colors.danger.copy(alpha = 0.12f))
        t.status == Status.Cancelled -> Triple("Cancelled", colors.muted, colors.line.copy(alpha = 0.5f))
        t.pathType == "relay" -> Triple("Relay", colors.accent, colors.accentSoft)
        t.pathType == "direct" -> Triple("Direct", colors.accent, colors.accentSoft)
        else -> Triple("…", colors.muted, colors.line.copy(alpha = 0.5f))
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

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun NewTransferSheet(onReceive: (String) -> Unit, onSend: (String) -> Unit) {
    val colors = Envoix.colors
    var room by remember { mutableStateOf("") }
    val valid = room.trim().length >= 3 && room.contains("-")

    Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 32.dp)) {
        Text("New transfer", color = colors.text, fontSize = 22.sp, fontWeight = FontWeight.ExtraBold)
        Spacer(Modifier.height(4.dp))
        Text(
            "Both sides share one code, e.g. 246810-cobalt-fox.",
            color = colors.muted, fontSize = 14.sp,
        )
        Spacer(Modifier.height(16.dp))
        OutlinedTextField(
            value = room,
            onValueChange = { room = it.trim() },
            label = { Text("Room code") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(20.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            SheetButton("Receive", filled = false, enabled = valid, modifier = Modifier.weight(1f)) {
                onReceive(room.trim())
            }
            SheetButton("Send file", filled = true, enabled = valid, modifier = Modifier.weight(1f)) {
                onSend(room.trim())
            }
        }
    }
}

@Composable
private fun SheetButton(
    text: String,
    filled: Boolean,
    enabled: Boolean,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    val colors = Envoix.colors
    val alpha = if (enabled) 1f else 0.4f
    Box(
        modifier
            .height(52.dp)
            .clip(RoundedCornerShape(14.dp))
            .then(
                if (filled) Modifier.background(colors.accent.copy(alpha = alpha))
                else Modifier.border(1.5.dp, colors.accent.copy(alpha = alpha), RoundedCornerShape(14.dp))
            )
            .clickable(enabled = enabled, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text,
            color = if (filled) androidx.compose.ui.graphics.Color.White else colors.accent.copy(alpha = alpha),
            fontWeight = FontWeight.Bold,
            fontSize = 16.sp,
        )
    }
}

/* ---- formatting helpers ---- */

private fun title(t: Transfer): String {
    val verb = if (t.direction == Direction.Send) "Upload" else "Download"
    val name = t.fileName ?: if (t.direction == Direction.Send) "file" else "incoming"
    return "$verb · $name"
}

private fun subtitle(t: Transfer): String = when {
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
        Status.Connecting -> "connecting"
        Status.Completed -> "complete"
        Status.Failed -> "failed"
        Status.Cancelled -> "cancelled"
        else -> "—"
    }
    val mbps = t.speedBps / 1_000_000.0
    return if (mbps >= 1) "${(mbps * 10).roundToInt() / 10.0} MB/s"
    else "${(t.speedBps / 1000).roundToInt()} KB/s"
}

private fun etaText(t: Transfer): String {
    if (t.status != Status.Transferring || t.speedBps <= 0 || t.total <= 0) return "—"
    val remain = (t.total - t.bytes).coerceAtLeast(0)
    val secs = (remain / t.speedBps).roundToInt()
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
