package dev.envoix.app.ui

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.emptyFlow

internal data class RoomControlInvite(
    val code: String,
    val payload: String,
    val expiresAtEpochMs: Long,
)

internal data class RoomTransferOffer(
    val id: String,
    val transferInvite: String,
    val rootNames: List<String>,
    val itemCount: Int,
    val totalBytes: Long,
)

internal data class RoomTransferOfferDraft(
    val id: String,
    val transferInvite: String,
    val rootNames: List<String>,
    val itemCount: Int,
    val totalBytes: Long,
)

internal enum class RoomLifetimePolicy {
    Idle15Minutes,
    UntilForegroundEnds,
}

internal data class RoomLifetimeSnapshot(
    val revision: Long,
    val policy: RoomLifetimePolicy,
    val idleDeadlineEpochMs: Long?,
)

internal enum class RoomCloseReason {
    UserEnded,
    IdleExpired,
    InvitationExpired,
    PeerEnded,
    Backgrounded,
    NetworkLost,
    ProtocolFailure,
}

internal sealed interface RoomControlEvent {
    data class Hosting(
        val invite: RoomControlInvite,
    ) : RoomControlEvent

    data object Joining : RoomControlEvent

    data class Connected(
        val peerName: String?,
        val creator: Boolean,
        val lifetime: RoomLifetimeSnapshot,
    ) : RoomControlEvent

    data class IncomingOffer(
        val offer: RoomTransferOffer,
    ) : RoomControlEvent

    data class OfferAccepted(
        val offerId: String,
    ) : RoomControlEvent

    data class OfferRejected(
        val offerId: String,
        val reason: String?,
    ) : RoomControlEvent

    data class CommandFailed(
        val command: String,
        val offerId: String?,
        val message: String,
    ) : RoomControlEvent

    data class LifetimeChanged(
        val lifetime: RoomLifetimeSnapshot,
    ) : RoomControlEvent

    data class Closed(
        val reason: RoomCloseReason,
    ) : RoomControlEvent

    data class Failed(
        val message: String,
    ) : RoomControlEvent
}

/**
 * Native-facing boundary for the foreground room-control session.
 *
 * Implementations must emit [RoomControlEvent.Connected] only after the peer is
 * authenticated. UI navigation is never treated as proof of a connection.
 */
internal interface RoomControlGateway {
    val available: Boolean
    val events: Flow<RoomControlEvent>

    suspend fun host(
        displayName: String,
        broker: String,
        relay: String,
    )

    suspend fun join(
        input: String,
        displayName: String,
    )

    suspend fun refreshInvite()

    suspend fun offerTransfer(draft: RoomTransferOfferDraft)

    suspend fun respondToOffer(
        offerId: String,
        accept: Boolean,
    )

    suspend fun updatePolicy(policy: RoomLifetimePolicy)

    suspend fun updateTransferActive(active: Boolean)

    suspend fun close(reason: RoomCloseReason)
}

internal object UnavailableRoomControlGateway : RoomControlGateway {
    override val available: Boolean = false
    override val events: Flow<RoomControlEvent> = emptyFlow()

    override suspend fun host(
        displayName: String,
        broker: String,
        relay: String,
    ) = Unit

    override suspend fun join(
        input: String,
        displayName: String,
    ) = Unit

    override suspend fun refreshInvite() = Unit

    override suspend fun offerTransfer(draft: RoomTransferOfferDraft) = Unit

    override suspend fun respondToOffer(
        offerId: String,
        accept: Boolean,
    ) = Unit

    override suspend fun updatePolicy(policy: RoomLifetimePolicy) = Unit

    override suspend fun updateTransferActive(active: Boolean) = Unit

    override suspend fun close(reason: RoomCloseReason) = Unit
}

/**
 * Replaced by the JNI-backed adapter at application startup. Keeping the
 * default unavailable makes unsupported builds fail closed instead of showing
 * a fabricated Connected room.
 */
internal object RoomControlGatewayProvider {
    var gateway: RoomControlGateway = UnavailableRoomControlGateway
}

internal object RoomControlInviteFormat {
    private val humanCode = Regex("""(?i)^R\d{6}-[a-z0-9]+-[a-z0-9]+$""")

    fun looksLikeRoomInvite(input: String): Boolean {
        val normalized = input.trim()
        return normalized.startsWith("envoix://room/", ignoreCase = true) ||
            humanCode.matches(normalized)
    }
}
