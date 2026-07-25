package dev.envoix.app.ui

import androidx.compose.foundation.Canvas
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
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.OpenInNew
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Devices
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.MailOutline
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.material3.rememberSwipeToDismissBoxState
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
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.Diagnostics
import dev.envoix.app.Direction
import dev.envoix.app.InviteCodec
import dev.envoix.app.LogUpload
import dev.envoix.app.R
import dev.envoix.app.Room
import dev.envoix.app.SettingsStore
import dev.envoix.app.Status
import dev.envoix.app.Transfer
import dev.envoix.app.TransferPresentationPolicy
import dev.envoix.app.TransferProgressPresentation
import dev.envoix.app.humanBytes
import dev.envoix.app.isTerminal
import dev.envoix.app.smoothedBps
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

@OptIn(ExperimentalMaterial3Api::class, ExperimentalComposeUiApi::class)
@Composable
fun HomeScreen(
    transfers: List<Transfer>,
    initialSharedUris: List<android.net.Uri> = emptyList(),
    onSharedUrisConsumed: () -> Unit = {},
    onReceive: (code: String, broker: String, relay: String, qrPayload: String?, copyApproved: Boolean) -> Unit,
    onSend: (code: String, broker: String, relay: String, jobId: String, qrPayload: String?) -> Unit,
    onPauseResume: (Long) -> Unit,
    onApproveReceive: (Long) -> Unit,
    onCancel: (Long) -> Unit,
    onRemove: (Long) -> Unit,
    onOpenDiscovery: () -> Unit,
    onOpenLogs: () -> Unit,
    onOpenSettings: () -> Unit,
    onOpen: (Transfer) -> Unit,
    onShare: (Transfer) -> Unit,
    initialPairingInput: String? = null,
) {
    val colors = Envoix.colors
    var sheetRole by remember { mutableStateOf<String?>(null) }
    val expanded = remember { mutableStateListOf<Long>() }
    val listState = rememberLazyListState()
    // A just-created transfer lands at the top (newest-first sort); bring it
    // into view instead of leaving it above the fold.
    val newestId = transfers.maxOfOrNull { it.id } ?: -1L
    LaunchedEffect(newestId) {
        if (newestId >= 0) listState.animateScrollToItem(0)
    }
    LaunchedEffect(initialSharedUris) {
        if (initialSharedUris.isNotEmpty()) sheetRole = "send"
    }
    LaunchedEffect(initialPairingInput) {
        initialPairingInput
            ?.takeIf(String::isNotBlank)
            ?.let(InviteCodec::parseForRouting)
            ?.let { sheetRole = it.joinerRole }
    }
    val active =
        transfers.count { !it.status.isTerminal }

    Scaffold(
        modifier = Modifier.semantics { testTagsAsResourceId = true },
        containerColor = colors.bg,
    ) { inner ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(inner)
                .padding(horizontal = 20.dp),
        ) {
            Header(active, onOpenDiscovery, onOpenLogs, onOpenSettings)
            Spacer(Modifier.height(18.dp))
            Text(
                appString(R.string.transfer_files_title),
                color = colors.text,
                fontSize = 28.sp,
                fontWeight = FontWeight.ExtraBold,
            )
            Text(
                appString(R.string.choose_device_action),
                color = colors.muted,
                fontSize = 14.sp,
                modifier = Modifier.padding(top = 3.dp),
            )
            Spacer(Modifier.height(14.dp))
            HomeActionCard(
                title = appString(R.string.send_action_title),
                subtitle = appString(R.string.send_action_subtitle),
                icon = Icons.Default.Share,
                testTag = "home_send",
            ) {
                sheetRole = "send"
            }
            Spacer(Modifier.height(10.dp))
            HomeActionCard(
                title = appString(R.string.receive_action_title),
                subtitle = appString(R.string.receive_action_subtitle),
                icon = Icons.Default.Download,
                testTag = "home_receive",
            ) {
                sheetRole = "receive"
            }
            Spacer(Modifier.height(18.dp))
            Text(
                appString(R.string.activity_title),
                color = colors.text,
                fontSize = 16.sp,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(8.dp))
            if (transfers.isEmpty()) {
                EmptyState()
            } else {
                LazyColumn(
                    modifier = Modifier.weight(1f),
                    state = listState,
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                    contentPadding = PaddingValues(top = 4.dp, bottom = 24.dp),
                ) {
                    items(transfers.sortedByDescending { it.id }, key = { it.id }) { t ->
                        TransferCard(
                            t = t,
                            expanded = t.id in expanded,
                            onToggleDetail = { if (it in expanded) expanded.remove(it) else expanded.add(it) },
                            onPauseResume = onPauseResume,
                            onApproveReceive = onApproveReceive,
                            onCancel = onCancel,
                            onRemove = onRemove,
                            onOpen = onOpen,
                            onShare = onShare,
                        )
                    }
                }
            }
        }
    }

    sheetRole?.let { initialRole ->
        ModalBottomSheet(
            onDismissRequest = {
                sheetRole = null
            },
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            containerColor = colors.surface,
        ) {
            NewTransferSheet(
                initialRole = initialRole,
                initialSources = initialSharedUris,
                initialPairingInput = initialPairingInput,
                onReceive = { c, b, r, qr, copyApproved ->
                    sheetRole = null
                    onReceive(c, b, r, qr, copyApproved)
                },
                onSend = { c, b, r, jobId, qr ->
                    sheetRole = null
                    onSharedUrisConsumed()
                    onSend(c, b, r, jobId, qr)
                },
            )
        }
    }
}

