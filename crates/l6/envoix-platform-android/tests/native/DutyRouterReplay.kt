// Drives the REAL DutyRouter with fake effects, against order frames the Rust
// authority encoded. Nothing here is a copy of the router: the file under test
// is the one the app ships, compiled unchanged, which is what makes this
// evidence rather than a second opinion.
//
// It prints one line per vector; the Rust side asserts on those lines.

package app.envoix.host

import com.envoix.bindings.duty.DutyProvenanceView
import com.envoix.bindings.duty.LockDirectiveView
import com.envoix.bindings.duty.NoticeView
import com.envoix.bindings.duty.OutcomeCodeView
import com.envoix.bindings.duty.PublicationWorkView
import java.io.File

/** Records what the router asked for, so the Rust side can assert the typed value. */
class RecordingEffects : DutyEffects {
    val seen = mutableListOf<String>()

    override fun postNotice(
        provenance: DutyProvenanceView,
        notice: NoticeView,
    ): OutcomeCodeView {
        seen += "notice=${notice.name} card=${provenance.card}"
        return OutcomeCodeView.COMPLETED
    }

    override fun holdLock(directive: LockDirectiveView): OutcomeCodeView {
        seen += "lock=${directive.name}"
        return OutcomeCodeView.COMPLETED
    }

    override fun assertForeground(activeTransfers: Long): OutcomeCodeView {
        seen += "foreground=$activeTransfers"
        return OutcomeCodeView.COMPLETED
    }

    override fun publish(
        work: PublicationWorkView,
        provenance: DutyProvenanceView,
    ): OutcomeCodeView? {
        seen += "publish staged=${work.staged} name=${work.displayName} total=${work.totalBytes}"
        return OutcomeCodeView.COMPLETED
    }

    override fun carryReceipt(): OutcomeCodeView {
        seen += "courier"
        return OutcomeCodeView.INTERNAL
    }

    override fun bindSource(provenance: DutyProvenanceView): OutcomeCodeView {
        seen += "source card=${provenance.card} gen=${provenance.generation}"
        return OutcomeCodeView.COMPLETED
    }
}

fun main(args: Array<String>) {
    // One order frame per line, exactly as the Rust encoder emitted it.
    File(args[0]).readLines().filter { it.isNotBlank() }.forEachIndexed { index, frame ->
        val effects = RecordingEffects()
        val report = DutyRouter.route(frame.toByteArray(Charsets.UTF_8), effects)
        val outcome =
            report?.let {
                // Decoding our own report proves the encoder produced something
                // the contract accepts, not merely something non-null.
                val decoded =
                    com.envoix.bindings.duty.EnvoixDutyCodec
                        .decode(String(it, Charsets.UTF_8))
                (decoded.body as com.envoix.bindings.duty.DutyBody.Report).value.outcome.name
            } ?: "OUTSTANDING"
        println("$index effects=[${effects.seen.joinToString(";")}] report=$outcome")
    }
}
