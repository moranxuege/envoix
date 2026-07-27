package dev.envoix.app.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.Keyboard
import androidx.compose.material.icons.filled.QrCodeScanner
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Smartphone
import androidx.compose.material.icons.filled.Visibility
import androidx.compose.material.icons.filled.VisibilityOff
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
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.discovery.DiscoveredPeer
import dev.envoix.app.discovery.NearbyVisibility

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

@Composable
internal fun MainRoomInviteCard(
    control: RoomControlUiState,
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
            control.invite != null
    val joining = control.phase == RoomControlPhase.Joining
    Column(
        Modifier
            .testTag("hub_room_invite")
            .fillMaxWidth()
            .clip(RoundedCornerShape(22.dp))
            .background(colors.surface)
            .border(1.dp, colors.line, RoundedCornerShape(22.dp))
            .clickable(enabled = !revealed && !joining, onClick = onReveal)
            .padding(horizontal = 16.dp, vertical = 18.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(Modifier.size(190.dp), contentAlignment = Alignment.Center) {
            when {
                revealed -> QrCode(requireNotNull(control.invite).payload, side = 190.dp)
                joining ->
                    CircularProgressIndicator(
                        color = colors.accent,
                        modifier = Modifier.size(34.dp),
                    )
                control.phase == RoomControlPhase.Hosting && control.inviteRevealed ->
                    CircularProgressIndicator(color = colors.accent, modifier = Modifier.size(34.dp))
                else -> BlurredQrPlaceholder()
            }
        }
        Spacer(Modifier.height(13.dp))
        if (revealed) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    requireNotNull(control.invite).code,
                    color = colors.text,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.Bold,
                    fontFamily = FontFamily.Monospace,
                )
                Spacer(Modifier.width(4.dp))
                IconButton(
                    onClick = {
                        clipboard.setText(AnnotatedString(requireNotNull(control.invite).code))
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
                IconButton(
                    onClick = onHide,
                    modifier = Modifier.testTag("hub_hide_room_code"),
                ) {
                    Icon(
                        Icons.Default.VisibilityOff,
                        appText("Hide room code", "隐藏房间码"),
                        tint = colors.muted,
                        modifier = Modifier.size(19.dp),
                    )
                }
                IconButton(
                    onClick = onRefresh,
                    modifier = Modifier.testTag("hub_refresh_room_code"),
                ) {
                    Icon(
                        Icons.Default.Refresh,
                        appText("Refresh room code", "刷新房间码"),
                        tint = colors.accent,
                        modifier = Modifier.size(19.dp),
                    )
                }
            }
        } else {
            Text(
                if (joining) {
                    appText("Joining room…", "正在加入房间…")
                } else {
                    "R••••••-••••-••••"
                },
                color = colors.muted,
                fontSize = 15.sp,
                fontWeight = FontWeight.Bold,
                fontFamily = FontFamily.Monospace,
            )
            Spacer(Modifier.height(5.dp))
            Text(
                if (control.phase == RoomControlPhase.Hosting && control.inviteRevealed) {
                    appText("Creating your room…", "正在创建房间…")
                } else if (joining) {
                    appText(
                        "Waiting for an authenticated connection",
                        "正在等待经过认证的连接",
                    )
                } else {
                    appText("Tap to reveal your room QR and code", "轻触显示房间二维码和房间码")
                },
                color = colors.muted,
                fontSize = 12.sp,
                textAlign = TextAlign.Center,
            )
        }
        control.error?.let {
            Spacer(Modifier.height(8.dp))
            Text(it, color = colors.danger, fontSize = 12.sp, textAlign = TextAlign.Center)
        }
        if (control.phase == RoomControlPhase.Hosting || joining) {
            Spacer(Modifier.height(6.dp))
            TextButton(
                onClick = onEndWaiting,
                modifier = Modifier.testTag("hub_end_waiting_room"),
            ) {
                Icon(
                    Icons.Default.Close,
                    null,
                    tint = colors.danger,
                    modifier = Modifier.size(17.dp),
                )
                Spacer(Modifier.width(5.dp))
                Text(
                    if (joining) {
                        appText("Cancel", "取消")
                    } else {
                        appText("Stop waiting", "停止等待")
                    },
                    color = colors.danger,
                )
            }
        }
    }
}

