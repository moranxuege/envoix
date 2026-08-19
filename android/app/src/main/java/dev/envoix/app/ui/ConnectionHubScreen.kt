package dev.envoix.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.NfcPhoneHostingState
import dev.envoix.app.NfcPhoneReaderState
import dev.envoix.app.R
import dev.envoix.app.discovery.BleVerificationInvitation
import dev.envoix.app.discovery.DiscoveredPeer
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.DiscoveryUiState
import dev.envoix.app.discovery.NearbyPairingSelection
import dev.envoix.app.discovery.NearbyRendezvousOffer
import dev.envoix.app.discovery.NearbyVisibility
import dev.envoix.app.discovery.ProviderAvailability
import dev.envoix.app.discovery.canOfferNearbyRoom

@OptIn(ExperimentalComposeUiApi::class)
@Composable
internal fun ConnectionHubScreen(
    control: RoomControlUiState,
    onShareViaNfc: () -> Unit,
    onStopNfcSharing: () -> Unit,
    onScanNfc: () -> Unit,
    onRevealInvite: () -> Unit,
    onHideInvite: () -> Unit,
    onRefreshInvite: () -> Unit,
    onEndWaitingRoom: () -> Unit,
    onJoinInvite: (String) -> Unit,
    nfcPhoneHosting: NfcPhoneHostingState,
    nfcPhoneReader: NfcPhoneReaderState,
    onNearbyRoom: (
        selection: NearbyPairingSelection,
        deliver: (String, (String?) -> Unit) -> Unit,
    ) -> Unit,
    onReturnToRoom: () -> Unit,
    onActivity: () -> Unit,
    onRooms: () -> Unit,
    onSettings: () -> Unit,
    onAcceptIncomingOffer: (NearbyRendezvousOffer, String?) -> Boolean,
    onCancelReplacement: () -> Unit,
    onConfirmReplacement: () -> Unit,
    onExternalActivityChanged: (Boolean) -> Unit,
    pendingShareCount: Int = 0,
    discovery: DiscoveryUiState,
    nearbyDisplayName: String,
    nearbyVisibility: NearbyVisibility,
    onToggleDiscovery: () -> Unit,
    onRequestNearbyPermission: () -> Unit,
    onOfferNearbyInvite: (NearbyPairingSelection, String, (String?) -> Unit) -> Unit,
    onConsumeNearbyOffer: (String) -> Unit,
    onSaveNearbyDisplayName: (String) -> Boolean,
    onSetNearbyVisibility: (NearbyVisibility) -> Unit,
) {
    val colors = Envoix.colors
    var scannerOpen by remember { mutableStateOf(false) }
    var codeDialogOpen by remember { mutableStateOf(false) }
    var identityDialogOpen by remember { mutableStateOf(false) }
    var visibilityDialogOpen by remember { mutableStateOf(false) }
    var nfcDialogOpen by remember { mutableStateOf(false) }
    var wifiAwareDialogOpen by remember { mutableStateOf(false) }
    var nearbyListExpanded by rememberSaveable { mutableStateOf(true) }
    var localError by remember { mutableStateOf<String?>(null) }
    val invalidDisplayName = appString(R.string.hub_invalid_nearby_name)
    val unsupportedInvitation = appString(R.string.hub_unsupported_invitation)

    Column(
        Modifier
            .semantics { testTagsAsResourceId = true }
            .testTag("connection_hub")
            .fillMaxSize()
            .background(colors.bg),
    ) {
        ConnectionHubAppBar(
            onActivity = onActivity,
            onRooms = onRooms,
            onSettings = onSettings,
        )
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(start = 18.dp, end = 18.dp, bottom = 28.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            if (pendingShareCount > 0) {
                item {
                    Text(
                        appQuantityString(
                            R.plurals.hub_pending_share_count,
                            pendingShareCount,
                            pendingShareCount,
                        ),
                        color = colors.accentStrong,
                        fontSize = 13.sp,
                        fontWeight = FontWeight.SemiBold,
                        modifier =
                            Modifier
                                .fillMaxWidth()
                                .background(colors.accentSoft)
                                .padding(12.dp),
                    )
                }
            }
            item {
                MainRoomInviteCard(
                    control = control,
                    onScan = {
                        localError = null
                        scannerOpen = true
                    },
                    onEnterCode = {
                        localError = null
                        codeDialogOpen = true
                    },
                    onReveal = onRevealInvite,
                    onHide = onHideInvite,
                    onRefresh = onRefreshInvite,
                    onEndWaiting = onEndWaitingRoom,
                    onReturnToRoom = onReturnToRoom,
                )
            }
            item {
                NearbyIdentityRow(
                    displayName = nearbyDisplayName,
                    visibility = nearbyVisibility,
                    onEditName = { identityDialogOpen = true },
                    onVisibility = { visibilityDialogOpen = true },
                )
            }
            item {
                NearbySectionHeader(
                    listExpanded = nearbyListExpanded,
                    wifiAwareStatus = discovery.statuses[DiscoverySource.WifiAware],
                    nfcPhoneHosting = nfcPhoneHosting,
                    nfcPhoneReader = nfcPhoneReader,
                    discoveryActive = discovery.active,
                    onWifiAware = { wifiAwareDialogOpen = true },
                    onNfc = { nfcDialogOpen = true },
                    onToggleList = { nearbyListExpanded = !nearbyListExpanded },
                    onToggleDiscovery = onToggleDiscovery,
                )
            }
            if (nearbyListExpanded) {
                if (discovery.statuses.values.any {
                        it.availability == ProviderAvailability.PermissionRequired
                    }
                ) {
                    item {
                        Button(
                            onClick = onRequestNearbyPermission,
                        ) {
                            Text(appString(R.string.hub_allow_nearby_access))
                        }
                    }
                }
                if (discovery.peers.isEmpty()) {
                    item {
                        val message =
                            when (
                                nearbyEmptyState(
                                    active = discovery.active,
                                    availabilities =
                                        discovery.statuses.values.map { it.availability },
                                )
                            ) {
                                NearbyEmptyState.Paused ->
                                    appString(R.string.hub_discovery_paused)
                                NearbyEmptyState.Unavailable ->
                                    appString(R.string.hub_discovery_unavailable)
                                NearbyEmptyState.Looking ->
                                    appString(R.string.hub_discovery_looking)
                            }
                        Text(
                            message,
                            color = colors.muted,
                            fontSize = 14.sp,
                            modifier =
                                Modifier
                                    .fillMaxWidth()
                                    .padding(top = 4.dp)
                                    .testTag("hub_nearby_empty"),
                        )
                    }
                } else {
                    items(discovery.peers, key = DiscoveredPeer::peerKey) { peer ->
                        val selection = NearbyPairingSelection.from(peer)
                        NearbyDeviceCard(
                            peer = peer,
                            peers = discovery.peers,
                            enabled = canOfferNearbyRoom(selection),
                        ) {
                            onNearbyRoom(selection) { invite, completion ->
                                onOfferNearbyInvite(
                                    selection,
                                    invite,
                                    completion,
                                )
                            }
                        }
                    }
                }
            }
            localError?.let { message ->
                item { Text(message, color = colors.danger, fontSize = 13.sp) }
            }
        }
    }

    if (scannerOpen) {
        FullScreenScanner(
            onScanned = {
                scannerOpen = false
                onJoinInvite(it)
            },
            onClose = { scannerOpen = false },
            onExternalActivityChanged = onExternalActivityChanged,
        )
    }
    if (codeDialogOpen) {
        EnterRoomCodeDialog(
            error = localError,
            onDismiss = { codeDialogOpen = false },
            onContinue = {
                codeDialogOpen = false
                onJoinInvite(it)
            },
        )
    }
    if (identityDialogOpen) {
        EditNearbyNameDialog(
            currentName = nearbyDisplayName,
            onDismiss = { identityDialogOpen = false },
            onSave = { value ->
                if (onSaveNearbyDisplayName(value)) {
                    identityDialogOpen = false
                } else {
                    localError = invalidDisplayName
                }
            },
        )
    }
    if (visibilityDialogOpen) {
        NearbyVisibilityDialog(
            selected = nearbyVisibility,
            onDismiss = { visibilityDialogOpen = false },
            onSelect = {
                onSetNearbyVisibility(it)
                visibilityDialogOpen = false
            },
        )
    }
    if (nfcDialogOpen) {
        NfcNearbyActionsDialog(
            roomPhase = control.phase,
            hosting = nfcPhoneHosting,
            reader = nfcPhoneReader,
            onDismiss = { nfcDialogOpen = false },
            onScan = onScanNfc,
            onShare = onShareViaNfc,
            onStopSharing = onStopNfcSharing,
        )
    }
    if (wifiAwareDialogOpen) {
        WifiAwareDiscoveryDialog(
            status = discovery.statuses[DiscoverySource.WifiAware],
            onDismiss = { wifiAwareDialogOpen = false },
        )
    }
    discovery.incomingRendezvousOffers.firstOrNull()?.let { offer ->
        val verificationOffer = BleVerificationInvitation.isPublicOffer(offer.invite)
        IncomingNearbyInvitationDialog(
            offerId = offer.requestId,
            roomInvitation = RoomControlInviteFormat.looksLikeRoomInvite(offer.invite),
            verificationOffer = verificationOffer,
            peerName =
                offer.senderDisplayName
                    ?: appString(R.string.nearby_envoix_device),
            onAccept = { code ->
                if (!onAcceptIncomingOffer(offer, code)) {
                    localError = unsupportedInvitation
                }
                onConsumeNearbyOffer(offer.requestId)
            },
            onReject = {
                onConsumeNearbyOffer(offer.requestId)
            },
        )
    }
    if (control.replacementRequested) {
        val canReturnToRoom =
            control.connected ||
                control.phase == RoomControlPhase.Legacy
        AlertDialog(
            onDismissRequest =
                if (canReturnToRoom) {
                    onReturnToRoom
                } else {
                    onCancelReplacement
                },
            title = { Text(appString(R.string.hub_room_replacement_title)) },
            text = {
                Text(appString(R.string.hub_room_replacement_explanation))
            },
            confirmButton = {
                TextButton(onClick = onConfirmReplacement) {
                    Text(appString(R.string.hub_end_and_replace))
                }
            },
            dismissButton = {
                TextButton(
                    onClick =
                        if (canReturnToRoom) {
                            onReturnToRoom
                        } else {
                            onCancelReplacement
                        },
                ) {
                    Text(
                        if (canReturnToRoom) {
                            appString(R.string.hub_return_to_room)
                        } else {
                            appString(R.string.hub_keep_current_room)
                        },
                    )
                }
            },
            containerColor = colors.surface,
        )
    }
}

internal enum class NearbyEmptyState {
    Paused,
    Unavailable,
    Looking,
}

internal fun nearbyEmptyState(
    active: Boolean,
    availabilities: Collection<ProviderAvailability>,
): NearbyEmptyState {
    if (!active) return NearbyEmptyState.Paused
    val unavailableStates =
        setOf(
            ProviderAvailability.PermissionRequired,
            ProviderAvailability.Disabled,
            ProviderAvailability.Unsupported,
            ProviderAvailability.TemporarilyUnavailable,
            ProviderAvailability.Reserved,
            ProviderAvailability.Error,
        )
    return if (
        availabilities.isNotEmpty() &&
        availabilities.all { it in unavailableStates }
    ) {
        NearbyEmptyState.Unavailable
    } else {
        NearbyEmptyState.Looking
    }
}
