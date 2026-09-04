package dev.envoix.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Devices
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.Keyboard
import androidx.compose.material.icons.filled.Nfc
import androidx.compose.material.icons.filled.QrCodeScanner
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Smartphone
import androidx.compose.material.icons.filled.Visibility
import androidx.compose.material.icons.filled.VisibilityOff
import androidx.compose.material.icons.filled.WifiTethering
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.InviteCodec
import dev.envoix.app.NfcPhoneHostingState
import dev.envoix.app.NfcPhoneHostingStatus
import dev.envoix.app.NfcPhoneReaderState
import dev.envoix.app.NfcPhoneReaderStatus
import dev.envoix.app.R
import dev.envoix.app.discovery.DiscoveredPeer
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.NearbyVisibility
import dev.envoix.app.discovery.ProviderAvailability
import dev.envoix.app.discovery.ProviderStatus

@Composable
internal fun ConnectionHubAppBar(
    onActivity: () -> Unit,
    onRooms: () -> Unit,
    onSettings: () -> Unit,
) {
    val colors = Envoix.colors
    Box(
        Modifier
            .fillMaxWidth()
            .height(62.dp)
            .padding(horizontal = 12.dp),
    ) {
        Text(
            appString(R.string.app_name),
            color = colors.text,
            fontSize = 21.sp,
            fontWeight = FontWeight.ExtraBold,
            modifier = Modifier.align(Alignment.Center),
        )
        Row(
            Modifier.align(Alignment.CenterStart),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            HubUtilityButton(
                icon = Icons.Default.History,
                description = appString(R.string.activity_title),
                testTag = "hub_activity",
                onClick = onActivity,
            )
            HubUtilityButton(
                icon = Icons.Default.Smartphone,
                description = appString(R.string.hub_rooms),
                testTag = "hub_rooms",
                onClick = onRooms,
            )
        }
        Box(Modifier.align(Alignment.CenterEnd)) {
            HubUtilityButton(
                icon = Icons.Default.Settings,
                description = appString(R.string.settings),
                testTag = "hub_settings",
                onClick = onSettings,
            )
        }
    }
}

@Composable
private fun HubUtilityButton(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    description: String,
    testTag: String,
    onClick: () -> Unit,
) {
    val colors = Envoix.colors
    IconButton(
        onClick = onClick,
        modifier =
            Modifier
                .testTag(testTag)
                .size(40.dp)
                .clip(CircleShape)
                .background(colors.surface),
    ) {
        Icon(icon, description, tint = colors.muted, modifier = Modifier.size(20.dp))
    }
}

private val RoomInviteMaximumSide = 240.dp
private val RoomInviteViewportHeight = 240.dp
private val RoomInviteHeaderHeight = 60.dp

internal data class MainRoomInviteQrLayout(
    val side: Dp,
    val viewportHeight: Dp,
    val showsActions: Boolean,
)

internal fun resolveMainRoomInviteQrLayout(
    maxWidth: Dp,
    revealed: Boolean,
): MainRoomInviteQrLayout =
    MainRoomInviteQrLayout(
        side = minOf(maxWidth, RoomInviteMaximumSide),
        viewportHeight = RoomInviteViewportHeight,
        showsActions = !revealed,
    )

