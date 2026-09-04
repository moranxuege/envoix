package dev.envoix.app

import dev.envoix.app.ffi.EnvoixRuntimeSettings
import dev.envoix.app.ffi.FfiManifestEntryKindV2
import dev.envoix.app.ffi.FfiManifestOfferPageV2
import dev.envoix.app.ffi.FfiManifestOfferSummaryV2
import dev.envoix.app.ffi.FfiManifestV2Cancellation
import dev.envoix.app.ffi.FfiPathPolicy
import dev.envoix.app.ffi.FfiPendingManifestV2Receive
import dev.envoix.app.ffi.FfiPlatformManifestV2Completion
import dev.envoix.app.ffi.FfiPlatformReceiveDestinationV2
import dev.envoix.app.ffi.FfiRememberedCredentialVault
import dev.envoix.app.ffi.FfiRendezvousPlan
import dev.envoix.app.ffi.FfiTransferDirection
import dev.envoix.app.ffi.FfiTransferMode
import dev.envoix.app.ffi.FfiTransferRequest
import dev.envoix.app.ffi.ManifestV2PlatformDestination
import dev.envoix.app.ffi.TransferObserver
import dev.envoix.app.ffi.envoixCoreInfo
import dev.envoix.app.ffi.receiveTransferOfferV2
import java.util.concurrent.atomic.AtomicBoolean

internal data class InvitationManifestV2ReceiveRequest(
    val stateDirectory: String,
    val language: String,
    val broker: String,
    val relay: String,
    val invitationReference: String,
    val creator: Boolean,
    val rememberConsent: Boolean,
)

internal data class RememberedManifestV2ReceiveRequest(
    val stateDirectory: String,
    val language: String,
    val broker: String,
    val relay: String,
    val credentialReference: String,
    val generation: Long,
    val previousGeneration: Long?,
)

internal data class ManifestV2ReceiveOffer(
    val jobId: String,
    val generation: Long,
    val selectionRevision: Long,
    val rootCount: Int,
    val fileCount: Int,
    val directoryCount: Int,
    val totalBytes: Long,
    val exceptional: Boolean,
    val inventoryPreview: List<TransferInventoryEntry>,
    val inventoryHasMore: Boolean,
)

internal data class ManifestV2ReceiveDestination(
    val verifiedStagingDirectory: String,
    val verifiedStagingAllocatableBytes: Long,
    val exceptionalTransferApproved: Boolean,
)

internal data class ManifestV2SavedRoot(
    val rootId: Int,
    val finalName: String,
    val uri: String,
)

internal data class ManifestV2ReceiveCompletion(
    val jobId: String,
    val totalBytes: Long,
    val savedRoots: List<ManifestV2SavedRoot>,
)

internal interface ManifestV2ReceiveNativePending : AutoCloseable {
    fun summary(): FfiManifestOfferSummaryV2

    fun listEntries(
        offset: UInt,
        limit: UInt,
    ): FfiManifestOfferPageV2

    suspend fun receive(
        destination: FfiPlatformReceiveDestinationV2,
        platformDestination: ManifestV2PlatformDestination,
        observer: TransferObserver,
    ): FfiPlatformManifestV2Completion

    fun cancel()
}

internal interface ManifestV2ReceiveNativeCore {
    fun newCancellation(): ManifestV2SessionCancellation

    suspend fun receiveOffer(
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        stateDirectory: String,
        cancellation: ManifestV2SessionCancellation,
        credentialVault: FfiRememberedCredentialVault,
        observer: TransferObserver,
    ): ManifestV2ReceiveNativePending
}

internal class ManifestV2PendingReceive(
    private val native: ManifestV2ReceiveNativePending,
    val offer: ManifestV2ReceiveOffer,
) : AutoCloseable {
    private val closed = AtomicBoolean(false)

    suspend fun receive(
        destination: ManifestV2ReceiveDestination,
        platformDestination: ManifestV2PlatformDestination,
        observer: ManifestV2SessionObserver,
    ): ManifestV2ReceiveCompletion {
        check(!closed.get()) { "Manifest v2 pending receive is closed" }
        destination.validate()
        val completion =
            native.receive(
                destination =
                    FfiPlatformReceiveDestinationV2(
                        verifiedStagingDirectory = destination.verifiedStagingDirectory,
                        verifiedStagingAllocatableBytes =
                            destination.verifiedStagingAllocatableBytes.toULong(),
                        exceptionalTransferApproved = destination.exceptionalTransferApproved,
                    ),
                platformDestination = platformDestination,
                observer = UniFfiManifestV2Observer(observer),
            )
        check(completion.transfer.jobId == offer.jobId) {
            "Manifest v2 completion job does not match its authenticated offer"
        }
        return ManifestV2ReceiveCompletion(
            jobId = completion.transfer.jobId,
            totalBytes = completion.transfer.totalPlaintextBytes.checkedLong("completion bytes"),
            savedRoots =
                completion.savedRoots.map { root ->
                    ManifestV2SavedRoot(
                        rootId = root.rootId.checkedInt("saved root ID"),
                        finalName = root.finalName,
                        uri = root.uri,
                    )
                },
        )
    }

    fun cancel() {
        if (!closed.get()) native.cancel()
    }

    override fun close() {
        if (closed.compareAndSet(false, true)) native.close()
    }
}

