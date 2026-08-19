package dev.envoix.app

import dev.envoix.app.ffi.EnvoixRuntimeSettings
import dev.envoix.app.ffi.FfiDestinationCommitReplyV2
import dev.envoix.app.ffi.FfiDestinationCommitRequestV2
import dev.envoix.app.ffi.FfiDestinationPlanReplyV2
import dev.envoix.app.ffi.FfiDestinationPlanRequestV2
import dev.envoix.app.ffi.FfiDestinationSavedRootV2
import dev.envoix.app.ffi.FfiManifestEntryKindV2
import dev.envoix.app.ffi.FfiManifestOfferEntryV2
import dev.envoix.app.ffi.FfiManifestOfferPageV2
import dev.envoix.app.ffi.FfiManifestOfferSummaryV2
import dev.envoix.app.ffi.FfiManifestV2Completion
import dev.envoix.app.ffi.FfiPlatformManifestV2Completion
import dev.envoix.app.ffi.FfiPlatformReceiveDestinationV2
import dev.envoix.app.ffi.FfiRememberedCredentialVault
import dev.envoix.app.ffi.FfiTransferMode
import dev.envoix.app.ffi.FfiTransferRequest
import dev.envoix.app.ffi.ManifestV2PlatformDestination
import dev.envoix.app.ffi.TransferObserver
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class ManifestV2ReceiveGatewayTest {
    @Test
    fun `invitation offer distinguishes creator Room state from joiner invite`() =
        runTest {
            val creatorNative = FakeManifestV2ReceiveNativeCore()
            val creatorCancellation = creatorNative.newCancellation()
            ManifestV2ReceiveGateway(creatorNative)
                .receiveInvitationOffer(
                    invitationRequest("123456-a1b2-c3d4", creator = true),
                    creatorCancellation,
                    RejectingReceiveCredentialVault,
                    NoopManifestV2SessionObserver,
                ).close()

            assertEquals(FfiTransferMode.ROOM, creatorNative.request?.mode)
            assertEquals("123456-a1b2-c3d4", creatorNative.request?.code)
            assertEquals("", creatorNative.request?.invite)
            assertTrue(creatorNative.request?.rememberConsent == true)
            assertSame(creatorCancellation, creatorNative.cancellation)

            val joinerNative = FakeManifestV2ReceiveNativeCore()
            ManifestV2ReceiveGateway(joinerNative)
                .receiveInvitationOffer(
                    invitationRequest("envoix://invite/v2/secret", creator = false),
                    joinerNative.newCancellation(),
                    RejectingReceiveCredentialVault,
                    NoopManifestV2SessionObserver,
                ).close()

            assertEquals(FfiTransferMode.INVITE, joinerNative.request?.mode)
            assertEquals("", joinerNative.request?.code)
            assertEquals("envoix://invite/v2/secret", joinerNative.request?.invite)
        }

    @Test
    fun `remembered offer keeps opaque reference and generations typed`() =
        runTest {
            val native = FakeManifestV2ReceiveNativeCore()
            ManifestV2ReceiveGateway(native)
                .receiveRememberedOffer(
                    request =
                        RememberedManifestV2ReceiveRequest(
                            stateDirectory = "/tmp/state",
                            language = "zh",
                            broker = "broker.example",
                            relay = "relay.example",
                            credentialReference = "credential-ref",
                            generation = 8,
                            previousGeneration = 7,
                        ),
                    cancellation = native.newCancellation(),
                    credentialVault = RejectingReceiveCredentialVault,
                    observer = NoopManifestV2SessionObserver,
                ).close()

            assertEquals(FfiTransferMode.REMEMBERED, native.request?.mode)
            assertEquals("credential-ref", native.request?.rememberedCredentialRef)
            assertEquals(8uL, native.request?.rememberedGeneration)
            assertEquals(7uL, native.request?.rememberedPreviousGeneration)
            assertEquals("zh", native.settings?.language)
        }

    @Test
    fun `offer projection is bounded and completion waits for the platform gate`() =
        runTest {
            val native = FakeManifestV2ReceiveNativeCore()
            val pending =
                ManifestV2ReceiveGateway(native).receiveInvitationOffer(
                    invitationRequest("invite", creator = false),
                    native.newCancellation(),
                    RejectingReceiveCredentialVault,
                    NoopManifestV2SessionObserver,
                )

            assertEquals(JOB_ID, pending.offer.jobId)
            assertEquals(1, pending.offer.rootCount)
            assertEquals(
                "report.txt",
                pending.offer.inventoryPreview
                    .single()
                    .name,
            )
            assertTrue(pending.offer.inventoryHasMore)

            val receive =
                async {
                    pending.receive(
                        destination =
                            ManifestV2ReceiveDestination(
                                verifiedStagingDirectory = "/tmp/verified",
                                verifiedStagingAllocatableBytes = 4096,
                                exceptionalTransferApproved = true,
                            ),
                        platformDestination = NoopPlatformDestination,
                        observer = NoopManifestV2SessionObserver,
                    )
                }
            assertFalse(receive.isCompleted)
            native.pending.completion.complete(completion())

            val completed = receive.await()
            assertEquals(42L, completed.totalBytes)
            assertEquals("content://downloads/report", completed.savedRoots.single().uri)
            assertEquals(4096uL, native.pending.destination?.verifiedStagingAllocatableBytes)
            assertFalse(native.pending.closed)

            pending.close()
            pending.close()
            assertEquals(1, native.pending.closeCount)
        }

    @Test
    fun `invalid native offer closes its pending handle`() =
        runTest {
            val native =
                FakeManifestV2ReceiveNativeCore(
                    pending =
                        FakeManifestV2ReceiveNativePending(
                            offerSummary = summary(totalBytes = ULong.MAX_VALUE),
                        ),
                )

            val result =
                runCatching {
                    ManifestV2ReceiveGateway(native).receiveInvitationOffer(
                        invitationRequest("invite", creator = false),
                        native.newCancellation(),
                        RejectingReceiveCredentialVault,
                        NoopManifestV2SessionObserver,
                    )
                }

            assertTrue(result.isFailure)
            assertEquals(1, native.pending.closeCount)
        }

    private fun invitationRequest(
        reference: String,
        creator: Boolean,
    ) = InvitationManifestV2ReceiveRequest(
        stateDirectory = "/tmp/state",
        language = "en",
        broker = "broker.example",
        relay = "relay.example",
        invitationReference = reference,
        creator = creator,
        rememberConsent = true,
    )

    private companion object {
        const val JOB_ID = "00112233445566778899aabbccddeeff"
    }
}

