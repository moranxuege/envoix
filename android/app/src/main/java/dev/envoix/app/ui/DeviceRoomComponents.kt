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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.Direction
import dev.envoix.app.R
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
                RoomStatus(appString(R.string.room_authenticated), colors.success)
            RoomControlPhase.Closed ->
                RoomStatus(control.closeReason.roomEndedLabel(), colors.danger)
            RoomControlPhase.Failed ->
                RoomStatus(appString(R.string.connection_failed), colors.danger)
            else -> legacyState
        }
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onClick = onBack) {
            Icon(
                Icons.AutoMirrored.Filled.ArrowBack,
                contentDescription = appString(R.string.common_back),
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
                        appString(R.string.room_header_title)
                    RoomControlPhase.Closed, RoomControlPhase.Failed ->
                        appString(R.string.room_header_ended)
                    else -> appString(R.string.room_header_one_time)
                },
                color =
                    if (control.phase == RoomControlPhase.Closed ||
                        control.phase == RoomControlPhase.Failed
                    ) {
                        colors.danger
                    } else {
                        colors.warning
                    },
                fontSize = 14.sp,
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
                contentDescription = appString(R.string.activity_title),
                tint = colors.accent,
            )
        }
        IconButton(
            onClick = onSettings,
            modifier = Modifier.testTag("room_settings"),
        ) {
            Icon(
                Icons.Default.Settings,
                contentDescription = appString(R.string.settings),
                tint = colors.accent,
            )
        }
    }
}

@Composable
private fun RoomCloseReason?.roomEndedLabel(): String =
    when (this) {
        RoomCloseReason.IdleExpired -> appString(R.string.room_closed_after_idle)
        RoomCloseReason.InvitationExpired -> appString(R.string.room_invitation_expired)
        RoomCloseReason.PeerEnded -> appString(R.string.room_peer_ended)
        RoomCloseReason.Backgrounded -> appString(R.string.room_background_closed)
        RoomCloseReason.NetworkLost -> appString(R.string.connection_lost)
        RoomCloseReason.ProtocolFailure -> appString(R.string.room_connection_failed)
        else -> appString(R.string.room_ended)
    }

@Composable
internal fun RoomTransferSummary(
    transfer: Transfer,
    defaultDestinationLabel: String,
    onOpen: (Transfer) -> Unit,
    onShare: (Transfer) -> Unit,
) {
    val colors = Envoix.colors
    val saveLocation =
        resolvedSavedDestinationLabel(
            recordedDestinationLabel = transfer.savedDestinationLabel,
            fallbackDestinationLabel = defaultDestinationLabel,
        )
    val itemCount = transfer.fileCount + transfer.directoryCount
    val title =
        when {
            transfer.savedUris.size == 1 && !transfer.savedName.isNullOrBlank() -> transfer.savedName
            !transfer.fileName.isNullOrBlank() -> transfer.fileName
            itemCount > 0 -> appQuantityString(R.plurals.room_item_count, itemCount, itemCount)
            transfer.direction == Direction.Send -> appString(R.string.room_outgoing_transfer)
            else -> appString(R.string.room_incoming_transfer)
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
                appString(R.string.room_saved_to, saveLocation),
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
                            Text(appString(R.string.common_open), color = colors.accent)
                        }
                    }
                    if (transfer.savedUris.isNotEmpty()) {
                        TextButton(
                            onClick = { onShare(transfer) },
                            modifier = Modifier.testTag("room_share_received"),
                        ) {
                            Text(appString(R.string.common_share), color = colors.accent)
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
                appString(
                    R.string.transfer_progress_bytes,
                    humanBytes(transfer.bytes),
                    humanBytes(transfer.total),
                ),
                color = colors.muted,
                fontSize = 11.sp,
            )
        }
    }
}

