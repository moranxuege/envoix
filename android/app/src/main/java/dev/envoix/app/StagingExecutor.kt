package dev.envoix.app

import android.net.Uri
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import java.io.File

/**
 * The staging worker pool: one generation-stamped copy worker per card,
 * launched from a committed Preparing snapshot and retired by any
 * non-Preparing one. Extracted whole from [TransferService], which delegates
 * here (callback: [TransferService.transferScope]).
 */
internal class StagingExecutor(
    private val service: TransferService,
) {
    /** The live staging worker per card. A committed `Preparing` snapshot of a
     *  generation owns exactly one worker; a non-`Preparing` snapshot retires it.
     *  Owner-checked so a retired worker never clobbers a newer one. */
    private val stageWork = HashMap<Long, StageWork>()

    /** One staging copy worker. Owns its open streams so a retire can CLOSE them
     *  — which unblocks a stuck blocking read/write; cancelling the coroutine
     *  alone cannot. `ensureActive()` in the loop is a secondary check, and the
     *  reducer's generation stamp is the real correctness backstop. */
    private class StageWork(
        val generation: Int,
    ) {
        @Volatile var job: Job? = null

        @Volatile var input: java.io.Closeable? = null

        @Volatile var output: java.io.Closeable? = null

        @Volatile var retired = false

        fun retire() {
            retired = true
            runCatching { input?.close() }
            runCatching { output?.close() }
            job?.cancel()
        }
    }

    /** Ensure a staging worker of [generation] runs for [id], from a committed
     *  Preparing snapshot. Idempotent for the same generation; a new generation
     *  retires the old worker first. */
    fun ensureStaging(
        id: Long,
        spec: Spec,
        generation: Int,
    ) {
        val work = StageWork(generation)
        val toRetire =
            synchronized(stageWork) {
                val cur = stageWork[id]
                if (cur != null && cur.generation == generation && !cur.retired) {
                    return // already staging this generation
                }
                stageWork[id] = work
                cur
            }
        toRetire?.retire()
        work.job = service.transferScope(id).launch(Dispatchers.IO) { runStaging(id, spec, work) }
    }

    /** Retire [id]'s staging worker (a non-Preparing snapshot, or Remove): close
     *  its streams and cancel it. The incomplete partial is dropped by the
     *  worker's own finally. Owner-checked. */
    fun retireStaging(id: Long) {
        synchronized(stageWork) { stageWork.remove(id) }?.retire()
    }

    /** The staging copy, generation-stamped. Stores its streams in [work] so a
     *  retire can close them (unblocking a stuck read); a retire/failure drops
     *  the incomplete partial and emits NO stage callback (the machine has
     *  already left Preparing, and a stale one is dropped by the reducer anyway).
     *  A complete copy is kept. */
    private fun runStaging(
        id: Long,
        spec: Spec,
        work: StageWork,
    ) {
        val gen = work.generation
        val uri = spec.sourceUri?.let { Uri.parse(it) }
        if (uri == null) {
            // A restored Preparing whose source cannot be reopened.
            TransferTimeline.event(id, "platform.stage", "failed", outcome = "no_source")
            if (!work.retired) Native.stageFailed(id, gen, "source needs re-picking")
            clearStageWork(id, work)
            return
        }
        val out = File(spec.path)
        out.parentFile?.mkdirs()
        TransferTimeline.event(id, "platform.stage", "start", fields = mapOf("name" to out.name))
        var completed = false
        try {
            service.contentResolver.openInputStream(uri)!!.also { work.input = it }.use { input ->
                out.outputStream().also { work.output = it }.use { o ->
                    val buf = ByteArray(1 shl 20)
                    var copied = 0L
                    var last = 0L
                    while (true) {
                        if (work.retired) return // secondary check; close() is the primary unblock
                        val n = input.read(buf)
                        if (n < 0) break
                        o.write(buf, 0, n)
                        copied += n
                        val now = System.currentTimeMillis()
                        if (now - last > 150) {
                            last = now
                            Native.stageProgress(id, gen, copied)
                        }
                    }
                }
            }
            completed = true
        } catch (c: kotlinx.coroutines.CancellationException) {
            throw c // retired via cancel; propagate, no callback
        } catch (e: Throwable) {
            if (!work.retired) {
                TransferTimeline.event(
                    id,
                    "platform.stage",
                    "failed",
                    outcome = "copy",
                    // The exception TYPE, never .message — an openInputStream
                    // message embeds the full content:// URI, which would ship to
                    // the (public) log endpoint.
                    fields = mapOf("cause" to e.javaClass.simpleName),
                )
                Native.stageFailed(id, gen, "couldn't read the picked file")
            }
        } finally {
            if (!completed) out.delete() // drop the incomplete partial (retire/failure)
            clearStageWork(id, work)
        }
        if (completed) {
            TransferTimeline.event(id, "platform.stage", "complete")
            Native.stageComplete(id, gen)
        }
    }

    /** Owner-checked removal: only clear the map entry if it still holds [work]. */
    private fun clearStageWork(
        id: Long,
        work: StageWork,
    ) {
        synchronized(stageWork) { if (stageWork[id] === work) stageWork.remove(id) }
    }
}
