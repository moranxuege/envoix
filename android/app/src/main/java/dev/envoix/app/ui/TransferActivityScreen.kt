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
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.MeetingRoom
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.ConnectionPathKind
import dev.envoix.app.Status
import dev.envoix.app.Transfer
import dev.envoix.app.connectionPathLabel
import dev.envoix.app.humanBytes
import dev.envoix.app.isTerminal
import dev.envoix.app.smoothedBps
import dev.envoix.app.transferRateString

internal data class ActivityRoomGroup(
    val key: String,
    val label: String?,
    val isDirect: Boolean,
    val transfers: List<Transfer>,
)

internal data class ActivityRoomMetrics(
    val transferCount: Int,
    val activeCount: Int,
    val pausedCount: Int,
    val deliveredCount: Int,
    val failedCount: Int,
    val fileCount: Int,
    val directoryCount: Int,
    val bytes: Long,
    val total: Long,
    val currentBps: Double,
    val averageBps: Double,
    val etaSeconds: Double?,
)

internal enum class ActivityRoomStatusKind {
    Active,
    Paused,
    NeedsAttention,
    Completed,
    Finished,
}

private val ACTIVITY_TRANSFER_ORDER =
    compareByDescending<Transfer> { it.id }
        .thenBy { it.direction.name }
        .thenBy { it.jobId.orEmpty() }

private val MAX_PRESENTATION_BPS = Long.MAX_VALUE.toDouble()
private const val SECONDS_PER_MINUTE = 60L

internal fun groupTransfersForActivity(transfers: List<Transfer>): List<ActivityRoomGroup> =
    transfers
        .groupBy { transfer ->
            transfer.activityGroupId
                ?.trim()
                ?.takeIf(String::isNotEmpty)
                ?.let { "activity:$it" }
                ?: "transfer:${transfer.id}"
        }.map { (key, values) ->
            val sortedTransfers = values.sortedWith(ACTIVITY_TRANSFER_ORDER)
            ActivityRoomGroup(
                key = key,
                label =
                    sortedTransfers.firstNotNullOfOrNull { transfer ->
                        transfer.activityGroupLabel?.trim()?.takeIf(String::isNotEmpty)
                    },
                isDirect = sortedTransfers.all { it.activityGroupId.isNullOrBlank() },
                transfers = sortedTransfers,
            )
        }.sortedWith(
            compareByDescending<ActivityRoomGroup> { it.transfers.first().id }
                .thenBy(ActivityRoomGroup::key),
        )

internal fun activityRoomMetrics(transfers: List<Transfer>): ActivityRoomMetrics {
    val bytes = transfers.map { it.bytes }.saturatedNonNegativeLongSum()
    val reportedTotal = transfers.map { it.total }.saturatedNonNegativeLongSum()
    val total = maxOf(reportedTotal, bytes)
    val transferring = transfers.filter { it.status == Status.Transferring }
    val transferringBytes = transferring.map { it.bytes }.saturatedNonNegativeLongSum()
    val transferringReportedTotal = transferring.map { it.total }.saturatedNonNegativeLongSum()
    val transferringTotal = maxOf(transferringReportedTotal, transferringBytes)
    val currentBps =
        transferring
            .asSequence()
            .map(::smoothedBps)
            .saturatedFiniteBpsSum()
    return ActivityRoomMetrics(
        transferCount = transfers.size,
        activeCount = transfers.count { !it.status.isTerminal && it.status != Status.Paused },
        pausedCount = transfers.count { it.status == Status.Paused },
        deliveredCount = transfers.count { it.status == Status.Delivered },
        failedCount = transfers.count { it.status == Status.Failed },
        fileCount = transfers.map { it.fileCount }.saturatedNonNegativeIntSum(),
        directoryCount = transfers.map { it.directoryCount }.saturatedNonNegativeIntSum(),
        bytes = bytes,
        total = total,
        currentBps = currentBps,
        averageBps =
            transfers
                .asSequence()
                .map { it.bytes to it.avgBps }
                .weightedAverageBps(),
        etaSeconds =
            if (currentBps > 0.0 && transferringTotal > transferringBytes) {
                ((transferringTotal - transferringBytes).toDouble() / currentBps)
                    .takeIf { it.isFinite() && it >= 0.0 }
            } else {
                null
            },
    )
}

internal fun activityRoomStatusKind(metrics: ActivityRoomMetrics): ActivityRoomStatusKind =
    when {
        metrics.activeCount > 0 -> ActivityRoomStatusKind.Active
        metrics.pausedCount > 0 -> ActivityRoomStatusKind.Paused
        metrics.failedCount > 0 -> ActivityRoomStatusKind.NeedsAttention
        metrics.transferCount > 0 && metrics.deliveredCount == metrics.transferCount ->
            ActivityRoomStatusKind.Completed
        else -> ActivityRoomStatusKind.Finished
    }

