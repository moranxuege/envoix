package dev.envoix.app.ui

import dev.envoix.app.EXPECTED_FFI_API_VERSION
import dev.envoix.app.ffi.FfiRememberedRoomConnectMode
import dev.envoix.app.ffi.FfiRoomCloseReason
import dev.envoix.app.ffi.FfiRoomConnectMode
import dev.envoix.app.ffi.FfiRoomControlCancellation
import dev.envoix.app.ffi.FfiRoomControlEvent
import dev.envoix.app.ffi.FfiRoomControlInvite
import dev.envoix.app.ffi.FfiRoomControlSession
import dev.envoix.app.ffi.FfiRoomControlSnapshot
import dev.envoix.app.ffi.FfiRoomLifetimePolicy
import dev.envoix.app.ffi.FfiRoomLifetimeState
import dev.envoix.app.ffi.FfiRoomOfferRejection
import dev.envoix.app.ffi.FfiRoomTransferOffer
import dev.envoix.app.ffi.connectRememberedRoomControlSession
import dev.envoix.app.ffi.connectRoomControlSession
import dev.envoix.app.ffi.envoixCoreInfo
import dev.envoix.app.ffi.makeRoomControlInvite
import dev.envoix.app.ffi.parseRoomControlInvite

internal data class RoomControlConnectionRequest(
    val input: String,
    val displayName: String,
    val mode: FfiRoomConnectMode,
    val verifiedPairing: Boolean,
    val identityPath: String,
    val fallbackBroker: String,
    val fallbackRelay: String,
)

internal data class RememberedRoomControlConnectionRequest(
    val credentialReference: String,
    val generation: ULong,
    val displayName: String,
    val mode: FfiRememberedRoomConnectMode,
    val identityPath: String,
    val broker: String,
    val relay: String,
)

/** The generated UniFFI surface behind a small lifecycle-aware test seam. */
internal interface RoomControlNativeCore {
    fun makeInvitation(
        broker: String,
        relay: String,
    ): FfiRoomControlInvite

    fun parseInvitation(
        input: String,
        fallbackBroker: String,
        fallbackRelay: String,
    ): FfiRoomControlInvite

    fun newCancellation(): RoomControlNativeCancellation

    suspend fun connect(
        request: RoomControlConnectionRequest,
        cancellation: RoomControlNativeCancellation,
    ): RoomControlNativeSession

    suspend fun connectRemembered(
        request: RememberedRoomControlConnectionRequest,
        cancellation: RoomControlNativeCancellation,
    ): RoomControlNativeSession
}

internal interface RoomControlNativeCancellation : AutoCloseable {
    fun cancel()
}

internal interface RoomControlNativeSession : AutoCloseable {
    fun snapshot(): FfiRoomControlSnapshot

    suspend fun nextEvent(): FfiRoomControlEvent

    suspend fun offerTransfer(offer: FfiRoomTransferOffer): FfiRoomLifetimeState?

    suspend fun acceptOffer(offerId: String): FfiRoomLifetimeState?

    suspend fun rejectOffer(
        offerId: String,
        reason: FfiRoomOfferRejection,
    ): FfiRoomLifetimeState?

    suspend fun setPolicy(policy: FfiRoomLifetimePolicy): FfiRoomLifetimeState?

    suspend fun setLocalTransferActive(active: Boolean): FfiRoomLifetimeState?

    suspend fun close(reason: FfiRoomCloseReason)
}

internal object UniFfiRoomControlNativeCore : RoomControlNativeCore {
    private val compatibleBinding by lazy {
        val info = envoixCoreInfo()
        check(
            info.ffiApiVersion == EXPECTED_FFI_API_VERSION &&
                ROOM_CONTROL_CAPABILITY in info.capabilities &&
                REMEMBERED_ROOM_CONTROL_CAPABILITY in info.capabilities &&
                ROOM_CONTROL_ERROR_CAPABILITY in info.capabilities,
        ) {
            "Unsupported Envoix Room binding: FFI ${info.ffiApiVersion}"
        }
        true
    }

    override fun makeInvitation(
        broker: String,
        relay: String,
    ): FfiRoomControlInvite {
        requireCompatibleBinding()
        return makeRoomControlInvite(broker, relay)
    }

    override fun parseInvitation(
        input: String,
        fallbackBroker: String,
        fallbackRelay: String,
    ): FfiRoomControlInvite {
        requireCompatibleBinding()
        return parseRoomControlInvite(input, fallbackBroker, fallbackRelay)
    }

    override fun newCancellation(): RoomControlNativeCancellation {
        requireCompatibleBinding()
        return UniFfiRoomControlCancellation()
    }

    override suspend fun connect(
        request: RoomControlConnectionRequest,
        cancellation: RoomControlNativeCancellation,
    ): RoomControlNativeSession {
        val token = (cancellation as UniFfiRoomControlCancellation).value
        return UniFfiRoomControlSession(
            connectRoomControlSession(
                input = request.input,
                displayName = request.displayName,
                mode = request.mode,
                verifiedPairing = request.verifiedPairing,
                identityPath = request.identityPath,
                fallbackBroker = request.fallbackBroker,
                fallbackRelay = request.fallbackRelay,
                cancellation = token,
            ),
        )
    }

    override suspend fun connectRemembered(
        request: RememberedRoomControlConnectionRequest,
        cancellation: RoomControlNativeCancellation,
    ): RoomControlNativeSession {
        val token = (cancellation as UniFfiRoomControlCancellation).value
        return UniFfiRoomControlSession(
            connectRememberedRoomControlSession(
                rememberedCredentialRef = request.credentialReference,
                rememberedGeneration = request.generation,
                displayName = request.displayName,
                mode = request.mode,
                identityPath = request.identityPath,
                broker = request.broker,
                relay = request.relay,
                cancellation = token,
            ),
        )
    }

    private const val ROOM_CONTROL_CAPABILITY = "foreground_room_control_v5"
    private const val REMEMBERED_ROOM_CONTROL_CAPABILITY = "remembered_room_control_v1"
    private const val ROOM_CONTROL_ERROR_CAPABILITY = "typed_room_control_errors_v1"

    private fun requireCompatibleBinding() {
        check(compatibleBinding)
    }
}

private class UniFfiRoomControlCancellation(
    val value: FfiRoomControlCancellation = FfiRoomControlCancellation(),
) : RoomControlNativeCancellation {
    override fun cancel() = value.cancel()

    override fun close() = value.close()
}

private class UniFfiRoomControlSession(
    private val value: FfiRoomControlSession,
) : RoomControlNativeSession {
    override fun snapshot(): FfiRoomControlSnapshot = value.snapshot()

    override suspend fun nextEvent(): FfiRoomControlEvent = value.nextEvent()

    override suspend fun offerTransfer(offer: FfiRoomTransferOffer): FfiRoomLifetimeState? = value.offerTransfer(offer)

    override suspend fun acceptOffer(offerId: String): FfiRoomLifetimeState? = value.acceptOffer(offerId)

    override suspend fun rejectOffer(
        offerId: String,
        reason: FfiRoomOfferRejection,
    ): FfiRoomLifetimeState? = value.rejectOffer(offerId, reason)

    override suspend fun setPolicy(policy: FfiRoomLifetimePolicy): FfiRoomLifetimeState? = value.setPolicy(policy)

    override suspend fun setLocalTransferActive(active: Boolean): FfiRoomLifetimeState? = value.setLocalTransferActive(active)

    override suspend fun close(reason: FfiRoomCloseReason) = value.close(reason)

    override fun close() = value.close()
}
