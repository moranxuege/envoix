package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TransferPresentationPolicyTest {
    @Test
    fun `every state follows the shared action contract`() {
        val cases =
            mapOf(
                Status.Preparing to actions(cancel = true),
                Status.WaitingForPeer to actions(cancel = true),
                Status.Pairing to actions(cancel = true),
                Status.Connecting to actions(cancel = true),
                Status.AwaitingDecision to actions(cancel = true, approve = true),
                Status.Transferring to actions(cancel = true),
                Status.Verifying to actions(cancel = true),
                Status.Saving to actions(finalizing = true),
                Status.WaitingForReceiverSave to actions(finalizing = true),
                Status.FinalizingDelivery to actions(finalizing = true),
                Status.Paused to actions(cancel = true),
                Status.Delivered to actions(remove = true),
                Status.Failed to actions(remove = true),
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
                        recoveryAction =
                            if (state == Status.Failed) {
                                RecoveryAction.Resume
                            } else {
                                RecoveryAction.None
                            },
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
        assertFalse(
            TransferPresentationPolicy
                .actions(
                    Transfer(
                        id = 1,
                        direction = Direction.Send,
                        room = "test-room",
                        status = Status.Failed,
                        retryable = true,
                        recoveryAction = RecoveryAction.RePair,
                    ),
                ).canResume,
        )
    }

    @Test
    fun `post-payload stages retain progress until delivery is complete`() {
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
        assertEquals(TransferProgressPresentation.Hidden, TransferPresentationPolicy.progress(Status.Delivered))
    }

    @Test
    fun `per-entry verification stays transferring until aggregate bytes complete`() {
        val intermediate =
            TransferStatusPresentationReducer.decide(
                direction = Direction.Receive,
                current = Status.Transferring,
                reported = Status.Verifying,
                bytes = 512,
                total = 1_024,
            )
        assertEquals(Status.Transferring, intermediate.status)
        assertFalse(intermediate.shouldPublish)

        val nextEntry =
            TransferStatusPresentationReducer.decide(
                direction = Direction.Receive,
                current = intermediate.status,
                reported = Status.Transferring,
                bytes = 512,
                total = 1_024,
            )
        assertEquals(Status.Transferring, nextEntry.status)
        assertFalse(nextEntry.shouldPublish)

        val complete =
            TransferStatusPresentationReducer.decide(
                direction = Direction.Receive,
                current = nextEntry.status,
                reported = Status.Verifying,
                bytes = 1_024,
                total = 1_024,
            )
        assertEquals(Status.Verifying, complete.status)
        assertTrue(complete.shouldPublish)
    }

    @Test
    fun `final verification cannot regress to transferring`() {
        val finalVerification =
            TransferStatusPresentationReducer.decide(
                direction = Direction.Receive,
                current = Status.Transferring,
                reported = Status.Verifying,
                bytes = 1_024,
                total = 1_024,
            )
        val lateTransferring =
            TransferStatusPresentationReducer.decide(
                direction = Direction.Receive,
                current = finalVerification.status,
                reported = Status.Transferring,
                bytes = 1_024,
                total = 1_024,
            )
        assertEquals(Status.Verifying, lateTransferring.status)
        assertFalse(lateTransferring.shouldPublish)

        val saving =
            TransferStatusPresentationReducer.decide(
                direction = Direction.Receive,
                current = finalVerification.status,
                reported = Status.Saving,
                bytes = 1_024,
                total = 1_024,
            )
        assertEquals(Status.Saving, saving.status)
        assertTrue(saving.shouldPublish)
    }

    @Test
    fun `receiver suppression retains current state and sender phases remain exact`() {
        val suppressed =
            TransferStatusPresentationReducer.decide(
                direction = Direction.Receive,
                current = Status.Connecting,
                reported = Status.Verifying,
                bytes = 0,
                total = 1_024,
            )
        assertEquals(Status.Connecting, suppressed.status)
        assertFalse(suppressed.shouldPublish)

        val sender =
            TransferStatusPresentationReducer.decide(
                direction = Direction.Send,
                current = Status.Transferring,
                reported = Status.Verifying,
                bytes = 512,
                total = 1_024,
            )
        assertEquals(Status.Verifying, sender.status)
        assertTrue(sender.shouldPublish)
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
