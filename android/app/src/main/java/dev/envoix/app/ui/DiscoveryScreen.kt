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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.envoix.app.discovery.DiscoveredPeer
import dev.envoix.app.discovery.DiscoveryPermissions
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.DiscoveryUiState
import dev.envoix.app.discovery.DiscoveryViewModel
import dev.envoix.app.discovery.NearbyPairingSelection
import dev.envoix.app.discovery.ProviderAvailability
import dev.envoix.app.discovery.ProviderStatus

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun DiscoveryScreen(
    onBack: () -> Unit,
    onReceive: (code: String, broker: String, relay: String, qrPayload: String?, copyApproved: Boolean) -> Unit,
    onSend: (code: String, broker: String, relay: String, jobId: String, qrPayload: String?) -> Unit,
    discoveryViewModel: DiscoveryViewModel = viewModel(),
) {
    val colors = Envoix.colors
    val state by discoveryViewModel.uiState.collectAsStateWithLifecycle()
    val lifecycleOwner = LocalLifecycleOwner.current
    val permissionLauncher =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
            discoveryViewModel.restart()
        }
    var pairingSelection by remember { mutableStateOf<NearbyPairingSelection?>(null) }
    var initialPairingInput by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(state.incomingRendezvousOffer?.requestId) {
        val offer = state.incomingRendezvousOffer ?: return@LaunchedEffect
        if (pairingSelection == null) {
            pairingSelection =
                NearbyPairingSelection(
                    discoveryPeerKey = offer.senderPeerKey,
                    displayName = offer.senderDisplayName,
                    sources = setOf(DiscoverySource.Bluetooth),
                )
            initialPairingInput = offer.invite
            discoveryViewModel.consumeRendezvousOffer(offer.requestId)
        }
    }

    DisposableEffect(lifecycleOwner, discoveryViewModel) {
        val observer =
            LifecycleEventObserver { _, event ->
                when (event) {
                    Lifecycle.Event.ON_START -> discoveryViewModel.start()
                    Lifecycle.Event.ON_STOP -> discoveryViewModel.stop()
                    else -> Unit
                }
            }
        lifecycleOwner.lifecycle.addObserver(observer)
        if (lifecycleOwner.lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) {
            discoveryViewModel.start()
        }
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
            discoveryViewModel.stop()
        }
    }

    Column(Modifier.fillMaxSize().background(colors.bg)) {
        DiscoveryHeader(onBack = onBack, onRefresh = discoveryViewModel::restart)
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            verticalArrangement = Arrangement.spacedBy(12.dp),
            contentPadding =
                PaddingValues(
                    start = 20.dp,
                    end = 20.dp,
                    bottom = 32.dp,
                ),
        ) {
            item {
                Text(
                    "Visible as ${state.localName}",
                    color = colors.text,
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 16.sp,
                )
                Spacer(Modifier.height(4.dp))
                Text(
                    "Experimental BLE pairing is unauthenticated. A nearby attacker may impersonate or relay a device.",
                    color = colors.muted,
                    fontSize = 13.sp,
                    lineHeight = 18.sp,
                )
            }

            item { ProviderPanel(state) }

            val bluetoothStatus = state.statuses[DiscoverySource.Bluetooth]
            if (bluetoothStatus?.availability == ProviderAvailability.PermissionRequired) {
                item {
                    Button(
                        onClick = {
                            permissionLauncher.launch(DiscoveryPermissions.bluetoothRuntimePermissions())
                        },
                    ) {
                        Text("Grant Bluetooth access")
                    }
                }
            }

            item {
                Text(
                    "NEARBY DEVICES",
                    color = colors.muted,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.Bold,
                    letterSpacing = 0.8.sp,
                    modifier = Modifier.padding(top = 6.dp),
                )
            }

            if (state.peers.isEmpty()) {
                item {
                    Text(
                        if (state.active) "Searching for Envoix devices…" else "Discovery is paused.",
                        color = colors.muted,
                        fontSize = 15.sp,
                        modifier = Modifier.padding(vertical = 24.dp),
                    )
                }
            } else {
                items(state.peers, key = { peer -> peer.peerKey }) { peer ->
                    PeerCard(
                        peer = peer,
                        nowMs = state.nowMs,
                        onClick = {
                            pairingSelection = NearbyPairingSelection.from(peer)
                            initialPairingInput = null
                        },
                    )
                }
            }

            item {
                Text(
                    "BLE sends the Envoix invitation only after you tap Start. It does not prove that the selected device is the intended peer.",
                    color = colors.muted,
                    fontSize = 12.sp,
                    lineHeight = 17.sp,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }
        }
    }

    pairingSelection?.let { selection ->
        ModalBottomSheet(
            onDismissRequest = {
                pairingSelection = null
                initialPairingInput = null
            },
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            containerColor = colors.surface,
        ) {
            NewTransferSheet(
                nearbySelection = selection,
                initialPairingInput = initialPairingInput,
                onOfferInvite =
                    if (DiscoverySource.Bluetooth in selection.sources && initialPairingInput == null) {
                        { invite, completion ->
                            discoveryViewModel.offerInvite(selection.discoveryPeerKey, invite, completion)
                        }
                    } else {
                        null
                    },
                onReceive = { code, broker, relay, qrPayload, copyApproved ->
                    pairingSelection = null
                    initialPairingInput = null
                    onReceive(code, broker, relay, qrPayload, copyApproved)
                },
                onSend = { code, broker, relay, jobId, qrPayload ->
                    pairingSelection = null
                    initialPairingInput = null
                    onSend(code, broker, relay, jobId, qrPayload)
                },
            )
        }
    }
}

