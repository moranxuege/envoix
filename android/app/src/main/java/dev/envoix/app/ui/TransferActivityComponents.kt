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
import dev.envoix.app.ConnectionPathKind
import dev.envoix.app.Direction
import dev.envoix.app.R
import dev.envoix.app.Status
import dev.envoix.app.Transfer
import dev.envoix.app.TransferPresentationPolicy
import dev.envoix.app.TransferProgressPresentation
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
                    appString(R.string.activity_remove_transfer),
                    tint = Color.White,
                    modifier = Modifier.size(18.dp),
                )
                Spacer(Modifier.width(8.dp))
                Text(
                    appString(R.string.common_remove),
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
                                title(t),
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
                            subtitle(t, saveLocation),
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
                                    Stat(speedText(t))
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
            appString(
                R.string.room_offer_summary_format,
                appQuantityString(R.plurals.room_file_count, t.fileCount, t.fileCount),
                appQuantityString(R.plurals.room_folder_count, t.directoryCount, t.directoryCount),
                humanBytes(t.total),
            ),
            color = colors.accentStrong,
            fontSize = 12.sp,
            fontWeight = FontWeight.Bold,
        )
        t.inventoryPreview.take(3).forEach { entry ->
            Text(
                if (entry.directory) {
                    appString(R.string.activity_folder_entry, entry.name)
                } else {
                    appString(R.string.activity_file_entry, entry.name, humanBytes(entry.size))
                },
                color = colors.muted,
                fontSize = 11.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (t.inventoryPreview.size > 3 || t.inventoryHasMore) {
            Text(
                appString(R.string.activity_tap_for_complete_preview),
                color = colors.muted,
                fontSize = 11.sp,
            )
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
            CircleBtn(Icons.Default.PlayArrow, appString(R.string.activity_receive_transfer), filled = true) {
                onApproveReceive(t.id)
            }
        }
        if (actions.canPause) {
            CircleBtn(Icons.Default.Pause, appString(R.string.activity_pause_transfer), filled = true) {
                onPauseResume(t.id)
            }
        }
        if (actions.canResume) {
            CircleBtn(Icons.Default.Refresh, appString(R.string.activity_resume_transfer), filled = true) {
                onPauseResume(t.id)
            }
        }
        if (actions.canCancel) {
            CircleBtn(Icons.Default.Close, appString(R.string.activity_cancel_transfer), filled = false) {
                onCancel(t.id)
            }
        }
        if (t.status == Status.Delivered) {
            if (t.savedUri != null) {
                CircleBtn(
                    Icons.AutoMirrored.Filled.OpenInNew,
                    appString(R.string.activity_open_received_item),
                    filled = false,
                ) { onOpen(t) }
            }
            if (t.savedUris.isNotEmpty()) {
                CircleBtn(
                    Icons.Default.Share,
                    appString(R.string.activity_share_received_items),
                    filled = false,
                ) {
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
    val destination =
        destinationLabel.trim().takeIf(String::isNotEmpty)
            ?: appString(R.string.activity_downloads_folder)
    Column(Modifier.fillMaxWidth().padding(16.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
            Column(Modifier.weight(1f)) {
                Text(
                    if (t.direction == Direction.Send) {
                        appString(R.string.activity_waiting_to_send)
                    } else {
                        appString(R.string.activity_waiting_to_receive)
                    },
                    color = colors.text,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.Bold,
                )
                Text(
                    appString(
                        waitingTransferSubtitleResource(t.direction),
                        if (t.direction == Direction.Send) {
                            itemTitle(t)
                        } else {
                            destination
                        },
                    ),
                    color = colors.muted,
                    fontSize = 13.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                CircleBtn(Icons.Default.Pause, appString(R.string.activity_pause_transfer), filled = true) {
                    onPauseResume(t.id)
                }
                CircleBtn(Icons.Default.Close, appString(R.string.activity_cancel_transfer), filled = false) {
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
                appString(R.string.activity_copy_invite_link),
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
            appString(R.string.activity_scan_or_paste_invite),
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
                    appString(R.string.activity_speed_section),
                    color = colors.muted,
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Bold,
                    letterSpacing = 1.sp,
                )
                Text(
                    appString(R.string.activity_speed_summary, humanBps(avg), humanBps(peak)),
                    color = colors.muted,
                    fontSize = 11.sp,
                    fontFamily = FontFamily.Monospace,
                )
            }
            SpeedChart(t.speedHistory, t.avgBps)
        }
        Spacer(Modifier.height(4.dp))
        DetailRow(appString(R.string.activity_transfer_label), "#${t.id}")
        ConnectionPathKind.fromWireOrLegacy(t.pathAddr)?.let { kind ->
            DetailRow(
                appString(R.string.activity_data_path_label),
                appString(connectionPathLabelResource(kind)),
            )
        }
        DetailRow(
            appString(R.string.activity_transferred_label),
            appString(R.string.transfer_progress_bytes, humanBytes(t.bytes), humanBytes(t.total)),
        )
        TransferStageTimeline(t)
        if (t.rootCount > 0) {
            DetailRow(
                appString(R.string.activity_inventory_label),
                appString(
                    R.string.room_offer_summary_format,
                    appQuantityString(R.plurals.activity_root_count, t.rootCount, t.rootCount),
                    appQuantityString(R.plurals.room_file_count, t.fileCount, t.fileCount),
                    appQuantityString(R.plurals.room_folder_count, t.directoryCount, t.directoryCount),
                ),
            )
            DrawerLabel(appString(R.string.activity_authenticated_items))
            t.inventoryPreview.take(20).forEach { entry ->
                DetailRow(
                    if (entry.directory) {
                        appString(R.string.activity_folder_label)
                    } else {
                        appString(R.string.activity_file_label)
                    },
                    if (entry.directory) {
                        entry.name
                    } else {
                        appString(R.string.activity_name_size, entry.name, humanBytes(entry.size))
                    },
                )
            }
            if (t.inventoryPreview.size > 20 || t.inventoryHasMore) {
                Text(
                    appString(R.string.activity_more_items_available),
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
            val uploadLabel = appString(R.string.activity_upload)
            val uploadingLabel = appString(R.string.activity_uploading)
            val uploadedLabel = appString(R.string.activity_uploaded)
            val uploadFailedLabel = appString(R.string.transfer_status_failed)
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                DrawerLabel(appString(R.string.activity_transfer_log))
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
                    PillButton(
                        if (copied) {
                            appString(R.string.activity_copied)
                        } else {
                            appString(R.string.activity_copy)
                        },
                    ) {
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
    DrawerLabel(appString(R.string.activity_timeline_from_start))
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
                    appString(transferStageTimelineTitleResource(entry.stage)),
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
            Status.Delivered -> Triple(appString(R.string.transfer_status_delivered), colors.success, colors.successSoft)
            Status.Failed ->
                Triple(appString(R.string.transfer_status_failed), colors.danger, colors.danger.copy(alpha = 0.12f))
            Status.Canceled ->
                Triple(appString(R.string.transfer_status_canceled), colors.muted, colors.line.copy(alpha = 0.5f))
            Status.Paused ->
                Triple(appString(R.string.transfer_status_paused), colors.warning, colors.warning.copy(alpha = 0.14f))
            Status.Preparing -> Triple(appString(R.string.transfer_status_preparing), colors.accent, colors.accentSoft)
            Status.WaitingForPeer -> Triple(appString(R.string.activity_status_waiting), colors.accent, colors.accentSoft)
            Status.Pairing -> Triple(appString(R.string.activity_status_pairing), colors.accent, colors.accentSoft)
            Status.Connecting -> Triple(appString(R.string.transfer_status_connecting), colors.accent, colors.accentSoft)
            Status.AwaitingDecision ->
                Triple(appString(R.string.activity_status_review), colors.warning, colors.warning.copy(alpha = 0.14f))
            Status.Transferring ->
                if (t.direction == Direction.Send) {
                    Triple(appString(R.string.activity_status_sending), colors.accent, colors.accentSoft)
                } else {
                    Triple(appString(R.string.activity_status_receiving), colors.accent, colors.accentSoft)
                }
            Status.Verifying -> Triple(appString(R.string.transfer_status_verifying), colors.accent, colors.accentSoft)
            Status.Saving -> Triple(appString(R.string.activity_status_saving), colors.accent, colors.accentSoft)
            Status.WaitingForReceiverSave ->
                Triple(appString(R.string.activity_status_saving_remotely), colors.accent, colors.accentSoft)
            Status.FinalizingDelivery ->
                Triple(appString(R.string.activity_status_finalizing_delivery), colors.accent, colors.accentSoft)
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

@Composable
private fun title(t: Transfer): String {
    val arrow = if (t.direction == Direction.Send) "↑" else "↓"
    return appString(R.string.activity_transfer_title, arrow, itemTitle(t))
}

@Composable
private fun itemTitle(t: Transfer): String {
    val itemCount = t.fileCount + t.directoryCount
    return when {
        t.savedUris.size == 1 && !t.savedName.isNullOrBlank() -> t.savedName
        !t.fileName.isNullOrBlank() -> t.fileName
        itemCount > 0 -> appQuantityString(R.plurals.room_item_count, itemCount, itemCount)
        t.direction == Direction.Send -> appString(R.string.room_outgoing_transfer)
        else -> appString(R.string.room_incoming_transfer)
    }
}

@Composable
private fun subtitle(
    t: Transfer,
    saveLocation: String,
): String {
    if (t.status == Status.Delivered && t.savedUri != null) {
        return appString(R.string.activity_saved_destination, saveLocation)
    }
    val pathKind = ConnectionPathKind.fromWireOrLegacy(t.pathAddr)
    return if (pathKind == null) {
        appString(R.string.activity_one_time_transfer)
    } else {
        appString(connectionPathLabelResource(pathKind))
    }
}

private fun fraction(t: Transfer): Float {
    if (TransferPresentationPolicy.progress(t.status) == TransferProgressPresentation.Complete) return 1f
    if (t.total <= 0) return 0f
    return (t.bytes.toFloat() / t.total.toFloat()).coerceIn(0f, 1f)
}

@Composable
private fun speedText(t: Transfer): String {
    if (t.status != Status.Transferring || t.speedBps <= 0) {
        return when (t.status) {
            Status.Preparing -> appString(R.string.transfer_status_preparing)
            Status.WaitingForPeer -> appString(R.string.activity_status_waiting)
            Status.Pairing -> appString(R.string.activity_status_pairing)
            Status.Connecting -> appString(R.string.transfer_status_connecting)
            Status.AwaitingDecision -> appString(R.string.activity_status_review_required)
            Status.Transferring ->
                if (t.direction == Direction.Send) {
                    appString(R.string.activity_status_sending)
                } else {
                    appString(R.string.activity_status_receiving)
                }
            Status.Verifying -> appString(R.string.transfer_status_verifying)
            Status.Saving -> appString(R.string.activity_status_saving)
            Status.WaitingForReceiverSave -> appString(R.string.activity_status_receiver_saving)
            Status.FinalizingDelivery -> appString(R.string.activity_status_finalizing_delivery)
            Status.Delivered -> appString(R.string.transfer_status_delivered)
            Status.Paused -> appString(R.string.transfer_status_paused)
            Status.Failed -> appString(R.string.transfer_status_failed)
            Status.Canceled -> appString(R.string.transfer_status_canceled)
        }
    }
    val bps = smoothedBps(t)
    return transferRateString(bps)
}

@Composable
private fun etaText(t: Transfer): String {
    val bps = smoothedBps(t)
    if (t.status != Status.Transferring || bps <= 0 || t.total <= 0) return "—"
    val remain = (t.total - t.bytes).coerceAtLeast(0)
    val secs = (remain / bps).roundToInt()
    val m = secs / 60
    val s = secs % 60
    return appString(R.string.activity_eta_clock, m, s)
}

@Composable
private fun progressText(t: Transfer): String =
    appString(
        R.string.transfer_progress_bytes,
        humanBytes(t.bytes),
        humanBytes(t.total),
    )

private fun humanBps(bps: Double): String {
    if (bps <= 0) return "—"
    return transferRateString(bps)
}
