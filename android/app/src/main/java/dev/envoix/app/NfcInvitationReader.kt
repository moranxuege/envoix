package dev.envoix.app

import android.app.Activity
import android.content.pm.PackageManager
import android.nfc.NdefMessage
import android.nfc.NfcAdapter
import android.nfc.NfcManager
import android.nfc.Tag
import android.nfc.tech.IsoDep
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import androidx.activity.ComponentActivity
import dev.envoix.app.discovery.NfcReadinessOffer
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.util.concurrent.atomic.AtomicBoolean

internal interface NfcIsoDepTransceiver {
    fun transceive(command: ByteArray): ByteArray
}

/**
 * Reads only Envoix's private AID. Android's generic NDEF probe is deliberately
 * skipped by ReaderMode so the app never selects a wallet/payment application.
 */
internal object NfcPrivateInvitationReader {
    fun readNdefMessage(channel: NfcIsoDepTransceiver): ByteArray? {
        if (!channel.transceive(selectApplication()).isSuccessStatus()) return null
        if (!channel.transceive(selectNdefFile()).isSuccessStatus()) return null

        val lengthResponse = channel.transceive(readBinary(offset = 0, length = NDEF_LENGTH_BYTES))
        val lengthBytes = lengthResponse.successBody(expectedBytes = NDEF_LENGTH_BYTES) ?: return null
        val messageLength =
            ((lengthBytes[0].toInt() and 0xff) shl 8) or
                (lengthBytes[1].toInt() and 0xff)
        if (messageLength <= 0 || messageLength > NfcType4TagProtocol.MAX_NDEF_MESSAGE_BYTES) {
            return null
        }

        val message = ByteArray(messageLength)
        var messageOffset = 0
        while (messageOffset < message.size) {
            val count = minOf(MAX_READ_BYTES, message.size - messageOffset)
            val fileOffset = messageOffset + NDEF_LENGTH_BYTES
            val response = channel.transceive(readBinary(fileOffset, count))
            val body = response.successBody(expectedBytes = count) ?: return null
            body.copyInto(message, destinationOffset = messageOffset)
            messageOffset += count
        }
        return message
    }

    private fun selectApplication(): ByteArray =
        byteArrayOf(0x00, 0xa4.toByte(), 0x04, 0x00, NfcType4TagProtocol.ENVOIX_APPLICATION_AID.size.toByte()) +
            NfcType4TagProtocol.ENVOIX_APPLICATION_AID

    private fun selectNdefFile(): ByteArray =
        byteArrayOf(0x00, 0xa4.toByte(), 0x00, 0x0c, NfcType4TagProtocol.NDEF_FILE_ID.size.toByte()) +
            NfcType4TagProtocol.NDEF_FILE_ID

    private fun readBinary(
        offset: Int,
        length: Int,
    ): ByteArray =
        byteArrayOf(
            0x00,
            0xb0.toByte(),
            (offset ushr 8).toByte(),
            offset.toByte(),
            length.toByte(),
        )

    private fun ByteArray.isSuccessStatus(): Boolean =
        size == STATUS_BYTES &&
            this[0] == NfcType4TagProtocol.SUCCESS[0] &&
            this[1] == NfcType4TagProtocol.SUCCESS[1]

    private fun ByteArray.successBody(expectedBytes: Int): ByteArray? {
        if (size != expectedBytes + STATUS_BYTES ||
            this[lastIndex - 1] != NfcType4TagProtocol.SUCCESS[0] ||
            this[lastIndex] != NfcType4TagProtocol.SUCCESS[1]
        ) {
            return null
        }
        return copyOf(expectedBytes)
    }

    private const val NDEF_LENGTH_BYTES = 2
    private const val STATUS_BYTES = 2
    private const val MAX_READ_BYTES = 0xff
}

internal enum class NfcPhoneReaderStatus {
    Idle,
    Scanning,
    NfcUnavailable,
    NfcDisabled,
    ReaderUnavailable,
}

internal data class NfcPhoneReaderState(
    val status: NfcPhoneReaderStatus = NfcPhoneReaderStatus.Idle,
    val automatic: Boolean = false,
) {
    val scanning: Boolean
        get() = status == NfcPhoneReaderStatus.Scanning
}

