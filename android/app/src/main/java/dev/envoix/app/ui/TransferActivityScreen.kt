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
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.Transfer

@OptIn(ExperimentalComposeUiApi::class)
@Composable
internal fun TransferActivityScreen(
    transfers: List<Transfer>,
    onBack: () -> Unit,
    onPauseResume: (Long) -> Unit,
    onApproveReceive: (Long) -> Unit,
    onCancel: (Long) -> Unit,
    onRemove: (Long) -> Unit,
    onOpen: (Transfer) -> Unit,
    onShare: (Transfer) -> Unit,
) {
    val colors = Envoix.colors
    val expanded = remember { mutableStateListOf<Long>() }

    Column(
        Modifier
            .semantics { testTagsAsResourceId = true }
            .testTag("transfer_activity")
            .fillMaxSize()
            .background(colors.bg),
    ) {
        androidx.compose.foundation.layout.Row(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onBack) {
                Icon(
                    Icons.AutoMirrored.Filled.ArrowBack,
                    contentDescription = appText("Back", "返回"),
                    tint = colors.accent,
                )
            }
            Text(
                appText("Activity", "活动"),
                color = colors.text,
                fontSize = 24.sp,
                fontWeight = FontWeight.ExtraBold,
            )
        }

        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            if (transfers.isEmpty()) {
                item {
                    Text(
                        appText(
                            "No transfers yet. They will appear here after a room starts.",
                            "暂无传输；建立房间后，传输记录会显示在这里。",
                        ),
                        color = colors.muted,
                        fontSize = 14.sp,
                        modifier = Modifier.padding(vertical = 32.dp),
                    )
                }
            } else {
                items(transfers.sortedByDescending(Transfer::id), key = Transfer::id) { transfer ->
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
        }
    }
}
