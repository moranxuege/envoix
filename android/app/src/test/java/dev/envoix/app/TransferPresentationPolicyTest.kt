package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class TransferPresentationPolicyTest {
    @Test
    fun `every state follows the shared action contract`() {
        val cases =
            mapOf(
                Status.Preparing to actions(cancel = true),
                Status.WaitingForPeer to actions(pause = true, cancel = true),
                Status.Pairing to actions(pause = true, cancel = true),
                Status.Connecting to actions(pause = true, cancel = true),
                Status.AwaitingDecision to actions(cancel = true, approve = true),
                Status.Transferring to actions(pause = true, cancel = true),
                Status.Verifying to actions(pause = true, cancel = true),
                Status.Saving to actions(finalizing = true),
                Status.WaitingForReceiverSave to actions(finalizing = true),
                Status.FinalizingDelivery to actions(finalizing = true),
                Status.Paused to actions(resume = true, cancel = true),
                Status.Delivered to actions(remove = true),
                Status.Failed to actions(resume = true, remove = true),
                Status.Canceled to actions(remove = true),
            )

        cases.forEach { (state, expected) ->
            assertEquals(
                "Unexpected actions for $state",
                expected,
                TransferPresentationPolicy.actions(
                    Transfer(
                        id = 1,
                        direction = Direction.Send,
                        room = "test-room",
                        status = state,
                        retryable = state == Status.Failed,
                    ),
                ),
            )
        }
        assertFalse(
            TransferPresentationPolicy
                .actions(
                    Transfer(
                        id = 1,
                        direction = Direction.Send,
                        room = "test-room",
                        status = Status.Failed,
                    ),
                ).canResume,
        )
    }

    @Test
    fun `post-payload stages retain complete progress`() {
        assertEquals(TransferProgressPresentation.Hidden, TransferPresentationPolicy.progress(Status.Connecting))
        assertEquals(TransferProgressPresentation.Hidden, TransferPresentationPolicy.progress(Status.AwaitingDecision))
        assertEquals(TransferProgressPresentation.Active, TransferPresentationPolicy.progress(Status.Transferring))
        assertEquals(TransferProgressPresentation.Retained, TransferPresentationPolicy.progress(Status.Paused))
        assertEquals(TransferProgressPresentation.Retained, TransferPresentationPolicy.progress(Status.Failed))
        assertEquals(TransferProgressPresentation.Complete, TransferPresentationPolicy.progress(Status.Verifying))
        assertEquals(TransferProgressPresentation.Complete, TransferPresentationPolicy.progress(Status.Saving))
        assertEquals(
            TransferProgressPresentation.Complete,
            TransferPresentationPolicy.progress(Status.WaitingForReceiverSave),
        )
        assertEquals(
            TransferProgressPresentation.Complete,
            TransferPresentationPolicy.progress(Status.FinalizingDelivery),
        )
        assertEquals(TransferProgressPresentation.Complete, TransferPresentationPolicy.progress(Status.Delivered))
    }

    private fun actions(
        pause: Boolean = false,
        resume: Boolean = false,
        cancel: Boolean = false,
        approve: Boolean = false,
        remove: Boolean = false,
        finalizing: Boolean = false,
    ) = TransferActionAvailability(
        canPause = pause,
        canResume = resume,
        canCancel = cancel,
        canApprove = approve,
        canRemove = remove,
        isFinalizing = finalizing,
    )
}
