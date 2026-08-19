package dev.envoix.app.ui

import androidx.compose.foundation.background
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
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Smartphone
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.RememberedPeerSummary
import dev.envoix.app.RoomOutboxEntry
import dev.envoix.app.RoomOutboxState
import dev.envoix.app.Transfer
import dev.envoix.app.humanBytes

@Composable
internal fun RememberedRoomsScreen(
    state: RememberedRoomsUiState,
    onBack: () -> Unit,
    onOpenRoom: (String) -> Unit,
    onDismissError: () -> Unit,
) {
    val colors = Envoix.colors
    Column(
        Modifier
            .testTag("remembered_rooms")
            .fillMaxSize()
            .background(colors.bg),
    ) {
        RememberedRoomsAppBar(
            title = appText("Rooms", "房间"),
            onBack = onBack,
        )
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item {
                Text(
                    appText(
                        "Saved devices can reconnect while Envoix is open.",
                        "已保存的设备可在 Envoix 打开时重新连接。",
                    ),
                    color = colors.muted,
                    fontSize = 14.sp,
                    modifier = Modifier.padding(bottom = 4.dp),
                )
            }
            when {
                state.loading ->
                    item {
                        Box(
                            Modifier
                                .fillMaxWidth()
                                .padding(vertical = 36.dp),
                            contentAlignment = Alignment.Center,
                        ) {
                            CircularProgressIndicator(
                                modifier = Modifier.size(28.dp),
                                color = colors.accent,
                                strokeWidth = 3.dp,
                            )
                        }
                    }
                state.peers.isEmpty() ->
                    item {
                        EmptyRememberedRooms()
                    }
                else ->
                    items(state.peers, key = RememberedPeerSummary::relationshipId) { peer ->
                        RememberedRoomRow(
                            peer = peer,
                            connection = state.connections[peer.relationshipId],
                            transferState = state.transfers[peer.relationshipId],
                            onClick = { onOpenRoom(peer.relationshipId) },
                        )
                    }
            }
        }
    }
    state.error?.let {
        RememberedRoomErrorDialog(
            message = it,
            onDismiss = onDismissError,
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun RememberedRoomDetailScreen(
    peer: RememberedPeerSummary?,
    connection: RememberedRoomConnectionState?,
    transferState: RememberedRoomTransferState?,
    error: String?,
    connectionManager: RememberedRoomConnectionManager,
    onBack: () -> Unit,
    onRetry: (String) -> Unit,
    onRename: (String, String, () -> Unit) -> Unit,
    onForget: (String, () -> Unit) -> Unit,
    onQueuePrepared: (
        relationshipId: String,
        jobId: String,
        rootNames: List<String>,
        itemCount: Int,
        directoryCount: Int,
        totalBytes: Long,
        completion: (String?) -> Unit,
    ) -> Unit,
    onRetryOutbox: (String) -> Unit,
    onRemoveOutbox: (String) -> Unit,
    onAcceptIncoming: (String) -> Unit,
    onRejectIncoming: (String) -> Unit,
    onClearTransferError: (String) -> Unit,
    onOpenReceived: (Transfer) -> Unit,
    onShareReceived: (Transfer) -> Unit,
    onExternalActivityChanged: (Boolean) -> Unit,
    onDismissError: () -> Unit,
    transferPreferences: TransferSetupPreferences,
    sourcePreparationIntents: TransferSourcePreparationIntents,
    onSaveTreePicked: (android.net.Uri) -> Unit,
) {
    val colors = Envoix.colors
    val relationshipId = peer?.relationshipId
    DisposableEffect(relationshipId) {
        relationshipId?.let { connectionManager.setRoomOpen(it, true) }
        onDispose {
            relationshipId?.let { connectionManager.setRoomOpen(it, false) }
        }
    }
    if (peer == null) {
        Column(
            Modifier
                .testTag("remembered_room_missing")
                .fillMaxSize()
                .background(colors.bg),
        ) {
            RememberedRoomsAppBar(
                title = appText("Room", "房间"),
                onBack = onBack,
            )
            Text(
                appText(
                    "This saved room is no longer available.",
                    "此已保存房间已不可用。",
                ),
                color = colors.muted,
                modifier = Modifier.padding(24.dp),
            )
        }
        return
    }

    var renameOpen by remember(peer.relationshipId) { mutableStateOf(false) }
    var forgetOpen by remember(peer.relationshipId) { mutableStateOf(false) }
    var addFilesOpen by remember(peer.relationshipId) { mutableStateOf(false) }
    var addFilesSession by remember(peer.relationshipId) { mutableStateOf(0) }
    val transferPreparation =
        remember(peer.relationshipId, addFilesSession) {
            TransferDraftPreparationState(initialRole = "send")
        }
    DisposableEffect(transferPreparation) {
        onDispose {
            transferPreparation.discard()
        }
    }
    Column(
        Modifier
            .testTag("remembered_room_detail")
            .fillMaxSize()
            .background(colors.bg),
    ) {
        RememberedRoomsAppBar(
            title = peer.label,
            onBack = onBack,
        )
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            item {
                RememberedRoomStatusCard(connection)
            }
            transferState?.incomingOffer?.let { offer ->
                item {
                    RememberedIncomingOfferCard(
                        offer = offer,
                        busy = transferState.incomingBusy,
                        onAccept = { onAcceptIncoming(peer.relationshipId) },
                        onReject = { onRejectIncoming(peer.relationshipId) },
                    )
                }
            }
            transferState?.latestReceivedTransfer?.let { transfer ->
                item {
                    Column(
                        modifier = Modifier.testTag("remembered_room_latest_received"),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Text(
                            appText("LATEST RECEIVED", "最近接收"),
                            color = colors.muted,
                            fontSize = 12.sp,
                            fontWeight = FontWeight.Bold,
                            letterSpacing = 0.8.sp,
                        )
                        RoomTransferSummary(
                            transfer = transfer,
                            defaultDestinationLabel = transferPreferences.saveLocationLabel,
                            onOpen = onOpenReceived,
                            onShare = onShareReceived,
                        )
                    }
                }
            }
            item {
                Button(
                    onClick = {
                        addFilesSession += 1
                        addFilesOpen = true
                    },
                    enabled =
                        transferState?.incomingOffer == null &&
                            transferState?.incomingBusy != true,
                    modifier =
                        Modifier
                            .testTag("remembered_room_add_files")
                            .fillMaxWidth(),
                ) {
                    Icon(Icons.Default.Add, null, modifier = Modifier.size(19.dp))
                    Spacer(Modifier.width(8.dp))
                    Text(appText("Add files", "添加文件"))
                }
            }
            if (transferState?.outbox?.isNotEmpty() == true) {
                item {
                    Text(
                        appText("OUTBOX", "发送队列"),
                        color = colors.muted,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.Bold,
                        letterSpacing = 0.8.sp,
                    )
                }
                items(transferState.outbox, key = RoomOutboxEntry::id) { entry ->
                    RememberedRoomOutboxCard(
                        entry = entry,
                        onRetry = { onRetryOutbox(entry.id) },
                        onRemove = { onRemoveOutbox(entry.id) },
                    )
                }
            }
            if (connection?.phase == RememberedRoomConnectionPhase.NeedsAttention) {
                item {
                    Button(
                        onClick = { onRetry(peer.relationshipId) },
                        modifier =
                            Modifier
                                .testTag("remembered_room_retry")
                                .fillMaxWidth(),
                    ) {
                        Text(appText("Try again", "重试"))
                    }
                }
            }
            item {
                Text(
                    appText("ROOM CONTROLS", "房间控制"),
                    color = colors.muted,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.Bold,
                    letterSpacing = 0.8.sp,
                )
            }
            item {
                OutlinedButton(
                    onClick = { renameOpen = true },
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Icon(Icons.Default.Edit, null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(8.dp))
                    Text(appText("Rename room", "重命名房间"))
                }
            }
            item {
                OutlinedButton(
                    onClick = { forgetOpen = true },
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Icon(
                        Icons.Default.Delete,
                        null,
                        tint = colors.danger,
                        modifier = Modifier.size(18.dp),
                    )
                    Spacer(Modifier.width(8.dp))
                    Text(
                        appText("Forget this room", "忘记此房间"),
                        color = colors.danger,
                    )
                }
            }
        }
    }

    if (addFilesOpen) {
        ModalBottomSheet(
            onDismissRequest = {
                transferPreparation.discard()
                addFilesOpen = false
            },
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            containerColor = colors.surface,
        ) {
            NewTransferSheet(
                draftId = "remembered-${peer.relationshipId}-$addFilesSession",
                preparationState = transferPreparation,
                initialRole = "send",
                roomMode = true,
                connectedRoom = true,
                roomEndpoint = RoomControlEndpoint(peer.broker, peer.relay),
                preferences = transferPreferences,
                sourcePreparationIntents = sourcePreparationIntents,
                onSaveTreePicked = onSaveTreePicked,
                onReceive = { _, _, _, _, _, _, _ -> },
                onSend = { _, _, _, _, _, _, _ -> },
                onExternalActivityChanged = onExternalActivityChanged,
                onQueuePreparedSend = {
                    jobId,
                    rootNames,
                    itemCount,
                    directoryCount,
                    totalBytes,
                    completion,
                    ->
                    onQueuePrepared(
                        peer.relationshipId,
                        jobId,
                        rootNames,
                        itemCount,
                        directoryCount,
                        totalBytes,
                    ) { queueError ->
                        completion(queueError)
                        if (queueError == null) addFilesOpen = false
                    }
                },
            )
        }
    }

    if (renameOpen) {
        RenameRememberedRoomDialog(
            initialLabel = peer.label,
            onDismiss = { renameOpen = false },
            onRename = { label ->
                onRename(peer.relationshipId, label) {
                    renameOpen = false
                }
            },
        )
    }
    if (forgetOpen) {
        AlertDialog(
            onDismissRequest = { forgetOpen = false },
            title = { Text(appText("Forget this room?", "忘记此房间？")) },
            text = {
                Text(
                    appText(
                        "This removes the protected relationship and its waiting or failed file jobs from this device. Active transfers must finish first.",
                        "这会从此设备移除受保护的配对关系，以及等待中或失败的文件任务；进行中的传输必须先结束。",
                    ),
                )
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        onForget(peer.relationshipId) {
                            forgetOpen = false
                            onBack()
                        }
                    },
                ) {
                    Text(appText("Forget", "忘记"), color = colors.danger)
                }
            },
            dismissButton = {
                TextButton(onClick = { forgetOpen = false }) {
                    Text(appText("Cancel", "取消"))
                }
            },
        )
    }
    error?.let {
        RememberedRoomErrorDialog(
            message = it,
            onDismiss = onDismissError,
        )
    }
    transferState?.error?.let {
        RememberedRoomErrorDialog(
            message = it,
            onDismiss = { onClearTransferError(peer.relationshipId) },
        )
    }
}

