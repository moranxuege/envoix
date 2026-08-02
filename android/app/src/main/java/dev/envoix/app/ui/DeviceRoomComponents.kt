package dev.envoix.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.ArrowDownward
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.Devices
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.Direction
import dev.envoix.app.SettingsStore
import dev.envoix.app.Status
import dev.envoix.app.Transfer
import dev.envoix.app.humanBytes

internal data class RoomStatus(
    val label: String,
    val foreground: Color,
)

@Composable
internal fun RoomHeader(
    displayName: String,
    control: RoomControlUiState,
    legacyState: RoomStatus,
    onBack: () -> Unit,
    onActivity: () -> Unit,
    onSettings: () -> Unit,
) {
    val colors = Envoix.colors
    val state =
        when (control.phase) {
            RoomControlPhase.Connected ->
                RoomStatus(appText("Authenticated for this room", "已为此房间认证"), colors.success)
            RoomControlPhase.Closed ->
                RoomStatus(control.closeReason.roomEndedLabel(), colors.danger)
            RoomControlPhase.Failed ->
                RoomStatus(appText("Connection failed", "连接失败"), colors.danger)
            else -> legacyState
        }
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onClick = onBack) {
            Icon(
                Icons.AutoMirrored.Filled.ArrowBack,
                contentDescription = appText("Back", "返回"),
                tint = colors.accent,
            )
        }
        Box(
            Modifier.size(42.dp).clip(CircleShape).background(colors.accentSoft),
            contentAlignment = Alignment.Center,
        ) {
            Icon(Icons.Default.Devices, null, tint = colors.accent, modifier = Modifier.size(22.dp))
        }
        Spacer(Modifier.size(11.dp))
        Column(Modifier.weight(1f)) {
            Text(
                when (control.phase) {
                    RoomControlPhase.Connected ->
                        appText("ROOM", "房间")
                    RoomControlPhase.Closed, RoomControlPhase.Failed ->
                        appText("ROOM ENDED", "房间已结束")
                    else -> appText("ONE-TIME ROOM", "一次性房间")
                },
                color =
                    if (control.phase == RoomControlPhase.Closed ||
                        control.phase == RoomControlPhase.Failed
                    ) {
                        colors.danger
                    } else {
                        colors.warning
                    },
                fontSize = 10.sp,
                fontWeight = FontWeight.Bold,
                letterSpacing = 0.7.sp,
            )
            Text(
                displayName,
                color = colors.text,
                fontSize = 18.sp,
                fontWeight = FontWeight.Bold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(7.dp).clip(CircleShape).background(state.foreground))
                Spacer(Modifier.size(6.dp))
                Text(
                    state.label,
                    color = state.foreground,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.SemiBold,
                )
            }
        }
        IconButton(
            onClick = onActivity,
            modifier = Modifier.testTag("room_activity"),
        ) {
            Icon(
                Icons.Default.History,
                contentDescription = appText("Activity", "活动"),
                tint = colors.accent,
            )
        }
        IconButton(
            onClick = onSettings,
            modifier = Modifier.testTag("room_settings"),
        ) {
            Icon(
                Icons.Default.Settings,
                contentDescription = appText("Settings", "设置"),
                tint = colors.accent,
            )
        }
    }
}

@Composable
private fun RoomCloseReason?.roomEndedLabel(): String =
    when (this) {
        RoomCloseReason.IdleExpired -> appText("Closed after 15 minutes idle", "闲置 15 分钟后已关闭")
        RoomCloseReason.InvitationExpired -> appText("Invitation expired", "邀请已过期")
        RoomCloseReason.PeerEnded -> appText("The other device ended the room", "另一台设备已结束房间")
        RoomCloseReason.Backgrounded -> appText("Closed when Envoix left the foreground", "离开 Envoix 后已关闭")
        RoomCloseReason.NetworkLost -> appText("Connection lost", "连接已断开")
        RoomCloseReason.ProtocolFailure -> appText("Room connection failed", "房间连接失败")
        else -> appText("Room ended", "房间已结束")
    }