internal fun activityRoomDisplayName(
    label: String?,
    language: String,
    isDirect: Boolean = false,
): String =
    label
        ?.trim()
        ?.takeIf(String::isNotEmpty)
        ?: if (isDirect) {
            AppText.value("Direct transfer", "直接传输", language)
        } else {
            AppText.value("One-time room", "一次性房间", language)
        }

private fun Iterable<Long>.saturatedNonNegativeLongSum(): Long =
    fold(0L) { total, rawValue ->
        val value = rawValue.coerceAtLeast(0L)
        if (value > Long.MAX_VALUE - total) Long.MAX_VALUE else total + value
    }

private fun Iterable<Int>.saturatedNonNegativeIntSum(): Int =
    fold(0) { total, rawValue ->
        val value = rawValue.coerceAtLeast(0)
        if (value > Int.MAX_VALUE - total) Int.MAX_VALUE else total + value
    }

private fun Sequence<Double>.saturatedFiniteBpsSum(): Double =
    fold(0.0) { total, rawValue ->
        val value =
            rawValue
                .takeIf { it.isFinite() && it > 0.0 }
                ?.coerceAtMost(MAX_PRESENTATION_BPS)
                ?: 0.0
        if (value > MAX_PRESENTATION_BPS - total) MAX_PRESENTATION_BPS else total + value
    }

private fun Sequence<Pair<Long, Double>>.weightedAverageBps(): Double {
    var totalBytes = 0.0
    var totalSeconds = 0.0
    forEach { (rawBytes, rawBps) ->
        val bytes = rawBytes.coerceAtLeast(0L).toDouble()
        val bps =
            rawBps
                .takeIf { it.isFinite() && it > 0.0 }
                ?.coerceAtMost(MAX_PRESENTATION_BPS)
                ?: return@forEach
        if (bytes <= 0.0) return@forEach
        totalBytes += bytes
        totalSeconds += bytes / bps
    }
    if (!totalSeconds.isFinite() || totalSeconds <= 0.0) return 0.0
    return (totalBytes / totalSeconds)
        .takeIf { it.isFinite() }
        ?.coerceAtMost(MAX_PRESENTATION_BPS)
        ?: MAX_PRESENTATION_BPS
}