@Composable
private fun RememberedIncomingOfferCard(
    offer: RoomTransferOffer,
    busy: Boolean,
    onAccept: () -> Unit,
    onReject: () -> Unit,
) {
    val colors = Envoix.colors
    Column(
        Modifier
            .fillMaxWidth()
            .background(colors.accentSoft, RoundedCornerShape(20.dp))
            .padding(18.dp),
    ) {
        Text(
            appText("Incoming files", "收到文件邀请"),
            color = colors.text,
            fontSize = 17.sp,
            fontWeight = FontWeight.Bold,
        )
        Text(
            offer.rootNames.take(3).joinToString().ifBlank {
                appText("Prepared items", "已准备的项目")
            },
            color = colors.muted,
            fontSize = 14.sp,
            modifier = Modifier.padding(top = 6.dp),
        )
        Text(
            "${offer.itemCount} · ${humanBytes(offer.totalBytes)}",
            color = colors.muted,
            fontSize = 13.sp,
            modifier = Modifier.padding(top = 3.dp),
        )
        Row(
            Modifier
                .fillMaxWidth()
                .padding(top = 14.dp),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            OutlinedButton(
                onClick = onReject,
                enabled = !busy,
                modifier = Modifier.weight(1f),
            ) {
                Text(appText("Decline", "拒绝"))
            }
            Button(
                onClick = onAccept,
                enabled = !busy,
                modifier = Modifier.weight(1f),
            ) {
                Text(
                    if (busy) {
                        appText("Preparing…", "正在准备…")
                    } else {
                        appText("Receive", "接收")
                    },
                )
            }
        }
    }
}

