package dev.envoix.app

import dev.envoix.app.ffi.EnvoixRuntimeSettings
import dev.envoix.app.ffi.FfiDataPathKind
import dev.envoix.app.ffi.FfiFailureCode
import dev.envoix.app.ffi.FfiFailureOutcome
import dev.envoix.app.ffi.FfiFailureSessionDisposition
import dev.envoix.app.ffi.FfiManifestV2Cancellation
import dev.envoix.app.ffi.FfiManifestV2Completion
import dev.envoix.app.ffi.FfiManifestV2Phase
import dev.envoix.app.ffi.FfiPathPolicy
import dev.envoix.app.ffi.FfiRecoveryAction
import dev.envoix.app.ffi.FfiRendezvousPlan
import dev.envoix.app.ffi.FfiTransferDirection
import dev.envoix.app.ffi.FfiTransferFailure
import dev.envoix.app.ffi.FfiTransferJobV2
import dev.envoix.app.ffi.FfiTransferMode
import dev.envoix.app.ffi.FfiTransferRequest
import dev.envoix.app.ffi.FfiTransferStage
import dev.envoix.app.ffi.FfiTransferStageTiming
import dev.envoix.app.ffi.TransferObserver
import dev.envoix.app.ffi.envoixCoreInfo
import dev.envoix.app.ffi.restoreTransferJobV2
import dev.envoix.app.ffi.sendTransferJobV2

internal data class RememberedManifestV2SendRequest(
    val jobStoreDirectory: String,
    val jobId: String,
    val stateDirectory: String,
    val language: String,
    val broker: String,
    val relay: String,
    val credentialReference: String,
    val generation: Long,
    val previousGeneration: Long?,
)

internal data class ManifestV2SendCompletion(
    val jobId: String,
    val totalBytes: Long,
)

internal data class ManifestV2SendFailure(
    val cause: String,
    val retryable: Boolean,
    val recoveryAction: RecoveryAction,
    val outcome: FailureOutcome,
    val sessionDisposition: FailureSessionDisposition,
    val diagnosticMessage: String,
)

internal interface ManifestV2SendObserver {
    fun onStarted(
        itemCount: Long,
        totalBytes: Long,
    )

    fun onPhase(status: Status)

    fun onProgress(
        transferred: Long,
        total: Long,
    )

    fun onFailure(failure: ManifestV2SendFailure)

    fun onConnectionPath(path: ConnectionPathKind)

    fun onStageTiming(timing: TransferStageTiming)

    fun onDiagnostic(message: String)

    fun onRememberedCredential(
        opaqueCredential: ByteArray,
        generation: Long,
    ): Boolean
}

internal interface ManifestV2SendCancellation : AutoCloseable {
    fun cancel()
}

internal interface ManifestV2SendNativeJob : AutoCloseable {
    suspend fun sealForSend()
}

internal interface ManifestV2SendNativeCore {
    fun newCancellation(): ManifestV2SendCancellation

    suspend fun restoreJob(
        storeDirectory: String,
        jobId: String,
    ): ManifestV2SendNativeJob

    suspend fun send(
        job: ManifestV2SendNativeJob,
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        stateDirectory: String,
        cancellation: ManifestV2SendCancellation,
        observer: TransferObserver,
    ): FfiManifestV2Completion
}

internal class ManifestV2SendGateway(
    private val native: ManifestV2SendNativeCore = UniFfiManifestV2SendNativeCore,
) {
    fun newCancellation(): ManifestV2SendCancellation = native.newCancellation()

    suspend fun sendRemembered(
        request: RememberedManifestV2SendRequest,
        cancellation: ManifestV2SendCancellation,
        observer: ManifestV2SendObserver,
    ): ManifestV2SendCompletion {
        request.validate()
        val job = native.restoreJob(request.jobStoreDirectory, request.jobId)
        return try {
            job.sealForSend()
            val completion =
                native.send(
                    job = job,
                    settings = request.runtimeSettings(),
                    request = request.transferRequest(),
                    stateDirectory = request.stateDirectory,
                    cancellation = cancellation,
                    observer = UniFfiManifestV2SendObserver(observer),
                )
            check(completion.jobId == request.jobId) {
                "Manifest v2 completion job does not match the requested job"
            }
            ManifestV2SendCompletion(
                jobId = completion.jobId,
                totalBytes = completion.totalPlaintextBytes.checkedLong("completion bytes"),
            )
        } finally {
            job.close()
        }
    }

    companion object {
        val shared = ManifestV2SendGateway()
    }
}