internal interface NfcReaderLeasePlatform {
    fun unavailableStatus(): NfcPhoneReaderStatus?

    fun resetDiscoveryTechnology()

    fun enterIdleListenOnly()

    fun enableReader(onInvitation: (String?) -> Unit): Boolean

    fun disableReader()
}

internal class NfcReaderLeaseSession(
    private val platform: NfcReaderLeasePlatform,
    private val scheduleTimeout: (Long, () -> Unit) -> Unit,
    private val cancelTimeout: () -> Unit,
    private val onInvitation: (String?) -> Unit,
) {
    private val _state = MutableStateFlow(NfcPhoneReaderState())
    val state: StateFlow<NfcPhoneReaderState> = _state.asStateFlow()

    private val usedAutomaticOfferIds = LinkedHashSet<String>()
    private var resumed = false
    private var connectActive = false
    private var automaticGateOpen = true

    fun onResume() {
        resumed = true
    }

    fun enterConnect() {
        connectActive = true
        if (resumed && !state.value.scanning) {
            platform.enterIdleListenOnly()
        }
    }

    fun leaveConnect() {
        connectActive = false
        if (state.value.scanning) {
            platform.disableReader()
            cancelTimeout()
        }
        platform.resetDiscoveryTechnology()
        _state.value = NfcPhoneReaderState()
    }

    fun startAutomatic(
        offer: NfcReadinessOffer,
        nowMs: Long,
    ): Boolean {
        if (!automaticGateOpen ||
            offer.offerId in usedAutomaticOfferIds ||
            nowMs < offer.seenAtMs ||
            nowMs - offer.seenAtMs > MAX_READINESS_AGE_MS
        ) {
            return false
        }
        if (usedAutomaticOfferIds.size >= MAX_USED_AUTOMATIC_OFFERS) {
            usedAutomaticOfferIds.remove(usedAutomaticOfferIds.first())
        }
        usedAutomaticOfferIds += offer.offerId
        automaticGateOpen = false
        return start(automatic = true)
    }

    fun resetAutomaticGate() {
        automaticGateOpen = true
    }

    fun startManual(): Boolean = start(automatic = false)

    fun stop() {
        if (state.value.scanning) {
            platform.disableReader()
            cancelTimeout()
        }
        if (resumed && connectActive) {
            platform.enterIdleListenOnly()
        } else {
            platform.resetDiscoveryTechnology()
        }
        _state.value = NfcPhoneReaderState()
    }

    fun onPause() {
        resumed = false
        if (state.value.scanning) {
            platform.disableReader()
            cancelTimeout()
        }
        platform.resetDiscoveryTechnology()
        _state.value = NfcPhoneReaderState()
    }

    fun close() = onPause()

    private fun start(automatic: Boolean): Boolean {
        if (!resumed || !connectActive || state.value.scanning) return false
        platform.unavailableStatus()?.let { status ->
            _state.value = NfcPhoneReaderState(status)
            return false
        }
        platform.resetDiscoveryTechnology()
        if (!platform.enableReader(::complete)) {
            platform.enterIdleListenOnly()
            _state.value = NfcPhoneReaderState(NfcPhoneReaderStatus.ReaderUnavailable)
            return false
        }
        _state.value =
            NfcPhoneReaderState(
                status = NfcPhoneReaderStatus.Scanning,
                automatic = automatic,
            )
        scheduleTimeout(READER_LEASE_MS, ::stop)
        return true
    }

    private fun complete(invitation: String?) {
        if (!state.value.scanning) return
        platform.disableReader()
        cancelTimeout()
        if (resumed && connectActive) {
            platform.enterIdleListenOnly()
        } else {
            platform.resetDiscoveryTechnology()
        }
        _state.value = NfcPhoneReaderState()
        onInvitation(invitation)
    }

    companion object {
        internal const val READER_LEASE_MS = 12_000L
        internal const val MAX_READINESS_AGE_MS = 5_000L
        private const val MAX_USED_AUTOMATIC_OFFERS = 64
    }
}

