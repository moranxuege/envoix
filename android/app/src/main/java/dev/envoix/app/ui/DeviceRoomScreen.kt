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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
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
import dev.envoix.app.InviteCodec
import dev.envoix.app.Room
import dev.envoix.app.Transfer
import dev.envoix.app.discovery.DiscoveryViewModel
import dev.envoix.app.isTerminal

@OptIn(ExperimentalComposeUiApi::class, ExperimentalMaterial3Api::class)
@Composable
internal fun DeviceRoomScreen(
    draft: DeviceRoomDraft,
    transfers: List<Transfer>,
    onBack: () -> Unit,
    onReceive: (String, String, String, String?, Boolean) -> Unit,
    onSend: (String, String, String, String, String?) -> Unit,
    onPauseResume: (Long) -> Unit,
    onApproveReceive: (Long) -> Unit,
    onCancel: (Long) -> Unit,
    onRemove: (Long) -> Unit,
    onOpen: (Transfer) -> Unit,
    onShare: (Transfer) -> Unit,
    initialSources: List<Uri> = emptyList(),
    onInitialSourcesConsumed: () -> Unit = {},
    discoveryViewModel: DiscoveryViewModel,
) {
    val colors = Envoix.colors
    val expanded = remember { mutableStateListOf<Long>() }
    val discoveryState by discoveryViewModel.uiState.collectAsStateWithLifecycle()
    var pairingInput by remember(draft) { mutableStateOf(draft.pairingInput) }
    var hostedCode by remember(draft) { mutableStateOf(draft.hostedCode) }
    var hostedPayload by remember(draft) { mutableStateOf(draft.hostedPayload) }
    val roomIds =
        remember(draft) {
            mutableStateListOf<String>().apply {
                (draft.hostedCode ?: draft.pairingInput?.let(InviteCodec::parse)?.code)?.let { code ->
                    add(Room(code).id)
                }
            }
        }
    var setupRole by remember(draft) { mutableStateOf<String?>(null) }
    var setupUsesPending by remember(draft) { mutableStateOf(false) }
    var pendingRole by
        remember(draft) {
            mutableStateOf(
                when {
                    initialSources.isNotEmpty() -> "send"
                    draft.pairingInput != null || draft.hostedPayload != null ->
                        draft.directionAdapter.validRoomDirection()
                    else -> null
                },
            )
        }

    LaunchedEffect(initialSources) {
        if (initialSources.isNotEmpty()) pendingRole = "send"
    }
    val roomTransfers = transfers.filter { Room(it.room).id in roomIds }
    val active = roomTransfers.filterNot { it.status.isTerminal }
    val state = roomState(active)
    var confirmLeave by remember { mutableStateOf(false) }

    fun requestBack() {
        if (active.isEmpty()) {
            onBack()
        } else {
            confirmLeave = true
        }
    }

    BackHandler(onBack = ::requestBack)

    LaunchedEffect(
        discoveryState.incomingRendezvousOffer?.requestId,
        setupRole,
        pendingRole,
        draft.nearbySelection,
    ) {
        val offer = discoveryState.incomingRendezvousOffer ?: return@LaunchedEffect
        val selectedPeer = draft.nearbySelection ?: return@LaunchedEffect
        if (offer.senderPeerKey != selectedPeer.discoveryPeerKey ||
            setupRole != null ||
            pendingRole != null
        ) {
            return@LaunchedEffect
        }
        val parsed = InviteCodec.parse(offer.invite)
        discoveryViewModel.consumeRendezvousOffer(offer.requestId)
        if (parsed != null) {
            pairingInput = offer.invite
            hostedCode = null
            hostedPayload = null
            pendingRole = InviteCodec.oppositeRole(parsed.role) ?: "receive"
        }
    }

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
        )
        HorizontalDivider(color = colors.line)
        LazyColumn(
            modifier = Modifier.weight(1f),
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item {
                Text(
                    appText("TRANSFERS", "传输"),
                    color = colors.muted,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.Bold,
                    letterSpacing = 0.8.sp,
                )
            }
            pendingRole?.let { role ->
                item {
                    PendingRoomAction(
                        role = role,
                        pendingShareCount = initialSources.size,
                        onContinue = {
                            setupUsesPending = true
                            setupRole = role
                        },
                    )
                }
            }
            if (roomTransfers.isEmpty() && pendingRole == null) {
                item { EmptyRoomTimeline() }
            } else {
                items(roomTransfers.sortedByDescending(Transfer::id), key = Transfer::id) { transfer ->
                    TransferCard(
                        t = transfer,
                        expanded = transfer.id in expanded,
                        onToggleDetail = { id ->
                            if (id in expanded) expanded.remove(id) else expanded.add(id)
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
            item {
                Text(
                    appText(
                        "Each transfer connects independently.",
                        "每次传输都会单独建立连接。",
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
                setupUsesPending = false
                setupRole = "send"
            },
        )
    }

    setupRole?.let { role ->
        ModalBottomSheet(
            onDismissRequest = { setupRole = null },
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            containerColor = colors.surface,
        ) {
            NewTransferSheet(
                initialRole = role,
                initialPairingInput = pairingInput.takeIf { setupUsesPending },
                initialSources =
                    if (setupUsesPending && role == "send") {
                        initialSources
                    } else {
                        emptyList()
                    },
                nearbySelection = draft.nearbySelection,
                initialHostedCode = hostedCode.takeIf { setupUsesPending },
                initialHostedPayload = hostedPayload.takeIf { setupUsesPending },
                roomMode = true,
                onOfferInvite =
                    draft.nearbySelection?.let { selection ->
                        { invite, completion ->
                            discoveryViewModel.offerInvite(
                                selection.discoveryPeerKey,
                                invite,
                                completion,
                            )
                        }
                    },
                onReceive = { code, broker, relay, qrPayload, copyApproved ->
                    val consumedPending = setupUsesPending
                    setupRole = null
                    if (consumedPending) {
                        pendingRole = null
                        pairingInput = null
                        hostedCode = null
                        hostedPayload = null
                    }
                    Room(code).id.takeUnless(roomIds::contains)?.let(roomIds::add)
                    onReceive(code, broker, relay, qrPayload, copyApproved)
                },
                onSend = { code, broker, relay, jobId, qrPayload ->
                    val consumedPending = setupUsesPending
                    setupRole = null
                    if (consumedPending) {
                        pendingRole = null
                        pairingInput = null
                        hostedCode = null
                        hostedPayload = null
                    }
                    Room(code).id.takeUnless(roomIds::contains)?.let(roomIds::add)
                    if (consumedPending) onInitialSourcesConsumed()
                    onSend(code, broker, relay, jobId, qrPayload)
                },
            )
        }
    }

    if (confirmLeave) {
        AlertDialog(
            onDismissRequest = { confirmLeave = false },
            title = { Text(appText("Leave this room?", "离开这个房间？")) },
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
                    Text(appText("Leave room", "离开房间"))
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

private fun String.validRoomDirection(): String = if (this == "receive") "receive" else "send"