@Composable
internal fun MainRoomInviteCard(
    control: RoomControlUiState,
    onScan: () -> Unit,
    onEnterCode: () -> Unit,
    onReveal: () -> Unit,
    onHide: () -> Unit,
    onRefresh: () -> Unit,
    onEndWaiting: () -> Unit,
    onReturnToRoom: () -> Unit,
) {
    val colors = Envoix.colors
    val clipboard = LocalClipboardManager.current
    if (control.connected) {
        Row(
            Modifier
                .testTag("hub_current_room")
                .fillMaxWidth()
                .clip(RoundedCornerShape(20.dp))
                .background(colors.accentSoft)
                .clickable(onClick = onReturnToRoom)
                .padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Default.Smartphone, null, tint = colors.accent, modifier = Modifier.size(26.dp))
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    appString(R.string.hub_current_room_section),
                    color = colors.accentStrong,
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Bold,
                    letterSpacing = 0.7.sp,
                )
                Text(
                    control.peerName ?: appString(R.string.hub_connected_device),
                    color = colors.text,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.Bold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Icon(
                Icons.AutoMirrored.Filled.KeyboardArrowRight,
                appString(R.string.hub_return_to_room),
                tint = colors.accent,
            )
        }
        return
    }

    val revealed =
        control.phase == RoomControlPhase.Hosting &&
            control.inviteRevealed &&
            control.verificationCode == null &&
            control.invite != null
    val verifying =
        control.phase == RoomControlPhase.Hosting && control.verificationCode != null
    val joining = control.phase == RoomControlPhase.Joining
    val creating =
        control.phase == RoomControlPhase.Hosting &&
            control.inviteRevealed &&
            control.invite == null
    val roomStatus =
        when {
            verifying -> appString(R.string.hub_verify_nearby_device)
            revealed -> requireNotNull(control.invite).code
            creating -> appString(R.string.hub_creating_room)
            joining -> appString(R.string.hub_joining_room)
            control.invite != null -> appString(R.string.hub_room_ready_waiting)
            else -> appString(R.string.hub_no_active_room)
        }
    Column(
        Modifier
            .testTag("hub_room_invite")
            .fillMaxWidth()
            .clip(RoundedCornerShape(22.dp))
            .background(colors.surface)
            .border(1.dp, colors.line, RoundedCornerShape(22.dp))
            .padding(horizontal = 18.dp, vertical = 18.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Row(
            Modifier.fillMaxWidth().height(RoomInviteHeaderHeight),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(
                Modifier.weight(1f),
                verticalArrangement = Arrangement.Center,
            ) {
                Text(
                    appString(R.string.room_header_title),
                    color = colors.muted,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.Bold,
                    letterSpacing = 0.7.sp,
                )
                Text(
                    roomStatus,
                    color = if (control.invite == null) colors.muted else colors.accentStrong,
                    fontSize = 12.sp,
                    fontWeight = if (revealed) FontWeight.Bold else FontWeight.Normal,
                    fontFamily = if (revealed) FontFamily.Monospace else FontFamily.Default,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            if (revealed) {
                val roomCode = requireNotNull(control.invite).code
                IconButton(
                    onClick = {
                        clipboard.setText(AnnotatedString(roomCode))
                    },
                    modifier = Modifier.testTag("hub_copy_room_code"),
                ) {
                    Icon(
                        Icons.Default.ContentCopy,
                        appString(R.string.hub_copy_room_code),
                        tint = colors.muted,
                        modifier = Modifier.size(18.dp),
                    )
                }
            }
            if (control.phase == RoomControlPhase.Hosting && control.invite != null && !verifying) {
                IconButton(
                    onClick = onRefresh,
                    modifier = Modifier.testTag("hub_refresh_room_code"),
                ) {
                    Icon(
                        Icons.Default.Refresh,
                        appString(R.string.hub_renew_room_invitation),
                        tint = colors.accent,
                        modifier = Modifier.size(19.dp),
                    )
                }
            }
            if (control.phase == RoomControlPhase.Hosting || joining) {
                IconButton(
                    onClick = onEndWaiting,
                    modifier = Modifier.testTag("hub_end_waiting_room"),
                ) {
                    Icon(
                        Icons.Default.Close,
                        if (joining) {
                            appString(R.string.hub_cancel_joining_room)
                        } else {
                            appString(R.string.hub_close_room)
                        },
                        tint = colors.danger,
                        modifier = Modifier.size(19.dp),
                    )
                }
            }
        }
        Spacer(Modifier.height(8.dp))
        if (verifying) {
            Box(
                Modifier.fillMaxWidth().height(RoomInviteViewportHeight),
                contentAlignment = Alignment.Center,
            ) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Text(
                        requireNotNull(control.verificationCode).let {
                            "${it.take(3)} ${it.takeLast(3)}"
                        },
                        color = colors.accentStrong,
                        fontSize = 38.sp,
                        fontWeight = FontWeight.Bold,
                        fontFamily = FontFamily.Monospace,
                        modifier = Modifier.testTag("ble_verification_code"),
                    )
                    Spacer(Modifier.height(14.dp))
                    Text(
                        appString(R.string.hub_ble_verification_instruction),
                        color = colors.muted,
                        textAlign = TextAlign.Center,
                    )
                }
            }
        } else {
            BoxWithConstraints(Modifier.fillMaxWidth()) {
                val layout = resolveMainRoomInviteQrLayout(maxWidth, revealed)
                Box(
                    Modifier.fillMaxWidth().height(layout.viewportHeight),
                    contentAlignment = Alignment.Center,
                ) {
                    if (layout.showsActions) {
                        MainRoomInviteActions(
                            control = control,
                            joining = joining,
                            creating = creating,
                            onReveal = onReveal,
                            onScan = onScan,
                            onEnterCode = onEnterCode,
                            modifier = Modifier.size(layout.side),
                        )
                    } else {
                        MainRoomQrToggle(control, layout.side, onHide)
                    }
                }
            }
        }
        control.error?.let {
            Spacer(Modifier.height(8.dp))
            Text(it.resolve(), color = colors.danger, fontSize = 12.sp, textAlign = TextAlign.Center)
        }
    }
}

