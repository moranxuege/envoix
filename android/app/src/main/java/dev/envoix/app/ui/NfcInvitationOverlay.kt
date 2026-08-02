package dev.envoix.app.ui

import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import dev.envoix.app.NfcInvitationFailure
import dev.envoix.app.NfcInvitationUiState

@Composable
internal fun NfcInvitationOverlay(
    state: NfcInvitationUiState,
    onConfirm: () -> Unit,
    onCancelConfirmation: () -> Unit,
    onDismissFailure: () -> Unit,
) {
    if (state.confirmationPending) {
        AlertDialog(
            onDismissRequest = onCancelConfirmation,
            title = { Text(appText("Open NFC invitation?", "打开 NFC 邀请？")) },
            text = {
                Text(
                    appText(
                        "This untrusted tag only carries an invitation. Continue to validate it and use the normal room flow; NFC does not authenticate the other device.",
                        "此未受信任的标签仅携带邀请。继续后仍会验证并使用常规房间流程；NFC 不会认证另一台设备。",
                    ),
                )
            },
            confirmButton = {
                TextButton(onClick = onConfirm) {
                    Text(appText("Continue", "继续"))
                }
            },
            dismissButton = {
                TextButton(onClick = onCancelConfirmation) {
                    Text(appText("Cancel", "取消"))
                }
            },
        )
    }
    state.failure?.let { failure ->
        AlertDialog(
            onDismissRequest = onDismissFailure,
            title = { Text(appText("NFC could not continue", "NFC 无法继续")) },
            text = { Text(nfcFailureText(failure)) },
            confirmButton = {
                TextButton(onClick = onDismissFailure) {
                    Text(appText("OK", "确定"))
                }
            },
        )
    }
}

@Composable
private fun nfcFailureText(failure: NfcInvitationFailure): String =
    when (failure) {
        NfcInvitationFailure.InvalidTag ->
            appText(
                "The tag must contain exactly one valid Envoix NFC invitation.",
                "标签必须仅包含一个有效的 Envoix NFC 邀请。",
            )
    }