private object UniFfiManifestV2SendNativeCore : ManifestV2SendNativeCore {
    private val compatibleBinding by lazy {
        val info = envoixCoreInfo()
        check(
            info.ffiApiVersion == EXPECTED_FFI_API_VERSION &&
                REQUIRED_CAPABILITIES.all(info.capabilities::contains),
        ) {
            "Unsupported Envoix Manifest v2 session binding: FFI ${info.ffiApiVersion}"
        }
        true
    }

    override fun newCancellation(): ManifestV2SendCancellation {
        requireCompatibleBinding()
        return UniFfiManifestV2SendCancellation(FfiManifestV2Cancellation())
    }

    override suspend fun restoreJob(
        storeDirectory: String,
        jobId: String,
    ): ManifestV2SendNativeJob {
        requireCompatibleBinding()
        return UniFfiManifestV2SendNativeJob(restoreTransferJobV2(storeDirectory, jobId))
    }

    override suspend fun send(
        job: ManifestV2SendNativeJob,
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        stateDirectory: String,
        cancellation: ManifestV2SendCancellation,
        observer: TransferObserver,
    ): FfiManifestV2Completion {
        requireCompatibleBinding()
        check(job is UniFfiManifestV2SendNativeJob) { "Manifest v2 job belongs to another native core" }
        check(cancellation is UniFfiManifestV2SendCancellation) {
            "Manifest v2 cancellation belongs to another native core"
        }
        return sendTransferJobV2(
            job = job.value,
            settings = settings,
            request = request,
            stateDirectory = stateDirectory,
            cancellation = cancellation.value,
            observer = observer,
        )
    }

    private fun requireCompatibleBinding() {
        check(compatibleBinding)
    }

    private val REQUIRED_CAPABILITIES =
        setOf(
            "canonical_transfer_job_v2",
            "manifest_v2_session",
            "canonical_failure_projection_v1",
        )
}

private class UniFfiManifestV2SendNativeJob(
    val value: FfiTransferJobV2,
) : ManifestV2SendNativeJob {
    override suspend fun sealForSend() {
        value.sealForSend()
    }

    override fun close() = value.close()
}

private class UniFfiManifestV2SendCancellation(
    val value: FfiManifestV2Cancellation,
) : ManifestV2SendCancellation {
    override fun cancel() = value.cancel()

    override fun close() = value.close()
}

private class UniFfiManifestV2SendObserver(
    private val target: ManifestV2SendObserver,
) : TransferObserver {
    override fun onInviteReady(invite: String) = Unit

    override fun onStarted(
        itemCount: UInt,
        totalBytes: ULong,
    ) {
        val total = totalBytes.checkedLongOrReport("started bytes") ?: return
        target.onStarted(itemCount.toLong(), total)
    }

    override fun onPhase(phase: FfiManifestV2Phase) {
        phase.toStatus()?.let(target::onPhase)
    }

    override fun onProgress(
        transferred: ULong,
        total: ULong,
    ) {
        val checkedTransferred = transferred.checkedLongOrReport("transferred bytes") ?: return
        val checkedTotal = total.checkedLongOrReport("total bytes") ?: return
        target.onProgress(checkedTransferred, checkedTotal)
    }

    override fun onCompleted(bytes: ULong) = Unit

    override fun onTransferFailed(failure: FfiTransferFailure) {
        target.onFailure(failure.project())
    }

    override fun onConnectionPath(event: dev.envoix.app.ffi.FfiConnectionPathEvent) {
        target.onConnectionPath(event.pathKind.toConnectionPathKind())
    }

    override fun onStageTiming(event: FfiTransferStageTiming) {
        val timing = event.projectOrNull() ?: return
        target.onStageTiming(timing)
    }

    override fun onDiagnostic(message: String) {
        target.onDiagnostic(message)
    }

    override fun onRememberedCredential(
        opaqueCredential: ByteArray,
        generation: ULong,
    ): Boolean {
        val checkedGeneration = generation.toLongOrNull() ?: return false
        return target.onRememberedCredential(opaqueCredential, checkedGeneration)
    }

    private fun ULong.checkedLongOrReport(name: String): Long? =
        toLongOrNull()
            ?: run {
                target.onDiagnostic("Manifest v2 $name exceeded the Android range")
                null
            }
}