@Composable
private fun DiscoveryHeader(
    onBack: () -> Unit,
    onRefresh: () -> Unit,
) {
    val colors = Envoix.colors
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(
            Icons.AutoMirrored.Filled.ArrowBack,
            contentDescription = "Back",
            tint = colors.accent,
            modifier = Modifier.clip(CircleShape).clickable(onClick = onBack).padding(6.dp),
        )
        Spacer(Modifier.width(8.dp))
        Text(
            "Nearby devices",
            color = colors.text,
            fontSize = 26.sp,
            fontWeight = FontWeight.ExtraBold,
            modifier = Modifier.weight(1f),
        )
        Icon(
            Icons.Default.Refresh,
            contentDescription = "Restart discovery",
            tint = colors.accent,
            modifier =
                Modifier
                    .clip(CircleShape)
                    .clickable(onClick = onRefresh)
                    .padding(7.dp)
                    .size(22.dp),
        )
    }
}

@Composable
private fun ProviderPanel(state: DiscoveryUiState) {
    val colors = Envoix.colors
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(colors.surface)
            .border(1.dp, colors.line, RoundedCornerShape(16.dp))
            .padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        DiscoverySource.entries.forEach { source ->
            val status = state.statuses.getValue(source)
            ProviderStatusRow(status)
        }
    }
}

@Composable
private fun ProviderStatusRow(status: ProviderStatus) {
    val colors = Envoix.colors
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(status.source.title(), color = colors.text, fontWeight = FontWeight.SemiBold, fontSize = 14.sp)
            Text(
                status.detail,
                color = colors.muted,
                fontSize = 12.sp,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Spacer(Modifier.width(10.dp))
        StatusPill(status.availability)
    }
}

@Composable
private fun StatusPill(availability: ProviderAvailability) {
    val colors = Envoix.colors
    val (foreground, background) =
        when (availability) {
            ProviderAvailability.Ready -> colors.success to colors.successSoft
            ProviderAvailability.Starting -> colors.accent to colors.accentSoft
            ProviderAvailability.Reserved,
            ProviderAvailability.Stopped,
            -> colors.muted to colors.surfaceRaised
            ProviderAvailability.Degraded -> colors.warning to colors.surfaceRaised
            else -> colors.danger to colors.surfaceRaised
        }
    Text(
        availability.label(),
        color = foreground,
        fontWeight = FontWeight.Bold,
        fontSize = 11.sp,
        modifier = Modifier.clip(CircleShape).background(background).padding(horizontal = 9.dp, vertical = 5.dp),
    )
}

@Composable
private fun PeerCard(
    peer: DiscoveredPeer,
    nowMs: Long,
    onClick: () -> Unit,
) {
    val colors = Envoix.colors
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(colors.surface)
            .border(1.dp, colors.line, RoundedCornerShape(16.dp))
            .clickable(onClick = onClick)
            .padding(15.dp),
    ) {
        Text(
            peer.displayName ?: "Nearby Envoix device",
            color = colors.text,
            fontWeight = FontWeight.Bold,
            fontSize = 16.sp,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Spacer(Modifier.height(7.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(7.dp), verticalAlignment = Alignment.CenterVertically) {
            peer.sources.sortedBy(DiscoverySource::ordinal).forEach { source -> SourcePill(source) }
            peer.rssi?.let { rssi -> Text("$rssi dBm", color = colors.muted, fontSize = 12.sp) }
        }
        Spacer(Modifier.height(7.dp))
        val ageSeconds = ((nowMs - peer.lastSeenAtMs).coerceAtLeast(0) / 1_000)
        Text(
            if (ageSeconds == 0L) "Seen just now" else "Seen ${ageSeconds}s ago",
            color = colors.muted,
            fontSize = 12.sp,
        )
    }
}

@Composable
private fun SourcePill(source: DiscoverySource) {
    val colors = Envoix.colors
    Text(
        source.shortTitle(),
        color = colors.accent,
        fontWeight = FontWeight.Bold,
        fontSize = 11.sp,
        modifier = Modifier.clip(CircleShape).background(colors.accentSoft).padding(horizontal = 8.dp, vertical = 4.dp),
    )
}

private fun DiscoverySource.title(): String =
    when (this) {
        DiscoverySource.Bluetooth -> "Bluetooth LE"
        DiscoverySource.Mdns -> "mDNS / local network"
        DiscoverySource.WifiAware -> "Wi-Fi Aware"
    }

private fun DiscoverySource.shortTitle(): String =
    when (this) {
        DiscoverySource.Bluetooth -> "BLE"
        DiscoverySource.Mdns -> "mDNS"
        DiscoverySource.WifiAware -> "Aware"
    }

private fun ProviderAvailability.label(): String =
    when (this) {
        ProviderAvailability.PermissionRequired -> "Permission"
        ProviderAvailability.TemporarilyUnavailable -> "Unavailable"
        else -> name
    }
