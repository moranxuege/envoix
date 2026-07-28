package dev.envoix.app.discovery

import java.io.ByteArrayOutputStream
import java.nio.charset.StandardCharsets
import java.util.UUID

internal interface BleRendezvousSecurity {
    val mode: Byte
    val logName: String

    fun seal(plaintext: ByteArray): ByteArray

    fun open(payload: ByteArray): ByteArray?
}

/** Experimental carrier only. This mode provides no peer authentication or confidentiality. */
internal object InsecureBleRendezvousSecurity : BleRendezvousSecurity {
    override val mode: Byte = 0
    override val logName = "none"

    override fun seal(plaintext: ByteArray): ByteArray = plaintext.copyOf()

    override fun open(payload: ByteArray): ByteArray = payload.copyOf()
}

internal data class BleRendezvousInvite(
    val requestId: String,
    val senderPeerKey: String,
    val senderDisplayName: String?,
    val invite: String,
)

internal object BleRendezvousProtocol {
    val SERVICE_UUID: UUID = UUID.fromString("d5f3a2d8-8f4a-4b33-8a01-000000000001")
    val WRITE_CHARACTERISTIC_UUID: UUID = UUID.fromString("d5f3a2d8-8f4a-4b33-8a01-000000000002")

    const val FRAME_HEADER_SIZE = 16
    const val MAX_WIRE_PAYLOAD_BYTES = 4_096
    const val MAX_INVITE_BYTES = 2_048
    const val MAX_DISPLAY_NAME_BYTES = 192
    const val MIN_GATT_WRITE_BYTES = 20

    private const val FRAME_MAGIC_0: Byte = 0x45
    private const val FRAME_MAGIC_1: Byte = 0x58
    private const val FRAME_VERSION: Byte = 1
    private const val FRAME_TYPE_INVITE: Byte = 1
    private const val ENVELOPE_VERSION: Byte = 1
    private const val ENVELOPE_TYPE_INVITE: Byte = 1
    private const val PEER_KEY_BYTES = 16
    private const val ENVELOPE_FIXED_BYTES = 6 + PEER_KEY_BYTES
    private val INVITE_PREFIXES = listOf("envoix://pair/", "envoix://room/")

    fun encodeInvite(
        identity: LocalDiscoveryIdentity,
        invite: String,
        requestId: Long,
        maximumFrameBytes: Int,
        security: BleRendezvousSecurity = InsecureBleRendezvousSecurity,
    ): List<ByteArray>? {
        val peerKey = DiscoveryPeerRegistry.normalizePeerKey(identity.peerKey) ?: return null
        val inviteBytes = invite.trim().toByteArray(StandardCharsets.UTF_8)
        if (!supportedInvite(invite) ||
            inviteBytes.isEmpty() ||
            inviteBytes.size > MAX_INVITE_BYTES
        ) {
            return null
        }
        val nameBytes = boundedUtf8(identity.displayName, MAX_DISPLAY_NAME_BYTES)
        val plaintext =
            ByteArrayOutputStream(ENVELOPE_FIXED_BYTES + nameBytes.size + inviteBytes.size)
                .apply {
                    write(ENVELOPE_VERSION.toInt())
                    write(ENVELOPE_TYPE_INVITE.toInt())
                    write(peerKey.toByteArray(StandardCharsets.US_ASCII))
                    writeShort(nameBytes.size)
                    writeShort(inviteBytes.size)
                    write(nameBytes)
                    write(inviteBytes)
                }.toByteArray()
        val sealed = security.seal(plaintext)
        val wirePayload = byteArrayOf(security.mode) + sealed
        if (wirePayload.size > MAX_WIRE_PAYLOAD_BYTES || maximumFrameBytes <= FRAME_HEADER_SIZE) return null

        val chunkCapacity = maximumFrameBytes - FRAME_HEADER_SIZE
        return buildList {
            var offset = 0
            while (offset < wirePayload.size) {
                val count = minOf(chunkCapacity, wirePayload.size - offset)
                val frame = ByteArray(FRAME_HEADER_SIZE + count)
                frame[0] = FRAME_MAGIC_0
                frame[1] = FRAME_MAGIC_1
                frame[2] = FRAME_VERSION
                frame[3] = FRAME_TYPE_INVITE
                putLong(frame, 4, requestId)
                putUnsignedShort(frame, 12, wirePayload.size)
                putUnsignedShort(frame, 14, offset)
                wirePayload.copyInto(frame, FRAME_HEADER_SIZE, offset, offset + count)
                add(frame)
                offset += count
            }
        }
    }