@Composable
private fun RememberedRoomOutboxCard(
    entry: RoomOutboxEntry,
    onRetry: () -> Unit,
    onRemove: () -> Unit,
) {
    val colors = Envoix.colors
    val active =
        entry.state == RoomOutboxState.Preparing ||
            entry.state == RoomOutboxState.Offering ||
            entry.state == RoomOutboxState.Transferring
    Column(
        Modifier
            .fillMaxWidth()
            .background(colors.surface, RoundedCornerShape(18.dp))
            .padding(16.dp),
    ) {
        Text(
            entry.rootNames.joinToString().ifBlank {
                appText("Prepared items", "已准备的项目")
            },
            color = colors.text,
            fontWeight = FontWeight.SemiBold,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            "${roomOutboxStateText(entry.state)} · " +
                "${entry.itemCount} · ${humanBytes(entry.totalBytes)}",
            color =
                if (entry.state == RoomOutboxState.NeedsAttention) {
                    colors.danger
                } else {
                    colors.muted
                },
            fontSize = 13.sp,
            modifier = Modifier.padding(top = 5.dp),
        )
        entry.lastError?.let {
            Text(
                it,
                color = colors.muted,
                fontSize = 12.sp,
                modifier = Modifier.padding(top = 5.dp),
            )
        }
        Row(
            Modifier
                .fillMaxWidth()
                .padding(top = 10.dp),
            horizontalArrangement = Arrangement.End,
        ) {
            if (entry.state == RoomOutboxState.NeedsAttention) {
                TextButton(onClick = onRetry) {
                    Text(appText("Retry", "重试"))
                }
            }
            TextButton(onClick = onRemove, enabled = !active) {
                Text(appText("Remove", "移除"), color = colors.danger)
            }
        }
    }
}

