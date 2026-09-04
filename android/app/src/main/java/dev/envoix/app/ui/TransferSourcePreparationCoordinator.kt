package dev.envoix.app.ui

import android.content.Context
import android.net.Uri
import dev.envoix.app.ManifestV2JobGateway
import dev.envoix.app.ManifestV2SourceDecision
import dev.envoix.app.ManifestV2SourceStager
import dev.envoix.app.ManifestV2StageResult
import dev.envoix.app.PreparedManifestV2Source
import dev.envoix.app.TransferService
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.withLock
import java.io.File

internal data class TransferSourcePreparationMessages(
    val prepareFailed: String,
    val removeFailed: String,
    val selectionChanged: String,
    val authorizationFailed: String,
)

internal data class TransferSourcePreparationIntents(
    val addSources: (
        TransferDraftPreparationState,
        List<Uri>,
        Boolean,
        String,
        TransferSourcePreparationMessages,
    ) -> Unit,
    val removeSource: (
        TransferDraftPreparationState,
        PreparedManifestV2Source,
        TransferSourcePreparationMessages,
    ) -> Unit,
    val approvePartial: (TransferDraftPreparationState, PreparedManifestV2Source) -> Unit,
    val reauthorizeSource: (
        TransferDraftPreparationState,
        PreparedManifestV2Source,
        Uri,
        TransferSourcePreparationMessages,
    ) -> Unit,
)

/** Executes Android provider and native-job effects outside the Composable. */
internal class TransferSourcePreparationCoordinator(
    context: Context,
    private val scope: CoroutineScope,
    private val jobGateway: ManifestV2JobGateway = ManifestV2JobGateway.shared,
) {
    private val appContext = context.applicationContext

    val intents =
        TransferSourcePreparationIntents(
            addSources = ::addSources,
            removeSource = ::removeSource,
            approvePartial = ::approvePartial,
            reauthorizeSource = ::reauthorizeSource,
        )

    private fun addSources(
        preparation: TransferDraftPreparationState,
        uris: List<Uri>,
        directory: Boolean,
        compressionPolicy: String,
        messages: TransferSourcePreparationMessages,
    ) {
        if (uris.isEmpty()) return
        val sources = uris.map { ManifestV2SourceStager.sourceFromUri(appContext, it, directory) }
        scope.launch {
            preparation.mutex.withLock {
                if (!preparation.acceptsPreparationChanges()) return@withLock
                val fresh =
                    sources.filter { candidate ->
                        preparation.preparedSources.none { it.source.uri == candidate.uri }
                    }
                if (fresh.isEmpty()) return@withLock
                preparation.preparingCount.value += fresh.size
                preparation.error.value = null
                try {
                    val store = jobStoreDirectory()
                    preparation.jobStoreDirectory.value = store
                    val jobId =
                        preparation.preparedJobId.value
                            ?: jobGateway.create(store, compressionPolicy).jobId.also {
                                preparation.preparedJobId.value = it
                                preparation.stagingRootDirectory.value =
                                    File(appContext.filesDir, "manifest-v2/source-staging/$it").absolutePath
                            }
                    for (source in fresh) {
                        val staged = ManifestV2SourceStager.stage(appContext, jobId, source)
                        var attached = false
                        try {
                            val snapshot =
                                jobGateway.addStagedProviderRoot(
                                    store,
                                    jobId,
                                    ManifestV2SourceStager.stagedProviderRoot(source, staged),
                                )
                            attached = true
                            preparation.summary.value = snapshot
                            preparation.preparedSources +=
                                ManifestV2SourceStager.parsePreparedSnapshot(
                                    source,
                                    staged.root,
                                    snapshot,
                                )
                        } catch (error: Throwable) {
                            if (!attached) staged.root.parentFile?.deleteRecursively()
                            throw error
                        }
                    }
                } catch (error: Throwable) {
                    preparation.error.value = error.message ?: messages.prepareFailed
                } finally {
                    preparation.preparingCount.value -= fresh.size
                }
            }
        }
    }

    private fun removeSource(
        preparation: TransferDraftPreparationState,
        source: PreparedManifestV2Source,
        messages: TransferSourcePreparationMessages,
    ) {
        val jobId = preparation.preparedJobId.value ?: return
        scope.launch {
            preparation.mutex.withLock {
                if (!preparation.acceptsPreparationChanges()) return@withLock
                preparation.preparingCount.value += 1
                try {
                    val snapshot =
                        jobGateway.resolveSourceIssue(
                            jobStoreDirectory(),
                            jobId,
                            source.rootItemId,
                            ManifestV2SourceDecision.RemoveSelection,
                        )
                    preparation.preparedSources.remove(source)
                    source.localRoot.parentFile?.deleteRecursively()
                    preparation.summary.value = snapshot
                } catch (error: Throwable) {
                    preparation.error.value = error.message ?: messages.removeFailed
                } finally {
                    preparation.preparingCount.value -= 1
                }
            }
        }
    }

    private fun approvePartial(
        preparation: TransferDraftPreparationState,
        source: PreparedManifestV2Source,
    ) {
        val jobId = preparation.preparedJobId.value ?: return
        scope.launch {
            preparation.mutex.withLock {
                if (!preparation.acceptsPreparationChanges()) return@withLock
                val snapshot =
                    jobGateway.resolveSourceIssue(
                        jobStoreDirectory(),
                        jobId,
                        source.rootItemId,
                        ManifestV2SourceDecision.ApprovePartial,
                    )
                val index = preparation.preparedSources.indexOf(source)
                if (index >= 0) {
                    preparation.preparedSources[index] = source.copy(partialApproved = true)
                }
                preparation.summary.value = snapshot
                preparation.error.value = null
            }
        }
    }

    private fun reauthorizeSource(
        preparation: TransferDraftPreparationState,
        previous: PreparedManifestV2Source,
        uri: Uri,
        messages: TransferSourcePreparationMessages,
    ) {
        val jobId = preparation.preparedJobId.value ?: return
        scope.launch {
            preparation.mutex.withLock {
                if (!preparation.acceptsPreparationChanges()) return@withLock
                preparation.preparingCount.value += 1
                preparation.error.value = null
                val source = ManifestV2SourceStager.sourceFromUri(appContext, uri, true)
                var staged: ManifestV2StageResult? = null
                var committed = false
                try {
                    val stagedResult = ManifestV2SourceStager.stage(appContext, jobId, source)
                    staged = stagedResult
                    val snapshot =
                        jobGateway.reauthorizeStagedProviderSource(
                            jobStoreDirectory(),
                            jobId,
                            previous.rootItemId,
                            ManifestV2SourceStager.stagedProviderRoot(source, stagedResult),
                        )
                    committed = true
                    val replacement =
                        ManifestV2SourceStager.parsePreparedSnapshot(
                            source,
                            stagedResult.root,
                            snapshot,
                            previous.rootItemId,
                        )
                    val index = preparation.preparedSources.indexOf(previous)
                    check(index >= 0) { messages.selectionChanged }
                    preparation.preparedSources[index] = replacement
                    previous.localRoot.parentFile?.deleteRecursively()
                    preparation.summary.value = snapshot
                } catch (error: Throwable) {
                    if (!committed) staged?.root?.parentFile?.deleteRecursively()
                    preparation.error.value = error.message ?: messages.authorizationFailed
                } finally {
                    preparation.preparingCount.value -= 1
                }
            }
        }
    }

    private fun jobStoreDirectory(): String = TransferService.jobStoreDirectory(appContext).absolutePath
}
