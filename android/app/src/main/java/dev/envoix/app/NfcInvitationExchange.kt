package dev.envoix.app

import android.nfc.NdefMessage
import android.nfc.NdefRecord
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.nio.charset.StandardCharsets

internal object NfcInvitationNdefCodec {
    private const val UNCOMPRESSED_URI_PREFIX: Byte = 0

    fun messageFor(invitation: String): NdefMessage? {
        val carrierBytes = NfcInvitationContract.encode(invitation) ?: return null
        val record =
            NdefRecord(
                NdefRecord.TNF_WELL_KNOWN,
                NdefRecord.RTD_URI,
                ByteArray(0),
                byteArrayOf(UNCOMPRESSED_URI_PREFIX) + carrierBytes,
            )
        return NdefMessage(arrayOf(record))
    }

    fun invitationFrom(messages: List<NdefMessage>): String? {
        if (messages.size != 1) return null
        val records = messages.single().records
        if (records.size != 1) return null
        val record = records.single()
        if (record.tnf != NdefRecord.TNF_WELL_KNOWN ||
            !record.type.contentEquals(NdefRecord.RTD_URI) ||
            record.id.isNotEmpty() ||
            record.payload.firstOrNull() != UNCOMPRESSED_URI_PREFIX ||
            record.payload.size - 1 > NfcInvitationContract.maxCarrierBytes
        ) {
            return null
        }
        return NfcInvitationContract.decode(record.payload.copyOfRange(1, record.payload.size))
    }
}

internal enum class NfcInvitationFailure {
    InvalidTag,
}

internal data class NfcInvitationUiState(
    val confirmationPending: Boolean = false,
    val failure: NfcInvitationFailure? = null,
)

internal class NfcInvitationController {
    private val _state = MutableStateFlow(NfcInvitationUiState())

    val state: StateFlow<NfcInvitationUiState> = _state.asStateFlow()

    private var pendingInvitation: String? = null

    fun acceptDiscoveredMessages(messages: List<NdefMessage>) {
        acceptInvitation(NfcInvitationNdefCodec.invitationFrom(messages))
    }

    fun acceptDiscoveredCarrier(carrier: String) {
        acceptInvitation(
            NfcInvitationContract.decode(
                carrier.toByteArray(StandardCharsets.UTF_8),
            ),
        )
    }

    fun acceptDiscoveredInvitation(invitation: String?) {
        acceptInvitation(invitation)
    }

    fun confirmInvitation(onConfirmed: (String) -> Unit) {
        val invitation = pendingInvitation ?: return
        pendingInvitation = null
        _state.value =
            _state.value.copy(
                confirmationPending = false,
                failure = null,
            )
        onConfirmed(invitation)
    }

    fun cancelConfirmation() {
        pendingInvitation = null
        _state.value = _state.value.copy(confirmationPending = false)
    }

    fun dismissFailure() {
        _state.value = _state.value.copy(failure = null)
    }

    fun stop() {
        pendingInvitation = null
        _state.value = NfcInvitationUiState()
    }

    fun close() = stop()

    private fun acceptInvitation(invitation: String?) {
        if (invitation == null) {
            showFailure(NfcInvitationFailure.InvalidTag)
            return
        }
        pendingInvitation = invitation
        _state.value =
            _state.value.copy(
                confirmationPending = true,
                failure = null,
            )
    }

    private fun showFailure(reason: NfcInvitationFailure) {
        pendingInvitation = null
        _state.value =
            _state.value.copy(
                confirmationPending = false,
                failure = reason,
            )
    }
}