@Composable
internal fun RoomTransferSummary(
    transfer: Transfer,
    onOpen: (Transfer) -> Unit,
    onShare: (Transfer) -> Unit,
) {
    val colors = Envoix.colors
    val saveLocation =
        resolvedSavedDestinationLabel(
            recordedDestinationLabel = transfer.savedDestinationLabel,
            fallbackDestinationLabel = SettingsStore.saveLabel(LocalContext.current),
        )
    val itemCount = transfer.fileCount + transfer.directoryCount
    val title =
        when {
            transfer.savedUris.size == 1 && !transfer.savedName.isNullOrBlank() -> transfer.savedName
            !transfer.fileName.isNullOrBlank() -> transfer.fileName
            itemCount == 1 -> appText("1 item", "1 个项目")
            itemCount > 1 -> appText("$itemCount items", "$itemCount 个项目")
            transfer.direction == Direction.Send -> appText("Outgoing transfer", "待发送内容")
            else -> appText("Incoming transfer", "待接收内容")
        }
    val progress =
        if (transfer.total <= 0L) {
            0f
        } else {
            (transfer.bytes.toFloat() / transfer.total.toFloat()).coerceIn(0f, 1f)
        }
    val received = transfer.direction == Direction.Receive && transfer.status == Status.Delivered

    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(colors.surface)
            .border(1.dp, colors.line, RoundedCornerShape(16.dp))
            .padding(14.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(
                Modifier.size(38.dp).clip(CircleShape).background(colors.accentSoft),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    if (transfer.direction == Direction.Send) {
                        Icons.Default.ArrowUpward
                    } else {
                        Icons.Default.ArrowDownward
                    },
                    contentDescription = null,
                    tint = colors.accent,
                    modifier = Modifier.size(19.dp),
                )
            }
            Spacer(Modifier.size(10.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    title,
                    color = colors.text,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.Bold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    roomTransferStatus(transfer),
                    color = if (transfer.status == Status.Failed) colors.danger else colors.muted,
                    fontSize = 12.sp,
                )
            }
        }
        if (received) {
            Spacer(Modifier.height(10.dp))
            Text(
                appText("Saved to $saveLocation", "已保存到 $saveLocation"),
                color = colors.success,
                fontSize = 12.sp,
                fontWeight = FontWeight.SemiBold,
            )
            if (transfer.savedUri != null || transfer.savedUris.isNotEmpty()) {
                Row(
                    Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.End,
                ) {
                    if (transfer.savedUri != null) {
                        TextButton(
                            onClick = { onOpen(transfer) },
                            modifier = Modifier.testTag("room_open_received"),
                        ) {
                            Text(appText("Open", "打开"), color = colors.accent)
                        }
                    }
                    if (transfer.savedUris.isNotEmpty()) {
                        TextButton(
                            onClick = { onShare(transfer) },
                            modifier = Modifier.testTag("room_share_received"),
                        ) {
                            Text(appText("Share", "分享"), color = colors.accent)
                        }
                    }
                }
            }
        } else if (transfer.total > 0L && transfer.status != Status.Delivered) {
            Spacer(Modifier.height(10.dp))
            LinearProgressIndicator(
                progress = { progress },
                modifier = Modifier.fillMaxWidth().height(6.dp).clip(CircleShape),
                color = if (transfer.status == Status.Failed) colors.danger else colors.accent,
                trackColor = colors.line.copy(alpha = 0.6f),
            )
            Spacer(Modifier.height(6.dp))
            Text(
                "${humanBytes(transfer.bytes)} / ${humanBytes(transfer.total)}",
                color = colors.muted,
                fontSize = 11.sp,
            )
        }
    }
}

@Composable
private fun roomTransferStatus(transfer: Transfer): String =
    when (transfer.status) {
        Status.Preparing -> appText("Preparing", "正在准备")
        Status.WaitingForPeer -> appText("Waiting for the other device", "等待另一台设备")
        Status.Pairing, Status.Connecting -> appText("Connecting", "正在连接")
        Status.AwaitingDecision -> appText("Waiting for confirmation", "等待确认")
        Status.Transferring -> appText("Transferring", "正在传输")
        Status.Verifying -> appText("Verifying", "正在校验")
        Status.Saving, Status.WaitingForReceiverSave, Status.FinalizingDelivery ->
            appText("Finishing", "正在完成")
        Status.Paused -> appText("Paused", "已暂停")
        Status.Delivered ->
            if (transfer.direction == Direction.Send) {
                appText("Delivered", "已送达")
            } else {
                appText("Received", "已接收")
            }
        Status.Failed -> appText("Needs attention", "需要处理")
        Status.Canceled -> appText("Canceled", "已取消")
    }

