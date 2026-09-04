package dev.envoix.app

import dev.envoix.app.ffi.FfiDestinationCommitReplyV2
import dev.envoix.app.ffi.FfiDestinationCommitRequestV2
import dev.envoix.app.ffi.FfiDestinationPlanReplyV2
import dev.envoix.app.ffi.FfiDestinationPlanRequestV2
import dev.envoix.app.ffi.FfiManifestV2DestinationException
import dev.envoix.app.ffi.ManifestV2PlatformDestination
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** Android SAF/MediaStore adapter for Rust's typed delivery-result gate. */
internal class AndroidManifestV2PlatformDestination(
    private val writer: ManifestV2DestinationWriter,
    private val isActive: () -> Boolean,
    private val onCommitted: (destinationLabel: String) -> Unit,
) : ManifestV2PlatformDestination {
    override suspend fun plan(request: FfiDestinationPlanRequestV2): FfiDestinationPlanReplyV2 =
        onIo {
            check(isActive()) { "Manifest v2 receive attempt is no longer active" }
            writer.plan(request)
        }

    override suspend fun commit(request: FfiDestinationCommitRequestV2): FfiDestinationCommitReplyV2 =
        onIo {
            check(isActive()) { "Manifest v2 receive attempt is no longer active" }
            val result = writer.saveWithDestination(request)
            onCommitted(result.destinationLabel)
            result.reply
        }

    private suspend fun <T> onIo(operation: () -> T): T =
        withContext(Dispatchers.IO) {
            try {
                operation()
            } catch (canceled: CancellationException) {
                throw canceled
            } catch (error: Throwable) {
                throw FfiManifestV2DestinationException.Operation(
                    reason = error.message ?: "Android destination operation failed",
                )
            }
        }
}
