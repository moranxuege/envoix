package dev.envoix.app.discovery

import java.nio.ByteBuffer
import java.nio.CharBuffer
import java.nio.charset.CodingErrorAction
import java.nio.charset.StandardCharsets

/**
 * Optional BLE display-name metadata. The peer key remains solely encoded in
 * [BleDiscoveryUuid], so a changed or malformed name cannot change identity.
 */
internal object BleDiscoveryName {
    // A 128-bit service-data field has 18 bytes of overhead in a 31-byte legacy
    // scan response, leaving 13 bytes for a display name.
    const val MAX_SERVICE_DATA_BYTES = 13

    fun encodeServiceData(displayName: String?): ByteArray? = encode(displayName, MAX_SERVICE_DATA_BYTES)

    fun encode(
        displayName: String?,
        maximumBytes: Int,
    ): ByteArray? {
        require(maximumBytes > 0) { "maximumBytes must be positive" }
        val normalized = normalize(displayName) ?: return null
        val bounded = normalized.utf8Prefix(maximumBytes).trimEnd()
        return bounded.takeIf(String::isNotEmpty)?.toByteArray(StandardCharsets.UTF_8)
    }

    fun decode(
        serviceData: ByteArray?,
        localName: String?,
    ): String? = decodeServiceData(serviceData) ?: normalize(localName)

    fun decode(
        value: ByteArray?,
        maximumBytes: Int,
    ): String? {
        require(maximumBytes > 0) { "maximumBytes must be positive" }
        if (value == null || value.isEmpty() || value.size > maximumBytes) return null
        val decoded =
            runCatching {
                StandardCharsets.UTF_8
                    .newDecoder()
                    .onMalformedInput(CodingErrorAction.REPORT)
                    .onUnmappableCharacter(CodingErrorAction.REPORT)
                    .decode(ByteBuffer.wrap(value))
                    .toString()
            }.getOrNull()
        return normalize(decoded)
    }

    private fun decodeServiceData(value: ByteArray?): String? = decode(value, MAX_SERVICE_DATA_BYTES)

    private fun normalize(value: String?): String? {
        if (value == null || !value.hasStrictUtf8Encoding()) return null
        val sanitized =
            DiscoveryPeerRegistry
                .sanitizeDisplayName(value)
                ?.let { candidate ->
                    if (candidate.lastOrNull()?.let(Character::isHighSurrogate) == true) {
                        candidate.dropLast(1)
                    } else {
                        candidate
                    }
                }?.ifBlank { null }
                ?: return null
        return sanitized.takeUnless { candidate ->
            candidate.any { character -> character.isISOControl() } ||
                !candidate.hasStrictUtf8Encoding()
        }
    }
}

private fun String.hasStrictUtf8Encoding(): Boolean =
    runCatching {
        StandardCharsets.UTF_8
            .newEncoder()
            .onMalformedInput(CodingErrorAction.REPORT)
            .onUnmappableCharacter(CodingErrorAction.REPORT)
            .encode(CharBuffer.wrap(this))
    }.isSuccess

private fun String.utf8Prefix(maxBytes: Int): String {
    val result = StringBuilder(length)
    var index = 0
    var usedBytes = 0
    while (index < length) {
        val codePoint = codePointAt(index)
        val character = String(Character.toChars(codePoint))
        val byteCount = character.toByteArray(StandardCharsets.UTF_8).size
        if (usedBytes + byteCount > maxBytes) break
        result.append(character)
        usedBytes += byteCount
        index += Character.charCount(codePoint)
    }
    return result.toString()
}