@Composable
internal fun EmptyRoomTimeline() {
    val colors = Envoix.colors
    Box(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(18.dp))
            .background(colors.surface)
            .border(1.dp, colors.line, RoundedCornerShape(18.dp))
            .padding(horizontal = 20.dp, vertical = 26.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            appText(
                "No transfers in this room yet. Add files when you are ready.",
                "这个房间中还没有传输。准备好后即可添加文件。",
            ),
            color = colors.muted,
            fontSize = 13.sp,
            fontWeight = FontWeight.SemiBold,
        )
    }
}

@Composable
internal fun PendingRoomAction(
    role: String,
    pendingShareCount: Int,
    onContinue: () -> Unit,
) {
    val colors = Envoix.colors
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(colors.accentSoft)
            .padding(16.dp),
    ) {
        Text(
            if (role == "receive") {
                appText("A transfer invite is ready", "传输邀请已就绪")
            } else if (pendingShareCount > 0) {
                appText("$pendingShareCount shared items are ready", "$pendingShareCount 个共享项目已就绪")
            } else {
                appText("This device is ready for files", "此设备已准备好传输文件")
            },
            color = colors.accentStrong,
            fontSize = 14.sp,
            fontWeight = FontWeight.Bold,
        )
        Spacer(Modifier.height(4.dp))
        Text(
            if (role == "receive") {
                appText(
                    "Review the invitation and receive location.",
                    "确认邀请和接收位置。",
                )
            } else {
                appText(
                    "Review the selection before offering it to the other device.",
                    "发送给另一台设备前，请先确认所选内容。",
                )
            },
            color = colors.muted,
            fontSize = 12.sp,
            lineHeight = 17.sp,
        )
        Spacer(Modifier.height(12.dp))
        Button(onClick = onContinue, modifier = Modifier.testTag("room_review_invite")) {
            Text(
                when {
                    role == "receive" -> appText("Continue", "继续")
                    pendingShareCount > 0 ->
                        appText(
                            "Continue with $pendingShareCount items",
                            "继续发送 $pendingShareCount 个项目",
                        )
                    else -> appText("Choose files", "选择文件")
                },
            )
        }
    }
}