@Composable
private fun roomOutboxStateText(state: RoomOutboxState): String =
    when (state) {
        RoomOutboxState.Preparing -> appText("Queueing", "正在加入队列")
        RoomOutboxState.Queued -> appText("Waiting for peer", "等待对端上线")
        RoomOutboxState.Offering -> appText("Offering", "正在发送邀请")
        RoomOutboxState.Transferring -> appText("Transferring", "正在传输")
        RoomOutboxState.NeedsAttention -> appText("Needs attention", "需要处理")
    }

@Composable
private fun RememberedRoomsAppBar(
    title: String,
    onBack: () -> Unit,
) {
    val colors = Envoix.colors
    Row(
        Modifier
            .fillMaxWidth()
            .height(62.dp)
            .padding(horizontal = 12.dp),
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
            title,
            color = colors.text,
            fontSize = 24.sp,
            fontWeight = FontWeight.ExtraBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(start = 4.dp),
        )
    }
}

@Composable
private fun EmptyRememberedRooms() {
    val colors = Envoix.colors
    Column(
        Modifier
            .fillMaxWidth()
            .padding(vertical = 40.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(
            Icons.Default.Smartphone,
            null,
            tint = colors.muted,
            modifier = Modifier.size(34.dp),
        )
        Spacer(Modifier.height(12.dp))
        Text(
            appText("No saved rooms", "暂无已保存房间"),
            color = colors.text,
            fontWeight = FontWeight.SemiBold,
        )
        Text(
            appText(
                "Remember a device after a successful transfer.",
                "成功传输后可保存设备。",
            ),
            color = colors.muted,
            fontSize = 13.sp,
            modifier = Modifier.padding(top = 6.dp),
        )
    }
}

@Composable
private fun RememberedRoomRow(
    peer: RememberedPeerSummary,
    connection: RememberedRoomConnectionState?,
    transferState: RememberedRoomTransferState?,
    onClick: () -> Unit,
) {
    val colors = Envoix.colors
    val phase = connection?.phase ?: RememberedRoomConnectionPhase.Offline
    Row(
        Modifier
            .testTag("remembered_room_${peer.relationshipId}")
            .fillMaxWidth()
            .background(colors.surface, RoundedCornerShape(18.dp))
            .clickable(onClick = onClick)
            .padding(16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier
                .size(42.dp)
                .background(colors.accentSoft, CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                Icons.Default.Smartphone,
                null,
                tint = colors.accent,
                modifier = Modifier.size(22.dp),
            )
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(
                peer.label,
                color = colors.text,
                fontSize = 16.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            transferState?.incomingOffer?.let { offer ->
                Text(
                    appText(
                        "Incoming files · ${offer.itemCount} · ${humanBytes(offer.totalBytes)}",
                        "收到文件 · ${offer.itemCount} 项 · ${humanBytes(offer.totalBytes)}",
                    ),
                    color = colors.accentStrong,
                    fontSize = 13.sp,
                    fontWeight = FontWeight.Bold,
                    modifier = Modifier.padding(top = 3.dp),
                )
            }
            Text(
                rememberedConnectionText(phase),
                color = rememberedConnectionColor(phase),
                fontSize = if (transferState?.incomingOffer == null) 13.sp else 12.sp,
                modifier = Modifier.padding(top = 3.dp),
            )
        }
        Icon(
            Icons.AutoMirrored.Filled.KeyboardArrowRight,
            contentDescription = appText("Open", "打开"),
            tint = colors.muted,
        )
    }
}

@Composable
private fun RememberedRoomStatusCard(connection: RememberedRoomConnectionState?) {
    val colors = Envoix.colors
    val phase = connection?.phase ?: RememberedRoomConnectionPhase.Offline
    Column(
        Modifier
            .fillMaxWidth()
            .background(colors.surface, RoundedCornerShape(20.dp))
            .padding(18.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(
                Modifier
                    .size(10.dp)
                    .background(rememberedConnectionColor(phase), CircleShape),
            )
            Spacer(Modifier.width(9.dp))
            Text(
                rememberedConnectionText(phase),
                color = colors.text,
                fontSize = 17.sp,
                fontWeight = FontWeight.Bold,
            )
        }
        Text(
            rememberedConnectionDescription(phase),
            color = colors.muted,
            fontSize = 14.sp,
            modifier = Modifier.padding(top = 10.dp),
        )
        connection?.error?.takeIf(String::isNotBlank)?.let { message ->
            Text(
                message,
                color =
                    if (phase == RememberedRoomConnectionPhase.NeedsAttention) {
                        colors.danger
                    } else {
                        colors.muted
                    },
                fontSize = 13.sp,
                modifier = Modifier.padding(top = 8.dp),
            )
        }
    }
}

@Composable
private fun RenameRememberedRoomDialog(
    initialLabel: String,
    onDismiss: () -> Unit,
    onRename: (String) -> Unit,
) {
    var label by remember(initialLabel) { mutableStateOf(initialLabel) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(appText("Rename room", "重命名房间")) },
        text = {
            OutlinedTextField(
                value = label,
                onValueChange = { label = it },
                singleLine = true,
                label = { Text(appText("Room name", "房间名称")) },
            )
        },
        confirmButton = {
            TextButton(
                onClick = { onRename(label.trim()) },
                enabled = label.isNotBlank(),
            ) {
                Text(appText("Save", "保存"))
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text(appText("Cancel", "取消"))
            }
        },
    )
}

@Composable
private fun RememberedRoomErrorDialog(
    message: String,
    onDismiss: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(appText("Rooms unavailable", "房间不可用")) },
        text = { Text(message) },
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text(appText("OK", "确定"))
            }
        },
    )
}