@Composable
private fun MainRoomQrToggle(
    control: RoomControlUiState,
    side: Dp,
    onHide: () -> Unit,
) {
    Box(
        Modifier
            .size(side)
            .testTag("hub_room_qr_toggle")
            .clickable(
                onClickLabel = appString(R.string.hub_hide_room_qr),
                role = Role.Button,
                onClick = onHide,
            ),
        contentAlignment = Alignment.Center,
    ) {
        QrCode(requireNotNull(control.invite).payload, side = side)
    }
}

@Composable
private fun MainRoomInviteActions(
    control: RoomControlUiState,
    joining: Boolean,
    creating: Boolean,
    onReveal: () -> Unit,
    onScan: () -> Unit,
    onEnterCode: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier,
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        OutlinedButton(
            onClick = onReveal,
            enabled = !joining && !creating,
            modifier = Modifier.fillMaxWidth().weight(1f).testTag("hub_room_qr_toggle"),
            shape = RoundedCornerShape(16.dp),
            contentPadding = PaddingValues(horizontal = 8.dp, vertical = 10.dp),
        ) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                if (joining || creating) {
                    CircularProgressIndicator(
                        color = Envoix.colors.accent,
                        strokeWidth = 2.dp,
                        modifier = Modifier.size(20.dp),
                    )
                } else {
                    Icon(
                        if (control.invite == null) Icons.Default.Add else Icons.Default.Visibility,
                        null,
                        modifier = Modifier.size(22.dp),
                    )
                }
                Spacer(Modifier.height(7.dp))
                Text(
                    when {
                        creating -> appString(R.string.hub_creating_room)
                        joining -> appString(R.string.hub_joining_room)
                        control.invite == null -> appString(R.string.hub_create_room)
                        else -> appString(R.string.hub_reveal_qr)
                    },
                    textAlign = TextAlign.Center,
                )
            }
        }
        Row(
            Modifier.fillMaxWidth().weight(1f),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            OutlinedButton(
                onClick = onScan,
                modifier = Modifier.weight(1f).fillMaxHeight().testTag("hub_scan_qr"),
                shape = RoundedCornerShape(16.dp),
                contentPadding = PaddingValues(horizontal = 8.dp, vertical = 10.dp),
            ) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Icon(Icons.Default.QrCodeScanner, null, modifier = Modifier.size(22.dp))
                    Spacer(Modifier.height(7.dp))
                    Text(
                        appString(R.string.scan_qr),
                        textAlign = TextAlign.Center,
                    )
                }
            }
            OutlinedButton(
                onClick = onEnterCode,
                modifier = Modifier.weight(1f).fillMaxHeight().testTag("hub_enter_code"),
                shape = RoundedCornerShape(16.dp),
                contentPadding = PaddingValues(horizontal = 8.dp, vertical = 10.dp),
            ) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Icon(Icons.Default.Keyboard, null, modifier = Modifier.size(22.dp))
                    Spacer(Modifier.height(7.dp))
                    Text(
                        appString(R.string.hub_enter_code),
                        textAlign = TextAlign.Center,
                    )
                }
            }
        }
    }
}

