package app.envoix.host

import android.app.Notification
import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.database.sqlite.SQLiteConstraintException
import android.net.Uri
import android.os.PowerManager
import android.provider.MediaStore
import com.envoix.bindings.capability.SourceAcquisitionKeyView
import com.envoix.bindings.duty.DutyProvenanceView
import com.envoix.bindings.duty.LockDirectiveView
import com.envoix.bindings.duty.NoticeView
import com.envoix.bindings.duty.OutcomeCodeView
import com.envoix.bindings.duty.PublicationWorkView
import com.envoix.bindings.duty.SourceAcquiredView
import com.envoix.bindings.duty.SourceFailedView
import com.envoix.bindings.duty.SourceFailureView
import com.envoix.bindings.duty.SourceReportView
import com.envoix.bindings.duty.SourceRetentionView
import com.envoix.bindings.duty.SourceSeekabilityView
import java.io.File

/**
 * Executes typed platform work orders and produces typed reports.
 *
 * Idempotent per provenance by construction: every implemented kind is a
 * repeat-safe platform call (re-posting a notification, re-acquiring a held
 * lock, re-asserting foreground state). Publication is repeat-safe too — its
 * recovery journal reuses one deterministic MediaStore row and never loses the
 * last copy (see [publish]). Binding a picked source is repeat-safe by the same
 * standard: [SourcePicks] resolves an ACQUISITION to the SAME document however
 * often the duty is delivered. The remaining kinds (grants, staging roots, share
 * sheets) are deliberately NOT reported — an unreported duty stays outstanding
 * and is re-delivered, which is the honest state until F3 can execute it.
 */