@OptIn(ExperimentalComposeUiApi::class)
@Composable
internal fun TransferActivityScreen(
    transfers: List<Transfer>,
    onBack: () -> Unit,
    onPauseResume: (Long) -> Unit,
    onApproveReceive: (Long) -> Unit,
    onCancel: (Long) -> Unit,
    onRemove: (Long) -> Unit,
    onOpen: (Transfer) -> Unit,
    onShare: (Transfer) -> Unit,
) {
    val colors = Envoix.colors
    val expandedRooms = remember { mutableStateListOf<String>() }
    val expandedTransfers = remember { mutableStateListOf<Long>() }
    val roomGroups = remember(transfers) { groupTransfersForActivity(transfers) }

    Column(
        Modifier
            .semantics { testTagsAsResourceId = true }
            .testTag("transfer_activity")
            .fillMaxSize()
            .background(colors.bg),
    ) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onBack) {
                Icon(
                    Icons.AutoMirrored.Filled.ArrowBack,
                    contentDescription = appText("Back", "返回"),
                    tint = colors.accent,
                )
            }
            Text(
                appText("Activity", "活动"),
                color = colors.text,
                fontSize = 24.sp,
                fontWeight = FontWeight.ExtraBold,
            )
        }

        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            if (transfers.isEmpty()) {
                item {
                    Text(
                        appText(
                            "No transfers yet. They will appear here after a room starts.",
                            "暂无传输；建立房间后，传输记录会显示在这里。",
                        ),
                        color = colors.muted,
                        fontSize = 14.sp,
                        modifier = Modifier.padding(vertical = 32.dp),
                    )
                }
            } else {
                roomGroups.forEach { group ->
                    item(key = "room_header:${group.key}") {
                        val expanded = group.key in expandedRooms
                        ActivityRoomCard(
                            group = group,
                            expanded = expanded,
                            onToggleRoom = {
                                if (expanded) {
                                    expandedRooms.remove(group.key)
                                } else {
                                    expandedRooms.add(group.key)
                                }
                            },
                        )
                    }
                    if (group.key in expandedRooms) {
                        item(key = "room_section:${group.key}") {
                            Text(
                                appText("TRANSFERS", "传输记录"),
                                color = colors.muted,
                                fontSize = 10.sp,
                                fontWeight = FontWeight.Bold,
                                letterSpacing = 0.8.sp,
                                modifier = Modifier.padding(horizontal = 12.dp),
                            )
                        }
                        items(
                            items = group.transfers,
                            key = { transfer -> "room_transfer:${transfer.id}" },
                        ) { transfer ->
                            Box(
                                Modifier
                                    .testTag("activity_transfer_${transfer.id}")
                                    .fillMaxWidth()
                                    .padding(horizontal = 8.dp),
                            ) {
                                TransferCard(
                                    t = transfer,
                                    expanded = transfer.id in expandedTransfers,
                                    onToggleDetail = { id ->
                                        if (id in expandedTransfers) {
                                            expandedTransfers.remove(id)
                                        } else {
                                            expandedTransfers.add(id)
                                        }
                                    },
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
        }
    }
}

@Composable
private fun ActivityRoomCard(
    group: ActivityRoomGroup,
    expanded: Boolean,
    onToggleRoom: () -> Unit,
) {
    val colors = Envoix.colors
    val language = LocalAppLanguage.current
    val metrics = remember(group.transfers) { activityRoomMetrics(group.transfers) }
    val dataPaths =
        remember(group.transfers, language) {
            activityRoomDataPaths(group.transfers, language)
        }
    val status = activityRoomStatusKind(metrics)
    val progress =
        if (metrics.total <= 0L) {
            0f
        } else {
            (metrics.bytes.toDouble() / metrics.total.toDouble()).toFloat().coerceIn(0f, 1f)
        }
    val actionLabel =
        if (expanded) {
            appText("Collapse room activity", "收起房间活动")
        } else {
            appText("Expand room activity", "展开房间活动")
        }
    val expansionState =
        if (expanded) {
            appText("Expanded", "已展开")
        } else {
            appText("Collapsed", "已收起")
        }

    Column(
        Modifier
            .testTag(activityRoomTestTag(group.key))
            .fillMaxWidth()
            .clip(RoundedCornerShape(20.dp))
            .background(colors.surfaceRaised)
            .border(1.dp, colors.line, RoundedCornerShape(20.dp))
            .clickable(
                onClickLabel = actionLabel,
                role = Role.Button,
                onClick = onToggleRoom,
            ).semantics {
                stateDescription = expansionState
            }.padding(16.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(
                Modifier
                    .size(44.dp)
                    .clip(RoundedCornerShape(14.dp))
                    .background(colors.accentSoft),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    Icons.Default.MeetingRoom,
                    contentDescription = null,
                    tint = colors.accentStrong,
                    modifier = Modifier.size(22.dp),
                )
            }
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    activityRoomDisplayName(group.label, language, group.isDirect),
                    color = colors.text,
                    fontSize = 17.sp,
                    fontWeight = FontWeight.ExtraBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    activityRoomInventorySummary(metrics, language),
                    color = colors.muted,
                    fontSize = 12.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            ActivityRoomStatus(metrics, status)
            Spacer(Modifier.width(4.dp))
            Icon(
                if (expanded) Icons.Default.ExpandLess else Icons.Default.ExpandMore,
                contentDescription = null,
                tint = colors.muted,
            )
        }

        if (metrics.total > 0L) {
            Spacer(Modifier.height(12.dp))
            LinearProgressIndicator(
                progress = { progress },
                modifier = Modifier.fillMaxWidth().height(7.dp).clip(CircleShape),
                color =
                    when (status) {
                        ActivityRoomStatusKind.Active -> colors.accent
                        ActivityRoomStatusKind.Paused -> colors.warning
                        ActivityRoomStatusKind.NeedsAttention -> colors.danger
                        ActivityRoomStatusKind.Completed -> colors.success
                        ActivityRoomStatusKind.Finished -> colors.muted
                    },
                trackColor = colors.line.copy(alpha = 0.6f),
            )
            Spacer(Modifier.height(7.dp))
            Row(Modifier.fillMaxWidth()) {
                Text(
                    "${humanBytes(metrics.bytes)} / ${humanBytes(metrics.total)}",
                    color = colors.text,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                val performance = activityRoomPerformanceSummary(metrics, language)
                if (performance.isNotEmpty()) {
                    Spacer(Modifier.width(8.dp))
                    Text(
                        performance,
                        color = colors.muted,
                        fontSize = 12.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        textAlign = TextAlign.End,
                        modifier = Modifier.weight(1f),
                    )
                }
            }
        } else {
            val performance = activityRoomPerformanceSummary(metrics, language)
            if (performance.isNotEmpty()) {
                Spacer(Modifier.height(8.dp))
                Text(
                    performance,
                    color = colors.muted,
                    fontSize = 12.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        if (dataPaths.isNotEmpty()) {
            Spacer(Modifier.height(7.dp))
            Text(
                AppText.value("Data path", "数据路径", language) +
                    " · " +
                    dataPaths.joinToString(" · "),
                color = colors.muted,
                fontSize = 12.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

internal fun activityRoomDataPaths(
    transfers: List<Transfer>,
    language: String,
): List<String> {
    val observed =
        transfers
            .mapNotNull { transfer -> ConnectionPathKind.fromWireOrLegacy(transfer.pathAddr) }
            .toSet()
    return ConnectionPathKind.entries.mapNotNull { kind ->
        if (kind in observed) connectionPathLabel(kind.wire, language) else null
    }
}

internal fun activityRoomTestTag(groupKey: String): String =
    buildString(groupKey.length + "activity_room_".length) {
        append("activity_room_")
        groupKey.forEach { character ->
            append(if (character.isLetterOrDigit()) character else '_')
        }
    }

@Composable
private fun ActivityRoomStatus(
    metrics: ActivityRoomMetrics,
    status: ActivityRoomStatusKind,
) {
    val colors = Envoix.colors
    val (label, foreground, background) =
        when (status) {
            ActivityRoomStatusKind.Active ->
                Triple(
                    appText("${metrics.activeCount} active", "${metrics.activeCount} 个进行中"),
                    colors.accent,
                    colors.accentSoft,
                )
            ActivityRoomStatusKind.Paused ->
                Triple(
                    appText("Paused", "已暂停"),
                    colors.warning,
                    colors.warning.copy(alpha = 0.12f),
                )
            ActivityRoomStatusKind.NeedsAttention ->
                Triple(
                    appText("Needs attention", "需要处理"),
                    colors.danger,
                    colors.danger.copy(alpha = 0.12f),
                )
            ActivityRoomStatusKind.Completed ->
                Triple(appText("Completed", "已完成"), colors.success, colors.successSoft)
            ActivityRoomStatusKind.Finished ->
                Triple(appText("Finished", "已结束"), colors.muted, colors.line.copy(alpha = 0.5f))
        }
    Text(
        label,
        color = foreground,
        fontSize = 11.sp,
        fontWeight = FontWeight.Bold,
        modifier =
            Modifier
                .clip(CircleShape)
                .background(background)
                .padding(horizontal = 9.dp, vertical = 4.dp),
    )
}

private fun activityRoomInventorySummary(
    metrics: ActivityRoomMetrics,
    language: String,
): String {
    val parts =
        mutableListOf(
            AppText.value(
                "${metrics.transferCount} ${if (metrics.transferCount == 1) "transfer" else "transfers"}",
                "${metrics.transferCount} 次传输",
                language,
            ),
        )
    if (metrics.fileCount > 0) {
        parts +=
            AppText.value(
                "${metrics.fileCount} ${if (metrics.fileCount == 1) "file" else "files"}",
                "${metrics.fileCount} 个文件",
                language,
            )
    }
    if (metrics.directoryCount > 0) {
        parts +=
            AppText.value(
                "${metrics.directoryCount} ${if (metrics.directoryCount == 1) "folder" else "folders"}",
                "${metrics.directoryCount} 个文件夹",
                language,
            )
    }
    return parts.joinToString(" · ")
}

internal fun activityRoomPerformanceSummary(
    metrics: ActivityRoomMetrics,
    language: String,
): String {
    val speed =
        when {
            metrics.currentBps > 0.0 ->
                AppText.value(
                    "Now ${transferRateString(metrics.currentBps)}",
                    "当前 ${transferRateString(metrics.currentBps)}",
                    language,
                )
            metrics.averageBps > 0.0 ->
                AppText.value(
                    "Avg ${transferRateString(metrics.averageBps)}",
                    "平均 ${transferRateString(metrics.averageBps)}",
                    language,
                )
            else -> ""
        }
    val eta =
        metrics.etaSeconds
            ?.let {
                AppText.value(
                    "ETA ${activityEta(it)}",
                    "预计 ${activityEta(it)}",
                    language,
                )
            }.orEmpty()
    return listOf(speed, eta).filter(String::isNotEmpty).joinToString(" · ")
}

private fun activityEta(seconds: Double): String {
    val totalSeconds = seconds.coerceAtLeast(0.0).toLong()
    val minutes = totalSeconds / SECONDS_PER_MINUTE
    val remainingSeconds = totalSeconds % SECONDS_PER_MINUTE
    return if (minutes > 0L) "${minutes}m ${remainingSeconds}s" else "${remainingSeconds}s"
}