@Composable
internal fun NearbyIdentityRow(
    displayName: String,
    visibility: NearbyVisibility,
    onEditName: () -> Unit,
    onVisibility: () -> Unit,
) {
    val colors = Envoix.colors
    val visibilityColor =
        if (visibility == NearbyVisibility.Hidden) {
            colors.muted
        } else {
            colors.accentStrong
        }
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(colors.surface)
            .border(1.dp, colors.line, RoundedCornerShape(16.dp))
            .padding(horizontal = 18.dp, vertical = 14.dp),
    ) {
        Text(
            appString(R.string.hub_set_identity_section),
            color = colors.muted,
            fontSize = 14.sp,
            fontWeight = FontWeight.Bold,
            letterSpacing = 0.7.sp,
        )
        Spacer(Modifier.height(6.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                displayName,
                color = colors.text,
                fontSize = 15.sp,
                fontWeight = FontWeight.Bold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f).clickable(onClick = onEditName),
            )
            Icon(
                Icons.Default.Edit,
                appString(R.string.hub_edit_nearby_name),
                tint = colors.muted,
                modifier =
                    Modifier
                        .clip(CircleShape)
                        .clickable(onClick = onEditName)
                        .padding(7.dp)
                        .size(17.dp),
            )
            Spacer(Modifier.width(6.dp))
            Row(
                modifier =
                    Modifier
                        .clip(RoundedCornerShape(14.dp))
                        .background(
                            if (visibility == NearbyVisibility.Hidden) {
                                colors.bg
                            } else {
                                colors.accentSoft
                            },
                        ).clickable(onClick = onVisibility)
                        .padding(horizontal = 10.dp, vertical = 7.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(
                    if (visibility == NearbyVisibility.Hidden) {
                        Icons.Default.VisibilityOff
                    } else {
                        Icons.Default.Visibility
                    },
                    contentDescription = null,
                    tint = visibilityColor,
                    modifier = Modifier.size(14.dp),
                )
                Spacer(Modifier.width(4.dp))
                Text(
                    visibility.label(),
                    color = visibilityColor,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.SemiBold,
                )
            }
        }
    }
}

@Composable
private fun NearbyVisibility.label(): String =
    when (this) {
        NearbyVisibility.Hidden -> appString(R.string.hub_visibility_hidden)
        NearbyVisibility.EveryoneTenMinutes -> appString(R.string.hub_visibility_everyone_ten_minutes)
        NearbyVisibility.Foreground -> appString(R.string.hub_visibility_foreground)
    }

internal enum class WifiAwareDiscoveryUiState {
    Active,
    Starting,
    Unavailable,
}

internal fun wifiAwareDiscoveryUiState(status: ProviderStatus?): WifiAwareDiscoveryUiState =
    when (status?.availability) {
        ProviderAvailability.Ready -> WifiAwareDiscoveryUiState.Active
        ProviderAvailability.Starting -> WifiAwareDiscoveryUiState.Starting
        else -> WifiAwareDiscoveryUiState.Unavailable
    }

internal fun shouldShowWifiAwareDiscoveryAction(status: ProviderStatus?): Boolean =
    when (status?.availability) {
        ProviderAvailability.Ready,
        ProviderAvailability.Starting,
        -> true
        else -> false
    }

internal fun canShareRoomViaNfc(phase: RoomControlPhase): Boolean =
    phase == RoomControlPhase.None ||
        phase == RoomControlPhase.Hosting ||
        phase == RoomControlPhase.Closed ||
        phase == RoomControlPhase.Failed

