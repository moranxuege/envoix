package dev.envoix.app.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.envoix.app.InviteCodec
import dev.envoix.app.SettingsStore
import dev.envoix.app.discovery.DiscoveredPeer
import dev.envoix.app.discovery.DiscoveryPermissions
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.DiscoveryViewModel
import dev.envoix.app.discovery.NearbyPairingSelection
import dev.envoix.app.discovery.ProviderAvailability

@OptIn(ExperimentalComposeUiApi::class, ExperimentalMaterial3Api::class)
@Composable
internal fun ConnectionHubScreen(
    onOpenRoom: (DeviceRoomDraft) -> Unit,
    onActivity: () -> Unit,
    onSettings: () -> Unit,
    pendingShareCount: Int = 0,
    discoveryViewModel: DiscoveryViewModel,
) {
    val colors = Envoix.colors
    val state by discoveryViewModel.uiState.collectAsStateWithLifecycle()
    val settings by SettingsStore.settings.collectAsStateWithLifecycle()
    var action by remember { mutableStateOf<HubAction?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val permissionLauncher =
        rememberLauncherForActivityResult(
            ActivityResultContracts.RequestMultiplePermissions(),
        ) { discoveryViewModel.restart() }
    val nearbyName = appText("Nearby Envoix device", "附近的 Envoix 设备")
    val qrName = appText("Device from QR", "二维码设备")
    val codeName = appText("Device from code", "配对码设备")
    val waitingName = appText("Transfer room", "传输房间")
    val invalidInvite = appText("That is not a valid Envoix invite.", "这不是有效的 Envoix 邀请。")

    fun openInvite(
        input: String,
        displayName: String,
    ) {
        val normalized = input.trim()
        val parsed = InviteCodec.parse(normalized)
        if (parsed == null) {
            error = invalidInvite
            return
        }
        action = null
        error = null
        onOpenRoom(
            DeviceRoomDraft(
                displayName = displayName,
                pairingInput = normalized,
                directionAdapter =
                    InviteCodec.oppositeRole(parsed.role)
                        ?: settings.defaultRole.validDirection(),
            ),
        )
    }
    LaunchedEffect(state.incomingRendezvousOffer?.requestId) {
        val offer = state.incomingRendezvousOffer ?: return@LaunchedEffect
        val parsed = InviteCodec.parse(offer.invite)
        if (parsed != null) {
            onOpenRoom(
                DeviceRoomDraft(
                    displayName = offer.senderDisplayName ?: nearbyName,
                    pairingInput = offer.invite,
                    directionAdapter =
                        InviteCodec.oppositeRole(parsed.role)
                            ?: settings.defaultRole.validDirection(),
                    nearbySelection =
                        NearbyPairingSelection(
                            discoveryPeerKey = offer.senderPeerKey,
                            displayName = offer.senderDisplayName,
                            sources = setOf(DiscoverySource.Bluetooth),
                        ),
                ),
            )
        }
        discoveryViewModel.consumeRendezvousOffer(offer.requestId)
    }
    Column(
        Modifier
            .semantics { testTagsAsResourceId = true }
            .testTag("connection_hub")
            .fillMaxSize()
            .background(colors.bg),
    ) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                "Envoix",
                color = colors.text,
                fontSize = 21.sp,
                fontWeight = FontWeight.ExtraBold,
                modifier = Modifier.weight(1f),
            )
            IconButton(onClick = onActivity, modifier = Modifier.testTag("hub_activity")) {
                Icon(Icons.Default.History, appText("Activity", "活动"), tint = colors.muted)
            }
            IconButton(onClick = onSettings, modifier = Modifier.testTag("hub_settings")) {
                Icon(Icons.Default.Settings, appText("Settings", "设置"), tint = colors.muted)
            }
        }
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(start = 20.dp, end = 20.dp, bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            item {
                Text(
                    appText("Connect to a device", "连接设备"),
                    color = colors.text,
                    fontSize = 30.sp,
                    fontWeight = FontWeight.ExtraBold,
                )
                Spacer(Modifier.height(6.dp))
                Text(
                    appText(
                        "Nearby devices appear automatically. Choose one, then start a transfer.",
                        "附近设备会自动出现。选择一台设备，然后开始传输。",
                    ),
                    color = colors.muted,
                    fontSize = 14.sp,
                    lineHeight = 20.sp,
                )
            }
            if (pendingShareCount > 0) {
                item {
                    Text(
                        appText(
                            "$pendingShareCount items are ready. Choose a device to offer them.",
                            "已有 $pendingShareCount 个项目就绪。请选择设备发送。",
                        ),
                        color = colors.accent,
                        fontSize = 13.sp,
                        fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.fillMaxWidth().background(colors.accentSoft).padding(12.dp),
                    )
                }
            }
            item {
                Row(
                    Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    HubAction.entries.forEach { item ->
                        OutlinedButton(
                            onClick = {
                                error = null
                                action = item
                            },
                            modifier = Modifier.weight(1f).testTag(item.testTag()),
                            contentPadding = PaddingValues(horizontal = 4.dp),
                        ) {
                            Text(item.label(), maxLines = 1, fontSize = 12.sp)
                        }
                    }
                }
            }
            item {
                Text(
                    appText(
                        "NEARBY DEVICES · Visible as ${state.localName}",
                        "附近设备 · 本机显示为 ${state.localName}",
                    ),
                    color = colors.muted,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.Bold,
                )
            }
            if (state.statuses.values.any { it.availability == ProviderAvailability.PermissionRequired }) {
                item {
                    Button(
                        onClick = {
                            permissionLauncher.launch(DiscoveryPermissions.bluetoothRuntimePermissions())
                        },
                    ) {
                        Text(appText("Allow nearby access", "允许附近设备访问"))
                    }
                }
            }
            if (state.peers.isEmpty()) {
                item {
                    Text(
                        if (state.active) {
                            appText("Looking for nearby devices…", "正在寻找附近设备…")
                        } else {
                            appText("Nearby discovery is paused.", "附近设备发现已暂停。")
                        },
                        color = colors.muted,
                        fontSize = 14.sp,
                        modifier = Modifier.fillMaxWidth().padding(vertical = 22.dp),
                    )
                }
            } else {
                items(state.peers, key = DiscoveredPeer::peerKey) { peer ->
                    NearbyDeviceCard(
                        peer = peer,
                        canPairNearby = DiscoverySource.Bluetooth in peer.sources,
                    ) {
                        onOpenRoom(
                            DeviceRoomDraft(
                                displayName = peer.displayName ?: nearbyName,
                                directionAdapter = settings.defaultRole.validDirection(),
                                nearbySelection = NearbyPairingSelection.from(peer),
                            ),
                        )
                    }
                }
            }
            item {
                Text(
                    appText(
                        "Nearby names are not verified. A privately shared QR or code authenticates each transfer.",
                        "附近设备名称未经验证。私下分享的二维码或配对码会验证每次传输。",
                    ),
                    color = colors.muted,
                    fontSize = 12.sp,
                )
            }
        }
    }
    action?.let { selected ->
        ModalBottomSheet(
            onDismissRequest = {
                action = null
                error = null
            },
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            containerColor = colors.surface,
        ) {
            ConnectionActionSheet(
                action = selected,
                error = error,
                broker = settings.broker,
                relay = settings.relay,
                hostedRole = if (pendingShareCount > 0) "send" else "receive",
                onScanned = { openInvite(it, qrName) },
                onCode = { openInvite(it, codeName) },
                onOpenLocalRoom = { code, payload ->
                    action = null
                    error = null
                    onOpenRoom(
                        DeviceRoomDraft(
                            displayName = waitingName,
                            directionAdapter = if (pendingShareCount > 0) "send" else "receive",
                            hostedCode = code,
                            hostedPayload = payload,
                        ),
                    )
                },
            )
        }
    }
}