    internal class Assembler(
        private val security: BleRendezvousSecurity = InsecureBleRendezvousSecurity,
    ) {
        private var requestId: Long? = null
        private var totalLength = 0
        private var bytes = ByteArrayOutputStream()

        fun accept(frame: ByteArray): BleRendezvousInvite? {
            if (frame.size <= FRAME_HEADER_SIZE ||
                frame[0] != FRAME_MAGIC_0 ||
                frame[1] != FRAME_MAGIC_1 ||
                frame[2] != FRAME_VERSION ||
                frame[3] != FRAME_TYPE_INVITE
            ) {
                reset()
                return null
            }
            val incomingRequestId = getLong(frame, 4)
            val incomingTotal = getUnsignedShort(frame, 12)
            val incomingOffset = getUnsignedShort(frame, 14)
            val chunkSize = frame.size - FRAME_HEADER_SIZE
            if (incomingTotal <= 1 ||
                incomingTotal > MAX_WIRE_PAYLOAD_BYTES ||
                incomingOffset + chunkSize > incomingTotal
            ) {
                reset()
                return null
            }
            if (incomingOffset == 0) {
                requestId = incomingRequestId
                totalLength = incomingTotal
                bytes = ByteArrayOutputStream(incomingTotal)
            } else if (requestId != incomingRequestId || totalLength != incomingTotal || bytes.size() != incomingOffset) {
                reset()
                return null
            }
            bytes.write(frame, FRAME_HEADER_SIZE, chunkSize)
            if (bytes.size() != totalLength) return null

            val completedRequestId = checkNotNull(requestId)
            val payload = bytes.toByteArray()
            reset()
            if (payload.firstOrNull() != security.mode) return null
            val plaintext = security.open(payload.copyOfRange(1, payload.size)) ?: return null
            return decodeEnvelope(completedRequestId, plaintext)
        }

        fun reset() {
            requestId = null
            totalLength = 0
            bytes = ByteArrayOutputStream()
        }
    }

    private fun decodeEnvelope(
        requestId: Long,
        bytes: ByteArray,
    ): BleRendezvousInvite? {
        if (bytes.size < ENVELOPE_FIXED_BYTES || bytes[0] != ENVELOPE_VERSION || bytes[1] != ENVELOPE_TYPE_INVITE) {
            return null
        }
        val peerKey =
            String(bytes, 2, PEER_KEY_BYTES, StandardCharsets.US_ASCII).let(DiscoveryPeerRegistry::normalizePeerKey)
                ?: return null
        val nameLength = getUnsignedShort(bytes, 2 + PEER_KEY_BYTES)
        val inviteLength = getUnsignedShort(bytes, 4 + PEER_KEY_BYTES)
        if (nameLength > MAX_DISPLAY_NAME_BYTES ||
            inviteLength == 0 ||
            inviteLength > MAX_INVITE_BYTES ||
            ENVELOPE_FIXED_BYTES + nameLength + inviteLength != bytes.size
        ) {
            return null
        }
        val name =
            decodeUtf8(bytes, ENVELOPE_FIXED_BYTES, nameLength)
                ?.let(DiscoveryPeerRegistry::sanitizeDisplayName)
        val invite = decodeUtf8(bytes, ENVELOPE_FIXED_BYTES + nameLength, inviteLength)?.trim() ?: return null
        if (!supportedInvite(invite)) return null
        return BleRendezvousInvite(
            requestId = requestId.toULong().toString(16).padStart(16, '0'),
            senderPeerKey = peerKey,
            senderDisplayName = name,
            invite = invite,
        )
    }

    private fun boundedUtf8(
        value: String,
        maximumBytes: Int,
    ): ByteArray {
        val result = ByteArrayOutputStream(maximumBytes)
        for (character in value.trim()) {
            val encoded = character.toString().toByteArray(StandardCharsets.UTF_8)
            if (result.size() + encoded.size > maximumBytes) break
            result.write(encoded)
        }
        return result.toByteArray()
    }

    private fun supportedInvite(invite: String): Boolean {
        val normalized = invite.trim()
        return INVITE_PREFIXES.any { prefix ->
            normalized.startsWith(prefix, ignoreCase = true)
        }
    }

    private fun decodeUtf8(
        bytes: ByteArray,
        offset: Int,
        length: Int,
    ): String? =
        runCatching {
            val decoder = StandardCharsets.UTF_8.newDecoder()
            decoder.decode(java.nio.ByteBuffer.wrap(bytes, offset, length)).toString()
        }.getOrNull()

    private fun ByteArrayOutputStream.writeShort(value: Int) {
        write((value ushr 8) and 0xff)
        write(value and 0xff)
    }

    private fun putUnsignedShort(
        target: ByteArray,
        offset: Int,
        value: Int,
    ) {
        target[offset] = ((value ushr 8) and 0xff).toByte()
        target[offset + 1] = (value and 0xff).toByte()
    }

    private fun getUnsignedShort(
        source: ByteArray,
        offset: Int,
    ): Int = ((source[offset].toInt() and 0xff) shl 8) or (source[offset + 1].toInt() and 0xff)

    private fun putLong(
        target: ByteArray,
        offset: Int,
        value: Long,
    ) {
        for (index in 0 until Long.SIZE_BYTES) {
            target[offset + index] = (value ushr (56 - index * 8)).toByte()
        }
    }

    private fun getLong(
        source: ByteArray,
        offset: Int,
    ): Long {
        var value = 0L
        for (index in 0 until Long.SIZE_BYTES) {
            value = (value shl 8) or (source[offset + index].toLong() and 0xff)
        }
        return value
    }
}