@Composable
internal fun NearbySectionHeader(
    listExpanded: Boolean,
    wifiAwareStatus: ProviderStatus?,
    nfcPhoneHosting: NfcPhoneHostingState,
    nfcPhoneReader: NfcPhoneReaderState,
    discoveryActive: Boolean,
    onWifiAware: () -> Unit,
    onNfc: () -> Unit,
    onToggleList: () -> Unit,
    onToggleDiscovery: () -> Unit,
) {
    val colors = Envoix.colors
    val wifiAwareActive =
        wifiAwareDiscoveryUiState(wifiAwareStatus) == WifiAwareDiscoveryUiState.Active
    val nfcActive = nfcPhoneHosting.armed || nfcPhoneReader.scanning
    Row(
        Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            appString(R.string.hub_nearby_devices_section),
            color = colors.muted,
            fontSize = 14.sp,
            fontWeight = FontWeight.Bold,
            letterSpacing = 0.8.sp,
            modifier = Modifier.weight(1f),
        )
        TextButton(
            onClick = onToggleDiscovery,
            modifier =
                Modifier
                    .heightIn(min = 40.dp)
                    .testTag("hub_restart_nearby"),
            contentPadding = PaddingValues(horizontal = 6.dp, vertical = 4.dp),
        ) {
            Text(
                if (discoveryActive) {
                    appString(R.string.hub_stop_discovery)
                } else {
                    appString(R.string.hub_start_discovery)
                },
                color = colors.accent,
                fontSize = 12.sp,
            )
        }
        if (shouldShowWifiAwareDiscoveryAction(wifiAwareStatus)) {
            TextButton(
                onClick = onWifiAware,
                modifier =
                    Modifier
                        .heightIn(min = 40.dp)
                        .testTag("hub_wifi_aware"),
                contentPadding = PaddingValues(horizontal = 6.dp, vertical = 4.dp),
            ) {
                Icon(
                    Icons.Default.WifiTethering,
                    appString(R.string.hub_wifi_aware),
                    tint = if (wifiAwareActive) colors.accent else colors.muted,
                    modifier = Modifier.size(16.dp),
                )
                Spacer(Modifier.width(3.dp))
                Text(
                    appString(R.string.hub_wifi_aware_short),
                    color = if (wifiAwareActive) colors.accent else colors.muted,
                    fontSize = 12.sp,
                )
            }
        }
        TextButton(
            onClick = onNfc,
            modifier =
                Modifier
                    .heightIn(min = 40.dp)
                    .testTag("hub_nfc"),
            contentPadding = PaddingValues(horizontal = 6.dp, vertical = 4.dp),
        ) {
            Icon(
                Icons.Default.Nfc,
                appString(R.string.hub_nfc_nearby_room),
                tint = if (nfcActive) colors.accent else colors.muted,
                modifier = Modifier.size(16.dp),
            )
            Spacer(Modifier.width(3.dp))
            Text(
                appString(R.string.hub_nfc_short),
                color = if (nfcActive) colors.accent else colors.muted,
                fontSize = 12.sp,
            )
        }
        IconButton(
            onClick = onToggleList,
            modifier =
                Modifier
                    .size(40.dp)
                    .testTag("hub_toggle_nearby_list"),
        ) {
            Icon(
                if (listExpanded) Icons.Default.ExpandLess else Icons.Default.ExpandMore,
                if (listExpanded) {
                    appString(R.string.hub_hide_nearby_devices)
                } else {
                    appString(R.string.hub_show_nearby_devices)
                },
                tint = colors.muted,
                modifier = Modifier.size(19.dp),
            )
        }
    }
}

@Composable
internal fun NfcNearbyActionsDialog(
    roomPhase: RoomControlPhase,
    hosting: NfcPhoneHostingState,
    reader: NfcPhoneReaderState,
    onDismiss: () -> Unit,
    onScan: () -> Unit,
    onShare: () -> Unit,
    onStopSharing: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(appString(R.string.hub_nfc_nearby_room)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(
                    appString(R.string.hub_nfc_explanation),
                    color = Envoix.colors.muted,
                    fontSize = 13.sp,
                )
                Text(
                    nfcHostingStatusLabel(hosting.status),
                    color = if (hosting.armed) Envoix.colors.accentStrong else Envoix.colors.muted,
                    fontSize = 12.sp,
                )
                Text(
                    nfcReaderStatusLabel(reader),
                    color = if (reader.scanning) Envoix.colors.accentStrong else Envoix.colors.muted,
                    fontSize = 12.sp,
                )
                OutlinedButton(
                    onClick = onScan,
                    modifier =
                        Modifier
                            .fillMaxWidth()
                            .testTag("hub_scan_nfc"),
                ) {
                    Icon(Icons.Default.Nfc, null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(7.dp))
                    Text(
                        if (reader.scanning) {
                            appString(R.string.hub_stop_nfc_scan)
                        } else {
                            appString(R.string.hub_scan_another_phone)
                        },
                    )
                }
                OutlinedButton(
                    onClick = if (hosting.armed) onStopSharing else onShare,
                    enabled = hosting.armed || canShareRoomViaNfc(roomPhase),
                    modifier =
                        Modifier
                            .fillMaxWidth()
                            .testTag(
                                if (hosting.armed) {
                                    "hub_stop_nfc_share"
                                } else {
                                    "hub_share_room_via_nfc"
                                },
                            ),
                ) {
                    Icon(Icons.Default.Nfc, null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(7.dp))
                    Text(
                        if (hosting.armed) {
                            appString(R.string.hub_stop_nfc_sharing)
                        } else {
                            appString(R.string.hub_create_or_share_room)
                        },
                    )
                }
                if (!hosting.armed && !canShareRoomViaNfc(roomPhase)) {
                    Text(
                        appString(R.string.hub_end_room_before_nfc),
                        color = Envoix.colors.muted,
                        fontSize = 12.sp,
                    )
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text(appString(R.string.common_done))
            }
        },
        containerColor = Envoix.colors.surface,
    )
}

