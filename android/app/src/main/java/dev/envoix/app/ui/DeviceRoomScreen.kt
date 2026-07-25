package dev.envoix.app.ui

import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
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
    transferDraft: RoomTransferDraft?,
    transfers: List<Transfer>,
    onBack: () -> Unit,
    onActivity: () -> Unit,
    onSettings: () -> Unit,
    onBeginTransfer: (roleAdapter: String, usesPendingAction: Boolean) -> Unit,
    onShowRoomQr: () -> Unit,
    onDismissTransfer: () -> Unit,
    onTransferStarted: (code: String, consumePendingShares: Boolean) -> Unit,
    onAcceptIncomingOffer: (NearbyRendezvousOffer) -> Boolean,
    onReceive: (String, String, String, String?, Boolean) -> Unit,
    onSend: (String, String, String, String, String?) -> Unit,
    initialSources: List<Uri> = emptyList(),
    discoveryViewModel: DiscoveryViewModel,
) {
    val colors = Envoix.colors
    val discoveryState by discoveryViewModel.uiState.collectAsStateWithLifecycle()
    val setupUsesPending = transferDraft?.usesPendingAction == true
    val pendingRole = draft.pendingRoleAdapter
    val roomTransfers = transfers.filter { it.room in draft.transferCodes }
    val active = roomTransfers.filterNot { it.status.isTerminal }
    val nearbyAvailable =
        draft.nearbySelection?.discoveryPeerKey?.let { selectedKey ->
            discoveryState.peers.any { it.peerKey == selectedKey }
        }
    val state =
        roomState(
            active = active,
            hasNearbyContext = draft.nearbySelection != null,
            nearbyAvailable = nearbyAvailable,
        )
    var confirmLeave by remember { mutableStateOf(false) }

    fun requestBack() {
        if (active.isEmpty()) {
            onBack()
        } else {
            confirmLeave = true
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
            state = state,
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
            pendingRole?.let { role ->
                item {
                    PendingRoomAction(
                        role = role,
                        pendingShareCount = initialSources.size,
                        onContinue = {
                            onBeginTransfer(role, true)
                        },
                    )
                }
            }
            if (roomTransfers.isEmpty() && pendingRole == null) {
                item { EmptyRoomTimeline() }
            } else {
                items(roomTransfers.sortedByDescending(Transfer::id), key = Transfer::id) { transfer ->
                    RoomTransferSummary(transfer)
                }
            }
            item {
                Text(
                    appText(
                        "This is a one-time room. Each transfer connects and authenticates separately.",
                        "这是一个一次性房间。每次传输都会单独连接并完成认证。",
                    ),
                    color = colors.muted,
                    fontSize = 11.sp,
                    lineHeight = 16.sp,
                    modifier = Modifier.padding(horizontal = 4.dp, vertical = 6.dp),
                )
            }
        }
        RoomActions(
            onAddFiles = {
                onBeginTransfer("send", false)
            },
            onShowQr = onShowRoomQr,
            onClose = ::requestBack,
        )
    }

    transferDraft?.let { activeDraft ->
        val role = activeDraft.roleAdapter
        val nearbySelection =
            draft.nearbySelection.takeUnless {
                activeDraft.showQrInitially
            }
        ModalBottomSheet(
            onDismissRequest = onDismissTransfer,
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
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
                onOfferInvite =
                    nearbySelection
                        ?.takeIf { DiscoverySource.Bluetooth in it.sources }
                        ?.let { selection ->
                            { invite, completion ->
                                discoveryViewModel.offerInvite(
                                    selection.discoveryPeerKey,
                                    invite,
                                    completion,
                                )
                            }
                        },
                onReceive = { code, broker, relay, qrPayload, copyApproved ->
                    onReceive(code, broker, relay, qrPayload, copyApproved)
                    onTransferStarted(code, false)
                },
                onSend = { code, broker, relay, jobId, qrPayload ->
                    onSend(code, broker, relay, jobId, qrPayload)
                    onTransferStarted(code, setupUsesPending && initialSources.isNotEmpty())
                },
            )
        }
    }

    if (transferDraft == null) {
        discoveryState.incomingRendezvousOffers.firstOrNull()?.let { offer ->
            AlertDialog(
                onDismissRequest = {
                    discoveryViewModel.consumeRendezvousOffer(offer.requestId)
                },
                title = { Text(appText("Incoming file offer", "收到文件邀请")) },
                text = {
                    Text(
                        appText(
                            "Review this unverified experimental Bluetooth invitation before receiving files.",
                            "接收文件前，请检查此未经验证的实验性蓝牙邀请。",
                        ),
                    )
                },
                confirmButton = {
                    TextButton(
                        onClick = {
                            onAcceptIncomingOffer(offer)
                            discoveryViewModel.consumeRendezvousOffer(offer.requestId)
                        },
                        modifier = Modifier.testTag("nearby_offer_accept"),
                    ) {
                        Text(appText("Accept", "接受"))
                    }
                },
                dismissButton = {
                    TextButton(
                        onClick = { discoveryViewModel.consumeRendezvousOffer(offer.requestId) },
                        modifier = Modifier.testTag("nearby_offer_reject"),
                    ) {
                        Text(appText("Reject", "拒绝"))
                    }
                },
                containerColor = colors.surface,
            )
        }
    }

    if (confirmLeave) {
        AlertDialog(
            onDismissRequest = { confirmLeave = false },
            title = { Text(appText("Close this one-time room?", "关闭这个一次性房间？")) },
            text = {
                Text(
                    appText(
                        "Active transfers will continue in Activity. You can monitor or stop them there.",
                        "进行中的传输会继续，并可在“活动”页面中查看或停止。",
                    ),
                )
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        confirmLeave = false
                        onBack()
                    },
                ) {
                    Text(appText("Close room", "关闭房间"))
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmLeave = false }) {
                    Text(appText("Stay", "留下"))
                }
            },
            containerColor = colors.surface,
        )
    }
}
