package dev.envoix.app

import dev.envoix.app.ffi.EnvoixRuntimeSettings
import dev.envoix.app.ffi.FfiConnectionPathEvent
import dev.envoix.app.ffi.FfiConnectionPathEventKind
import dev.envoix.app.ffi.FfiDataPathKind
import dev.envoix.app.ffi.FfiFailureCategory
import dev.envoix.app.ffi.FfiFailureCode
import dev.envoix.app.ffi.FfiFailureOrigin
import dev.envoix.app.ffi.FfiFailureOutcome
import dev.envoix.app.ffi.FfiFailurePhase
import dev.envoix.app.ffi.FfiFailureSessionDisposition
import dev.envoix.app.ffi.FfiManifestV2Completion
import dev.envoix.app.ffi.FfiManifestV2Phase
import dev.envoix.app.ffi.FfiPathPolicy
import dev.envoix.app.ffi.FfiRecoveryAction
import dev.envoix.app.ffi.FfiTransferDirection
import dev.envoix.app.ffi.FfiTransferFailure
import dev.envoix.app.ffi.FfiTransferMode
import dev.envoix.app.ffi.FfiTransferRequest
import dev.envoix.app.ffi.FfiTransferStage
import dev.envoix.app.ffi.FfiTransferStageTiming
import dev.envoix.app.ffi.TransferObserver
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class ManifestV2SendGatewayTest {
    @Test
    fun `remembered send seals the job and uses a typed Room-only request`() =
        runTest {
            val native = FakeManifestV2SendNativeCore()
            val cancellation = native.newCancellation()

            val completion =
                ManifestV2SendGateway(native).sendRemembered(
                    request(),
                    cancellation,
                    RecordingManifestV2SendObserver(),
                )

            assertEquals(JOB_ID, completion.jobId)
            assertEquals(42L, completion.totalBytes)
            assertEquals("/tmp/jobs", native.restoredStoreDirectory)
            assertEquals(JOB_ID, native.restoredJobId)
            assertEquals(1, native.job.sealCount)
            assertEquals(1, native.job.closeCount)
            assertSame(cancellation, native.sentCancellation)
            assertEquals(FfiTransferDirection.SEND, native.sentRequest?.direction)
            assertEquals(FfiTransferMode.REMEMBERED, native.sentRequest?.mode)
            assertEquals("credential-ref", native.sentRequest?.rememberedCredentialRef)
            assertEquals(7uL, native.sentRequest?.rememberedGeneration)
            assertEquals(6uL, native.sentRequest?.rememberedPreviousGeneration)
            assertTrue(native.sentRequest?.rendezvous?.useRoom == true)
            assertFalse(native.sentRequest?.rendezvous?.useMdns ?: true)
            assertEquals(FfiPathPolicy.AUTO, native.sentRequest?.pathPolicy)
            assertEquals("https://broker.example", native.sentSettings?.serverUrl)
            assertEquals("https://relay.example", native.sentSettings?.relayUrl)
        }

    @Test
    fun `invitation send distinguishes creator Room state from a joiner invite`() =
        runTest {
            val creatorNative = FakeManifestV2SendNativeCore()
            ManifestV2SendGateway(creatorNative).sendInvitation(
                invitationRequest("123456-a1b2-c3d4", creator = true),
                creatorNative.newCancellation(),
                RecordingManifestV2SendObserver(),
            )

            assertEquals(FfiTransferMode.ROOM, creatorNative.sentRequest?.mode)
            assertEquals("123456-a1b2-c3d4", creatorNative.sentRequest?.code)
            assertEquals("", creatorNative.sentRequest?.invite)
            assertTrue(creatorNative.sentRequest?.rememberConsent == true)
            assertEquals(1, creatorNative.job.closeCount)

            val joinerNative = FakeManifestV2SendNativeCore()
            ManifestV2SendGateway(joinerNative).sendInvitation(
                invitationRequest("envoix://invite/v2/secret", creator = false),
                joinerNative.newCancellation(),
                RecordingManifestV2SendObserver(),
            )

            assertEquals(FfiTransferMode.INVITE, joinerNative.sentRequest?.mode)
            assertEquals("", joinerNative.sentRequest?.code)
            assertEquals("envoix://invite/v2/secret", joinerNative.sentRequest?.invite)
            assertEquals(1, joinerNative.job.closeCount)
        }

    @Test
    fun `send failure still closes the restored job`() =
        runTest {
            val native = FakeManifestV2SendNativeCore()
            native.sendResult = { error("native send failed") }

            val failure =
                runCatching {
                    ManifestV2SendGateway(native).sendRemembered(
                        request(),
                        native.newCancellation(),
                        RecordingManifestV2SendObserver(),
                    )
                }

            assertEquals("native send failed", failure.exceptionOrNull()?.message)
            assertEquals(1, native.job.sealCount)
            assertEquals(1, native.job.closeCount)
        }

    @Test
    fun `negative remembered generation is rejected before opening a job`() =
        runTest {
            val native = FakeManifestV2SendNativeCore()

            val failure =
                runCatching {
                    ManifestV2SendGateway(native).sendRemembered(
                        request().copy(generation = -1),
                        native.newCancellation(),
                        RecordingManifestV2SendObserver(),
                    )
                }

            assertTrue(failure.exceptionOrNull() is IllegalArgumentException)
            assertEquals(null, native.restoredJobId)
            assertEquals(0, native.job.closeCount)
        }

    @Test
    fun `observer projects typed facts and defers terminal delivery to native return`() =
        runTest {
            val native = FakeManifestV2SendNativeCore()
            val target = RecordingManifestV2SendObserver()
            native.sendResult = { observer ->
                observer.onStarted(3u, 42uL)
                observer.onPhase(FfiManifestV2Phase.TRANSFERRING)
                observer.onPhase(FfiManifestV2Phase.DELIVERED)
                observer.onProgress(21uL, 42uL)
                observer.onConnectionPath(
                    FfiConnectionPathEvent(
                        pathKind = FfiDataPathKind.DIRECT_IPV6,
                        eventKind = FfiConnectionPathEventKind.SELECTED,
                    ),
                )
                observer.onStageTiming(
                    FfiTransferStageTiming(
                        stage = FfiTransferStage.FIRST_PAYLOAD,
                        direction = FfiTransferDirection.SEND,
                        attemptId = 2uL,
                        transferId = JOB_ID,
                        elapsedUs = 15uL,
                        deltaUs = 5uL,
                    ),
                )
                observer.onTransferFailed(
                    FfiTransferFailure(
                        code = FfiFailureCode.NETWORK_LOST,
                        category = FfiFailureCategory.NETWORK,
                        phase = FfiFailurePhase.CONNECTING,
                        origin = FfiFailureOrigin.PEER,
                        direction = FfiTransferDirection.SEND,
                        retryable = true,
                        recoveryAction = FfiRecoveryAction.RESUME,
                        outcome = FfiFailureOutcome.FAILED,
                        sessionDisposition = FfiFailureSessionDisposition.RETAIN_FOR_RECOVERY,
                        userMessageKey = "transfer.network_lost",
                        diagnosticMessage = "connection closed",
                    ),
                )
                assertTrue(observer.onRememberedCredential(byteArrayOf(1, 2, 3), 8uL))
                observer.onCompleted(42uL)
                completion()
            }

            ManifestV2SendGateway(native).sendRemembered(
                request(),
                native.newCancellation(),
                target,
            )

            assertEquals(listOf(3L to 42L), target.started)
            assertEquals(listOf(Status.Transferring), target.phases)
            assertEquals(listOf(21L to 42L), target.progress)
            assertEquals(listOf(ConnectionPathKind.DirectIpv6), target.paths)
            assertEquals(TransferStage.FirstPayload, target.timings.single().stage)
            assertEquals(2L, target.timings.single().attemptId)
            assertEquals("network_lost", target.failures.single().cause)
            assertTrue(target.failures.single().retryable)
            assertEquals(RecoveryAction.Resume, target.failures.single().recoveryAction)
            assertEquals(FailureOutcome.Failed, target.failures.single().outcome)
            assertEquals(
                FailureSessionDisposition.RetainForRecovery,
                target.failures.single().sessionDisposition,
            )
            assertArrayEquals(byteArrayOf(1, 2, 3), target.rememberedCredential)
            assertEquals(8L, target.rememberedGeneration)
        }

    @Test
    fun `cancellation lifetime is owned by the caller`() {
        val native = FakeManifestV2SendNativeCore()
        val cancellation = ManifestV2SendGateway(native).newCancellation()

        cancellation.cancel()
        cancellation.close()

        assertSame(native.cancellation, cancellation)
        assertEquals(1, native.cancellation.cancelCount)
        assertEquals(1, native.cancellation.closeCount)
    }

    private fun request() =
        RememberedManifestV2SendRequest(
            jobStoreDirectory = "/tmp/jobs",
            jobId = JOB_ID,
            stateDirectory = "/tmp/state",
            language = "en",
            broker = "https://broker.example",
            relay = "https://relay.example",
            credentialReference = "credential-ref",
            generation = 7,
            previousGeneration = 6,
        )

    private fun invitationRequest(
        reference: String,
        creator: Boolean,
    ) = InvitationManifestV2SendRequest(
        jobStoreDirectory = "/tmp/jobs",
        jobId = JOB_ID,
        stateDirectory = "/tmp/state",
        language = "en",
        broker = "https://broker.example",
        relay = "https://relay.example",
        invitationReference = reference,
        creator = creator,
        rememberConsent = true,
    )

    private companion object {
        const val JOB_ID = "00112233445566778899aabbccddeeff"
    }
}

