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
            "Envoix",
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
                description = appText("Activity", "活动"),
                testTag = "hub_activity",
                onClick = onActivity,
            )
            HubUtilityButton(
                icon = Icons.Default.Smartphone,
                description = appText("Rooms", "房间"),
                testTag = "hub_rooms",
                onClick = onRooms,
            )
        }
        Box(Modifier.align(Alignment.CenterEnd)) {
            HubUtilityButton(
                icon = Icons.Default.Settings,
                description = appText("Settings", "设置"),
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
                    appText("CURRENT ROOM", "当前房间"),
                    color = colors.accentStrong,
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Bold,
                    letterSpacing = 0.7.sp,
                )
                Text(
                    control.peerName ?: appText("Connected device", "已连接设备"),
                    color = colors.text,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.Bold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Icon(
                Icons.AutoMirrored.Filled.KeyboardArrowRight,
                appText("Return to room", "返回房间"),
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
            verifying -> appText("Verify nearby device", "验证附近设备")
            revealed -> requireNotNull(control.invite).code
            creating -> appText("Creating room…", "正在创建房间…")
            joining -> appText("Joining room…", "正在加入房间…")
            control.invite != null -> appText("Ready · Waiting for another device", "已就绪 · 正在等待另一台设备")
            else -> appText("No active room", "没有活动房间")
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
                    appText("ROOM", "房间"),
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
                        appText("Copy room code", "复制房间码"),
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
                        appText("Renew room invitation", "续期房间邀请"),
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
                            appText("Cancel joining room", "取消加入房间")
                        } else {
                            appText("Close room", "关闭房间")
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
                        appText(
                            "Enter this code on the other device. It is never sent over Bluetooth.",
                            "请在另一台设备上输入此验证码。验证码不会通过蓝牙发送。",
                        ),
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
            Text(it, color = colors.danger, fontSize = 12.sp, textAlign = TextAlign.Center)
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
                onClickLabel = appText("Hide room QR", "隐藏房间二维码"),
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
                        creating -> appText("Creating room…", "正在创建房间…")
                        joining -> appText("Joining room…", "正在加入房间…")
                        control.invite == null -> appText("Create room", "创建房间")
                        else -> appText("Reveal QR", "显示二维码")
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
                        appText("Scan QR", "扫描"),
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
                        appText("Enter code", "输入码"),
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
            appText("SET IDENTITY", "设置身份"),
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
                appText("Edit nearby name", "编辑附近名称"),
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
        NearbyVisibility.Hidden -> appText("Hidden", "已隐藏")
        NearbyVisibility.EveryoneTenMinutes -> appText("Everyone · 10 min", "所有人 · 10 分钟")
        NearbyVisibility.Foreground -> appText("While open", "打开时可见")
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
            appText("NEARBY DEVICES", "附近设备"),
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
                if (discoveryActive) appText("Stop", "停止") else appText("Start", "开始搜索"),
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
                    appText("Wi-Fi Aware", "Wi-Fi Aware"),
                    tint = if (wifiAwareActive) colors.accent else colors.muted,
                    modifier = Modifier.size(16.dp),
                )
                Spacer(Modifier.width(3.dp))
                Text(
                    appText("Aware", "Aware"),
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
                appText("NFC nearby room", "NFC 附近房间"),
                tint = if (nfcActive) colors.accent else colors.muted,
                modifier = Modifier.size(16.dp),
            )
            Spacer(Modifier.width(3.dp))
            Text(
                "NFC",
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
                    appText("Hide nearby devices", "隐藏附近设备")
                } else {
                    appText("Show nearby devices", "显示附近设备")
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
        title = { Text(appText("NFC nearby room", "NFC 附近房间")) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(
                    appText(
                        "NFC carries a short-lived room invitation. The room still authenticates the connection, and file data uses the selected network path.",
                        "NFC 只携带短时房间邀请。房间仍会验证连接，文件数据通过自动选定的网络路径传输。",
                    ),
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
                            appText("Stop NFC scan", "停止 NFC 扫描")
                        } else {
                            appText("Scan another phone", "扫描另一台手机")
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
                            appText("Stop sharing by NFC", "停止通过 NFC 分享")
                        } else {
                            appText("Create or share this room", "创建或分享此房间")
                        },
                    )
                }
                if (!hosting.armed && !canShareRoomViaNfc(roomPhase)) {
                    Text(
                        appText(
                            "End or leave the current room before sharing a new NFC invitation.",
                            "请先结束或离开当前房间，再分享新的 NFC 邀请。",
                        ),
                        color = Envoix.colors.muted,
                        fontSize = 12.sp,
                    )
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text(appText("Done", "完成"))
            }
        },
        containerColor = Envoix.colors.surface,
    )
}

