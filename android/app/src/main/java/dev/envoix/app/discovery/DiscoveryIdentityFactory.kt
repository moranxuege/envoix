package dev.envoix.app.discovery

import android.os.Build
import java.security.SecureRandom

internal data class LocalDiscoveryIdentity(
    val peerKey: String,
    val displayName: String,
)

internal object DiscoveryIdentityFactory {
    fun create(
        displayName: String = Build.MODEL,
        fillRandomBytes: (ByteArray) -> Unit = SecureRandom()::nextBytes,
    ): LocalDiscoveryIdentity {
        val peerKey =
            ByteArray(DiscoveryPeerRegistry.PEER_KEY_HEX_LENGTH / 2)
                .also(fillRandomBytes)
                .joinToString(separator = "") { byte -> "%02x".format(byte.toInt() and 0xff) }
        val normalizedName = DiscoveryPeerRegistry.sanitizeDisplayName(displayName) ?: "Android device"
        return LocalDiscoveryIdentity(peerKey = peerKey, displayName = normalizedName)
    }
}
