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
import dev.envoix.app.R
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
            title = appString(R.string.hub_rooms),
            onBack = onBack,
        )
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item {
                Text(
                    appString(R.string.remembered_rooms_reconnect_explanation),
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
    onRoomOpenChanged: (relationshipId: String, open: Boolean) -> Unit,
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
        relationshipId?.let { onRoomOpenChanged(it, true) }
        onDispose {
            relationshipId?.let { onRoomOpenChanged(it, false) }
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
                title = appString(R.string.remembered_room_title),
                onBack = onBack,
            )
            Text(
                appString(R.string.remembered_room_missing),
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
                            appString(R.string.remembered_latest_received_section),
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
                    Text(appString(R.string.room_add_files))
                }
            }
            if (transferState?.outbox?.isNotEmpty() == true) {
                item {
                    Text(
                        appString(R.string.remembered_outbox_section),
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
                        Text(appString(R.string.common_retry))
                    }
                }
            }
            item {
                Text(
                    appString(R.string.remembered_room_controls_section),
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
                    Text(appString(R.string.remembered_rename_room))
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
                        appString(R.string.remembered_forget_room),
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
            title = { Text(appString(R.string.remembered_forget_room_title)) },
            text = {
                Text(appString(R.string.remembered_forget_room_explanation))
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
                    Text(appString(R.string.remembered_forget_action), color = colors.danger)
                }
            },
            dismissButton = {
                TextButton(onClick = { forgetOpen = false }) {
                    Text(appString(R.string.common_cancel))
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
            appString(R.string.remembered_incoming_files),
            color = colors.text,
            fontSize = 17.sp,
            fontWeight = FontWeight.Bold,
        )
        Text(
            offer.rootNames.take(3).joinToString().ifBlank {
                appString(R.string.remembered_prepared_items)
            },
            color = colors.muted,
            fontSize = 14.sp,
            modifier = Modifier.padding(top = 6.dp),
        )
        Text(
            appString(
                R.string.remembered_item_size_summary,
                offer.itemCount,
                humanBytes(offer.totalBytes),
            ),
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
                Text(appString(R.string.common_decline))
            }
            Button(
                onClick = onAccept,
                enabled = !busy,
                modifier = Modifier.weight(1f),
            ) {
                Text(
                    if (busy) {
                        appString(R.string.remembered_preparing)
                    } else {
                        appString(R.string.receive_action_title)
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
                appString(R.string.remembered_prepared_items)
            },
            color = colors.text,
            fontWeight = FontWeight.SemiBold,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            appString(
                R.string.remembered_outbox_summary,
                roomOutboxStateText(entry.state),
                entry.itemCount,
                humanBytes(entry.totalBytes),
            ),
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
                    Text(appString(R.string.common_retry))
                }
            }
            TextButton(onClick = onRemove, enabled = !active) {
                Text(appString(R.string.common_remove), color = colors.danger)
            }
        }
    }
}

@Composable
private fun roomOutboxStateText(state: RoomOutboxState): String =
    when (state) {
        RoomOutboxState.Preparing -> appString(R.string.remembered_outbox_queueing)
        RoomOutboxState.Queued -> appString(R.string.remembered_outbox_waiting_peer)
        RoomOutboxState.Offering -> appString(R.string.remembered_outbox_offering)
        RoomOutboxState.Transferring -> appString(R.string.transfer_status_transferring)
        RoomOutboxState.NeedsAttention -> appString(R.string.transfer_status_needs_attention)
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
                contentDescription = appString(R.string.common_back),
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
            appString(R.string.remembered_rooms_empty_title),
            color = colors.text,
            fontWeight = FontWeight.SemiBold,
        )
        Text(
            appString(R.string.remembered_rooms_empty_explanation),
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
                    appString(
                        R.string.remembered_incoming_files_summary,
                        offer.itemCount,
                        humanBytes(offer.totalBytes),
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
            contentDescription = appString(R.string.common_open),
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
        title = { Text(appString(R.string.remembered_rename_room)) },
        text = {
            OutlinedTextField(
                value = label,
                onValueChange = { label = it },
                singleLine = true,
                label = { Text(appString(R.string.remembered_room_name)) },
            )
        },
        confirmButton = {
            TextButton(
                onClick = { onRename(label.trim()) },
                enabled = label.isNotBlank(),
            ) {
                Text(appString(R.string.common_save))
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text(appString(R.string.common_cancel))
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
        title = { Text(appString(R.string.remembered_rooms_unavailable)) },
        text = { Text(message) },
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text(appString(R.string.common_ok))
            }
        },
    )
}

@Composable
private fun rememberedConnectionText(phase: RememberedRoomConnectionPhase): String =
    when (phase) {
        RememberedRoomConnectionPhase.Offline -> appString(R.string.remembered_connection_offline)
        RememberedRoomConnectionPhase.Waiting -> appString(R.string.remembered_connection_waiting)
        RememberedRoomConnectionPhase.Connecting -> appString(R.string.remembered_connection_connecting)
        RememberedRoomConnectionPhase.Connected -> appString(R.string.remembered_connection_connected)
        RememberedRoomConnectionPhase.NeedsAttention -> appString(R.string.transfer_status_needs_attention)
    }

@Composable
private fun rememberedConnectionDescription(phase: RememberedRoomConnectionPhase): String =
    when (phase) {
        RememberedRoomConnectionPhase.Offline ->
            appString(R.string.remembered_connection_offline_description)
        RememberedRoomConnectionPhase.Waiting ->
            appString(R.string.remembered_connection_waiting_description)
        RememberedRoomConnectionPhase.Connecting ->
            appString(R.string.remembered_connection_connecting_description)
        RememberedRoomConnectionPhase.Connected ->
            appString(R.string.remembered_connection_connected_description)
        RememberedRoomConnectionPhase.NeedsAttention ->
            appString(R.string.remembered_connection_attention_description)
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
