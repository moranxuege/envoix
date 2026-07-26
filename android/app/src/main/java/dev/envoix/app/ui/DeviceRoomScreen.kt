package dev.envoix.app.ui

import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.SheetValue
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.envoix.app.Transfer
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.DiscoveryViewModel
import dev.envoix.app.discovery.NearbyRendezvousOffer
import dev.envoix.app.isTerminal

@OptIn(ExperimentalComposeUiApi::class, ExperimentalMaterial3Api::class)
@Composable
internal fun DeviceRoomScreen(
    draft: DeviceRoomDraft,
    control: RoomControlUiState,
    transferDraft: RoomTransferDraft?,
    transfers: List<Transfer>,
    onBack: () -> Unit,
    onActivity: () -> Unit,
    onSettings: () -> Unit,
    onBeginTransfer: (roleAdapter: String, usesPendingAction: Boolean) -> Unit,
    onShowRoomQr: () -> Unit,
    onDismissTransfer: () -> Unit,
    onTransferStarted: (code: String, consumePendingShares: Boolean) -> Unit,
    onOfferRoomTransfer: (RoomTransferOfferDraft, (String?) -> Unit) -> Unit,
    incomingOfferBusy: Boolean,
    incomingOfferError: String?,
    onAcceptIncomingRoomOffer: () -> Unit,
    onRejectIncomingRoomOffer: () -> Unit,
    onKeepOpen: (Boolean) -> Unit,
    onEndRoom: () -> Unit,
    onDismissEndedRoom: () -> Unit,
    onRoomActiveTransfers: (Int) -> Unit,
    onExternalActivityChanged: (Boolean) -> Unit,
    onAcceptIncomingOffer: (NearbyRendezvousOffer) -> Boolean,
    onReceive: (String, String, String, String?, Boolean, String?, String?) -> Unit,
    onSend: (String, String, String, String, String?, String?, String?) -> Unit,
    onOpenReceived: (Transfer) -> Unit,
    onShareReceived: (Transfer) -> Unit,
    initialSources: List<Uri> = emptyList(),
    discoveryViewModel: DiscoveryViewModel,
) {
    val colors = Envoix.colors
    val discoveryState by discoveryViewModel.uiState.collectAsStateWithLifecycle()
    val setupUsesPending = transferDraft?.usesPendingAction == true
    val pendingRole = draft.pendingRoleAdapter
    val visiblePendingRole =
        pendingRole?.takeIf { role ->
            control.phase == RoomControlPhase.Legacy ||
                (role == "send" && initialSources.isNotEmpty())
        }
    val roomTransfers = transfers.filter { it.room in draft.transferCodes }
    val active = roomTransfers.filterNot { it.status.isTerminal }
    val connectedRoom = control.connected && draft.controlSession
    val legacyRoom = control.phase == RoomControlPhase.Legacy
    val nearbyAvailable =
        draft.nearbySelection?.discoveryPeerKey?.let { selectedKey ->
            discoveryState.peers.any { it.peerKey == selectedKey }
        }
    var confirmEnd by remember { mutableStateOf(false) }

    LaunchedEffect(active.size) {
        onRoomActiveTransfers(active.size)
    }

    fun requestBack() {
        when {
            control.phase == RoomControlPhase.Closed ||
                control.phase == RoomControlPhase.Failed ->
                onDismissEndedRoom()
            legacyRoom && active.isEmpty() -> onBack()
            else -> confirmEnd = true
        }
    }

    BackHandler(onBack = ::requestBack)

    Column(
        Modifier
            .semantics { testTagsAsResourceId = true }
            .testTag("device_room")
            .fillMaxSize()
            .background(colors.bg),
    ) {
        RoomHeader(
            displayName = draft.displayName,
            control = control,
            legacyState =
                roomState(
                    active = active,
                    hasNearbyContext = draft.nearbySelection != null,
                    nearbyAvailable = nearbyAvailable,
                ),
            onBack = ::requestBack,
            onActivity = onActivity,
            onSettings = onSettings,
        )
        HorizontalDivider(color = colors.line)
        LazyColumn(
            modifier = Modifier.weight(1f),
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item {
                androidx.compose.foundation.layout.Row(
                    modifier = Modifier.fillParentMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
                ) {
                    Text(
                        appText("ROOM ACTIVITY", "房间活动"),
                        color = colors.muted,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.Bold,
                        letterSpacing = 0.8.sp,
                    )
                    TextButton(onClick = onActivity) {
                        Text(appText("All Activity", "全部活动"))
                    }
                }
            }
            control.incomingOffer?.let { offer ->
                item {
                    IncomingRoomOfferCard(
                        offer = offer,
                        busy = incomingOfferBusy,
                        error = incomingOfferError,
                        onAccept = onAcceptIncomingRoomOffer,
                        onReject = onRejectIncomingRoomOffer,
                    )
                }
            }
            visiblePendingRole?.let { role ->
                item {
                    PendingRoomAction(
                        role = role,
                        pendingShareCount = initialSources.size,
                        onContinue = { onBeginTransfer(role, true) },
                    )
                }
            }
            if (roomTransfers.isEmpty() && pendingRole == null && control.incomingOffer == null) {
                item { EmptyRoomTimeline() }
            } else {
                items(roomTransfers.sortedByDescending(Transfer::id), key = Transfer::id) { transfer ->
                    RoomTransferSummary(
                        transfer = transfer,
                        onOpen = onOpenReceived,
                        onShare = onShareReceived,
                    )
                }
            }
        }
        RoomControlPanel(
            control = control,
            legacy = legacyRoom,
            onAddFiles = { onBeginTransfer("send", false) },
            onShowQr = onShowRoomQr,
            onKeepOpen = onKeepOpen,
            onEnd = {
                if (legacyRoom && active.isEmpty()) {
                    onBack()
                } else {
                    confirmEnd = true
                }
            },
            onDone = onDismissEndedRoom,
        )
    }

    transferDraft?.let { activeDraft ->
        val role = activeDraft.roleAdapter
        val dismissalBlocked =
            activeDraft.preparation.rendezvousBusy.value ||
                (connectedRoom && control.outgoingOfferPending)
        val nearbySelection =
            draft.nearbySelection
                .takeUnless { activeDraft.showQrInitially || connectedRoom }
        ModalBottomSheet(
            onDismissRequest = {
                if (!dismissalBlocked) onDismissTransfer()
            },
            sheetState =
                rememberModalBottomSheetState(
                    skipPartiallyExpanded = true,
                    confirmValueChange = { target ->
                        target != SheetValue.Hidden || !dismissalBlocked
                    },
                ),
            containerColor = colors.surface,
        ) {
            NewTransferSheet(
                draftId = activeDraft.id,
                preparationState = activeDraft.preparation,
                showQrInitially = activeDraft.showQrInitially,
                initialRole = role,
                initialPairingInput = draft.pairingInput.takeIf { setupUsesPending },
                initialSources =
                    if (setupUsesPending && role == "send") {
                        initialSources
                    } else {
                        emptyList()
                    },
                nearbySelection = nearbySelection,
                nearbyDeliveryAvailable = nearbyAvailable != false,
                initialHostedCode = draft.hostedCode.takeIf { setupUsesPending },
                initialHostedPayload = draft.hostedPayload.takeIf { setupUsesPending },
                roomMode = true,
                connectedRoom = connectedRoom,
                onExternalActivityChanged = onExternalActivityChanged,
                onBeforeStart = null,
                onPrepareReceiveBeforeDecision = null,
                onOfferInvite =
                    when {
                        connectedRoom && role == "send" -> onOfferRoomTransfer
                        else ->
                            nearbySelection
                                ?.takeIf { DiscoverySource.Bluetooth in it.sources }
                                ?.let { selection ->
                                    { offer, completion ->
                                        discoveryViewModel.offerInvite(
                                            selection.discoveryPeerKey,
                                            offer.transferInvite,
                                            completion,
                                        )
                                    }
                                }
                    },
                onReceive = {
                    code,
                    broker,
                    relay,
                    qrPayload,
                    copyApproved,
                    rememberLabel,
                    rememberedRelationshipId,
                    ->
                    onReceive(
                        code,
                        broker,
                        relay,
                        qrPayload,
                        copyApproved,
                        rememberLabel,
                        rememberedRelationshipId,
                    )
                    onTransferStarted(code, false)
                },
                onSend = {
                    code,
                    broker,
                    relay,
                    jobId,
                    qrPayload,
                    rememberLabel,
                    rememberedRelationshipId,
                    ->
                    onSend(
                        code,
                        broker,
                        relay,
                        jobId,
                        qrPayload,
                        rememberLabel,
                        rememberedRelationshipId,
                    )
                    onTransferStarted(code, setupUsesPending && initialSources.isNotEmpty())
                },
            )
        }
    }

    if (legacyRoom && transferDraft == null) {
        discoveryState.incomingRendezvousOffers.firstOrNull()?.let { offer ->
            IncomingNearbyInvitationDialog(
                roomInvitation = RoomControlInviteFormat.looksLikeRoomInvite(offer.invite),
                peerName =
                    offer.senderDisplayName
                        ?: appText("Nearby Envoix device", "附近的 Envoix 设备"),
                onAccept = {
                    onAcceptIncomingOffer(offer)
                    discoveryViewModel.consumeRendezvousOffer(offer.requestId)
                },
                onReject = {
                    discoveryViewModel.consumeRendezvousOffer(offer.requestId)
                },
            )
        }
    }

    if (confirmEnd) {
        AlertDialog(
            onDismissRequest = { confirmEnd = false },
            title = { Text(appText("End this room?", "结束这个房间？")) },
            text = {
                Text(
                    appText(
                        "New file offers will stop. Transfers already in progress will continue in Activity.",
                        "结束后将无法发送新文件。已经开始的传输会继续显示在“活动”中。",
                    ),
                )
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        confirmEnd = false
                        if (legacyRoom) onBack() else onEndRoom()
                    },
                ) {
                    Text(appText("End room", "结束房间"), color = colors.danger)
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmEnd = false }) {
                    Text(appText("Keep room", "保留房间"))
                }
            },
            containerColor = colors.surface,
        )
    }
}