@Composable
private fun rememberedConnectionText(phase: RememberedRoomConnectionPhase): String =
    when (phase) {
        RememberedRoomConnectionPhase.Offline -> appText("Offline", "离线")
        RememberedRoomConnectionPhase.Waiting -> appText("Waiting for device", "正在等待设备")
        RememberedRoomConnectionPhase.Connecting -> appText("Connecting…", "正在连接…")
        RememberedRoomConnectionPhase.Connected -> appText("Connected", "已连接")
        RememberedRoomConnectionPhase.NeedsAttention -> appText("Needs attention", "需要处理")
    }

@Composable
private fun rememberedConnectionDescription(phase: RememberedRoomConnectionPhase): String =
    when (phase) {
        RememberedRoomConnectionPhase.Offline ->
            appText(
                "Open Envoix on both devices to reconnect.",
                "请在两台设备上打开 Envoix 以重新连接。",
            )
        RememberedRoomConnectionPhase.Waiting ->
            appText(
                "Envoix is ready. Open this room on the other device.",
                "Envoix 已就绪；请在另一台设备上打开此房间。",
            )
        RememberedRoomConnectionPhase.Connecting ->
            appText(
                "Authenticating the protected relationship.",
                "正在验证受保护的配对关系。",
            )
        RememberedRoomConnectionPhase.Connected ->
            appText(
                "The protected control connection is active.",
                "受保护的控制连接已建立。",
            )
        RememberedRoomConnectionPhase.NeedsAttention ->
            appText(
                "The relationship was preserved. Retry when both devices are available.",
                "配对关系仍已保留；请在两台设备都可用时重试。",
            )
    }

@Composable
private fun rememberedConnectionColor(phase: RememberedRoomConnectionPhase): Color {
    val colors = Envoix.colors
    return when (phase) {
        RememberedRoomConnectionPhase.Offline -> colors.muted
        RememberedRoomConnectionPhase.Waiting -> colors.warning
        RememberedRoomConnectionPhase.Connecting -> colors.accent
        RememberedRoomConnectionPhase.Connected -> colors.success
        RememberedRoomConnectionPhase.NeedsAttention -> colors.danger
    }
}