@Composable
private fun NearbyDeviceCard(
    peer: DiscoveredPeer,
    canPairNearby: Boolean,
    onClick: () -> Unit,
) {
    val colors = Envoix.colors
    val bluetoothLabel = appText("Bluetooth", "蓝牙")
    val localNetworkLabel = appText("Local network", "局域网")
    val wifiAwareLabel = appText("Wi-Fi Aware", "Wi-Fi Aware")
    val sourceText =
        listOfNotNull(
            bluetoothLabel.takeIf { DiscoverySource.Bluetooth in peer.sources },
            localNetworkLabel.takeIf { DiscoverySource.Mdns in peer.sources },
            wifiAwareLabel.takeIf { DiscoverySource.WifiAware in peer.sources },
        ).joinToString(" · ")
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(colors.surface)
            .border(1.dp, colors.line, RoundedCornerShape(16.dp))
            .clickable(enabled = canPairNearby, onClick = onClick)
            .padding(15.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(
                peer.displayName ?: appText("Nearby Envoix device", "附近的 Envoix 设备"),
                color = colors.text,
                fontSize = 15.sp,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(3.dp))
            Text(
                sourceText,
                color = colors.muted,
                fontSize = 11.sp,
            )
        }
        Text(
            if (canPairNearby) {
                appText("Pair", "配对")
            } else {
                appText("Use QR/code", "使用二维码/配对码")
            },
            color = if (canPairNearby) colors.accent else colors.muted,
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
        )
    }
}

