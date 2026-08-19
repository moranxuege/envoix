package dev.envoix.app.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.OpenInNew
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
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
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.Direction
import dev.envoix.app.Status
import dev.envoix.app.Transfer
import dev.envoix.app.TransferPresentationPolicy
import dev.envoix.app.TransferProgressPresentation
import dev.envoix.app.connectionPathLabel
import dev.envoix.app.humanBytes
import dev.envoix.app.isTerminal
import dev.envoix.app.smoothedBps
import dev.envoix.app.transferRateString
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun TransferCard(
    t: Transfer,
    presentation: TransferActivityPresentationEnvironment,
    expanded: Boolean,
    onToggleDetail: (Long) -> Unit,
    onPauseResume: (Long) -> Unit,
    onApproveReceive: (Long) -> Unit,
    onCancel: (Long) -> Unit,
    onRemove: (Long) -> Unit,
    onOpen: (Transfer) -> Unit,
    onShare: (Transfer) -> Unit,
    onUploadDiagnostics: suspend (Transfer) -> Boolean,
    diagnosticsForCopy: (Transfer) -> String?,
) {
    val colors = Envoix.colors
    val language = LocalAppLanguage.current
    val saveLocation =
        resolvedSavedDestinationLabel(
            recordedDestinationLabel = t.savedDestinationLabel,
            fallbackDestinationLabel = presentation.defaultDestinationLabel,
        )
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
                WaitingBody(
                    t = t,
                    destinationLabel = presentation.defaultDestinationLabel,
                    onPauseResume = onPauseResume,
                    onCancel = onCancel,
                )
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
                            subtitle(t, language, saveLocation),
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
                            IncomingInventoryPreview(t)
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
            if (expanded) {
                DetailDrawer(
                    t,
                    presentation = presentation,
                    onUploadDiagnostics = onUploadDiagnostics,
                    diagnosticsForCopy = diagnosticsForCopy,
                )
            }
        }
    }
}

@Composable
private fun IncomingInventoryPreview(t: Transfer) {
    val colors = Envoix.colors
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(colors.accentSoft)
            .padding(horizontal = 10.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        Text(
            appText(
                "${t.fileCount} files · ${t.directoryCount} folders · ${humanBytes(t.total)}",
                "${t.fileCount} 个文件 · ${t.directoryCount} 个文件夹 · ${humanBytes(t.total)}",
            ),
            color = colors.accentStrong,
            fontSize = 12.sp,
            fontWeight = FontWeight.Bold,
        )
        t.inventoryPreview.take(3).forEach { entry ->
            Text(
                if (entry.directory) {
                    appText("Folder · ${entry.name}", "文件夹 · ${entry.name}")
                } else {
                    appText(
                        "File · ${entry.name} · ${humanBytes(entry.size)}",
                        "文件 · ${entry.name} · ${humanBytes(entry.size)}",
                    )
                },
                color = colors.muted,
                fontSize = 11.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (t.inventoryPreview.size > 3 || t.inventoryHasMore) {
            Text(appText("Tap for the complete preview", "点击查看完整预览"), color = colors.muted, fontSize = 11.sp)
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
            CircleBtn(Icons.Default.PlayArrow, appText("Receive transfer", "接收传输"), filled = true) {
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
    destinationLabel: String,
    onPauseResume: (Long) -> Unit,
    onCancel: (Long) -> Unit,
) {
    val colors = Envoix.colors
    val language = LocalAppLanguage.current
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
                    waitingTransferSubtitle(
                        direction = t.direction,
                        itemTitle = itemTitle(t, language),
                        destinationLabel = destinationLabel,
                        language = language,
                    ),
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
            Modifier
                .fillMaxWidth()
                .clickable {
                    clip.setText(AnnotatedString(checkNotNull(t.qrPayload)))
                },
            horizontalArrangement = Arrangement.Center,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                appText("Copy invite link", "复制邀请链接"),
                color = colors.muted,
                fontSize = 13.sp,
            )
            Spacer(Modifier.width(6.dp))
            Icon(
                Icons.Default.ContentCopy,
                contentDescription = null,
                tint = colors.muted,
                modifier = Modifier.size(18.dp),
            )
        }
        Spacer(Modifier.height(2.dp))
        Text(
            appText("Scan this QR or paste the invite link", "扫描此二维码或粘贴邀请链接"),
            color = colors.muted,
            fontSize = 12.sp,
            modifier = Modifier.fillMaxWidth(),
            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
        )
    }
}

/** Expanded on tap: speed history, user-facing stage timing, details, and developer logs. */
@Composable
private fun DetailDrawer(
    t: Transfer,
    presentation: TransferActivityPresentationEnvironment,
    onUploadDiagnostics: suspend (Transfer) -> Boolean,
    diagnosticsForCopy: (Transfer) -> String?,
) {
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
        DetailRow(appText("Transfer", "传输"), "#${t.id}")
        connectionPathLabel(t.pathAddr, LocalAppLanguage.current)?.let { path ->
            DetailRow(appText("Data path", "数据路径"), path)
        }
        DetailRow(appText("Transferred", "已传输"), "${humanBytes(t.bytes)} / ${humanBytes(t.total)}")
        TransferStageTimeline(t)
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
        if (t.log.isNotEmpty() && presentation.developerMode) {
            val clip = LocalClipboardManager.current
            var copied by remember(t.id) { mutableStateOf(false) }
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
                    if (presentation.canUploadDiagnostics) {
                        PillButton(upload.ifEmpty { uploadLabel }) {
                            upload = uploadingLabel
                            scope.launch {
                                val ok = onUploadDiagnostics(t)
                                upload = if (ok) uploadedLabel else uploadFailedLabel
                            }
                        }
                    }
                    PillButton(if (copied) appText("Copied ✓", "已复制 ✓") else appText("Copy", "复制")) {
                        diagnosticsForCopy(t)?.let { text ->
                            clip.setText(
                                AnnotatedString(text),
                            )
                            copied = true
                        }
                    }
                }
            }
            Spacer(Modifier.height(6.dp))
            LogBox(t.log)
        }
    }
}

@Composable
private fun TransferStageTimeline(t: Transfer) {
    val entries = latestTransferStageTimeline(t.stageTimings)
    if (entries.isEmpty()) return
    val colors = Envoix.colors
    val language = LocalAppLanguage.current
    DrawerLabel(appText("Timeline · from start", "时间线 · 从开始计时"))
    Column(
        Modifier
            .fillMaxWidth()
            .testTag("transfer_stage_timing_${t.id}"),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        entries.forEach { entry ->
            Row(
                Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(
                    Modifier
                        .size(6.dp)
                        .clip(CircleShape)
                        .background(colors.accent),
                )
                Spacer(Modifier.width(8.dp))
                Text(
                    transferStageTimelineTitle(entry.stage, language),
                    color = colors.text,
                    fontSize = 12.sp,
                    modifier = Modifier.weight(1f),
                )
                Text(
                    formatTransferStageElapsed(entry.elapsedFromSessionUs),
                    color = colors.muted,
                    fontSize = 11.sp,
                    fontFamily = FontFamily.Monospace,
                )
            }
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
    saveLocation: String,
): String =
    when {
        t.status == Status.Delivered && t.savedUri != null ->
            savedDestinationSubtitle(saveLocation, language)
        t.pathAddr != null ->
            connectionPathLabel(t.pathAddr, language)
                ?: AppText.value("One-time transfer", "一次性传输", language)
        else -> AppText.value("One-time transfer", "一次性传输", language)
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
    return transferRateString(bps)
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
    return transferRateString(bps)
}
