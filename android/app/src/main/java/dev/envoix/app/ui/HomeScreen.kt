package dev.envoix.app.ui

import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.List
import androidx.compose.material.icons.automirrored.filled.OpenInNew
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.SwapHoriz
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
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
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.Direction
import dev.envoix.app.InviteCodec
import dev.envoix.app.LogUpload
import dev.envoix.app.SettingsStore
import dev.envoix.app.Status
import dev.envoix.app.Transfer
import dev.envoix.app.TransferAction
import dev.envoix.app.availableTransferActions
import dev.envoix.app.isTerminal
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

private enum class HomeTab { Transfer, Activity, Settings }

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(
    transfers: List<Transfer>,
    onReceive: (code: String, broker: String, relay: String, qrPayload: String?) -> Unit,
    onSend: (code: String, broker: String, relay: String, file: android.net.Uri, qrPayload: String?, transferInvite: String?) -> Unit,
    onPauseResume: (Long) -> Unit,
    onCancel: (Long) -> Unit,
    onRemove: (Long) -> Unit,
    onOpenLogs: () -> Unit,
    onOpen: (Transfer) -> Unit,
) {
    val colors = Envoix.colors
    var tab by remember { mutableStateOf(HomeTab.Transfer) }
    val expanded = remember { mutableStateListOf<Long>() }
    val active =
        transfers.count {
            !it.status.isTerminal && it.status != Status.Paused
        }

    Scaffold(
        containerColor = colors.bg,
        bottomBar = {
            BottomTabs(
                selected = tab,
                active = active,
                onSelect = { tab = it },
            )
        },
    ) { inner ->
        Column(
            Modifier
                .fillMaxSize()
                .testTag(EnvoixTestTags.HOME_ROOT)
                .padding(inner),
        ) {
            when (tab) {
                HomeTab.Transfer ->
                    TransferPane(
                        transfers = transfers,
                        active = active,
                        onShowActivity = { tab = HomeTab.Activity },
                        onReceive = onReceive,
                        onSend = onSend,
                    )
                HomeTab.Activity ->
                    ActivityPane(
                        transfers = transfers,
                        active = active,
                        expanded = expanded,
                        onOpenLogs = onOpenLogs,
                        onPauseResume = onPauseResume,
                        onCancel = onCancel,
                        onRemove = onRemove,
                        onOpen = onOpen,
                    )
                HomeTab.Settings -> SettingsScreen(onBack = { tab = HomeTab.Transfer }, showBack = false)
            }
        }
    }
}

@Composable
private fun BottomTabs(
    selected: HomeTab,
    active: Int,
    onSelect: (HomeTab) -> Unit,
) {
    val colors = Envoix.colors
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 14.dp, vertical = 8.dp)
            .shadow(12.dp, RoundedCornerShape(26.dp))
            .clip(RoundedCornerShape(26.dp))
            .background(colors.surface.copy(alpha = 0.96f))
            .border(1.dp, colors.line.copy(alpha = 0.9f), RoundedCornerShape(26.dp))
            .padding(5.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        BottomTab(
            label = "Transfer",
            icon = Icons.Default.SwapHoriz,
            selected = selected == HomeTab.Transfer,
            modifier = Modifier.weight(1f).testTag(EnvoixTestTags.TRANSFER_TAB),
        ) { onSelect(HomeTab.Transfer) }
        BottomTab(
            label = if (active > 0) "Activity $active" else "Activity",
            icon = Icons.AutoMirrored.Filled.List,
            selected = selected == HomeTab.Activity,
            modifier = Modifier.weight(1f).testTag(EnvoixTestTags.ACTIVITY_TAB),
        ) { onSelect(HomeTab.Activity) }
        BottomTab(
            label = "Settings",
            icon = Icons.Default.Settings,
            selected = selected == HomeTab.Settings,
            modifier = Modifier.weight(1f).testTag(EnvoixTestTags.SETTINGS_TAB),
        ) { onSelect(HomeTab.Settings) }
    }
}

