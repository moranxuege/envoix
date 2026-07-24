package dev.envoix.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Devices
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.Status
import dev.envoix.app.Transfer

internal data class RoomStatus(
    val label: String,
    val foreground: Color,
)

@Composable
internal fun RoomHeader(
    displayName: String,
    state: RoomStatus,
    onBack: () -> Unit,
) {
    val colors = Envoix.colors
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onClick = onBack) {
            Icon(
                Icons.AutoMirrored.Filled.ArrowBack,
                contentDescription = appText("Back", "返回"),
                tint = colors.accent,
            )
        }
        Box(
            Modifier.size(42.dp).clip(CircleShape).background(colors.accentSoft),
            contentAlignment = Alignment.Center,
        ) {
            Icon(Icons.Default.Devices, null, tint = colors.accent, modifier = Modifier.size(22.dp))
        }
        Spacer(Modifier.size(11.dp))
        Column(Modifier.weight(1f)) {
            Text(
                displayName,
                color = colors.text,
                fontSize = 18.sp,
                fontWeight = FontWeight.Bold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(7.dp).clip(CircleShape).background(state.foreground))
                Spacer(Modifier.size(6.dp))
                Text(
                    state.label,
                    color = state.foreground,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.SemiBold,
                )
            }
        }
    }
}

@Composable
internal fun EmptyRoomTimeline() {
    val colors = Envoix.colors
    Box(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(18.dp))
            .background(colors.surface)
            .border(1.dp, colors.line, RoundedCornerShape(18.dp))
            .padding(horizontal = 20.dp, vertical = 26.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            appText(
                "No transfers in this room yet. Add files when you are ready.",
                "这个房间中还没有传输。准备好后即可添加文件。",
            ),
            color = colors.muted,
            fontSize = 13.sp,
            fontWeight = FontWeight.SemiBold,
        )
    }
}

@Composable
internal fun PendingRoomAction(
    role: String,
    pendingShareCount: Int,
    onContinue: () -> Unit,
) {
    val colors = Envoix.colors
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(colors.accentSoft)
            .padding(16.dp),
    ) {
        Text(
            if (role == "receive") {
                appText("A transfer invite is ready", "传输邀请已就绪")
            } else if (pendingShareCount > 0) {
                appText("$pendingShareCount shared items are ready", "$pendingShareCount 个共享项目已就绪")
            } else {
                appText("This device is ready for files", "此设备已准备好传输文件")
            },
            color = colors.accentStrong,
            fontSize = 14.sp,
            fontWeight = FontWeight.Bold,
        )
        Spacer(Modifier.height(4.dp))
        Text(
            if (role == "receive") {
                appText(
                    "Review where received files will be saved, then start waiting.",
                    "确认接收文件的保存位置，然后开始等待。",
                )
            } else {
                appText(
                    "Review the selection before offering it to the other device.",
                    "发送给另一台设备前，请先确认所选内容。",
                )
            },
            color = colors.muted,
            fontSize = 12.sp,
            lineHeight = 17.sp,
        )
        Spacer(Modifier.height(12.dp))
        Button(onClick = onContinue, modifier = Modifier.testTag("room_review_invite")) {
            Text(
                when {
                    role == "receive" -> appText("Review invite", "查看邀请")
                    pendingShareCount > 0 ->
                        appText(
                            "Continue with $pendingShareCount items",
                            "继续发送 $pendingShareCount 个项目",
                        )
                    else -> appText("Choose files", "选择文件")
                },
            )
        }
    }
}

@Composable
internal fun RoomActions(onAddFiles: () -> Unit) {
    val colors = Envoix.colors
    Box(
        Modifier
            .fillMaxWidth()
            .background(colors.surface)
            .padding(horizontal = 16.dp, vertical = 12.dp)
            .navigationBarsPadding(),
    ) {
        RoomActionButton(
            label = appText("Add files", "添加文件"),
            icon = Icons.Default.Add,
            onClick = onAddFiles,
            modifier = Modifier.fillMaxWidth().testTag("room_add_files"),
        )
    }
}

@Composable
private fun RoomActionButton(
    label: String,
    icon: ImageVector,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = Envoix.colors
    Row(
        modifier
            .height(50.dp)
            .clip(RoundedCornerShape(14.dp))
            .background(colors.accent)
            .clickable(onClick = onClick),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(icon, null, tint = Color.White, modifier = Modifier.size(19.dp))
        Spacer(Modifier.size(7.dp))
        Text(label, color = Color.White, fontSize = 14.sp, fontWeight = FontWeight.Bold)
    }
}

@Composable
internal fun roomState(active: List<Transfer>): RoomStatus {
    val colors = Envoix.colors
    val waiting =
        active.all {
            it.status == Status.Preparing ||
                it.status == Status.WaitingForPeer ||
                it.status == Status.Pairing ||
                it.status == Status.Connecting
        }
    return when {
        active.isEmpty() ->
            RoomStatus(
                label = appText("No active transfer", "无进行中的传输"),
                foreground = colors.muted,
            )
        waiting ->
            RoomStatus(
                label = appText("Waiting", "等待中"),
                foreground = colors.warning,
            )
        else ->
            RoomStatus(
                label = appText("${active.size} active", "${active.size} 个进行中"),
                foreground = colors.success,
            )
    }
}