@Composable
private fun nfcHostingStatusLabel(status: NfcPhoneHostingStatus): String =
    when (status) {
        NfcPhoneHostingStatus.Idle -> appString(R.string.hub_nfc_sharing_off)
        NfcPhoneHostingStatus.Armed ->
            appString(R.string.hub_nfc_sharing_ready)
        NfcPhoneHostingStatus.RequiresAndroid15 ->
            appString(R.string.hub_nfc_requires_android_15)
        NfcPhoneHostingStatus.NfcUnavailable ->
            appString(R.string.hub_nfc_unavailable)
        NfcPhoneHostingStatus.NfcDisabled ->
            appString(R.string.hub_nfc_enable_to_share)
        NfcPhoneHostingStatus.HceUnavailable ->
            appString(R.string.hub_nfc_phone_sharing_unavailable)
        NfcPhoneHostingStatus.ListenOnlyUnavailable ->
            appString(R.string.hub_nfc_safe_sharing_unavailable)
        NfcPhoneHostingStatus.HceActivationFailed ->
            appString(R.string.hub_nfc_sharing_start_failed)
        NfcPhoneHostingStatus.InvalidInvitation ->
            appString(R.string.hub_room_invitation_not_ready)
    }

@Composable
private fun nfcReaderStatusLabel(state: NfcPhoneReaderState): String =
    when (state.status) {
        NfcPhoneReaderStatus.Idle -> appString(R.string.hub_nfc_scanning_off)
        NfcPhoneReaderStatus.Scanning ->
            if (state.automatic) {
                appString(R.string.hub_nfc_nearby_phone_detected)
            } else {
                appString(R.string.hub_nfc_scanning_ready)
            }
        NfcPhoneReaderStatus.NfcUnavailable ->
            appString(R.string.hub_nfc_scanning_unavailable)
        NfcPhoneReaderStatus.NfcDisabled ->
            appString(R.string.hub_nfc_enable_to_scan)
        NfcPhoneReaderStatus.ReaderUnavailable ->
            appString(R.string.hub_nfc_scanning_start_failed)
    }

@Composable
internal fun WifiAwareDiscoveryDialog(
    status: ProviderStatus?,
    onDismiss: () -> Unit,
) {
    val state = wifiAwareDiscoveryUiState(status)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(appString(R.string.hub_wifi_aware)) },
        text = {
            Text(
                when (state) {
                    WifiAwareDiscoveryUiState.Active ->
                        appString(R.string.hub_wifi_aware_active)
                    WifiAwareDiscoveryUiState.Starting ->
                        appString(R.string.hub_wifi_aware_starting)
                    WifiAwareDiscoveryUiState.Unavailable ->
                        appString(R.string.hub_wifi_aware_unavailable)
                },
                color = Envoix.colors.muted,
                fontSize = 13.sp,
            )
        },
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text(appString(R.string.common_done))
            }
        },
        containerColor = Envoix.colors.surface,
    )
}