private fun RememberedManifestV2SendRequest.validate() {
    require(jobStoreDirectory.isNotBlank()) { "Manifest v2 job store directory is required" }
    require(jobId.isNotBlank()) { "Manifest v2 job ID is required" }
    require(stateDirectory.isNotBlank()) { "Manifest v2 state directory is required" }
    require(broker.isNotBlank()) { "Remembered Manifest v2 send requires a broker" }
    require(credentialReference.isNotBlank()) { "Remembered Manifest v2 credential reference is required" }
    require(generation >= 0L) { "Remembered Manifest v2 generation cannot be negative" }
    require(previousGeneration == null || previousGeneration >= 0L) {
        "Remembered Manifest v2 previous generation cannot be negative"
    }
}

private fun RememberedManifestV2SendRequest.runtimeSettings() =
    EnvoixRuntimeSettings(
        concurrentTransfers = true,
        language = language,
        serverUrl = broker,
        relayUrl = relay,
        configPath = "",
        speedLimitMbps = 0uL,
    )

private fun RememberedManifestV2SendRequest.transferRequest() =
    FfiTransferRequest(
        direction = FfiTransferDirection.SEND,
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
        rendezvous =
            FfiRendezvousPlan(
                useRoom = true,
                useMdns = false,
                internetAvailable = true,
            ),
    )

private fun ULong.toLongOrNull(): Long? = takeIf { it <= Long.MAX_VALUE.toULong() }?.toLong()

private fun ULong.checkedLong(name: String): Long = toLongOrNull() ?: error("Manifest v2 $name exceeded the Android range")

private fun FfiManifestV2Phase.toStatus(): Status? =
    when (this) {
        FfiManifestV2Phase.PAIRING -> Status.Pairing
        FfiManifestV2Phase.CONNECTING -> Status.Connecting
        FfiManifestV2Phase.TRANSFERRING -> Status.Transferring
        FfiManifestV2Phase.VERIFYING -> Status.Verifying
        FfiManifestV2Phase.SAVING -> Status.Saving
        FfiManifestV2Phase.WAITING_FOR_RECEIVER_SAVE -> Status.WaitingForReceiverSave
        FfiManifestV2Phase.FINALIZING_DELIVERY -> Status.FinalizingDelivery
        FfiManifestV2Phase.WAITING_FOR_PEER -> Status.WaitingForPeer
        FfiManifestV2Phase.DELIVERED -> null
    }

private fun FfiTransferFailure.project() =
    ManifestV2SendFailure(
        cause = code.wireName(),
        retryable = retryable,
        recoveryAction = recoveryAction.toRecoveryAction(),
        outcome =
            when (outcome) {
                FfiFailureOutcome.CANCELED -> FailureOutcome.Canceled
                FfiFailureOutcome.FAILED -> FailureOutcome.Failed
            },
        sessionDisposition =
            when (sessionDisposition) {
                FfiFailureSessionDisposition.RETAIN_FOR_RECOVERY ->
                    FailureSessionDisposition.RetainForRecovery
                FfiFailureSessionDisposition.RELEASE -> FailureSessionDisposition.Release
            },
        diagnosticMessage = diagnosticMessage,
    )

private fun FfiRecoveryAction.toRecoveryAction() =
    when (this) {
        FfiRecoveryAction.RETRY -> RecoveryAction.Retry
        FfiRecoveryAction.RESUME -> RecoveryAction.Resume
        FfiRecoveryAction.CHOOSE_FOLDER -> RecoveryAction.ChooseFolder
        FfiRecoveryAction.OPEN_SETTINGS -> RecoveryAction.OpenSettings
        FfiRecoveryAction.RE_PAIR -> RecoveryAction.RePair
        FfiRecoveryAction.NONE -> RecoveryAction.None
    }