private class AndroidNfcReaderLeasePlatform(
    private val activity: Activity,
) : NfcReaderLeasePlatform {
    private val handler = Handler(Looper.getMainLooper())
    private val adapter =
        activity
            .getSystemService(NfcManager::class.java)
            ?.defaultAdapter
    private val discoveryTechnology =
        adapter?.let { NfcDiscoveryTechnologyBridge.forActivity(activity, it) }

    override fun unavailableStatus(): NfcPhoneReaderStatus? =
        when {
            adapter == null ||
                !activity.packageManager.hasSystemFeature(PackageManager.FEATURE_NFC) ->
                NfcPhoneReaderStatus.NfcUnavailable
            runCatching { adapter?.isEnabled == true }.getOrDefault(false).not() ->
                NfcPhoneReaderStatus.NfcDisabled
            else -> null
        }

    override fun resetDiscoveryTechnology() {
        discoveryTechnology?.reset()
    }

    override fun enterIdleListenOnly() {
        discoveryTechnology?.enterListenOnly()
    }

    override fun enableReader(onInvitation: (String?) -> Unit): Boolean {
        val nfcAdapter = adapter ?: return false
        val attempted = AtomicBoolean(false)
        return runCatching {
            nfcAdapter.enableReaderMode(
                activity,
                { tag ->
                    if (!attempted.compareAndSet(false, true)) return@enableReaderMode
                    val invitation = readInvitation(tag)
                    handler.post { onInvitation(invitation) }
                },
                NfcAdapter.FLAG_READER_NFC_A or
                    NfcAdapter.FLAG_READER_SKIP_NDEF_CHECK or
                    NfcAdapter.FLAG_READER_NO_PLATFORM_SOUNDS,
                Bundle().apply {
                    putInt(NfcAdapter.EXTRA_READER_PRESENCE_CHECK_DELAY, PRESENCE_CHECK_DELAY_MS)
                },
            )
        }.isSuccess
    }

    override fun disableReader() {
        adapter?.let { nfcAdapter ->
            runCatching { nfcAdapter.disableReaderMode(activity) }
        }
    }

    private fun readInvitation(tag: Tag): String? {
        val isoDep = IsoDep.get(tag) ?: return null
        return try {
            isoDep.connect()
            isoDep.timeout = TRANSCEIVE_TIMEOUT_MS
            val messageBytes =
                NfcPrivateInvitationReader.readNdefMessage(
                    object : NfcIsoDepTransceiver {
                        override fun transceive(command: ByteArray): ByteArray = isoDep.transceive(command)
                    },
                ) ?: return null
            val message = runCatching { NdefMessage(messageBytes) }.getOrNull() ?: return null
            NfcInvitationNdefCodec.invitationFrom(listOf(message))
        } catch (_: Exception) {
            null
        } finally {
            runCatching { isoDep.close() }
        }
    }

    private companion object {
        const val PRESENCE_CHECK_DELAY_MS = 250
        const val TRANSCEIVE_TIMEOUT_MS = 3_000
    }
}

internal class NfcInvitationReaderController(
    activity: ComponentActivity,
    onInvitation: (String?) -> Unit,
) {
    private val handler = Handler(Looper.getMainLooper())
    private val timeout =
        Runnable {
            timeoutAction?.invoke()
        }
    private var timeoutAction: (() -> Unit)? = null
    private val session =
        NfcReaderLeaseSession(
            platform = AndroidNfcReaderLeasePlatform(activity),
            scheduleTimeout = { delayMs, action ->
                timeoutAction = action
                handler.removeCallbacks(timeout)
                handler.postDelayed(timeout, delayMs)
            },
            cancelTimeout = {
                handler.removeCallbacks(timeout)
                timeoutAction = null
            },
            onInvitation = { invitation ->
                handler.post { onInvitation(invitation) }
            },
        )

    val state: StateFlow<NfcPhoneReaderState> = session.state

    fun onResume() = session.onResume()

    fun enterConnect() = session.enterConnect()

    fun leaveConnect() = session.leaveConnect()

    fun startAutomatic(
        offer: NfcReadinessOffer,
        nowMs: Long,
    ): Boolean = session.startAutomatic(offer, nowMs)

    fun startManual(): Boolean = session.startManual()

    fun resetAutomaticGate() = session.resetAutomaticGate()

    fun stop() = session.stop()

    fun onPause() = session.onPause()

    fun close() {
        handler.removeCallbacks(timeout)
        timeoutAction = null
        session.close()
    }
}