@Composable
private fun nfcHostingStatusLabel(status: NfcPhoneHostingStatus): String =
    when (status) {
        NfcPhoneHostingStatus.Idle -> appText("Sharing: off", "分享：已关闭")
        NfcPhoneHostingStatus.Armed ->
            appText("Sharing: ready — hold the phones together", "分享：已就绪，请将两台手机靠近")
        NfcPhoneHostingStatus.RequiresAndroid15 ->
            appText("Sharing requires Android 15 or later", "分享需要 Android 15 或更高版本")
        NfcPhoneHostingStatus.NfcUnavailable ->
            appText("This phone does not provide NFC", "此手机不支持 NFC")
        NfcPhoneHostingStatus.NfcDisabled ->
            appText("Turn on NFC to share", "请打开 NFC 后分享")
        NfcPhoneHostingStatus.HceUnavailable ->
            appText("NFC phone sharing is unavailable", "NFC 手机分享不可用")
        NfcPhoneHostingStatus.ListenOnlyUnavailable ->
            appText("Safe NFC sharing mode is unavailable", "安全 NFC 分享模式不可用")
        NfcPhoneHostingStatus.HceActivationFailed ->
            appText("NFC sharing could not start", "无法启动 NFC 分享")
        NfcPhoneHostingStatus.InvalidInvitation ->
            appText("The room invitation is not ready", "房间邀请尚未就绪")
    }

@Composable
private fun nfcReaderStatusLabel(state: NfcPhoneReaderState): String =
    when (state.status) {
        NfcPhoneReaderStatus.Idle -> appText("Scanning: off", "扫描：已关闭")
        NfcPhoneReaderStatus.Scanning ->
            if (state.automatic) {
                appText("Scanning: nearby phone detected", "扫描：已检测到附近手机")
            } else {
                appText("Scanning: hold the phones together", "扫描：请将两台手机靠近")
            }
        NfcPhoneReaderStatus.NfcUnavailable ->
            appText("This phone does not provide NFC scanning", "此手机不支持 NFC 扫描")
        NfcPhoneReaderStatus.NfcDisabled ->
            appText("Turn on NFC to scan", "请打开 NFC 后扫描")
        NfcPhoneReaderStatus.ReaderUnavailable ->
            appText("NFC scanning could not start", "无法启动 NFC 扫描")
    }

