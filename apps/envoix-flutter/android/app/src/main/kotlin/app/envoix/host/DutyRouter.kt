package app.envoix.host

import com.envoix.bindings.duty.DutyAnswerView
import com.envoix.bindings.duty.DutyBody
import com.envoix.bindings.duty.DutyOrderView
import com.envoix.bindings.duty.DutyProvenanceView
import com.envoix.bindings.duty.DutyReportView
import com.envoix.bindings.duty.EnvoixDutyCodec
import com.envoix.bindings.duty.LockDirectiveView
import com.envoix.bindings.duty.NoticeView
import com.envoix.bindings.duty.OutcomeCodeView
import com.envoix.bindings.duty.PublicationWorkView
import com.envoix.bindings.duty.SourceReportView
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

    /**
     * Takes hold of the document chosen for this acquisition and reports what
     * the platform will promise about it.
     *
     * NOT an outcome code. `completed` cannot say whether the hold survives a
     * restart or whether the source can be re-read from an offset, and those two
     * facts are what decide whether the send streams from the provider or must
     * copy first. An acquisition that answered only an outcome forced the
     * authority to invent them.
     */
    fun bindSource(provenance: DutyProvenanceView): SourceReportView
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
        val answer = answerFor(issued, effects) ?: return null
        return EnvoixDutyCodec
            .encode(DutyReportView(provenance = issued.provenance, answer = answer))
            .toByteArray(Charsets.UTF_8)
    }

    /**
     * The answer in the vocabulary this duty's kind speaks. The source handle is
     * the one kind that does not answer an outcome; the `when` is over the
     * generated sealed union with no `else`, so a new kind cannot silently
     * inherit the wrong one.
     */
    private fun answerFor(
        issued: DutyOrderView,
        effects: DutyEffects,
    ): DutyAnswerView? =
        when (val work = issued.work) {
            is WorkView.Notification ->
                DutyAnswerView.Outcome(effects.postNotice(issued.provenance, work.value.notice))
            is WorkView.Lock -> DutyAnswerView.Outcome(effects.holdLock(work.value.directive))
            is WorkView.Foreground ->
                DutyAnswerView.Outcome(effects.assertForeground(work.value.activeTransfers))
            is WorkView.Publication ->
                effects.publish(work.value, issued.provenance)?.let(DutyAnswerView::Outcome)
            WorkView.Courier -> DutyAnswerView.Outcome(effects.carryReceipt())
            WorkView.SourceHandle -> DutyAnswerView.Source(effects.bindSource(issued.provenance))
            // Shapes the vocabulary carries that this platform does not execute.
            // Outstanding is the honest answer, not a fabricated failure.
            WorkView.Grant, WorkView.Staging, WorkView.OpenShare -> null
        }
}