private class FakeManifestV2SendNativeCore : ManifestV2SendNativeCore {
    val job = FakeManifestV2SendNativeJob()
    val cancellation = FakeManifestV2SendCancellation()
    var restoredStoreDirectory: String? = null
    var restoredJobId: String? = null
    var sentSettings: EnvoixRuntimeSettings? = null
    var sentRequest: FfiTransferRequest? = null
    var sentCancellation: ManifestV2SessionCancellation? = null
    var sendResult: suspend (TransferObserver) -> FfiManifestV2Completion = { completion() }

    override fun newCancellation(): ManifestV2SessionCancellation = cancellation

    override suspend fun restoreJob(
        storeDirectory: String,
        jobId: String,
    ): ManifestV2SendNativeJob {
        restoredStoreDirectory = storeDirectory
        restoredJobId = jobId
        return job
    }

    override suspend fun send(
        job: ManifestV2SendNativeJob,
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        stateDirectory: String,
        cancellation: ManifestV2SessionCancellation,
        observer: TransferObserver,
    ): FfiManifestV2Completion {
        assertSame(this.job, job)
        assertEquals("/tmp/state", stateDirectory)
        sentSettings = settings
        sentRequest = request
        sentCancellation = cancellation
        return sendResult(observer)
    }
}

private class FakeManifestV2SendNativeJob : ManifestV2SendNativeJob {
    var sealCount = 0
    var closeCount = 0