@Composable
internal fun WifiAwareDiscoveryDialog(
    status: ProviderStatus?,
    onDismiss: () -> Unit,
) {
    val state = wifiAwareDiscoveryUiState(status)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Wi-Fi Aware") },
        text = {
            Text(
                when (state) {
                    WifiAwareDiscoveryUiState.Active ->
                        appText(
                            "Wi-Fi Aware discovery is active. Envoix still chooses the data path automatically for each transfer.",
                            "Wi-Fi Aware 发现已启用。Envoix 仍会为每次传输自动选择数据路径。",
                        )
                    WifiAwareDiscoveryUiState.Starting ->
                        appText(
                            "Wi-Fi Aware discovery is starting.",
                            "Wi-Fi Aware 发现正在启动。",
                        )
                    WifiAwareDiscoveryUiState.Unavailable ->
                        appText(
                            "Wi-Fi Aware discovery is not connected in this Android build yet. Nearby continues over Bluetooth and local-network discovery.",
                            "此 Android 版本尚未接入 Wi-Fi Aware 发现。附近发现仍会通过蓝牙和局域网继续工作。",
                        )
                },
                color = Envoix.colors.muted,
                fontSize = 13.sp,
            )
        },
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text(appText("Done", "完成"))
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
                    appText("Nearby Envoix device", "附近的 Envoix 设备"),
                ),
                color = colors.text,
                fontSize = 15.sp,
                fontWeight = FontWeight.Bold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                "${
                    nearbyDiscoverySourceLabel(peer.sources, LocalAppLanguage.current)
                } · ${
                    if (enabled) {
                        if (DiscoverySource.Bluetooth in peer.sources && peer.nearbyInviteRoute == null) {
                            appText("Tap to verify", "轻触验证")
                        } else {
                            appText("Unverified", "未验证")
                        }
                    } else {
                        appText("Discovery only", "仅可发现")
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
                appText("Open room", "打开房间"),
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
    language: String,
): String {
    val labels =
        listOf(
            DiscoverySource.Bluetooth to AppText.value("BLE", "BLE", language),
            DiscoverySource.Mdns to AppText.value("Local network", "局域网", language),
            DiscoverySource.WifiAware to AppText.value("Wi-Fi Aware", "Wi-Fi Aware", language),
        ).mapNotNull { (source, label) -> label.takeIf { source in sources } }
    return labels.joinToString(" · ").ifEmpty {
        AppText.value("Nearby", "附近", language)
    }
}

@Composable
internal fun EnterRoomCodeDialog(
    error: String?,
    onDismiss: () -> Unit,
    onContinue: (String) -> Unit,
) {
    val clipboard = LocalClipboardManager.current
    val emptyClipboardMessage = appText("Clipboard is empty", "剪贴板为空")
    var typed by remember { mutableStateOf("") }
    var inlineError by remember(error) { mutableStateOf(error) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(appText("Room code or invite link", "房间码或邀请链接")) },
        text = {
            Column {
                OutlinedTextField(
                    value = typed,
                    onValueChange = {
                        typed = InviteCodec.formatRoomCode(it)
                        inlineError = null
                    },
                    singleLine = true,
                    label = { Text(appText("Room code or invite link", "房间码或邀请链接")) },
                    placeholder = { Text("123456-a1b2-c3d4") },
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
                    Text(appText("Paste", "粘贴"))
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
                Text(appText("Continue", "继续"))
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text(appText("Cancel", "取消"))
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
        title = { Text(appText("Other devices can find you as", "其他设备可以发现你为"), fontSize = 16.sp) },
        text = {
            OutlinedTextField(
                value = typed,
                onValueChange = { if (it.length <= 48) typed = it },
                singleLine = true,
                label = { Text(appText("Visible as", "显示为")) },
                supportingText = {
                    Text(appText("This identity is temporary.", "此身份为临时身份。"))
                },
                modifier = Modifier.fillMaxWidth(),
            )
        },
        confirmButton = {
            TextButton(onClick = { onSave(typed) }, enabled = typed.isNotBlank()) {
                Text(appText("Save", "保存"))
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text(appText("Cancel", "取消"))
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
        title = { Text(appText("Who can find you?", "谁可以发现你？")) },
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
                Text(appText("Done", "完成"))
            }
        },
        containerColor = Envoix.colors.surface,
    )
}

@Composable
private fun NearbyVisibility.description(): String =
    when (this) {
        NearbyVisibility.Hidden ->
            appText("You can still find and connect to other devices.", "你仍可发现并连接其他设备。")
        NearbyVisibility.EveryoneTenMinutes ->
            appText("Nearby people can find you for ten minutes.", "附近的人可以在十分钟内发现你。")
        NearbyVisibility.Foreground ->
            appText("Nearby people can find you while Envoix is open.", "Envoix 打开时，附近的人可以发现你。")
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
                    appText("Verify nearby device", "验证附近设备")
                } else if (roomInvitation) {
                    appText("Room invitation", "房间邀请")
                } else {
                    appText("File invitation", "文件邀请")
                },
            )
        },
        text = {
            if (verificationOffer) {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text(
                        appText(
                            "Ask $peerName for the six-digit code shown in Envoix.",
                            "请向 $peerName 确认 Envoix 中显示的六位验证码。",
                        ),
                    )
                    OutlinedTextField(
                        value = code,
                        onValueChange = { value ->
                            code = value.filter { it in '0'..'9' }.take(6)
                        },
                        label = { Text(appText("Verification code", "验证码")) },
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
                        appText(
                            "$peerName wants to open a room with you.",
                            "$peerName 想与你建立房间。",
                        )
                    } else {
                        appText(
                            "$peerName wants to transfer files.",
                            "$peerName 想要传输文件。",
                        )
                    },
                )
            }
        },
        confirmButton = {
            TextButton(
                onClick = { onAccept(code.takeIf { verificationOffer }) },
                enabled = !verificationOffer || code.length == 6,
            ) {
                Text(if (verificationOffer) appText("Verify", "验证") else appText("Accept", "接受"))
            }
        },
        dismissButton = {
            TextButton(onClick = onReject) {
                Text(appText("Reject", "拒绝"))
            }
        },
        containerColor = Envoix.colors.surface,
    )
}