@Composable
internal fun IncomingRoomOfferCard(
    offer: RoomTransferOffer,
    destination: RoomDestinationPresentation,
    busy: Boolean,
    error: String?,
    onAccept: () -> Unit,
    onReject: () -> Unit,
) {
    val colors = Envoix.colors
    val fileCount = (offer.itemCount - offer.directoryCount).coerceAtLeast(0)
    val visibleRoots = offer.rootNames.take(3)
    val hiddenItemCount = (offer.itemCount - visibleRoots.size).coerceAtLeast(0)
    val fileSummary = if (fileCount == 1) "1 file" else "$fileCount files"
    val folderSummary =
        if (offer.directoryCount == 1) "1 folder" else "${offer.directoryCount} folders"
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(18.dp))
            .background(colors.accentSoft)
            .border(1.dp, colors.accent.copy(alpha = 0.22f), RoundedCornerShape(18.dp))
            .padding(16.dp),
    ) {
        Text(
            appText("Incoming transfer", "收到传输邀请"),
            color = colors.accentStrong,
            fontSize = 15.sp,
            fontWeight = FontWeight.Bold,
        )
        Spacer(Modifier.height(10.dp))
        Text(
            appText("OFFER SUMMARY", "传输摘要"),
            color = colors.muted,
            fontSize = 11.sp,
            fontWeight = FontWeight.Bold,
            letterSpacing = 0.6.sp,
        )
        Text(
            appText(
                "$fileSummary · $folderSummary · ${humanBytes(offer.totalBytes)}",
                "$fileCount 个文件 · ${offer.directoryCount} 个文件夹 · ${humanBytes(offer.totalBytes)}",
            ),
            color = colors.text,
            fontSize = 13.sp,
        )
        Spacer(Modifier.height(10.dp))
        Text(
            appText("DESTINATION", "保存位置"),
            color = colors.muted,
            fontSize = 11.sp,
            fontWeight = FontWeight.Bold,
            letterSpacing = 0.6.sp,
        )
        Text(
            destination.label,
            color = if (destination.ready) colors.text else colors.danger,
            fontSize = 13.sp,
        )
        if (visibleRoots.isNotEmpty()) {
            Spacer(Modifier.height(10.dp))
            Text(
                appText("CONTENTS", "内容"),
                color = colors.muted,
                fontSize = 11.sp,
                fontWeight = FontWeight.Bold,
                letterSpacing = 0.6.sp,
            )
            visibleRoots.forEach { rootName ->
                Text(
                    rootName,
                    color = colors.text,
                    fontSize = 13.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        if (hiddenItemCount > 0) {
            if (visibleRoots.isEmpty()) Spacer(Modifier.height(10.dp))
            Text(
                appText(
                    "+$hiddenItemCount more",
                    "另有 $hiddenItemCount 项",
                ),
                color = colors.muted,
                fontSize = 12.sp,
                fontWeight = FontWeight.Bold,
            )
        }
        error?.let {
            Spacer(Modifier.height(8.dp))
            Text(it, color = colors.danger, fontSize = 12.sp)
        }
        Spacer(Modifier.height(12.dp))
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            OutlinedButton(
                onClick = onReject,
                enabled = !busy,
                modifier = Modifier.weight(1f).testTag("room_offer_reject"),
            ) {
                Text(appText("Decline", "拒绝"))
            }
            Button(
                onClick = onAccept,
                enabled = !busy,
                modifier = Modifier.weight(1f).testTag("room_offer_accept"),
            ) {
                Text(
                    if (busy) {
                        appText("Preparing receiver…", "正在准备接收…")
                    } else {
                        appText("Receive", "接收")
                    },
                )
            }
        }
    }
}

@Composable
internal fun RoomControlPanel(
    control: RoomControlUiState,
    legacy: Boolean,
    onAddFiles: () -> Unit,
    onShowQr: () -> Unit,
    onKeepOpen: (Boolean) -> Unit,
    onEnd: () -> Unit,
    onDone: () -> Unit,
) {
    val colors = Envoix.colors
    val terminal =
        control.phase == RoomControlPhase.Closed ||
            control.phase == RoomControlPhase.Failed
    val canAddFiles =
        !terminal &&
            (
                legacy ||
                    (
                        control.connected &&
                            control.incomingOffer == null &&
                            !control.outgoingOfferPending
                    )
            )
    Column(
        Modifier
            .fillMaxWidth()
            .background(colors.surface)
            .padding(horizontal = 16.dp, vertical = 12.dp)
            .navigationBarsPadding(),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        if (terminal) {
            Text(
                if (control.phase == RoomControlPhase.Failed) {
                    control.error ?: appText("Room connection failed", "房间连接失败")
                } else {
                    control.closeReason.roomEndedLabel()
                },
                color = colors.danger,
                fontSize = 13.sp,
                fontWeight = FontWeight.Bold,
                modifier =
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(12.dp))
                        .background(colors.danger.copy(alpha = 0.09f))
                        .padding(12.dp),
            )
            Spacer(Modifier.height(8.dp))
        } else {
            control.error?.let { error ->
                Text(
                    error,
                    color = colors.danger,
                    fontSize = 12.sp,
                    modifier =
                        Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(12.dp))
                            .background(colors.danger.copy(alpha = 0.09f))
                            .padding(10.dp),
                )
                Spacer(Modifier.height(8.dp))
            }
        }
        RoomActionButton(
            label = appText("Add files", "添加文件"),
            icon = Icons.Default.Add,
            onClick = onAddFiles,
            enabled = canAddFiles,
            modifier = Modifier.fillMaxWidth().testTag("room_add_files"),
        )
        when {
            terminal -> {
                Spacer(Modifier.height(8.dp))
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        appText("Room closed", "房间已关闭"),
                        color = colors.muted,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.weight(1f),
                    )
                    TextButton(onClick = onDone, modifier = Modifier.testTag("room_done")) {
                        Text(appText("Done", "完成"), color = colors.accent)
                    }
                }
            }
            legacy -> {
                TextButton(onClick = onShowQr, modifier = Modifier.testTag("room_show_qr")) {
                    Text(appText("Show transfer QR", "显示传输二维码"), color = colors.accent)
                }
                OutlinedButton(
                    onClick = onEnd,
                    modifier = Modifier.fillMaxWidth().testTag("room_close"),
                    colors = ButtonDefaults.outlinedButtonColors(contentColor = colors.danger),
                ) {
                    Text(appText("End room", "结束房间"))
                }
            }
            else -> {
                Spacer(Modifier.height(8.dp))
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(
                            roomLifetimeLabel(control),
                            color = colors.muted,
                            fontSize = 12.sp,
                            fontWeight = FontWeight.SemiBold,
                        )
                        if (!control.creator) {
                            Text(
                                appText("The room creator controls its lifetime.", "房间时限由创建者控制。"),
                                color = colors.muted,
                                fontSize = 10.sp,
                            )
                        }
                    }
                    if (control.creator) {
                        TextButton(
                            onClick = {
                                onKeepOpen(
                                    control.policy != RoomLifetimePolicy.UntilForegroundEnds,
                                )
                            },
                            modifier = Modifier.testTag("room_keep_open"),
                        ) {
                            Text(
                                if (control.policy == RoomLifetimePolicy.UntilForegroundEnds) {
                                    appText("Use 15 min", "使用 15 分钟")
                                } else {
                                    appText("Keep open", "保持开启")
                                },
                                color = colors.accent,
                            )
                        }
                    }
                }
                OutlinedButton(
                    onClick = onEnd,
                    modifier = Modifier.fillMaxWidth().testTag("room_close"),
                    colors = ButtonDefaults.outlinedButtonColors(contentColor = colors.danger),
                ) {
                    Text(appText("End room", "结束房间"))
                }
            }
        }
    }
}