@Composable
@OptIn(ExperimentalComposeUiApi::class)
private fun ConnectionActionSheet(
    action: HubAction,
    error: String?,
    broker: String,
    relay: String,
    hostedRole: String,
    onScanned: (String) -> Unit,
    onCode: (String) -> Unit,
    onOpenLocalRoom: (String, String) -> Unit,
) {
    val colors = Envoix.colors
    var typed by remember(action) { mutableStateOf("") }
    val generated = remember(broker, relay, hostedRole) { InviteCodec.generate(hostedRole, broker, relay) }
    Column(
        Modifier
            .semantics { testTagsAsResourceId = true }
            .fillMaxWidth()
            .padding(start = 20.dp, end = 20.dp, bottom = 32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(action.label(), color = colors.text, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Spacer(Modifier.height(14.dp))
        when (action) {
            HubAction.Scan -> InlineScanner(onScanned = onScanned, modifier = Modifier.fillMaxWidth())
            HubAction.Show ->
                if (generated == null) {
                    Text(appText("Could not create an invite.", "无法创建邀请。"), color = colors.danger)
                } else {
                    QrCode(generated.second, side = 190.dp)
                    Spacer(Modifier.height(10.dp))
                    Text(
                        generated.first,
                        color = colors.text,
                        fontWeight = FontWeight.Bold,
                    )
                    Spacer(Modifier.height(12.dp))
                    Button(
                        onClick = { onOpenLocalRoom(generated.first, generated.second) },
                        modifier = Modifier.testTag("hub_open_room"),
                    ) {
                        Text(appText("Open room", "打开房间"))
                    }
                }
            HubAction.Code -> {
                OutlinedTextField(
                    value = typed,
                    onValueChange = { typed = it },
                    singleLine = true,
                    label = { Text(appText("Pairing code", "配对码")) },
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(12.dp))
                Button(onClick = { onCode(typed) }, enabled = typed.isNotBlank()) {
                    Text(appText("Continue", "继续"))
                }
            }
        }
        error?.let {
            Spacer(Modifier.height(10.dp))
            Text(it, color = colors.danger, fontSize = 13.sp)
        }
    }
}

private enum class HubAction { Scan, Show, Code }

private fun HubAction.testTag(): String =
    when (this) {
        HubAction.Scan -> "hub_scan_qr"
        HubAction.Show -> "hub_show_qr"
        HubAction.Code -> "hub_enter_code"
    }

@Composable
private fun HubAction.label() =
    when (this) {
        HubAction.Scan -> appText("Scan QR", "扫描二维码")
        HubAction.Show -> appText("Show QR", "显示二维码")
        HubAction.Code -> appText("Enter code", "输入配对码")
    }

private fun String.validDirection(): String = if (this == "send") "send" else "receive"
