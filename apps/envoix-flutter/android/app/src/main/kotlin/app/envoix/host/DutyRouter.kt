package app.envoix.host

import com.envoix.bindings.duty.DutyBody
import com.envoix.bindings.duty.DutyOrderView
import com.envoix.bindings.duty.DutyProvenanceView
import com.envoix.bindings.duty.DutyReportView
import com.envoix.bindings.duty.EnvoixDutyCodec
import com.envoix.bindings.duty.LockDirectiveView
import com.envoix.bindings.duty.NoticeView
import com.envoix.bindings.duty.OutcomeCodeView
import com.envoix.bindings.duty.PublicationWorkView
import com.envoix.bindings.duty.WorkView

/**
 * The platform side of one duty, with every Android API behind a name.
 *
 * Split out so the DECISION — which arm, with which typed payload, reported or
 * left outstanding — can be executed and asserted without a device. That half
 * used to be provable only by reading Kotlin source text, which is exactly how
 * a producer/consumer mismatch survived every gate: the labels agreed while the
 * shapes did not.
 *
 * `null` from any member means "leave this duty outstanding": no report is
 * produced, the ledger admits nothing, and the duty is re-delivered on the next
 * attachment. It is the honest answer for work this platform cannot perform
 * yet, and it is deliberately distinct from reporting a failure.
 */
interface DutyEffects {
    fun postNotice(
        provenance: DutyProvenanceView,
        notice: NoticeView,
    ): OutcomeCodeView

    fun holdLock(directive: LockDirectiveView): OutcomeCodeView

    /** The service asserts foreground state on boot; this is the acknowledgement. */
    fun assertForeground(activeTransfers: Long): OutcomeCodeView

    fun publish(
        work: PublicationWorkView,
        provenance: DutyProvenanceView,
    ): OutcomeCodeView?

    fun carryReceipt(): OutcomeCodeView

    fun bindSource(provenance: DutyProvenanceView): OutcomeCodeView
}

/**
 * Decodes one duty order and routes it to [DutyEffects].
 *
 * The `when` is over the generated sealed [WorkView] with no `else`, so the
 * Kotlin compiler — not a text scrape — is what makes a new arm impossible to
 * forget.
 */
object DutyRouter {
    /** Routes one encoded order; null leaves the duty outstanding. */
    fun route(
        order: ByteArray,
        effects: DutyEffects,
    ): ByteArray? {
        val frame =
            runCatching { EnvoixDutyCodec.decode(String(order, Charsets.UTF_8)) }.getOrNull()
                ?: return null
        // A report is a well-formed frame this side issues, never receives.
        val issued = (frame.body as? DutyBody.Order)?.value ?: return null
        val outcome = outcomeFor(issued, effects) ?: return null
        return EnvoixDutyCodec
            .encode(DutyReportView(provenance = issued.provenance, outcome = outcome))
            .toByteArray(Charsets.UTF_8)
    }

    private fun outcomeFor(
        issued: DutyOrderView,
        effects: DutyEffects,
    ): OutcomeCodeView? =
        when (val work = issued.work) {
            is WorkView.Notification -> effects.postNotice(issued.provenance, work.value.notice)
            is WorkView.Lock -> effects.holdLock(work.value.directive)
            is WorkView.Foreground -> effects.assertForeground(work.value.activeTransfers)
            is WorkView.Publication -> effects.publish(work.value, issued.provenance)
            WorkView.Courier -> effects.carryReceipt()
            WorkView.SourceHandle -> effects.bindSource(issued.provenance)
            // Shapes the vocabulary carries that this platform does not execute.
            // Outstanding is the honest answer, not a fabricated failure.
            WorkView.Grant, WorkView.Staging, WorkView.OpenShare -> null
        }
}
