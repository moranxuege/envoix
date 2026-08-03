package dev.envoix.app.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
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
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.envoix.app.NfcPhoneHostingState
import dev.envoix.app.NfcPhoneReaderState
import dev.envoix.app.SettingsStore
import dev.envoix.app.discovery.DiscoveredPeer
import dev.envoix.app.discovery.DiscoveryPermissions
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.DiscoveryViewModel
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
    onAcceptIncomingOffer: (NearbyRendezvousOffer) -> Boolean,
    onCancelReplacement: () -> Unit,
    onConfirmReplacement: () -> Unit,
    onExternalActivityChanged: (Boolean) -> Unit,
    pendingShareCount: Int = 0,
    discoveryViewModel: DiscoveryViewModel,
) {
    val colors = Envoix.colors
    val discovery by discoveryViewModel.uiState.collectAsStateWithLifecycle()
    val settings by SettingsStore.settings.collectAsStateWithLifecycle()
    var scannerOpen by remember { mutableStateOf(false) }
    var codeDialogOpen by remember { mutableStateOf(false) }
    var identityDialogOpen by remember { mutableStateOf(false) }
    var visibilityDialogOpen by remember { mutableStateOf(false) }
    var nfcDialogOpen by remember { mutableStateOf(false) }
    var wifiAwareDialogOpen by remember { mutableStateOf(false) }
    var nearbyListExpanded by rememberSaveable { mutableStateOf(true) }
    var localError by remember { mutableStateOf<String?>(null) }
    val permissionLauncher =
        rememberLauncherForActivityResult(
            ActivityResultContracts.RequestMultiplePermissions(),
        ) { discoveryViewModel.restart() }

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
                        appText(
                            "$pendingShareCount items are ready. Connect to a device to offer them.",
                            "已有 $pendingShareCount 个项目就绪。连接设备后即可发送。",
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
                    displayName = settings.nearbyDisplayName,
                    visibility =
                        NearbyVisibility.fromPersisted(
                            settings.nearbyVisibility,
                        ),
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
                    onWifiAware = { wifiAwareDialogOpen = true },
                    onNfc = { nfcDialogOpen = true },
                    onToggleList = { nearbyListExpanded = !nearbyListExpanded },
                )
            }
            if (nearbyListExpanded) {
                if (discovery.statuses.values.any {
                        it.availability == ProviderAvailability.PermissionRequired
                    }
                ) {
                    Text(
                        appText("NEARBY DEVICES", "附近设备"),
                        color = colors.muted,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.Bold,
                        letterSpacing = 0.8.sp,
                        modifier = Modifier.weight(1f),
                    )
                    TextButton(
                        onClick = {
                            if (discovery.active) discoveryViewModel.stop()
                            else discoveryViewModel.start()
                        },
                        modifier = Modifier.testTag("hub_restart_nearby"),
                    ) {
                        Text(
                            if (discovery.active) appText("Stop", "停止") else appText("Start", "开始搜索"),
                            color = colors.accent,
                        )
                    }
                }
            }
            if (discovery.statuses.values.any {
                    it.availability == ProviderAvailability.PermissionRequired
                }
            ) {
                item {
                    Button(
                        onClick = {
                            permissionLauncher.launch(
                                DiscoveryPermissions.bluetoothRuntimePermissions(),
                            )
                        },
                    ) {
                        Text(appText("Allow nearby access", "允许附近设备访问"))
                    }
                }
            }
                        ) {
                            Text(appText("Allow nearby access", "允许附近设备访问"))
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
                                    appText(
                                        "Nearby discovery is paused.",
                                        "附近发现已暂停。",
                                    )
                                NearbyEmptyState.Unavailable ->
                                    appText(
                                        "Nearby discovery is currently unavailable.",
                                        "附近发现当前不可用。",
                                    )
                                NearbyEmptyState.Looking ->
                                    appText(
                                        "Looking for nearby devices…",
                                        "正在寻找附近设备…",
                                    )
                            }
                        Text(
                            message,
                            color = colors.muted,
                            fontSize = 14.sp,
                            modifier =
                                Modifier
                                    .fillMaxWidth()
                                    .padding(vertical = 20.dp)
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
                                discoveryViewModel.offerInvite(
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
            currentName = settings.nearbyDisplayName,
            onDismiss = { identityDialogOpen = false },
            onSave = { value ->
                if (SettingsStore.setNearbyDisplayName(value)) {
                    identityDialogOpen = false
                } else {
                    localError =
                        AppText.value(
                            "Enter a name between 1 and 48 characters.",
                            "请输入 1 到 48 个字符的名称。",
                            settings.language,
                        )
                }
            },
        )
    }
    if (visibilityDialogOpen) {
        NearbyVisibilityDialog(
            selected = NearbyVisibility.fromPersisted(settings.nearbyVisibility),
            onDismiss = { visibilityDialogOpen = false },
            onSelect = {
                SettingsStore.setNearbyVisibility(it.persistedValue)
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
        IncomingNearbyInvitationDialog(
            roomInvitation = RoomControlInviteFormat.looksLikeRoomInvite(offer.invite),
            peerName =
                offer.senderDisplayName
                    ?: appText("Nearby Envoix device", "附近的 Envoix 设备"),
            onAccept = {
                if (!onAcceptIncomingOffer(offer)) {
                    localError =
                        AppText.value(
                            "This invitation is not supported.",
                            "暂不支持这个邀请。",
                            settings.language,
                        )
                }
                discoveryViewModel.consumeRendezvousOffer(offer.requestId)
            },
            onReject = {
                discoveryViewModel.consumeRendezvousOffer(offer.requestId)
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
            title = { Text(appText("Another room is active", "已有一个房间")) },
            text = {
                Text(
                    appText(
                        "Envoix can keep one room at a time. End the current room before starting another.",
                        "Envoix 同时只能保留一个房间。开始新房间前需要结束当前房间。",
                    ),
                )
            },
            confirmButton = {
                TextButton(onClick = onConfirmReplacement) {
                    Text(appText("End and replace", "结束并替换"))
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
                            appText("Return to room", "返回房间")
                        } else {
                            appText("Keep current", "保留当前房间")
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