private fun FfiFailureCode.wireName() =
    when (this) {
        FfiFailureCode.USER_CANCELED -> "user_canceled"
        FfiFailureCode.NETWORK_LOST -> "network_lost"
        FfiFailureCode.AUTHENTICATION_FAILED -> "authentication_failed"
        FfiFailureCode.ROOM_NOT_FOUND -> "room_not_found"
        FfiFailureCode.ROOM_EXPIRED -> "room_expired"
        FfiFailureCode.ROOM_FULL -> "room_full"
        FfiFailureCode.ROOM_RATE_LIMITED -> "room_rate_limited"
        FfiFailureCode.ROOM_UNDER_ATTACK -> "room_under_attack"
        FfiFailureCode.ENDPOINT_RATE_LIMITED -> "endpoint_rate_limited"
        FfiFailureCode.IP_RATE_LIMITED -> "ip_rate_limited"
        FfiFailureCode.SERVER_BUSY -> "server_busy"
        FfiFailureCode.MALFORMED_JOIN -> "malformed_join"
        FfiFailureCode.UNSUPPORTED_RENDEZVOUS_VERSION -> "unsupported_rendezvous_version"
        FfiFailureCode.UNSUPPORTED_FEATURE -> "unsupported_feature"
        FfiFailureCode.INTERNAL_ERROR -> "internal_error"
        FfiFailureCode.SENDER_SOURCE_UNAVAILABLE -> "sender_source_unavailable"
        FfiFailureCode.SENDER_PERMISSION_LOST -> "sender_permission_lost"
        FfiFailureCode.SENDER_SOURCE_CHANGED -> "sender_source_changed"
        FfiFailureCode.SENDER_ITEM_REMOVED -> "sender_item_removed"
        FfiFailureCode.SENDER_CANCELED -> "sender_canceled"
        FfiFailureCode.PROTOCOL_OR_INTEGRITY_FAILURE -> "protocol_or_integrity_failure"
        FfiFailureCode.RECEIVER_SPACE_INSUFFICIENT -> "receiver_space_insufficient"
        FfiFailureCode.RECEIVER_DESTINATION_DECISION_REQUIRED ->
            "receiver_destination_decision_required"
        FfiFailureCode.RECEIVER_DESTINATION_UNAVAILABLE -> "receiver_destination_unavailable"
        FfiFailureCode.RECEIVER_SAVE_FAILED -> "receiver_save_failed"
        FfiFailureCode.RECEIVER_REUSED_OBJECT_LOST -> "receiver_reused_object_lost"
        FfiFailureCode.RECEIVER_FINALIZATION_OUTCOME_UNKNOWN ->
            "receiver_finalization_outcome_unknown"
    }

private fun FfiDataPathKind.toConnectionPathKind() =
    when (this) {
        FfiDataPathKind.DIRECT -> ConnectionPathKind.Direct
        FfiDataPathKind.DIRECT_IPV4 -> ConnectionPathKind.DirectIpv4
        FfiDataPathKind.DIRECT_IPV6 -> ConnectionPathKind.DirectIpv6
        FfiDataPathKind.RELAY -> ConnectionPathKind.Relay
        FfiDataPathKind.WIFI_AWARE -> ConnectionPathKind.WifiAware
        FfiDataPathKind.OTHER -> ConnectionPathKind.Other
    }

private fun FfiTransferStageTiming.projectOrNull(): TransferStageTiming? {
    val attempt = attemptId.toLongOrNull() ?: return null
    val elapsed = elapsedUs.toLongOrNull() ?: return null
    val delta = deltaUs.toLongOrNull() ?: return null
    return TransferStageTimingParser.parse(
        stageWire = stage.toTransferStage().wire,
        directionWire = direction.toDirection().wire,
        attemptId = attempt,
        transferId = transferId,
        elapsedUs = elapsed,
        deltaUs = delta,
    )
}

private fun FfiTransferStage.toTransferStage() =
    when (this) {
        FfiTransferStage.SESSION_STARTED -> TransferStage.SessionStarted
        FfiTransferStage.CONNECTION_READY -> TransferStage.ConnectionReady
        FfiTransferStage.AUTHENTICATION_STARTED -> TransferStage.AuthenticationStarted
        FfiTransferStage.AUTHENTICATION_COMPLETE -> TransferStage.AuthenticationComplete
        FfiTransferStage.MANIFEST_OFFER -> TransferStage.ManifestOffer
        FfiTransferStage.MANIFEST_ACCEPTED -> TransferStage.ManifestAccepted
        FfiTransferStage.FIRST_PAYLOAD -> TransferStage.FirstPayload
        FfiTransferStage.PAYLOAD_COMPLETE -> TransferStage.PayloadComplete
        FfiTransferStage.DELIVERY_COMPLETE -> TransferStage.DeliveryComplete
        FfiTransferStage.CANCELED -> TransferStage.Canceled
        FfiTransferStage.FAILED -> TransferStage.Failed
    }

private fun FfiTransferDirection.toDirection() =
    when (this) {
        FfiTransferDirection.SEND -> Direction.Send
        FfiTransferDirection.RECEIVE -> Direction.Receive
    }