@Composable
internal fun NearbyDeviceCard(
    peer: DiscoveredPeer,
    peers: List<DiscoveredPeer>,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    val colors = Envoix.colors
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(colors.surface)
            .border(1.dp, colors.line, RoundedCornerShape(16.dp))
            .clickable(enabled = enabled, onClick = onClick)
            .padding(15.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier.size(38.dp).clip(CircleShape).background(colors.accentSoft),
            contentAlignment = Alignment.Center,
        ) {
            Icon(Icons.Default.Devices, null, tint = colors.accent, modifier = Modifier.size(20.dp))
        }
        Spacer(Modifier.width(11.dp))
        Column(Modifier.weight(1f)) {
            Text(
                nearbyPeerDisplayName(
                    peer,
                    peers,
                    appString(R.string.nearby_envoix_device),
                ),
                color = colors.text,
                fontSize = 15.sp,
                fontWeight = FontWeight.Bold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                "${
                    nearbyDiscoverySourceLabel(
                        sources = peer.sources,
                        bluetooth = appString(R.string.hub_discovery_source_ble),
                        localNetwork = appString(R.string.hub_discovery_source_local_network),
                        wifiAware = appString(R.string.hub_wifi_aware),
                        fallback = appString(R.string.hub_discovery_source_nearby),
                    )
                } · ${
                    if (enabled) {
                        if (DiscoverySource.Bluetooth in peer.sources && peer.nearbyInviteRoute == null) {
                            appString(R.string.hub_tap_to_verify)
                        } else {
                            appString(R.string.hub_unverified)
                        }
                    } else {
                        appString(R.string.hub_discovery_only)
                    }
                }",
                color = colors.muted,
                fontSize = 12.sp,
                fontWeight = FontWeight.SemiBold,
            )
        }
        if (enabled) {
            Icon(
                Icons.AutoMirrored.Filled.KeyboardArrowRight,
                appString(R.string.hub_open_room),
                tint = colors.muted,
            )
        }
    }
}

internal fun nearbyPeerDisplayName(
    peer: DiscoveredPeer,
    peers: List<DiscoveredPeer>,
    fallback: String,
): String {
    fun baseName(candidate: DiscoveredPeer): String = candidate.displayName?.trim()?.takeIf(String::isNotEmpty) ?: fallback

    val name = baseName(peer)
    val duplicateCount = peers.count { baseName(it).equals(name, ignoreCase = true) }
    if (duplicateCount <= 1) return name
    return "$name · ${peer.peerKey.takeLast(4).uppercase()}"
}

internal fun nearbyDiscoverySourceLabel(
    sources: Set<DiscoverySource>,
    bluetooth: String,
    localNetwork: String,
    wifiAware: String,
    fallback: String,
): String {
    val labels =
        listOf(
            DiscoverySource.Bluetooth to bluetooth,
            DiscoverySource.Mdns to localNetwork,
            DiscoverySource.WifiAware to wifiAware,
        ).mapNotNull { (source, label) -> label.takeIf { source in sources } }
    return labels.joinToString(" · ").ifEmpty { fallback }
}

@Composable
internal fun EnterRoomCodeDialog(
    error: String?,
    onDismiss: () -> Unit,
    onContinue: (String) -> Unit,
) {
    val clipboard = LocalClipboardManager.current
    val emptyClipboardMessage = appString(R.string.hub_clipboard_empty)
    var typed by remember { mutableStateOf("") }
    var inlineError by remember(error) { mutableStateOf(error) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(appString(R.string.hub_room_code_or_link)) },
        text = {
            Column {
                OutlinedTextField(
                    value = typed,
                    onValueChange = {
                        typed = InviteCodec.formatRoomCode(it)
                        inlineError = null
                    },
                    singleLine = true,
                    label = { Text(appString(R.string.hub_room_code_or_link)) },
                    placeholder = { Text(appString(R.string.hub_room_code_placeholder)) },
                    modifier = Modifier.fillMaxWidth(),
                )
                TextButton(
                    onClick = {
                        val pasted =
                            clipboard
                                .getText()
                                ?.text
                                ?.trim()
                                .orEmpty()
                        if (pasted.isEmpty()) {
                            inlineError = emptyClipboardMessage
                        } else {
                            typed = pasted
                            inlineError = null
                        }
                    },
                    modifier =
                        Modifier
                            .align(Alignment.End)
                            .testTag("room_code_paste"),
                ) {
                    Text(appString(R.string.common_paste))
                }
                inlineError?.let {
                    Spacer(Modifier.height(8.dp))
                    Text(it, color = Envoix.colors.danger, fontSize = 12.sp)
                }
            }
        },
        confirmButton = {
            TextButton(
                onClick = { onContinue(typed.trim()) },
                enabled = typed.isNotBlank(),
            ) {
                Text(appString(R.string.common_continue))
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text(appString(R.string.common_cancel))
            }
        },
        containerColor = Envoix.colors.surface,
    )
}