internal class ManifestV2ReceiveGateway(
    private val native: ManifestV2ReceiveNativeCore = UniFfiManifestV2ReceiveNativeCore,
) {
    fun newCancellation(): ManifestV2SessionCancellation = native.newCancellation()

    suspend fun receiveInvitationOffer(
        request: InvitationManifestV2ReceiveRequest,
        cancellation: ManifestV2SessionCancellation,
        credentialVault: ManifestV2RememberedCredentialVault,
        observer: ManifestV2SessionObserver,
    ): ManifestV2PendingReceive {
        request.validate()
        return receiveOffer(
            stateDirectory = request.stateDirectory,
            settings = manifestV2RuntimeSettings(request.language, request.broker, request.relay),
            request = request.transferRequest(),
            cancellation = cancellation,
            credentialVault = credentialVault,
            observer = observer,
        )
    }

    suspend fun receiveRememberedOffer(
        request: RememberedManifestV2ReceiveRequest,
        cancellation: ManifestV2SessionCancellation,
        credentialVault: ManifestV2RememberedCredentialVault,
        observer: ManifestV2SessionObserver,
    ): ManifestV2PendingReceive {
        request.validate()
        return receiveOffer(
            stateDirectory = request.stateDirectory,
            settings = manifestV2RuntimeSettings(request.language, request.broker, request.relay),
            request = request.transferRequest(),
            cancellation = cancellation,
            credentialVault = credentialVault,
            observer = observer,
        )
    }

    private suspend fun receiveOffer(
        stateDirectory: String,
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        cancellation: ManifestV2SessionCancellation,
        credentialVault: ManifestV2RememberedCredentialVault,
        observer: ManifestV2SessionObserver,
    ): ManifestV2PendingReceive {
        val pending =
            native.receiveOffer(
                settings = settings,
                request = request,
                stateDirectory = stateDirectory,
                cancellation = cancellation,
                credentialVault = UniFfiRememberedCredentialVault(credentialVault),
                observer = UniFfiManifestV2Observer(observer),
            )
        return try {
            ManifestV2PendingReceive(
                native = pending,
                offer = projectOffer(pending.summary(), pending.listEntries(0u, INVENTORY_PREVIEW_LIMIT)),
            )
        } catch (error: Throwable) {
            pending.close()
            throw error
        }
    }

    companion object {
        private const val INVENTORY_PREVIEW_LIMIT = 128u
        val shared = ManifestV2ReceiveGateway()
    }
}

private object UniFfiManifestV2ReceiveNativeCore : ManifestV2ReceiveNativeCore {
    private val compatibleBinding by lazy {
        val info = envoixCoreInfo()
        check(
            info.ffiApiVersion == EXPECTED_FFI_API_VERSION &&
                REQUIRED_CAPABILITIES.all(info.capabilities::contains),
        ) {
            "Unsupported Envoix Manifest v2 receive binding: FFI ${info.ffiApiVersion}"
        }
        true
    }

    override fun newCancellation(): ManifestV2SessionCancellation {
        requireCompatibleBinding()
        return UniFfiManifestV2SessionCancellation(FfiManifestV2Cancellation())
    }

    override suspend fun receiveOffer(
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        stateDirectory: String,
        cancellation: ManifestV2SessionCancellation,
        credentialVault: FfiRememberedCredentialVault,
        observer: TransferObserver,
    ): ManifestV2ReceiveNativePending {
        requireCompatibleBinding()
        check(cancellation is UniFfiManifestV2SessionCancellation) {
            "Manifest v2 cancellation belongs to another native core"
        }
        return UniFfiManifestV2ReceiveNativePending(
            receiveTransferOfferV2(
                settings = settings,
                request = request,
                stateDirectory = stateDirectory,
                cancellation = cancellation.value,
                credentialVault = credentialVault,
                observer = observer,
            ),
        )
    }

    private fun requireCompatibleBinding() {
        check(compatibleBinding)
    }

    private val REQUIRED_CAPABILITIES =
        setOf(
            "manifest_v2_session",
            "canonical_failure_projection_v1",
            "platform_manifest_v2_destination_v1",
            "typed_remembered_credential_vault_v1",
        )
}

