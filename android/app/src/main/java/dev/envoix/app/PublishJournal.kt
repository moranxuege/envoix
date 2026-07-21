package dev.envoix.app

import android.net.Uri
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import java.io.File

/**
 * Publish journaling and pending-publication recovery for received files:
 * sweeps finalized staging files into Downloads behind a crash-safe sidecar
 * journal, retries stuck publishes, and re-verifies the public artifact before
 * a receipt is served. Extracted whole from [TransferService], which delegates
 * here (callbacks: [TransferService.transferScope], [TransferService.addLog]).
 */
internal class PublishJournal(
    private val service: TransferService,
) {
    /** Ids with an in-flight publish-retry loop, so a re-rendered `completed`
     *  snapshot doesn't spawn a second one. */
    private val publishing = java.util.Collections.synchronizedSet(HashSet<Long>())

    /** Completed receive cards whose public artifact is being checked before
     *  the private receipt may be served. */
    private val publicationChecks = java.util.Collections.synchronizedSet(HashSet<Long>())

    /** Backoff for re-attempting a stuck publish in-session (a non-collision
     *  failure — collisions self-resolve in `commit`). After these, the card is
     *  marked failed and the bytes wait in staging for a restart re-drive. */
    private val publishRetryBackoffMs = longArrayOf(2_000, 5_000, 15_000)

    /** A durable receipt proves protocol completion, not continued ownership of
     *  the user-visible copy. Check the public artifact before serving it. */
    fun reverifyPublishedFile(id: Long) {
        val transfer = TransferRepository.transfers.value.firstOrNull { it.id == id } ?: return
        if (transfer.direction != Direction.Receive || transfer.publicationInvalid) return
        val uri = transfer.savedUri?.let(Uri::parse)
        val size = transfer.publishedSize
        val sha256 = transfer.publishedSha256
        if (uri == null || size == null || sha256 == null) {
            markPublicationInvalid(id, PUBLICATION_EVIDENCE_MISSING_MESSAGE, "evidence_missing")
            return
        }
        if (!publicationChecks.add(id)) return
        service.transferScope(id).launch(Dispatchers.IO) {
            try {
                val actual = MediaStoreSaver.inspect(service, uri).getOrNull()
                val expected = MediaStoreSaver.PublicationEvidence(size, sha256)
                if (actual != null && expected.matches(actual)) {
                    TransferTimeline.event(id, "platform.publication", "verify", outcome = "matched")
                    Native.sessionIntent(id, "reverify")
                } else {
                    markPublicationInvalid(id, PUBLICATION_INVALID_MESSAGE, "missing_or_changed")
                }
            } finally {
                publicationChecks.remove(id)
            }
        }
    }

    private fun markPublicationInvalid(
        id: Long,
        message: String,
        outcome: String,
    ) {
        TransferRepository.update(id) {
            it.copy(
                savedUri = null,
                publishedSize = null,
                publishedSha256 = null,
                publicationInvalid = true,
                error = message,
                log = service.addLog(it.log, message),
            )
        }
        syncExtras(id)
        TransferTimeline.event(id, "platform.publication", "verify", outcome = outcome)
        OpLog.add("publication invalid id=$id outcome=$outcome", id)
    }

    /**
     * Publish every finalized file in one staging dir to Downloads, deleting
     * the staging copy on success (receipt sidecars stay - they re-confirm a
     * lost CompleteAck after the file is published away). With per-card
     * staging (Phase 4) the dir holds only [attributeTo]'s own artifacts; the
     * unattributed call in [gcStaging] is the one legacy exception, draining
     * pre-Phase-4 shared-staging residue.
     */
    fun sweepStaging(
        outputDir: String,
        attributeTo: Long?,
    ) {
        val finals =
            File(outputDir)
                .listFiles { f -> f.isFile && !f.name.startsWith(".") } ?: return
        for (src in finals) publishOne(src, attributeTo)
    }

    /** After the synchronous first sweep, if a completed receive still has staged
     *  files (a non-collision publish failure — collisions self-resolve in
     *  `commit`), retry with backoff so it doesn't wait for a restart; mark the
     *  card failed after exhausting. One retry loop per id. */
    fun scheduleRepublishIfNeeded(
        id: Long,
        dir: String,
    ) {
        if (!hasUnpublished(dir)) return
        if (!publishing.add(id)) return
        service.transferScope(id).launch(Dispatchers.IO) {
            try {
                for (delayMs in publishRetryBackoffMs) {
                    kotlinx.coroutines.delay(delayMs)
                    sweepStaging(dir, attributeTo = id)
                    if (!hasUnpublished(dir)) return@launch
                }
                markPublishFailed(id)
            } finally {
                publishing.remove(id)
            }
        }
    }

    private fun hasUnpublished(dir: String): Boolean =
        File(dir).listFiles { f -> f.isFile && !f.name.startsWith(".") }?.isNotEmpty() ?: false

    /** Terminal (for now) publish failure: durable, and surfaced on the card so the
     *  user isn't left thinking the file silently vanished. A restart re-drives the
     *  publish (the bytes stay in staging); a later success clears it. */
    private fun markPublishFailed(id: Long) {
        TransferRepository.update(id) {
            if (it.publishFailed) {
                it
            } else {
                it.copy(
                    publishFailed = true,
                    log = service.addLog(it.log, "couldn't save to Downloads — kept, will retry"),
                )
            }
        }
        syncExtras(id)
    }

    /** The publish sidecar journal for one staged file: `.envoix-publish.<name>.json`
     *  beside it, holding the reserved target URI (written before the copy) and
     *  the committed URI (written after). Lets a crash mid-publish recover:
     *  drop a half-written candidate, or adopt an already-committed one. */
    private fun publishJournal(src: File) = File(src.parentFile, ".envoix-publish.${src.name}.json")

    /**
     * Publish one finalized staging file, journaled. Recovery first: a surviving
     * journal means a prior publish was interrupted — adopt its committed target
     * (if it still resolves) or delete the half-written candidate — then a fresh
     * reserve → copy → commit → delete-staging, recording each step first.
     */
    private fun publishOne(
        src: File,
        attributeTo: Long?,
    ) {
        // Per-transfer timeline events, only when this file is attributed to a
        // card (the unattributed gcStaging drain has no session to route to).
        fun tl(
            event: String,
            outcome: String = "",
            fields: Map<String, String> = emptyMap(),
        ) = attributeTo?.let {
            TransferTimeline.event(it, "platform.publish", event, outcome = outcome, fields = fields)
        }

        val journal = publishJournal(src)
        // --- recovery: a journal survived a crash mid-publish ---
        runCatching { org.json.JSONObject(journal.readText()) }.getOrNull()?.let { prior ->
            val committed = prior.optString("committed_uri").ifEmpty { null }
            val journalEvidence =
                if (prior.has("published_size") && prior.has("published_sha256")) {
                    MediaStoreSaver.PublicationEvidence(
                        prior.optLong("published_size"),
                        prior.optString("published_sha256"),
                    )
                } else {
                    null
                }
            val expectedEvidence =
                journalEvidence
                    ?: runCatching { src.inputStream().use(MediaStoreSaver::hash) }.getOrNull()
            val publicEvidence =
                committed?.let { MediaStoreSaver.inspect(service, Uri.parse(it)).getOrNull() }
            if (committed != null &&
                expectedEvidence != null &&
                publicEvidence != null &&
                expectedEvidence.matches(publicEvidence)
            ) {
                // Commit had landed; the crash was before staging was cleared.
                // Adopt under the name it was actually published as (may be bumped).
                val publishedName = prior.optString("published_name").ifEmpty { src.name }
                adopt(
                    attributeTo,
                    expectedSourceName = src.name,
                    publishedName = publishedName,
                    uri = committed,
                    evidence = expectedEvidence,
                )
                src.delete()
                journal.delete()
                tl("adopt", fields = mapOf("uri" to TransferTimeline.redactUri(committed)))
                LogStore.append("app: adopted already-published $publishedName")
                return
            }
            // Reserved but never committed (or the user deleted it): drop the
            // half-written candidate so we do not leave a truncated file, then
            // fall through to a fresh publish.
            prior.optString("target").ifEmpty { null }?.let { MediaStoreSaver.delete(service, Uri.parse(it)) }
            journal.delete()
        }

        // --- fresh publish ---
        val s = SettingsStore.settings.value
        val identical =
            MediaStoreSaver
                .findIdentical(service, src, src.name, s.saveTreeUri, s.saveFolder)
                .onFailure {
                    tl("deduplicate", outcome = "lookup_failed", fields = mapOf("cause" to it.javaClass.simpleName))
                }.getOrNull()
        if (identical != null) {
            adopt(
                attributeTo,
                expectedSourceName = src.name,
                publishedName = identical.displayName,
                uri = identical.uri.toString(),
                evidence = identical.evidence,
            )
            attributeTo?.let { id ->
                TransferRepository.update(id) {
                    it.copy(log = service.addLog(it.log, "already present in Downloads · identical content"))
                }
            }
            src.delete()
            journal.delete()
            tl("deduplicate", outcome = "matched", fields = mapOf("uri" to TransferTimeline.redactUri(identical.uri.toString())))
            LogStore.append("app: reused identical public file ${identical.displayName}")
            return
        }
        val target = MediaStoreSaver.reserve(service, src.name, s.saveTreeUri, s.saveFolder)
        if (target == null) {
            tl("failed", outcome = "reserve", fields = mapOf("name" to src.name))
            return
        }
        tl("reserve", fields = mapOf("uri" to TransferTimeline.redactUri(target.uri.toString())))
        // Record the reservation BEFORE any byte is copied, and GATE the copy on
        // it: if we can't durably record the target, don't copy bytes into a
        // user-visible destination we could never recover or clean up.
        if (!writePublishJournal(journal, target.uri.toString(), target.mediaStorePending, committed = null, publishedName = null)) {
            MediaStoreSaver.delete(service, target.uri)
            tl("failed", outcome = "journal_reserve")
            LogStore.append("app: could not record publish reservation for ${src.name}; not copying")
            return
        }
        val copy = MediaStoreSaver.copyInto(service, src, target)
        if (copy.isFailure) {
            MediaStoreSaver.delete(service, target.uri)
            journal.delete()
            tl(
                "failed",
                outcome = "copy",
                // Type only — a copy IOException's .message can carry the
                // destination URI/path (same leak class as the staging cause).
                fields = mapOf("cause" to (copy.exceptionOrNull()?.javaClass?.simpleName ?: "unknown")),
            )
            return
        }
        val evidence = copy.getOrThrow()
        val committed = MediaStoreSaver.commit(service, target)
        if (committed.isFailure) {
            // A colliding _data (same-named file already published) or other
            // publish error must not crash the service: drop the pending target
            // and leave the file in staging for a later retry.
            MediaStoreSaver.delete(service, target.uri)
            journal.delete()
            tl(
                "failed",
                outcome = "commit",
                fields = mapOf("cause" to (committed.exceptionOrNull()?.javaClass?.simpleName ?: "unknown")),
            )
            return
        }
        val outcome = committed.getOrThrow()
        // Record the commit (with the name it actually landed under) BEFORE clearing
        // staging, so a crash here recovers by adopting (never re-publishing =
        // duplicate). The write is best-effort: surface a failure rather than
        // swallow it, but still adopt + clear in-line so no duplicate is created —
        // the crash-in-this-window gap is the separate publication barrier.
        val journaled =
            writePublishJournal(
                journal,
                target.uri.toString(),
                target.mediaStorePending,
                committed = outcome.uri.toString(),
                publishedName = outcome.displayName,
                publishedSize = evidence.size,
                publishedSha256 = evidence.sha256,
            )
        if (!journaled) {
            tl("failed", outcome = "journal")
            LogStore.append("app: publish journal write failed (published as ${outcome.displayName})")
        }
        tl("commit", fields = mapOf("uri" to TransferTimeline.redactUri(outcome.uri.toString())))
        adopt(
            attributeTo,
            expectedSourceName = src.name,
            publishedName = outcome.displayName,
            uri = outcome.uri.toString(),
            evidence = evidence,
        )
        src.delete()
        journal.delete()
        tl("staging_deleted")
        LogStore.append("app: saved ${outcome.displayName} to Downloads")
    }

    private fun writePublishJournal(
        journal: File,
        target: String,
        pending: Boolean,
        committed: String?,
        publishedName: String?,
        publishedSize: Long? = null,
        publishedSha256: String? = null,
    ): Boolean {
        val obj =
            org.json
                .JSONObject()
                .put("target", target)
                .put("pending", pending)
        committed?.let { obj.put("committed_uri", it) }
        publishedName?.let { obj.put("published_name", it) }
        publishedSize?.let { obj.put("published_size", it) }
        publishedSha256?.let { obj.put("published_sha256", it) }
        // Atomic: write a temp then rename over the journal, so recovery always
        // reads a COMPLETE journal (the old one or the new one) — never a
        // half-written, unparsable document it can neither act on nor clean.
        return runCatching {
            val tmp = File(journal.parentFile, "${journal.name}.tmp")
            tmp.writeText(obj.toString())
            if (!tmp.renameTo(journal)) {
                tmp.delete()
                error("journal rename failed")
            }
        }.isSuccess
    }

    /** Attribute a published URI to its card. [expectedSourceName] is the transfer
     *  identity (matched against `fileName`, never overwritten by the published
     *  name); [publishedName] is the platform display name it actually landed
     *  under, which may be a collision-bumped "name (1)". */
    private fun adopt(
        attributeTo: Long?,
        expectedSourceName: String,
        publishedName: String,
        uri: String,
        evidence: MediaStoreSaver.PublicationEvidence,
    ) {
        if (attributeTo == null) return
        TransferRepository.update(attributeTo) {
            if (it.fileName == null || it.fileName == expectedSourceName) {
                it.copy(
                    fileName = it.fileName ?: expectedSourceName,
                    savedUri = uri,
                    publishedName = publishedName,
                    publishedSize = evidence.size,
                    publishedSha256 = evidence.sha256,
                    publicationInvalid = false,
                    publishFailed = false,
                )
            } else {
                it
            }
        }
        syncExtras(attributeTo)
    }

    /** Push the card's platform context (QR payload, saved URI) into the
     *  transfer's durable record, so it survives restarts. */
    private fun syncExtras(id: Long) {
        val t = TransferRepository.transfers.value.firstOrNull { it.id == id } ?: return
        val extras = org.json.JSONObject()
        t.qrPayload?.let { extras.put("qr", it) }
        t.savedUri?.let { extras.put("saved_uri", it) }
        t.publishedName?.let { extras.put("published_name", it) }
        t.publishedSize?.let { extras.put("published_size", it) }
        t.publishedSha256?.let { extras.put("published_sha256", it) }
        if (t.publicationInvalid) extras.put("publication_invalid", true)
        if (t.publishFailed) extras.put("publish", "failed")
        val err = Native.setSessionExtras(id, extras.toString())
        if (err.isNotEmpty()) LogStore.append("app: $err (id=$id)")
    }

    companion object {
        private const val PUBLICATION_INVALID_MESSAGE =
            "Saved file was deleted or changed. Receive it again."
        private const val PUBLICATION_EVIDENCE_MISSING_MESSAGE =
            "This older delivery cannot be verified safely. Receive it again."
    }
}
