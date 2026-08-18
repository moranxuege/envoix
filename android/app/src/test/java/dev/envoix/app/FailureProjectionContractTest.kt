package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class FailureProjectionContractTest {
    @Test
    fun `typed terminal outcomes map directly to product states`() {
        assertEquals(Status.Canceled, FailureOutcome.fromWire("canceled")?.status)
        assertEquals(Status.Failed, FailureOutcome.fromWire("failed")?.status)
        assertNull(FailureOutcome.fromWire("user_canceled"))
    }

    @Test
    fun `session disposition has an exact wire contract`() {
        assertEquals(
            FailureSessionDisposition.RetainForRecovery,
            FailureSessionDisposition.fromWire("retain_for_recovery"),
        )
        assertEquals(
            FailureSessionDisposition.Release,
            FailureSessionDisposition.fromWire("release"),
        )
        assertNull(FailureSessionDisposition.fromWire("retryable"))
    }
}
