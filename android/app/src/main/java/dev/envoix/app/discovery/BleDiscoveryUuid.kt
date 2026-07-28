package dev.envoix.app.discovery

import java.util.UUID

/**
 * BLE discovery carries the install fingerprint in a 128-bit service UUID.
 * Apple peripheral advertising accepts service UUIDs but not service-data payloads,
 * so this representation is intentionally shared-platform friendly.
 */
internal object BleDiscoveryUuid {
    val FILTER_BASE_UUID: UUID = UUID.fromString("d5f3a2d8-8f4a-4b33-0000-000000000000")
    val FILTER_MASK_UUID: UUID = UUID.fromString("ffffffff-ffff-ffff-0000-000000000000")

    fun encode(peerKey: String): UUID? {
        val normalized = DiscoveryPeerRegistry.normalizePeerKey(peerKey) ?: return null
        val leastSignificantBits = java.lang.Long.parseUnsignedLong(normalized, 16)
        return UUID(FILTER_BASE_UUID.mostSignificantBits, leastSignificantBits)
    }

    fun decode(uuid: UUID?): String? {
        if (uuid == null || uuid.mostSignificantBits != FILTER_BASE_UUID.mostSignificantBits) return null
        return "%016x".format(uuid.leastSignificantBits)
    }
}