@Composable
private fun roomTransferStatus(transfer: Transfer): String =
    when (transfer.status) {
        Status.Preparing -> appString(R.string.transfer_status_preparing)
        Status.WaitingForPeer -> appString(R.string.transfer_status_waiting_peer)
        Status.Pairing, Status.Connecting -> appString(R.string.transfer_status_connecting)
        Status.AwaitingDecision -> appString(R.string.transfer_status_awaiting_confirmation)
        Status.Transferring -> appString(R.string.transfer_status_transferring)
        Status.Verifying -> appString(R.string.transfer_status_verifying)
        Status.Saving, Status.WaitingForReceiverSave, Status.FinalizingDelivery ->
            appString(R.string.transfer_status_finishing)
        Status.Paused -> appString(R.string.transfer_status_paused)
        Status.Delivered ->
            if (transfer.direction == Direction.Send) {
                appString(R.string.transfer_status_delivered)
            } else {
                appString(R.string.transfer_status_received)
            }
        Status.Failed -> appString(R.string.transfer_status_needs_attention)
        Status.Canceled -> appString(R.string.transfer_status_canceled)
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
            appString(R.string.room_empty_timeline),
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
                appString(R.string.room_transfer_invite_ready)
            } else if (pendingShareCount > 0) {
                appQuantityString(
                    R.plurals.room_shared_items_ready,
                    pendingShareCount,
                    pendingShareCount,
                )
            } else {
                appString(R.string.room_ready_for_files)
            },
            color = colors.accentStrong,
            fontSize = 14.sp,
            fontWeight = FontWeight.Bold,
        )
        Spacer(Modifier.height(4.dp))
        Text(
            if (role == "receive") {
                appString(R.string.room_review_invitation_destination)
            } else {
                appString(R.string.room_review_selection)
            },
            color = colors.muted,
            fontSize = 12.sp,
            lineHeight = 17.sp,
        )
        Spacer(Modifier.height(12.dp))
        Button(onClick = onContinue, modifier = Modifier.testTag("room_review_invite")) {
            Text(
                when {
                    role == "receive" -> appString(R.string.common_continue)
                    pendingShareCount > 0 ->
                        appQuantityString(
                            R.plurals.room_continue_with_items,
                            pendingShareCount,
                            pendingShareCount,
                        )
                    else -> appString(R.string.room_choose_files)
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
    val fileSummary = appQuantityString(R.plurals.room_file_count, fileCount, fileCount)
    val folderSummary =
        appQuantityString(
            R.plurals.room_folder_count,
            offer.directoryCount,
            offer.directoryCount,
        )
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(18.dp))
            .background(colors.accentSoft)
            .border(1.dp, colors.accent.copy(alpha = 0.22f), RoundedCornerShape(18.dp))
            .padding(16.dp),
    ) {
        Text(
            appString(R.string.room_incoming_offer),
            color = colors.accentStrong,
            fontSize = 15.sp,
            fontWeight = FontWeight.Bold,
        )
        Spacer(Modifier.height(10.dp))
        Text(
            appString(R.string.room_offer_summary_section),
            color = colors.muted,
            fontSize = 11.sp,
            fontWeight = FontWeight.Bold,
            letterSpacing = 0.6.sp,
        )
        Text(
            appString(
                R.string.room_offer_summary_format,
                fileSummary,
                folderSummary,
                humanBytes(offer.totalBytes),
            ),
            color = colors.text,
            fontSize = 13.sp,
        )
        Spacer(Modifier.height(10.dp))
        Text(
            appString(R.string.room_destination_section),
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
                appString(R.string.room_contents_section),
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
                appQuantityString(
                    R.plurals.room_more_items,
                    hiddenItemCount,
                    hiddenItemCount,
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
                Text(appString(R.string.common_decline))
            }
            Button(
                onClick = onAccept,
                enabled = !busy,
                modifier = Modifier.weight(1f).testTag("room_offer_accept"),
            ) {
                Text(
                    if (busy) {
                        appString(R.string.room_preparing_receiver)
                    } else {
                        appString(R.string.receive_action_title)
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
                    control.error ?: appString(R.string.room_connection_failed)
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
            label = appString(R.string.room_add_files),
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
                        appString(R.string.room_closed),
                        color = colors.muted,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.weight(1f),
                    )
                    TextButton(onClick = onDone, modifier = Modifier.testTag("room_done")) {
                        Text(appString(R.string.common_done), color = colors.accent)
                    }
                }
            }
            legacy -> {
                TextButton(onClick = onShowQr, modifier = Modifier.testTag("room_show_qr")) {
                    Text(appString(R.string.room_show_transfer_qr), color = colors.accent)
                }
                OutlinedButton(
                    onClick = onEnd,
                    modifier = Modifier.fillMaxWidth().testTag("room_close"),
                    colors = ButtonDefaults.outlinedButtonColors(contentColor = colors.danger),
                ) {
                    Text(appString(R.string.room_end_action))
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
                                appString(R.string.room_creator_controls_lifetime),
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
                                    appString(R.string.room_use_15_minutes)
                                } else {
                                    appString(R.string.room_keep_open)
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
                    Text(appString(R.string.room_end_action))
                }
            }
        }
    }
}

@Composable
private fun roomLifetimeLabel(control: RoomControlUiState): String {
    if (control.policy == RoomLifetimePolicy.UntilForegroundEnds) {
        return appString(R.string.room_kept_open_while_active)
    }
    val deadline =
        control.idleDeadlineEpochMs
            ?: return appString(R.string.room_idle_timer_paused)
    val seconds = ((deadline - control.nowEpochMs).coerceAtLeast(0L) + 999L) / 1_000L
    if (seconds == 0L) {
        return if (control.creator) {
            appString(R.string.room_closing)
        } else {
            appString(R.string.room_waiting_for_creator)
        }
    }
    val minutesPart = seconds / 60L
    val secondsPart = seconds % 60L
    return appString(
        R.string.room_closes_after_idle,
        "%d:%02d".format(minutesPart, secondsPart),
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
                            appString(R.string.room_nearby_unverified)
                        hasNearbyContext ->
                            appString(R.string.room_nearby_not_visible)
                        else -> appString(R.string.room_ready_unverified)
                    },
                foreground = if (hasNearbyContext && nearbyAvailable != true) colors.warning else colors.muted,
            )
        waiting ->
            RoomStatus(
                label = appString(R.string.room_waiting),
                foreground = colors.warning,
            )
        else ->
            RoomStatus(
                label =
                    appQuantityString(
                        R.plurals.active_transfer_count,
                        active.size,
                        active.size,
                    ),
                foreground = colors.success,
            )
    }
}
