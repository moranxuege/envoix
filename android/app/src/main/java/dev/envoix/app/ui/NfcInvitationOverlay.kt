package dev.envoix.app.ui

import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import dev.envoix.app.NfcInvitationFailure
import dev.envoix.app.NfcInvitationUiState
import dev.envoix.app.R

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
            title = { Text(appString(R.string.nfc_open_invitation_title)) },
            text = {
                Text(appString(R.string.nfc_untrusted_invitation_explanation))
            },
            confirmButton = {
                TextButton(onClick = onConfirm) {
                    Text(appString(R.string.common_continue))
                }
            },
            dismissButton = {
                TextButton(onClick = onCancelConfirmation) {
                    Text(appString(R.string.common_cancel))
                }
            },
        )
    }
    state.failure?.let { failure ->
        AlertDialog(
            onDismissRequest = onDismissFailure,
            title = { Text(appString(R.string.nfc_invitation_failed_title)) },
            text = { Text(nfcFailureText(failure)) },
            confirmButton = {
                TextButton(onClick = onDismissFailure) {
                    Text(appString(R.string.common_ok))
                }
            },
        )
    }
}

@Composable
private fun nfcFailureText(failure: NfcInvitationFailure): String =
    when (failure) {
        NfcInvitationFailure.InvalidTag ->
            appString(R.string.nfc_invalid_tag_message)
    }