@Composable
private fun Header(
    active: Int,
    onOpenDiscovery: () -> Unit,
    onOpenLogs: () -> Unit,
    onOpenSettings: () -> Unit,
) {
    val colors = Envoix.colors
    Row(
        Modifier
            .fillMaxWidth()
            .padding(top = 16.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(
                Modifier
                    .size(42.dp)
                    .clip(RoundedCornerShape(13.dp))
                    .background(colors.accentSoft),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    Icons.Default.MailOutline,
                    contentDescription = null,
                    tint = colors.accentStrong,
                    modifier = Modifier.size(24.dp),
                )
            }
            Spacer(Modifier.width(10.dp))
            Text("Envoix", color = colors.text, fontSize = 26.sp, fontWeight = FontWeight.ExtraBold)
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (active > 0) {
                Pill(
                    text = appString(R.string.active_transfer_count, active),
                    fg = colors.success,
                    bg = colors.successSoft,
                )
                Spacer(Modifier.width(8.dp))
            }
            Icon(
                Icons.Default.Devices,
                contentDescription = appString(R.string.nearby_devices),
                tint = colors.accent,
                modifier =
                    Modifier
                        .clip(CircleShape)
                        .clickable(onClick = onOpenDiscovery)
                        .padding(6.dp)
                        .size(22.dp),
            )
            Text(
                appString(R.string.logs),
                color = colors.accent,
                fontWeight = FontWeight.Bold,
                fontSize = 14.sp,
                modifier =
                    Modifier
                        .clip(RoundedCornerShape(10.dp))
                        .clickable(onClick = onOpenLogs)
                        .padding(horizontal = 10.dp, vertical = 6.dp),
            )
            Icon(
                Icons.Default.Settings,
                contentDescription = appString(R.string.settings),
                tint = colors.accent,
                modifier =
                    Modifier
                        .clip(CircleShape)
                        .clickable(onClick = onOpenSettings)
                        .padding(6.dp)
                        .size(22.dp),
            )
        }
    }
}

@Composable
private fun HomeActionCard(
    title: String,
    subtitle: String,
    icon: ImageVector,
    testTag: String,
    onClick: () -> Unit,
) {
    val colors = Envoix.colors
    Row(
        Modifier
            .testTag(testTag)
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(colors.surfaceRaised)
            .border(1.dp, colors.line, RoundedCornerShape(16.dp))
            .clickable(onClick = onClick)
            .padding(14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier
                .size(48.dp)
                .clip(RoundedCornerShape(14.dp))
                .background(colors.accentSoft),
            contentAlignment = Alignment.Center,
        ) {
            Icon(icon, contentDescription = null, tint = colors.accentStrong, modifier = Modifier.size(24.dp))
        }
        Spacer(Modifier.width(13.dp))
        Column(Modifier.weight(1f)) {
            Text(title, color = colors.text, fontSize = 17.sp, fontWeight = FontWeight.Bold)
            Text(
                subtitle,
                color = colors.muted,
                fontSize = 13.sp,
                lineHeight = 18.sp,
                modifier = Modifier.padding(top = 2.dp),
            )
        }
        Text("›", color = colors.muted, fontSize = 26.sp)
    }
}