    override suspend fun sealForSend() {
        sealCount += 1
    }

    override fun close() {
        closeCount += 1
    }
}

private class FakeManifestV2SendCancellation : ManifestV2SessionCancellation {
    var cancelCount = 0
    var closeCount = 0

    override fun cancel() {
        cancelCount += 1
    }

    override fun close() {
        closeCount += 1
    }
}

private class RecordingManifestV2SendObserver : ManifestV2SessionObserver {
    val started = mutableListOf<Pair<Long, Long>>()
    val phases = mutableListOf<Status>()
    val progress = mutableListOf<Pair<Long, Long>>()
    val failures = mutableListOf<ManifestV2SessionFailure>()
    val paths = mutableListOf<ConnectionPathKind>()
    val timings = mutableListOf<TransferStageTiming>()
    val diagnostics = mutableListOf<String>()
    var rememberedCredential: ByteArray? = null
    var rememberedGeneration: Long? = null

    override fun onStarted(
        itemCount: Long,
        totalBytes: Long,
    ) {
        started += itemCount to totalBytes
    }

    override fun onPhase(status: Status) {
        phases += status
    }

    override fun onProgress(
        transferred: Long,
        total: Long,
    ) {
        progress += transferred to total
    }

    override fun onFailure(failure: ManifestV2SessionFailure) {
        failures += failure
    }

    override fun onConnectionPath(path: ConnectionPathKind) {
        paths += path
    }

    override fun onStageTiming(timing: TransferStageTiming) {
        timings += timing
    }

    override fun onDiagnostic(message: String) {
        diagnostics += message
    }

    override fun onRememberedCredential(
        opaqueCredential: ByteArray,
        generation: Long,
    ): Boolean {
        rememberedCredential = opaqueCredential
        rememberedGeneration = generation
        return true
    }
}

private fun completion() =
    FfiManifestV2Completion(
        jobId = "00112233445566778899aabbccddeeff",
        entryCount = 3u,
        totalPlaintextBytes = 42uL,
        deliveryProofDigest = ByteArray(32),
        savedPaths = emptyList(),
    )