@Composable
private fun BottomTab(
    label: String,
    icon: ImageVector,
    selected: Boolean,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    val colors = Envoix.colors
    val background by animateColorAsState(
        if (selected) colors.accentSoft else Color.Transparent,
        label = "bottom-tab-background",
    )
    val foreground by animateColorAsState(
        if (selected) colors.accentStrong else colors.muted,
        label = "bottom-tab-foreground",
    )
    Column(
        modifier
            .heightIn(min = 58.dp)
            .clip(RoundedCornerShape(21.dp))
            .background(background)
            .clickable(onClick = onClick)
            .padding(horizontal = 4.dp, vertical = 7.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Icon(icon, contentDescription = label, tint = foreground, modifier = Modifier.size(22.dp))
        Spacer(Modifier.height(2.dp))
        Text(
            label,
            color = foreground,
            fontSize = 11.sp,
            fontWeight = if (selected) FontWeight.Bold else FontWeight.Medium,
            maxLines = 1,
        )
    }
}

@Composable
private fun TransferPane(
    transfers: List<Transfer>,
    active: Int,
    onShowActivity: () -> Unit,
    onReceive: (code: String, broker: String, relay: String, qrPayload: String?) -> Unit,
    onSend: (code: String, broker: String, relay: String, file: android.net.Uri, qrPayload: String?, transferInvite: String?) -> Unit,
) {
    Column(Modifier.fillMaxSize().padding(horizontal = EnvoixDimens.ScreenPadding)) {
        TransferHeader(active = active)
        Spacer(Modifier.height(12.dp))
        NewTransferSheet(
            modifier = Modifier.weight(1f),
            onReceive = { c, b, r, qr ->
                onReceive(c, b, r, qr)
                onShowActivity()
            },
            onSend = { c, b, r, uri, qr, invite ->
                onSend(c, b, r, uri, qr, invite)
                onShowActivity()
            },
        )
    }
}

@Composable
private fun TransferHeader(active: Int) {
    val colors = Envoix.colors
    Row(
        Modifier
            .fillMaxWidth()
            .padding(top = 16.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column {
            Text("ENVOIX", color = colors.accentStrong, fontSize = 11.sp, fontWeight = FontWeight.Bold, letterSpacing = 2.sp)
            Text("Transfer", color = colors.text, fontSize = 30.sp, fontWeight = FontWeight.ExtraBold)
        }
        if (active > 0) {
            Pill(text = "$active active", fg = colors.success, bg = colors.successSoft)
        }
    }
}

@Composable
private fun ActivityPane(
    transfers: List<Transfer>,
    active: Int,
    expanded: MutableList<Long>,
    onOpenLogs: () -> Unit,
    onPauseResume: (Long) -> Unit,
    onCancel: (Long) -> Unit,
    onRemove: (Long) -> Unit,
    onOpen: (Transfer) -> Unit,
) {
    Column(Modifier.fillMaxSize().padding(horizontal = EnvoixDimens.ScreenPadding)) {
        ActivityHeader(active, onOpenLogs)
        Spacer(Modifier.height(12.dp))
        if (transfers.isEmpty()) {
            EmptyState()
        } else {
            LazyColumn(
                modifier = Modifier.testTag(EnvoixTestTags.ACTIVITY_LIST),
                verticalArrangement = Arrangement.spacedBy(12.dp),
                contentPadding = PaddingValues(top = 4.dp, bottom = 20.dp),
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

@Composable
private fun ActivityHeader(
    active: Int,
    onOpenLogs: () -> Unit,
) {
    val colors = Envoix.colors
    Row(
        Modifier.fillMaxWidth().padding(top = 16.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column {
            Text("ENVOIX", color = colors.accentStrong, fontSize = 11.sp, fontWeight = FontWeight.Bold, letterSpacing = 2.sp)
            Text("Activity", color = colors.text, fontSize = 30.sp, fontWeight = FontWeight.ExtraBold)
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
                modifier =
                    Modifier
                        .clip(RoundedCornerShape(10.dp))
                        .testTag(EnvoixTestTags.LOGS_BUTTON)
                        .clickable(onClick = onOpenLogs)
                        .background(colors.accentSoft)
                        .padding(horizontal = 14.dp, vertical = 11.dp),
            )
        }
    }
}

@Composable
private fun EmptyState() {
    val colors = Envoix.colors
    Box(Modifier.fillMaxWidth().padding(top = 80.dp), contentAlignment = Alignment.Center) {
        Text(
            "No transfers yet.\nStart a send or receive from the Transfer tab.",
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
    val terminal = t.status.isTerminal
    val dismissState =
        rememberSwipeToDismissBoxState(
            confirmValueChange = {
                if (it == SwipeToDismissBoxValue.EndToStart) {
                    if (terminal) onRemove(t.id) else onCancel(t.id)
                    terminal
                } else {
                    false
                }
            },
        )
    SwipeToDismissBox(
        state = dismissState,
        enableDismissFromStartToEnd = false,
        enableDismissFromEndToStart = true,
        backgroundContent = {
            Row(
                Modifier
                    .fillMaxSize()
                    .clip(RoundedCornerShape(EnvoixDimens.CardRadius))
                    .background(colors.danger)
                    .padding(horizontal = 22.dp),
                horizontalArrangement = Arrangement.End,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(
                    if (terminal) Icons.Default.Delete else Icons.Default.Close,
                    null,
                    tint = Color.White,
                    modifier = Modifier.size(18.dp),
                )
                Spacer(Modifier.width(8.dp))
                Text(
                    if (terminal) "Delete" else "Cancel",
                    color = Color.White,
                    fontWeight = FontWeight.Bold,
                    fontSize = 14.sp,
                )
            }
        },
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(EnvoixDimens.CardRadius))
                .background(if (cancelled) colors.line else colors.surface)
                .border(1.dp, colors.line, RoundedCornerShape(EnvoixDimens.CardRadius))
                .combinedClickable(
                    onClick = {
                        if (t.status == Status.Completed && t.savedUri != null) {
                            onOpen(t)
                        } else {
                            onToggleDetail(t.id)
                        }
                    },
                    onLongClick = { onToggleDetail(t.id) },
                ),
        ) {
            if ((t.status == Status.Waiting || t.status == Status.Connecting) && t.qrPayload != null) {
                WaitingBody(t, onCancel)
            } else {
                Column(Modifier.fillMaxWidth().padding(16.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            title(t),
                            color = if (cancelled) colors.muted else colors.text,
                            fontSize = 16.sp,
                            fontWeight = FontWeight.Bold,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier =
                                Modifier
                                    .weight(1f)
                                    .testTag(EnvoixTestTags.activityTitle(t.id)),
                        )
                        Spacer(Modifier.width(10.dp))
                        PathBadge(t)
                    }
                    subtitle(t)?.let { subtitle ->
                        Text(
                            subtitle,
                            color = colors.muted,
                            fontSize = 13.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                    Spacer(Modifier.height(12.dp))
                    LinearProgressIndicator(
                        progress = { fraction(t) },
                        modifier = Modifier.fillMaxWidth().height(8.dp).clip(CircleShape),
                        color =
                            when {
                                failed -> colors.danger
                                t.status == Status.Paused -> colors.warning
                                cancelled -> colors.muted
                                else -> colors.accent
                            },
                        trackColor = colors.line.copy(alpha = 0.6f),
                    )
                    Spacer(Modifier.height(10.dp))
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                        Stat(speedText(t))
                        Stat(etaText(t))
                        Stat(sizeText(t))
                    }
                    if (t.error != null && (failed || t.status == Status.Publishing || t.status == Status.Unconfirmed)) {
                        Spacer(Modifier.height(8.dp))
                        Text(t.error, color = colors.danger, fontSize = 12.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    }
                    Spacer(Modifier.height(14.dp))
                    CardActions(
                        t = t,
                        expanded = expanded,
                        onToggleDetail = { onToggleDetail(t.id) },
                        onPauseResume = { onPauseResume(t.id) },
                        onCancel = { onCancel(t.id) },
                        onRemove = { onRemove(t.id) },
                        onOpen = { onOpen(t) },
                    )
                }
            }
            if (expanded) DetailDrawer(t)
        }
    }
}

@Composable
private fun CardActions(
    t: Transfer,
    expanded: Boolean,
    onToggleDetail: () -> Unit,
    onPauseResume: () -> Unit,
    onCancel: () -> Unit,
    onRemove: () -> Unit,
    onOpen: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        availableTransferActions(t).forEach { action ->
            val (label, icon) =
                when (action) {
                    TransferAction.Pause -> "Pause" to Icons.Default.Pause
                    TransferAction.Resume -> "Resume" to Icons.Default.PlayArrow
                    TransferAction.Retry -> "Retry" to Icons.Default.Refresh
                    TransferAction.Cancel -> "Cancel" to Icons.Default.Close
                    TransferAction.Delete -> "Delete" to Icons.Default.Delete
                    TransferAction.Open -> "Open" to Icons.AutoMirrored.Filled.OpenInNew
                }
            val primary =
                when (action) {
                    TransferAction.Pause,
                    TransferAction.Resume,
                    TransferAction.Retry,
                    TransferAction.Open,
                    -> true
                    TransferAction.Cancel,
                    TransferAction.Delete,
                    -> false
                }
            ActionButton(
                label = label,
                icon = icon,
                modifier =
                    Modifier
                        .weight(1f)
                        .testTag(EnvoixTestTags.activityAction(t.id, action.name)),
                primary = primary,
                danger = action == TransferAction.Cancel || action == TransferAction.Delete,
                onClick = {
                    when (action) {
                        TransferAction.Pause, TransferAction.Resume, TransferAction.Retry -> onPauseResume()
                        TransferAction.Cancel -> onCancel()
                        TransferAction.Delete -> onRemove()
                        TransferAction.Open -> onOpen()
                    }
                },
            )
        }
        ActionButton(
            if (expanded) "Less" else "Details",
            if (expanded) Icons.Default.ExpandLess else Icons.Default.ExpandMore,
            Modifier
                .weight(1f)
                .testTag(EnvoixTestTags.activityAction(t.id, "details")),
            onClick = onToggleDetail,
        )
    }
}

@Composable
private fun ActionButton(
    label: String,
    icon: ImageVector,
    modifier: Modifier = Modifier,
    primary: Boolean = false,
    danger: Boolean = false,
    onClick: () -> Unit,
) {
    val colors = Envoix.colors
    val background =
        when {
            primary -> colors.accent
            danger -> colors.danger.copy(alpha = 0.1f)
            else -> colors.surfaceRaised
        }
    val foreground =
        when {
            primary -> Color.White
            danger -> colors.danger
            else -> colors.text
        }
    Row(
        modifier
            .heightIn(min = 46.dp)
            .clip(RoundedCornerShape(13.dp))
            .background(background)
            .then(if (primary || danger) Modifier else Modifier.border(1.dp, colors.line, RoundedCornerShape(13.dp)))
            .clickable(onClick = onClick),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(icon, contentDescription = label, tint = foreground, modifier = Modifier.size(18.dp))
        Spacer(Modifier.width(5.dp))
        Text(label, color = foreground, fontSize = 12.sp, fontWeight = FontWeight.Bold, maxLines = 1)
    }
}

/** The waiting-to-pair variant of a card: shows the QR + code for a peer to scan
 *  or type, with only a Cancel action. Used for initiated sessions until they pair,
 *  then the card becomes the normal progress variant. */
@Composable
private fun WaitingBody(
    t: Transfer,
    onCancel: (Long) -> Unit,
) {
    val colors = Envoix.colors
    val context = LocalContext.current
    val directInvite = InviteCodec.isTransferInvite(t.qrPayload.orEmpty())
    val copiedText = if (directInvite) t.qrPayload.orEmpty() else t.room
    Column(Modifier.fillMaxWidth().padding(16.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
            Column(Modifier.weight(1f)) {
                Text(
                    if (t.direction == Direction.Send) "Waiting to send" else "Waiting to receive",
                    color = colors.text,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.Bold,
                )
                Text(
                    if (t.direction == Direction.Send) {
                        "Sending ${t.fileName ?: "a file"}"
                    } else {
                        "Saving to ${SettingsStore.saveLabel(context)}"
                    },
                    color = colors.muted,
                    fontSize = 13.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            ActionButton("Cancel", Icons.Default.Close, danger = true) { onCancel(t.id) }
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
                if (directInvite) "Invite link" else t.room,
                color = colors.text,
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
                fontFamily = FontFamily.Monospace,
            )
            Spacer(Modifier.width(8.dp))
            Icon(
                Icons.Default.ContentCopy,
                "Copy code",
                tint = colors.muted,
                modifier =
                    Modifier
                        .clip(CircleShape)
                        .clickable { clip.setText(AnnotatedString(copiedText)) }
                        .padding(6.dp)
                        .size(18.dp),
            )
        }
        Spacer(Modifier.height(2.dp))
        Text(
            if (directInvite) "Scan this invite" else "Scan or enter this code",
            color = colors.muted,
            fontSize = 12.sp,
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
                    color = colors.muted,
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Bold,
                    letterSpacing = 1.sp,
                )
                Text(
                    "avg ${humanBps(avg)} · peak ${humanBps(peak)}",
                    color = colors.muted,
                    fontSize = 11.sp,
                    fontFamily = FontFamily.Monospace,
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
                                val ok =
                                    LogUpload.upload(
                                        settings.logServer,
                                        t.room.substringBefore('-'),
                                        if (t.direction == Direction.Send) "send" else "receive",
                                        t.log.joinToString("\n"),
                                    )
                                upload = if (ok) "Uploaded ✓" else "Failed"
                            }
                        }
                    }
                    PillButton(if (copied) "Copied ✓" else "Copy") {
                        clip.setText(AnnotatedString(t.log.joinToString("\n")))
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
private fun PillButton(
    text: String,
    onClick: () -> Unit,
) {
    val colors = Envoix.colors
    Text(
        text,
        color = colors.accent,
        fontSize = 12.sp,
        fontWeight = FontWeight.Bold,
        modifier =
            Modifier
                .clip(RoundedCornerShape(8.dp))
                .clickable(onClick = onClick)
                .background(colors.accentSoft)
                .padding(horizontal = 10.dp, vertical = 5.dp),
    )
}

@Composable
private fun SpeedChart(
    history: List<Double>,
    avgBps: Double,
) {
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
            Offset(0f, py(avg)),
            Offset(w, py(avg)),
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
private fun DetailRow(
    label: String,
    value: String,
) {
    val colors = Envoix.colors
    Row(Modifier.fillMaxWidth().padding(vertical = 3.dp), horizontalArrangement = Arrangement.SpaceBetween) {
        Text(label, color = colors.muted, fontSize = 12.sp)
        Text(
            value,
            color = colors.text,
            fontSize = 12.sp,
            fontFamily = FontFamily.Monospace,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false).padding(start = 12.dp),
        )
    }
}

@Composable
private fun PathBadge(t: Transfer) {
    val colors = Envoix.colors
    val (label, fg, bg) =
        when {
            t.status == Status.Completed -> Triple("Done", colors.success, colors.successSoft)
            t.status == Status.Failed -> Triple("Failed", colors.danger, colors.danger.copy(alpha = 0.12f))
            t.status == Status.Cancelled -> Triple("Cancelled", colors.muted, colors.line.copy(alpha = 0.5f))
            t.status == Status.Paused -> Triple("Paused", colors.warning, colors.warning.copy(alpha = 0.14f))
            t.status == Status.Publishing -> Triple("Saving", colors.warning, colors.warning.copy(alpha = 0.14f))
            t.status == Status.Unconfirmed -> Triple("Confirming", colors.warning, colors.warning.copy(alpha = 0.14f))
            t.pathType == "relay" -> Triple("Relay", colors.accent, colors.accentSoft)
            t.pathType == "direct" -> Triple("Direct", colors.accent, colors.accentSoft)
            else -> Triple("…", colors.muted, colors.line.copy(alpha = 0.5f))
        }
    Pill(label, fg, bg)
}

@Composable
private fun Pill(
    text: String,
    fg: androidx.compose.ui.graphics.Color,
    bg: androidx.compose.ui.graphics.Color,
) {
    Text(
        text,
        color = fg,
        fontSize = 12.sp,
        fontWeight = FontWeight.Bold,
        modifier =
            Modifier
                .clip(CircleShape)
                .background(bg)
                .padding(horizontal = 10.dp, vertical = 4.dp),
    )
}

@Composable
private fun Stat(text: String) {
    Text(text, color = Envoix.colors.text, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
}

// ---- formatting helpers ----

private fun title(t: Transfer): String {
    val verb = if (t.direction == Direction.Send) "Upload" else "Download"
    val name = t.fileName ?: if (t.direction == Direction.Send) "file" else "incoming"
    return "$verb · $name"
}

private fun subtitle(t: Transfer): String? =
    when {
        t.status == Status.Completed && t.savedUri != null -> "Saved to Downloads · tap to open"
        t.status == Status.Waiting -> "Waiting for another device"
        t.status == Status.Connecting -> "Connecting to another device"
        else -> null
    }

private fun fraction(t: Transfer): Float {
    if (t.status == Status.Completed) return 1f
    if (t.total <= 0) return 0f
    return (t.bytes.toFloat() / t.total.toFloat()).coerceIn(0f, 1f)
}

private fun speedText(t: Transfer): String {
    if (t.status != Status.Transferring || t.speedBps <= 0) {
        return when (t.status) {
            Status.Connecting -> "connecting"
            Status.Waiting -> "waiting"
            Status.Verifying -> "verifying"
            Status.Confirming, Status.Unconfirmed -> "confirming"
            Status.Publishing -> "saving"
            Status.Completed -> "complete"
            Status.Paused -> "paused"
            Status.Failed -> "failed"
            Status.Cancelled -> "cancelled"
            else -> "—"
        }
    }
    val mbps = t.speedBps / 1_000_000.0
    return if (mbps >= 1) {
        "${(mbps * 10).roundToInt() / 10.0} MB/s"
    } else {
        "${(t.speedBps / 1000).roundToInt()} KB/s"
    }
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
    var v = b.toDouble()
    var i = 0
    while (v >= 1024 && i < units.size - 1) {
        v /= 1024
        i++
    }
    return if (i == 0) "$b B" else "${(v * 10).roundToInt() / 10.0} ${units[i]}"
}

private fun humanBps(bps: Double): String {
    if (bps <= 0) return "—"
    val mbps = bps / 1_000_000.0
    return if (mbps >= 1) {
        "${(mbps * 10).roundToInt() / 10.0} MB/s"
    } else {
        "${(bps / 1000).roundToInt()} KB/s"
    }
}