@Composable
private fun EmptyState() {
    val colors = Envoix.colors
    Box(Modifier.fillMaxWidth().padding(vertical = 24.dp), contentAlignment = Alignment.Center) {
        Text(
            appText(
                "No activity yet. Transfers will appear here.",
                "暂无活动，传输任务会显示在这里。",
            ),
            color = colors.muted,
            fontSize = 13.sp,
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun TransferCard(
    t: Transfer,
    expanded: Boolean,
    onToggleDetail: (Long) -> Unit,
    onPauseResume: (Long) -> Unit,
    onApproveReceive: (Long) -> Unit,
    onCancel: (Long) -> Unit,
    onRemove: (Long) -> Unit,
    onOpen: (Transfer) -> Unit,
    onShare: (Transfer) -> Unit,
) {
    val colors = Envoix.colors
    val language = LocalAppLanguage.current
    val failed = t.status == Status.Failed
    val canceled = t.status == Status.Canceled
    val progressPresentation = TransferPresentationPolicy.progress(t.status)
    val dismissState =
        rememberSwipeToDismissBoxState(
            confirmValueChange = {
                if (t.status.isTerminal && it == SwipeToDismissBoxValue.EndToStart) {
                    onRemove(t.id)
                    true
                } else {
                    false
                }
            },
        )
    SwipeToDismissBox(
        state = dismissState,
        enableDismissFromStartToEnd = false,
        enableDismissFromEndToStart = t.status.isTerminal,
        backgroundContent = {
            Row(
                Modifier
                    .fillMaxSize()
                    .clip(RoundedCornerShape(16.dp))
                    .background(colors.danger)
                    .padding(horizontal = 22.dp),
                horizontalArrangement = Arrangement.End,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(
                    Icons.Default.Delete,
                    appText("Remove transfer", "移除传输"),
                    tint = Color.White,
                    modifier = Modifier.size(18.dp),
                )
                Spacer(Modifier.width(8.dp))
                Text(appText("Remove", "移除"), color = Color.White, fontWeight = FontWeight.Bold, fontSize = 14.sp)
            }
        },
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(16.dp))
                .background(if (canceled) colors.line else colors.surface)
                .border(1.dp, colors.line, RoundedCornerShape(16.dp))
                .clickable { onToggleDetail(t.id) },
        ) {
            if (t.status == Status.WaitingForPeer && t.qrPayload != null) {
                WaitingBody(t, onPauseResume, onCancel)
            } else {
                Row(Modifier.fillMaxWidth().padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(
                                title(t, language),
                                color = if (canceled) colors.muted else colors.text,
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
                            subtitle(t, language),
                            color = colors.muted,
                            fontSize = 13.sp,
                            fontFamily = FontFamily.Monospace,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        if (progressPresentation != TransferProgressPresentation.Hidden && t.total > 0) {
                            Spacer(Modifier.height(10.dp))
                            LinearProgressIndicator(
                                progress = { fraction(t) },
                                modifier = Modifier.fillMaxWidth().height(8.dp).clip(CircleShape),
                                color =
                                    when {
                                        failed -> colors.danger
                                        t.status == Status.Paused -> colors.warning
                                        canceled -> colors.muted
                                        else -> colors.accent
                                    },
                                trackColor = colors.line.copy(alpha = 0.6f),
                            )
                            Spacer(Modifier.height(10.dp))
                            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                                if (progressPresentation == TransferProgressPresentation.Active) {
                                    Stat(speedText(t, language))
                                    Stat(etaText(t))
                                }
                                Stat(progressText(t))
                            }
                        }
                        if (t.status == Status.AwaitingDecision && t.rootCount > 0) {
                            Spacer(Modifier.height(6.dp))
                            Text(
                                appText(
                                    "Tap to review the authenticated incoming list.",
                                    "点击查看已认证的待接收清单。",
                                ),
                                color = colors.warning,
                                fontSize = 12.sp,
                            )
                        }
                        if (failed && t.error != null) {
                            Spacer(Modifier.height(6.dp))
                            Text(t.error, color = colors.danger, fontSize = 12.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
                        }
                    }
                    Spacer(Modifier.width(10.dp))
                    CardControls(t, onPauseResume, onApproveReceive, onCancel, onOpen, onShare)
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
    onApproveReceive: (Long) -> Unit,
    onCancel: (Long) -> Unit,
    onOpen: (Transfer) -> Unit,
    onShare: (Transfer) -> Unit,
) {
    val actions = TransferPresentationPolicy.actions(t)
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (actions.canApprove) {
            CircleBtn(Icons.Default.PlayArrow, appText("Accept transfer", "接收传输"), filled = true) {
                onApproveReceive(t.id)
            }
        }
        if (actions.canPause) {
            CircleBtn(Icons.Default.Pause, appText("Pause transfer", "暂停传输"), filled = true) {
                onPauseResume(t.id)
            }
        }
        if (actions.canResume) {
            CircleBtn(Icons.Default.Refresh, appText("Resume transfer", "继续传输"), filled = true) {
                onPauseResume(t.id)
            }
        }
        if (actions.canCancel) {
            CircleBtn(Icons.Default.Close, appText("Cancel transfer", "取消传输"), filled = false) {
                onCancel(t.id)
            }
        }
        if (t.status == Status.Delivered) {
            if (t.savedUri != null) {
                CircleBtn(
                    Icons.AutoMirrored.Filled.OpenInNew,
                    appText("Open received item", "打开接收项目"),
                    filled = false,
                ) { onOpen(t) }
            }
            if (t.savedUris.isNotEmpty()) {
                CircleBtn(Icons.Default.Share, appText("Share received items", "分享接收项目"), filled = false) {
                    onShare(t)
                }
            }
        }
    }
}

@Composable
private fun CircleBtn(
    icon: ImageVector,
    contentDescription: String,
    filled: Boolean,
    onClick: () -> Unit,
) {
    val colors = Envoix.colors
    Box(
        Modifier
            .size(38.dp)
            .clip(CircleShape)
            .then(
                if (filled) {
                    Modifier.background(colors.accent)
                } else {
                    Modifier.border(1.5.dp, colors.line, CircleShape)
                },
            ).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            icon,
            contentDescription,
            tint = if (filled) Color.White else colors.muted,
            modifier = Modifier.size(18.dp),
        )
    }
}

/** The waiting-to-pair card: shows the QR + code until the core reports a match. */
@Composable
private fun WaitingBody(
    t: Transfer,
    onPauseResume: (Long) -> Unit,
    onCancel: (Long) -> Unit,
) {
    val colors = Envoix.colors
    val language = LocalAppLanguage.current
    val settings by SettingsStore.settings.collectAsState()
    Column(Modifier.fillMaxWidth().padding(16.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
            Column(Modifier.weight(1f)) {
                Text(
                    if (t.direction == Direction.Send) {
                        appText("Waiting to send", "等待发送")
                    } else {
                        appText("Waiting to receive", "等待接收")
                    },
                    color = colors.text,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.Bold,
                )
                Text(
                    if (t.direction == Direction.Send) {
                        appText(
                            "Sending ${itemTitle(t, language)}",
                            "准备发送 ${itemTitle(t, language)}",
                        )
                    } else {
                        appText(
                            "Saving to Downloads/${settings.saveFolder}",
                            "将保存到 Downloads/${settings.saveFolder}",
                        )
                    },
                    color = colors.muted,
                    fontSize = 13.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                CircleBtn(Icons.Default.Pause, appText("Pause transfer", "暂停传输"), filled = true) {
                    onPauseResume(t.id)
                }
                CircleBtn(Icons.Default.Close, appText("Cancel transfer", "取消传输"), filled = false) {
                    onCancel(t.id)
                }
            }
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
                color = colors.text,
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
                fontFamily = FontFamily.Monospace,
            )
            Spacer(Modifier.width(8.dp))
            Icon(
                Icons.Default.ContentCopy,
                appText("Copy code", "复制配对码"),
                tint = colors.muted,
                modifier =
                    Modifier
                        .clip(CircleShape)
                        .clickable { clip.setText(AnnotatedString(t.room)) }
                        .padding(6.dp)
                        .size(18.dp),
            )
        }
        Spacer(Modifier.height(2.dp))
        Text(
            appText("Scan or enter this code", "扫描或输入此配对码"),
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
                    appText("SPEED", "速度"),
                    color = colors.muted,
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Bold,
                    letterSpacing = 1.sp,
                )
                Text(
                    appText(
                        "avg ${humanBps(avg)} · peak ${humanBps(peak)}",
                        "平均 ${humanBps(avg)} · 峰值 ${humanBps(peak)}",
                    ),
                    color = colors.muted,
                    fontSize = 11.sp,
                    fontFamily = FontFamily.Monospace,
                )
            }
            SpeedChart(t.speedHistory, t.avgBps)
        }
        Spacer(Modifier.height(4.dp))
        DetailRow(appText("Room", "配对房间"), t.room)
        if (t.pathAddr != null) DetailRow(appText("Path", "连接路径"), t.pathAddr)
        DetailRow(appText("Transferred", "已传输"), "${humanBytes(t.bytes)} / ${humanBytes(t.total)}")
        if (t.rootCount > 0) {
            DetailRow(
                appText("Inventory", "清单"),
                appText(
                    "${t.rootCount} roots · ${t.fileCount} files · ${t.directoryCount} folders",
                    "${t.rootCount} 个根项目 · ${t.fileCount} 个文件 · ${t.directoryCount} 个文件夹",
                ),
            )
            DrawerLabel(appText("Authenticated items", "已认证项目"))
            t.inventoryPreview.take(20).forEach { entry ->
                DetailRow(
                    if (entry.directory) appText("Folder", "文件夹") else appText("File", "文件"),
                    if (entry.directory) entry.name else "${entry.name} · ${humanBytes(entry.size)}",
                )
            }
            if (t.inventoryPreview.size > 20 || t.inventoryHasMore) {
                Text(
                    appText(
                        "More items are available in bounded pages; only the first 20 are shown here.",
                        "还有更多项目可分页查看；此处仅显示前 20 项。",
                    ),
                    color = colors.muted,
                    fontSize = 11.sp,
                )
            }
        }
        if (t.log.isNotEmpty()) {
            val clip = LocalClipboardManager.current
            var copied by remember(t.id) { mutableStateOf(false) }
            val settings by SettingsStore.settings.collectAsState()
            val scope = rememberCoroutineScope()
            var upload by remember(t.id) { mutableStateOf("") }
            val uploadLabel = appText("Upload", "上传")
            val uploadingLabel = appText("Uploading…", "正在上传…")
            val uploadedLabel = appText("Uploaded ✓", "已上传 ✓")
            val uploadFailedLabel = appText("Failed", "失败")
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                DrawerLabel(appText("This transfer's log", "本次传输日志"))
                Row(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    if (settings.devMode && settings.logServer.isNotBlank()) {
                        PillButton(upload.ifEmpty { uploadLabel }) {
                            upload = uploadingLabel
                            scope.launch {
                                val ok =
                                    LogUpload.upload(
                                        settings.logServer,
                                        Room(t.room).id,
                                        if (t.direction == Direction.Send) "send" else "receive",
                                        Diagnostics.build(Diagnostics.Kind.Transfer, t.id),
                                    )
                                upload = if (ok) uploadedLabel else uploadFailedLabel
                            }
                        }
                    }
                    PillButton(if (copied) appText("Copied ✓", "已复制 ✓") else appText("Copy", "复制")) {
                        // The full durable log via the one assembler (clip-capped).
                        runCatching {
                            clip.setText(
                                AnnotatedString(
                                    Diagnostics.build(Diagnostics.Kind.Transfer, t.id, Diagnostics.CLIP_MAX),
                                ),
                            )
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
        when (t.status) {
            Status.Delivered -> Triple(appText("Delivered", "已送达"), colors.success, colors.successSoft)
            Status.Failed -> Triple(appText("Failed", "失败"), colors.danger, colors.danger.copy(alpha = 0.12f))
            Status.Canceled -> Triple(appText("Canceled", "已取消"), colors.muted, colors.line.copy(alpha = 0.5f))
            Status.Paused -> Triple(appText("Paused", "已暂停"), colors.warning, colors.warning.copy(alpha = 0.14f))
            Status.Preparing -> Triple(appText("Preparing", "正在准备"), colors.accent, colors.accentSoft)
            Status.WaitingForPeer -> Triple(appText("Waiting", "等待中"), colors.accent, colors.accentSoft)
            Status.Pairing -> Triple(appText("Pairing", "正在配对"), colors.accent, colors.accentSoft)
            Status.Connecting -> Triple(appText("Connecting", "正在连接"), colors.accent, colors.accentSoft)
            Status.AwaitingDecision -> Triple(appText("Review", "待确认"), colors.warning, colors.warning.copy(alpha = 0.14f))
            Status.Transferring ->
                if (t.direction == Direction.Send) {
                    Triple(appText("Sending", "正在发送"), colors.accent, colors.accentSoft)
                } else {
                    Triple(appText("Receiving", "正在接收"), colors.accent, colors.accentSoft)
                }
            Status.Verifying -> Triple(appText("Verifying", "正在验证"), colors.accent, colors.accentSoft)
            Status.Saving -> Triple(appText("Saving", "正在保存"), colors.accent, colors.accentSoft)
            Status.WaitingForReceiverSave ->
                Triple(appText("Saving remotely", "接收端正在保存"), colors.accent, colors.accentSoft)
            Status.FinalizingDelivery ->
                Triple(appText("Finalizing delivery", "正在确认送达"), colors.accent, colors.accentSoft)
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

private fun title(
    t: Transfer,
    language: String,
): String {
    val arrow = if (t.direction == Direction.Send) "↑" else "↓"
    return "$arrow ${itemTitle(t, language)}"
}

private fun itemTitle(
    t: Transfer,
    language: String,
): String {
    val itemCount = t.fileCount + t.directoryCount
    return when {
        t.savedUris.size == 1 && !t.savedName.isNullOrBlank() -> t.savedName
        !t.fileName.isNullOrBlank() -> t.fileName
        itemCount == 1 -> AppText.value("1 item", "1 个项目", language)
        itemCount > 1 -> AppText.value("$itemCount items", "$itemCount 个项目", language)
        t.direction == Direction.Send -> AppText.value("Outgoing transfer", "待发送内容", language)
        else -> AppText.value("Incoming transfer", "待接收内容", language)
    }
}

private fun subtitle(
    t: Transfer,
    language: String,
): String =
    when {
        t.status == Status.Delivered && t.savedUri != null ->
            AppText.value("Saved to Downloads · tap to open", "已保存到 Downloads · 点击打开", language)
        t.pathAddr != null -> t.pathAddr
        else -> AppText.value("room ${t.room}", "配对房间 ${t.room}", language)
    }

private fun fraction(t: Transfer): Float {
    if (TransferPresentationPolicy.progress(t.status) == TransferProgressPresentation.Complete) return 1f
    if (t.total <= 0) return 0f
    return (t.bytes.toFloat() / t.total.toFloat()).coerceIn(0f, 1f)
}

private fun speedText(
    t: Transfer,
    language: String,
): String {
    if (t.status != Status.Transferring || t.speedBps <= 0) {
        return when (t.status) {
            Status.Preparing -> AppText.value("preparing", "正在准备", language)
            Status.WaitingForPeer -> AppText.value("waiting", "正在等待", language)
            Status.Pairing -> AppText.value("pairing", "正在配对", language)
            Status.Connecting -> AppText.value("connecting", "正在连接", language)
            Status.AwaitingDecision -> AppText.value("review required", "需要确认", language)
            Status.Transferring ->
                if (t.direction == Direction.Send) {
                    AppText.value("sending", "正在发送", language)
                } else {
                    AppText.value("receiving", "正在接收", language)
                }
            Status.Verifying -> AppText.value("verifying", "正在验证", language)
            Status.Saving -> AppText.value("saving", "正在保存", language)
            Status.WaitingForReceiverSave -> AppText.value("receiver saving", "接收端正在保存", language)
            Status.FinalizingDelivery -> AppText.value("finalizing delivery", "正在确认送达", language)
            Status.Delivered -> AppText.value("delivered", "已送达", language)
            Status.Paused -> AppText.value("paused", "已暂停", language)
            Status.Failed -> AppText.value("failed", "失败", language)
            Status.Canceled -> AppText.value("canceled", "已取消", language)
        }
    }
    val bps = smoothedBps(t)
    val mbps = bps / 1_000_000.0
    return if (mbps >= 1) {
        "${(mbps * 10).roundToInt() / 10.0} MB/s"
    } else {
        "${(bps / 1000).roundToInt()} KB/s"
    }
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

private fun progressText(t: Transfer): String = "${humanBytes(t.bytes)} / ${humanBytes(t.total)}"

private fun humanBps(bps: Double): String {
    if (bps <= 0) return "—"
    val mbps = bps / 1_000_000.0
    return if (mbps >= 1) {
        "${(mbps * 10).roundToInt() / 10.0} MB/s"
    } else {
        "${(bps / 1000).roundToInt()} KB/s"
    }
}