@Composable
private fun BlurredQrPlaceholder() {
    val colors = Envoix.colors
    Box(
        Modifier
            .size(190.dp)
            .clip(RoundedCornerShape(14.dp))
            .background(Color.White)
            .padding(10.dp),
    ) {
        Canvas(Modifier.fillMaxSize()) {
            val cells = 23
            val cell = size.width / cells
            for (row in 0 until cells) {
                for (column in 0 until cells) {
                    if ((row * 31 + column * 17 + row * column) % 5 < 2) {
                        drawRect(
                            color = colors.muted.copy(alpha = 0.22f),
                            topLeft = Offset(column * cell, row * cell),
                            size = Size(cell * 1.5f, cell * 1.5f),
                        )
                    }
                }
            }
            drawRect(Color.White.copy(alpha = 0.62f))
        }
    }
}

@Composable
internal fun ConnectionMethodActions(
    onScan: () -> Unit,
    onEnterCode: () -> Unit,
) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        OutlinedButton(
            onClick = onScan,
            modifier = Modifier.weight(1f).testTag("hub_scan_qr"),
            contentPadding = PaddingValues(horizontal = 8.dp, vertical = 12.dp),
        ) {
            Icon(Icons.Default.QrCodeScanner, null, modifier = Modifier.size(19.dp))
            Spacer(Modifier.width(7.dp))
            Text(appText("Scan QR", "扫描二维码"), maxLines = 1)
        }
        OutlinedButton(
            onClick = onEnterCode,
            modifier = Modifier.weight(1f).testTag("hub_enter_code"),
            contentPadding = PaddingValues(horizontal = 8.dp, vertical = 12.dp),
        ) {
            Icon(Icons.Default.Keyboard, null, modifier = Modifier.size(19.dp))
            Spacer(Modifier.width(7.dp))
            Text(appText("Enter code", "输入房间码"), maxLines = 1)
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
            .padding(14.dp),
    ) {
        Text(
            appText("VISIBLE AS", "显示名称"),
            color = colors.muted,
            fontSize = 10.sp,
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

@Composable
internal fun NearbyDeviceCard(
    peer: DiscoveredPeer,
    onClick: () -> Unit,
) {
    val colors = Envoix.colors
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(colors.surface)
            .border(1.dp, colors.line, RoundedCornerShape(16.dp))
            .clickable(onClick = onClick)
            .padding(15.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier.size(38.dp).clip(CircleShape).background(colors.accentSoft),
            contentAlignment = Alignment.Center,
        ) {
            Icon(Icons.Default.Smartphone, null, tint = colors.accent, modifier = Modifier.size(20.dp))
        }
        Spacer(Modifier.width(11.dp))
        Column(Modifier.weight(1f)) {
            Text(
                peer.displayName ?: appText("Nearby Envoix device", "附近的 Envoix 设备"),
                color = colors.text,
                fontSize = 15.sp,
                fontWeight = FontWeight.Bold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                appText("Nearby · Unverified", "附近 · 未验证"),
                color = colors.muted,
                fontSize = 12.sp,
                fontWeight = FontWeight.SemiBold,
            )
        }
        Icon(
            Icons.AutoMirrored.Filled.KeyboardArrowRight,
            appText("Open room", "打开房间"),
            tint = colors.muted,
        )
    }
}

@Composable
internal fun EnterRoomCodeDialog(
    error: String?,
    onDismiss: () -> Unit,
    onContinue: (String) -> Unit,
) {
    var typed by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(appText("Enter room code", "输入房间码")) },
        text = {
            Column {
                OutlinedTextField(
                    value = typed,
                    onValueChange = { typed = it },
                    singleLine = true,
                    label = { Text(appText("Room code", "房间码")) },
                    placeholder = { Text("R123456-a1b2-c3d4") },
                    modifier = Modifier.fillMaxWidth(),
                )
                error?.let {
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
                Text(appText("Connect", "连接"))
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
        title = { Text(appText("Nearby name", "附近名称")) },
        text = {
            OutlinedTextField(
                value = typed,
                onValueChange = { if (it.length <= 48) typed = it },
                singleLine = true,
                label = { Text(appText("Visible as", "显示为")) },
                supportingText = {
                    Text(appText("This name is not a verified identity.", "此名称不代表已验证身份。"))
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
    roomInvitation: Boolean,
    peerName: String,
    onAccept: () -> Unit,
    onReject: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onReject,
        title = {
            Text(
                if (roomInvitation) {
                    appText("Room invitation", "房间邀请")
                } else {
                    appText("File invitation", "文件邀请")
                },
            )
        },
        text = {
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
        },
        confirmButton = {
            TextButton(onClick = onAccept) {
                Text(appText("Accept", "接受"))
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