private class UniFfiManifestV2ReceiveNativePending(
    private val value: FfiPendingManifestV2Receive,
) : ManifestV2ReceiveNativePending {
    override fun summary(): FfiManifestOfferSummaryV2 = value.summary()

    override fun listEntries(
        offset: UInt,
        limit: UInt,
    ): FfiManifestOfferPageV2 = value.listEntries(offset, limit)

    override suspend fun receive(
        destination: FfiPlatformReceiveDestinationV2,
        platformDestination: ManifestV2PlatformDestination,
        observer: TransferObserver,
    ): FfiPlatformManifestV2Completion = value.receiveWithPlatformDestination(destination, platformDestination, observer)

    override fun cancel() = value.cancel()

    override fun close() = value.close()
}

private fun InvitationManifestV2ReceiveRequest.validate() {
    require(stateDirectory.isNotBlank()) { "Manifest v2 state directory is required" }
    require(broker.isNotBlank()) { "Invitation Manifest v2 receive requires a broker" }
    require(invitationReference.isNotBlank()) { "Manifest v2 invitation reference is required" }
}

private fun RememberedManifestV2ReceiveRequest.validate() {
    require(stateDirectory.isNotBlank()) { "Manifest v2 state directory is required" }
    require(broker.isNotBlank()) { "Remembered Manifest v2 receive requires a broker" }
    require(credentialReference.isNotBlank()) { "Remembered Manifest v2 credential reference is required" }
    require(generation >= 0L) { "Remembered Manifest v2 generation cannot be negative" }
    require(previousGeneration == null || previousGeneration >= 0L) {
        "Remembered Manifest v2 previous generation cannot be negative"
    }
}

private fun InvitationManifestV2ReceiveRequest.transferRequest() =
    FfiTransferRequest(
        direction = FfiTransferDirection.RECEIVE,
        mode = if (creator) FfiTransferMode.ROOM else FfiTransferMode.INVITE,
        peerDescriptor = "",
        invite = if (creator) "" else invitationReference,
        code = if (creator) invitationReference else "",
        token = "",
        rememberConsent = rememberConsent,
        rememberedCredentialRef = "",
        rememberedGeneration = 0uL,
        rememberedPreviousGeneration = null,
        broker = broker,
        relay = relay,
        configPath = "",
        pathPolicy = FfiPathPolicy.AUTO,
        rendezvous = roomRendezvous(),
    )

private fun RememberedManifestV2ReceiveRequest.transferRequest() =
    FfiTransferRequest(
        direction = FfiTransferDirection.RECEIVE,
        mode = FfiTransferMode.REMEMBERED,
        peerDescriptor = "",
        invite = "",
        code = "",
        token = "",
        rememberConsent = false,
        rememberedCredentialRef = credentialReference,
        rememberedGeneration = generation.toULong(),
        rememberedPreviousGeneration = previousGeneration?.toULong(),
        broker = broker,
        relay = relay,
        configPath = "",
        pathPolicy = FfiPathPolicy.AUTO,
        rendezvous = roomRendezvous(),
    )

private fun roomRendezvous() =
    FfiRendezvousPlan(
        useRoom = true,
        useMdns = false,
        internetAvailable = true,
    )

private fun projectOffer(
    summary: FfiManifestOfferSummaryV2,
    page: FfiManifestOfferPageV2,
) = ManifestV2ReceiveOffer(
    jobId = summary.jobId,
    generation = summary.generation.toLong(),
    selectionRevision = summary.selectionRevision.checkedLong("selection revision"),
    rootCount = summary.rootCount.checkedInt("root count"),
    fileCount = summary.fileCount.checkedInt("file count"),
    directoryCount = summary.directoryCount.checkedInt("directory count"),
    totalBytes = summary.totalPlaintextBytes.checkedLong("offer bytes"),
    exceptional = summary.exceptionalOffer,
    inventoryPreview =
        page.entries.map { entry ->
            TransferInventoryEntry(
                entryId = entry.entryId.checkedInt("entry ID"),
                parentEntryId = entry.parentEntryId?.checkedInt("parent entry ID"),
                name = entry.name,
                directory = entry.kind == FfiManifestEntryKindV2.DIRECTORY,
                size = entry.plaintextSize.checkedLong("entry bytes"),
            )
        },
    inventoryHasMore = page.nextOffset != null,
)

private fun ManifestV2ReceiveDestination.validate() {
    require(verifiedStagingDirectory.isNotBlank()) { "Verified staging directory is required" }
    require(verifiedStagingAllocatableBytes >= 0L) { "Verified staging capacity cannot be negative" }
}

private fun UInt.checkedInt(name: String): Int =
    takeIf { it <= Int.MAX_VALUE.toUInt() }?.toInt()
        ?: error("Manifest v2 $name exceeded the Android range")

private fun ULong.checkedLong(name: String): Long =
    takeIf { it <= Long.MAX_VALUE.toULong() }?.toLong()
        ?: error("Manifest v2 $name exceeded the Android range")