class DutyExecutor(
    private val context: Context,
) : DutyEffects {
    private var wakeLock: PowerManager.WakeLock? = null
    private val resolver = context.contentResolver
    private val publicationJournal =
        context.getSharedPreferences("envoix-publication-journal", Context.MODE_PRIVATE)

    /**
     * Executes one encoded work order; null = leave the duty outstanding.
     *
     * The decision is [DutyRouter]'s and is Android-free, so it can be executed
     * and asserted off-device. This class supplies the effects.
     */
    fun execute(order: ByteArray): ByteArray? = DutyRouter.route(order, this)

    override fun postNotice(
        provenance: DutyProvenanceView,
        notice: NoticeView,
    ): OutcomeCodeView {
        val manager =
            context.getSystemService(android.app.NotificationManager::class.java)
        val text =
            when (notice) {
                NoticeView.TRANSFER_COMPLETE -> "Transfer complete"
                NoticeView.TRANSFER_FAILED -> "Transfer failed"
                NoticeView.ACTION_NEEDED -> "Envoix needs your attention"
            }
        val notification =
            Notification
                .Builder(context, EnvoixHostService.CHANNEL_ID)
                .setSmallIcon(android.R.drawable.stat_sys_download_done)
                .setContentTitle("Envoix")
                .setContentText(text)
                .build()
        manager.notify(noticeId(provenance.card), notification)
        return OutcomeCodeView.COMPLETED
    }

    /**
     * The service enters the foreground when it starts and stays there while it
     * runs, so this duty is an acknowledgement rather than a transition. The
     * count is accepted and unused: what it would change is already true.
     */
    override fun assertForeground(activeTransfers: Long): OutcomeCodeView = OutcomeCodeView.COMPLETED

    /**
     * The receipt courier. No platform carrier exists yet (F2/F3 wire the real
     * one), so the honest report is a failure: the duty is discharged for this
     * generation and re-issued on the next restore. Reporting "completed" would
     * tell the reducer a receipt was delivered when none was.
     */
    override fun carryReceipt(): OutcomeCodeView = OutcomeCodeView.INTERNAL

    /**
     * Binds the document the user picked to the card that asked for it and
     * proves it can still be read through its grant.
     *
     * The URI never leaves this side: the report carries an outcome and the
     * duty's own provenance, and the lane's vocabulary has no type that could
     * carry a handle even if this wanted to. A card with no outstanding pick —
     * a process death lost it, or nothing was ever chosen — reports the honest
     * `source_unreadable`, which is the outcome that means "re-pick".
     */
    override fun bindSource(provenance: DutyProvenanceView): SourceReportView {
        // The WHOLE provenance, never the card alone: the duty is issued for one
        // acquisition, and claiming by card would hand generation 2 the document
        // that generation 1 was given.
        val acquisition =
            SourceAcquisitionKeyView(
                card = provenance.card,
                generation = provenance.generation,
                request = provenance.request,
            )
        val claimed =
            SourcePicks.claim(context, acquisition)
                ?: return SourceReportView.Failed(SourceFailedView(SourceFailureView.UNREADABLE))
        // Retention is what the CLAIM actually took, not what the pick asked
        // for: `claim` writes durable ownership only when Android really
        // retained the permission, so a provider that granted only this process
        // is reported as such rather than promoted.
        val retention =
            if (SourcePicks.isPersisted(context, claimed)) {
                SourceRetentionView.PERSISTED
            } else {
                SourceRetentionView.PROCESS
            }
        val seekability =
            when (SourcePicks.probeSeekable(context, claimed)) {
                null ->
                    return SourceReportView.Failed(
                        SourceFailedView(SourceFailureView.UNREADABLE),
                    )
                true -> SourceSeekabilityView.SEEKABLE
                false -> SourceSeekabilityView.SEQUENTIAL_ONLY
            }
        return SourceReportView.Acquired(
            SourceAcquiredView(retention = retention, seekability = seekability),
        )
    }

    override fun holdLock(directive: LockDirectiveView): OutcomeCodeView {
        if (directive == LockDirectiveView.HOLD) {
            if (wakeLock == null) {
                val power = context.getSystemService(PowerManager::class.java)
                wakeLock =
                    power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "envoix:transfer").apply {
                        setReferenceCounted(false)
                        acquire(MAX_LOCK_MILLIS)
                    }
            }
        } else {
            wakeLock?.release()
            wakeLock = null
        }
        return OutcomeCodeView.COMPLETED
    }

    /**
     * Publishes the sealed artifact to shared storage with the recovery journal
     * that never loses the last copy: a deterministic pending row (reused on
     * replay, never duplicated), journaled before the copy, committed by
     * clearing IS_PENDING. The private source is deleted only by
     * [acknowledgePublication], after the C6 result is admitted AND the
     * committed row is verified. Returns null (duty left outstanding) until the
     * staged copy exists — the F2 pick/staging flow provides its real payload.
     */
    override fun publish(
        work: PublicationWorkView,
        provenance: DutyProvenanceView,
    ): OutcomeCodeView? {
        val staged = work.staged.ifEmpty { return null }
        val displayName = work.displayName.ifEmpty { return null }
        val source = File(File(context.filesDir, EnvoixHostService.STORAGE_ROOT), staged)
        if (!source.isFile) {
            return null
        }
        val key = recoveryKey(provenance)
        val prefix = "publication.$key"
        val pendingName = ".envoix-$key.pending"
        val collection = MediaStore.Downloads.EXTERNAL_CONTENT_URI
        val row =
            publicationJournal.getString("$prefix.row", null)?.let(Uri::parse)?.takeIf(::rowExists)
                ?: findPendingRow(collection, pendingName)
                ?: reservePendingRow(collection, pendingName)
        check(
            publicationJournal
                .edit()
                .putString("$prefix.row", row.toString())
                .putString("$prefix.source", source.absolutePath)
                .putString("$prefix.state", "reserved")
                .commit(),
        )
        resolver.openOutputStream(row, "rwt").use { output ->
            checkNotNull(output)
            source.inputStream().use { input -> input.copyTo(output) }
        }
        commit(row, displayName)
        check(
            publicationJournal
                .edit()
                .putString("$prefix.state", "committed")
                .commit(),
        )
        return OutcomeCodeView.COMPLETED
    }

    /**
     * Clears IS_PENDING under the artifact's display name. The shared
     * collection enforces UNIQUE(RELATIVE_PATH, DISPLAY_NAME), so two
     * provenances offering the same name collide — a rename, not the legacy
     * crash: " (2)", " (3)", ... until the name is free.
     */
    private fun commit(
        row: Uri,
        displayName: String,
    ) {
        for (attempt in 1..MAX_NAME_ATTEMPTS) {
            val name = uniquified(displayName, attempt)
            val committed =
                ContentValues().apply {
                    put(MediaStore.MediaColumns.DISPLAY_NAME, name)
                    put(MediaStore.MediaColumns.MIME_TYPE, PUBLICATION_MIME)
                    put(MediaStore.MediaColumns.RELATIVE_PATH, PUBLICATION_PATH)
                    put(MediaStore.MediaColumns.IS_PENDING, 0)
                }
            try {
                check(resolver.update(row, committed, null, null) == 1)
                return
            } catch (collision: SQLiteConstraintException) {
                if (attempt == MAX_NAME_ATTEMPTS) {
                    throw collision
                }
            }
        }
    }

    /** `payload.bin` -> `payload.bin`, `payload (2).bin`, `payload (3).bin`, ... */
    private fun uniquified(
        displayName: String,
        attempt: Int,
    ): String {
        if (attempt == 1) {
            return displayName
        }
        val dot = displayName.lastIndexOf('.')
        return if (dot <= 0) {
            "$displayName ($attempt)"
        } else {
            "${displayName.substring(0, dot)} ($attempt)${displayName.substring(dot)}"
        }
    }

    /**
     * Releases the private source after the C6 result was admitted — but only
     * once the committed MediaStore row is verified, so a crash in this final
     * window still leaves either the public row or the private copy. Wired by
     * the F2 post-admission callback.
     */
    fun acknowledgePublication(provenanceKey: String) {
        val prefix = "publication.$provenanceKey"
        val row = publicationJournal.getString("$prefix.row", null)?.let(Uri::parse) ?: return
        val source = publicationJournal.getString("$prefix.source", null)?.let(::File) ?: return
        if (!committedRowExists(row)) {
            return
        }
        source.delete()
        check(
            publicationJournal
                .edit()
                .putString("$prefix.state", "acknowledged")
                .commit(),
        )
    }

    private fun reservePendingRow(
        collection: Uri,
        pendingName: String,
    ): Uri {
        val values =
            ContentValues().apply {
                put(MediaStore.MediaColumns.DISPLAY_NAME, pendingName)
                put(MediaStore.MediaColumns.MIME_TYPE, PUBLICATION_MIME)
                put(MediaStore.MediaColumns.RELATIVE_PATH, PUBLICATION_PATH)
                put(MediaStore.MediaColumns.IS_PENDING, 1)
            }
        return checkNotNull(resolver.insert(collection, values))
    }

    private fun findPendingRow(
        collection: Uri,
        pendingName: String,
    ): Uri? =
        resolver
            .query(
                collection,
                arrayOf(MediaStore.MediaColumns._ID),
                "${MediaStore.MediaColumns.DISPLAY_NAME} = ? AND " +
                    "${MediaStore.MediaColumns.RELATIVE_PATH} = ?",
                arrayOf(pendingName, PUBLICATION_PATH),
                "${MediaStore.MediaColumns._ID} ASC",
            )?.use { cursor ->
                if (cursor.moveToFirst()) {
                    ContentUris.withAppendedId(collection, cursor.getLong(0))
                } else {
                    null
                }
            }

    private fun rowExists(uri: Uri): Boolean =
        resolver
            .query(uri, arrayOf(MediaStore.MediaColumns._ID), null, null, null)
            ?.use { cursor -> cursor.moveToFirst() }
            ?: false

    /** A row that exists AND is no longer pending — the only proof of a copy. */
    private fun committedRowExists(uri: Uri): Boolean =
        resolver
            .query(uri, arrayOf(MediaStore.MediaColumns.IS_PENDING), null, null, null)
            ?.use { cursor -> cursor.moveToFirst() && cursor.getInt(0) == 0 }
            ?: false

    /**
     * A per-card notice id, so two cards never overwrite each other's notice.
     * The sign bit keeps it clear of the service's positive foreground id.
     */
    private fun noticeId(card: String): Int = card.hashCode() or Int.MIN_VALUE

    /** Mirrors the Rust `WireProvenance::recovery_key` (`<card>-<gen:08x>-<request>`). */
    private fun recoveryKey(provenance: DutyProvenanceView): String =
        "%s-%08x-%s".format(
            provenance.card,
            provenance.generation,
            provenance.request,
        )

    companion object {
        /** Rename attempts before a display-name collision is a real failure. */
        const val MAX_NAME_ATTEMPTS = 64

        /** A transfer lock is always bounded; the host re-asserts as needed. */
        const val MAX_LOCK_MILLIS = 30L * 60L * 1000L

        /** F2 provides the real MIME/collection; these defaults keep publish safe today. */
        const val PUBLICATION_MIME = "application/octet-stream"
        const val PUBLICATION_PATH = "Download/Envoix"
    }
}