@Composable
internal fun EditNearbyNameDialog(
    currentName: String,
    onDismiss: () -> Unit,
    onSave: (String) -> Unit,
) {
    var typed by remember(currentName) { mutableStateOf(currentName) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(appString(R.string.hub_nearby_name_dialog_title), fontSize = 16.sp) },
        text = {
            OutlinedTextField(
                value = typed,
                onValueChange = { if (it.length <= 48) typed = it },
                singleLine = true,
                label = { Text(appString(R.string.hub_visible_as)) },
                supportingText = {
                    Text(appString(R.string.hub_identity_temporary))
                },
                modifier = Modifier.fillMaxWidth(),
            )
        },
        confirmButton = {
            TextButton(onClick = { onSave(typed) }, enabled = typed.isNotBlank()) {
                Text(appString(R.string.common_save))
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text(appString(R.string.common_cancel))
            }
        },
        containerColor = Envoix.colors.surface,
    )
}

@Composable
internal fun NearbyVisibilityDialog(
    selected: NearbyVisibility,
    onDismiss: () -> Unit,
    onSelect: (NearbyVisibility) -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(appString(R.string.hub_visibility_dialog_title)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                NearbyVisibility.entries.forEach { visibility ->
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(12.dp))
                            .background(
                                if (visibility == selected) {
                                    Envoix.colors.accentSoft
                                } else {
                                    Color.Transparent
                                },
                            ).clickable { onSelect(visibility) }
                            .padding(12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Column(Modifier.weight(1f)) {
                            Text(
                                visibility.label(),
                                color = Envoix.colors.text,
                                fontWeight = FontWeight.Bold,
                            )
                            Text(
                                visibility.description(),
                                color = Envoix.colors.muted,
                                fontSize = 12.sp,
                            )
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text(appString(R.string.common_done))
            }
        },
        containerColor = Envoix.colors.surface,
    )
}

@Composable
private fun NearbyVisibility.description(): String =
    when (this) {
        NearbyVisibility.Hidden ->
            appString(R.string.hub_visibility_hidden_description)
        NearbyVisibility.EveryoneTenMinutes ->
            appString(R.string.hub_visibility_everyone_description)
        NearbyVisibility.Foreground ->
            appString(R.string.hub_visibility_foreground_description)
    }

@Composable
internal fun IncomingNearbyInvitationDialog(
    offerId: String,
    roomInvitation: Boolean,
    verificationOffer: Boolean = false,
    peerName: String,
    onAccept: (String?) -> Unit,
    onReject: () -> Unit,
) {
    var code by remember(offerId) { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onReject,
        title = {
            Text(
                if (verificationOffer) {
                    appString(R.string.hub_verify_nearby_device)
                } else if (roomInvitation) {
                    appString(R.string.hub_room_invitation)
                } else {
                    appString(R.string.hub_file_invitation)
                },
            )
        },
        text = {
            if (verificationOffer) {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text(
                        appString(R.string.hub_ask_verification_code, peerName),
                    )
                    OutlinedTextField(
                        value = code,
                        onValueChange = { value ->
                            code = value.filter { it in '0'..'9' }.take(6)
                        },
                        label = { Text(appString(R.string.hub_verification_code)) },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                        textStyle =
                            androidx.compose.ui.text.TextStyle(
                                fontFamily = FontFamily.Monospace,
                                fontSize = 24.sp,
                                textAlign = TextAlign.Center,
                            ),
                        modifier = Modifier.fillMaxWidth().testTag("ble_verification_code_input"),
                    )
                }
            } else {
                Text(
                    if (roomInvitation) {
                        appString(R.string.hub_peer_wants_room, peerName)
                    } else {
                        appString(R.string.hub_peer_wants_transfer, peerName)
                    },
                )
            }
        },
        confirmButton = {
            TextButton(
                onClick = { onAccept(code.takeIf { verificationOffer }) },
                enabled = !verificationOffer || code.length == 6,
            ) {
                Text(
                    if (verificationOffer) {
                        appString(R.string.hub_verify)
                    } else {
                        appString(R.string.common_accept)
                    },
                )
            }
        },
        dismissButton = {
            TextButton(onClick = onReject) {
                Text(appString(R.string.common_reject))
            }
        },
        containerColor = Envoix.colors.surface,
    )
}