@Composable
private fun roomLifetimeLabel(control: RoomControlUiState): String {
    if (control.policy == RoomLifetimePolicy.UntilForegroundEnds) {
        return appText("Kept open while Envoix is active", "Envoix 使用期间保持开启")
    }
    val deadline =
        control.idleDeadlineEpochMs
            ?: return appText("Idle timer paused", "闲置计时已暂停")
    val seconds = ((deadline - control.nowEpochMs).coerceAtLeast(0L) + 999L) / 1_000L
    if (seconds == 0L) {
        return if (control.creator) {
            appText("Closing room…", "正在关闭房间…")
        } else {
            appText("Waiting for room creator…", "等待房间创建者…")
        }
    }
    val minutesPart = seconds / 60L
    val secondsPart = seconds % 60L
    return appText(
        "Closes after ${"%d:%02d".format(minutesPart, secondsPart)} idle",
        "闲置 ${"%d:%02d".format(minutesPart, secondsPart)} 后关闭",
    )
}

@Composable
private fun RoomActionButton(
    label: String,
    icon: ImageVector,
    onClick: () -> Unit,
    enabled: Boolean = true,
    modifier: Modifier = Modifier,
) {
    val colors = Envoix.colors
    Button(
        onClick = onClick,
        enabled = enabled,
        modifier = modifier.height(50.dp),
        colors =
            ButtonDefaults.buttonColors(
                containerColor = colors.accent,
                disabledContainerColor = colors.accent.copy(alpha = 0.38f),
            ),
        shape = RoundedCornerShape(14.dp),
    ) {
        Icon(icon, null, tint = Color.White, modifier = Modifier.size(19.dp))
        Spacer(Modifier.size(7.dp))
        Text(label, color = Color.White, fontSize = 14.sp, fontWeight = FontWeight.Bold)
    }
}

@Composable
internal fun roomState(
    active: List<Transfer>,
    hasNearbyContext: Boolean,
    nearbyAvailable: Boolean?,
): RoomStatus {
    val colors = Envoix.colors
    val waiting =
        active.all {
            it.status == Status.Preparing ||
                it.status == Status.WaitingForPeer ||
                it.status == Status.Pairing ||
                it.status == Status.Connecting
        }
    return when {
        active.isEmpty() ->
            RoomStatus(
                label =
                    when {
                        hasNearbyContext && nearbyAvailable == true ->
                            appText("Nearby · unverified", "附近可见 · 未验证")
                        hasNearbyContext ->
                            appText("Nearby device not visible", "附近设备当前不可见")
                        else -> appText("Ready · unverified", "已就绪 · 未验证")
                    },
                foreground = if (hasNearbyContext && nearbyAvailable != true) colors.warning else colors.muted,
            )
        waiting ->
            RoomStatus(
                label = appText("Waiting", "等待中"),
                foreground = colors.warning,
            )
        else ->
            RoomStatus(
                label = appText("${active.size} active", "${active.size} 个进行中"),
                foreground = colors.success,
            )
    }
}