private class FakeManifestV2ReceiveNativeCore(
    val pending: FakeManifestV2ReceiveNativePending = FakeManifestV2ReceiveNativePending(),
) : ManifestV2ReceiveNativeCore {
    var settings: EnvoixRuntimeSettings? = null
    var request: FfiTransferRequest? = null
    var cancellation: ManifestV2SessionCancellation? = null
    var observer: TransferObserver? = null

    override fun newCancellation(): ManifestV2SessionCancellation = FakeManifestV2ReceiveCancellation()

    override suspend fun receiveOffer(
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        stateDirectory: String,
        cancellation: ManifestV2SessionCancellation,
        credentialVault: FfiRememberedCredentialVault,
        observer: TransferObserver,
    ): ManifestV2ReceiveNativePending {
        this.settings = settings
        this.request = request
        this.cancellation = cancellation
        this.observer = observer
        return pending
    }
}

private class FakeManifestV2ReceiveNativePending(
    private val offerSummary: FfiManifestOfferSummaryV2 = summary(),
) : ManifestV2ReceiveNativePending {
    val completion = CompletableDeferred<FfiPlatformManifestV2Completion>()
    var destination: FfiPlatformReceiveDestinationV2? = null
    var cancelCount = 0
    var closeCount = 0
    val closed: Boolean get() = closeCount > 0

    override fun summary(): FfiManifestOfferSummaryV2 = offerSummary

    override fun listEntries(
        offset: UInt,
        limit: UInt,
    ) = FfiManifestOfferPageV2(
        entries =
            listOf(
                FfiManifestOfferEntryV2(
                    entryId = 0u,
                    rootId = 0u,
                    parentEntryId = null,
                    name = "report.txt",
                    kind = FfiManifestEntryKindV2.FILE,
                    plaintextSize = 42uL,
                    digestKnown = true,
                ),
            ),
        nextOffset = 1u,
    )

    override suspend fun receive(
        destination: FfiPlatformReceiveDestinationV2,
        platformDestination: ManifestV2PlatformDestination,
        observer: TransferObserver,
    ): FfiPlatformManifestV2Completion {
        this.destination = destination
        return completion.await()
    }

    override fun cancel() {
        cancelCount += 1
    }

    override fun close() {
        closeCount += 1
    }
}

private class FakeManifestV2ReceiveCancellation : ManifestV2SessionCancellation {
    override fun cancel() = Unit

    override fun close() = Unit
}

private object NoopPlatformDestination : ManifestV2PlatformDestination {
    override suspend fun plan(request: FfiDestinationPlanRequestV2) = FfiDestinationPlanReplyV2(emptyList())

    override suspend fun commit(request: FfiDestinationCommitRequestV2) = FfiDestinationCommitReplyV2(emptyList())
}

private object NoopManifestV2SessionObserver : ManifestV2SessionObserver {
    override fun onStarted(
        itemCount: Long,
        totalBytes: Long,
    ) = Unit

    override fun onPhase(status: Status) = Unit

    override fun onProgress(
        transferred: Long,
        total: Long,
    ) = Unit

    override fun onFailure(failure: ManifestV2SessionFailure) = Unit

    override fun onConnectionPath(path: ConnectionPathKind) = Unit

    override fun onStageTiming(timing: TransferStageTiming) = Unit

    override fun onDiagnostic(message: String) = Unit
}

private object RejectingReceiveCredentialVault : ManifestV2RememberedCredentialVault {
    override fun storeRememberedCredential(
        opaqueCredential: ByteArray,
        generation: Long,
    ): Boolean = false
}

private fun summary(totalBytes: ULong = 42uL) =
    FfiManifestOfferSummaryV2(
        jobId = "00112233445566778899aabbccddeeff",
        generation = 1u,
        selectionRevision = 1uL,
        rootCount = 1u,
        fileCount = 1u,
        directoryCount = 0u,
        totalPlaintextBytes = totalBytes,
        exceptionalOffer = false,
    )

private fun completion() =
    FfiPlatformManifestV2Completion(
        transfer =
            FfiManifestV2Completion(
                jobId = "00112233445566778899aabbccddeeff",
                entryCount = 1u,
                totalPlaintextBytes = 42uL,
                deliveryProofDigest = ByteArray(32),
                savedPaths = emptyList(),
            ),
        savedRoots =
            listOf(
                FfiDestinationSavedRootV2(
                    rootId = 0u,
                    finalName = "report.txt",
                    uri = "content://downloads/report",
                ),
            ),
    )
