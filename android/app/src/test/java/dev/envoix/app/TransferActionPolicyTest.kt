package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Test

class TransferActionPolicyTest {
    @Test
    fun actionPolicyMatchesCanonicalLifecycle() {
        assertActions(Status.Waiting, TransferAction.Pause, TransferAction.Cancel)
        assertActions(Status.Connecting, TransferAction.Pause, TransferAction.Cancel)
        assertActions(Status.Verifying, TransferAction.Pause, TransferAction.Cancel)
        assertActions(Status.Transferring, TransferAction.Pause, TransferAction.Cancel)
        assertActions(Status.Paused, TransferAction.Resume, TransferAction.Cancel)
        assertActions(Status.Unconfirmed, TransferAction.Retry, TransferAction.Cancel)
        assertActions(Status.Confirming)
        assertActions(Status.Publishing)
        assertActions(Status.Cancelled, TransferAction.Delete)
    }

    @Test
    fun publishingActionsRequireAnActualPublicationFailure() {
        assertEquals(
            listOf(TransferAction.Retry, TransferAction.Cancel),
            actions(Status.Publishing, error = "publish failed"),
        )
    }

    @Test
    fun failedRetryRequiresCanonicalRetryableFlag() {
        assertEquals(listOf(TransferAction.Delete), actions(Status.Failed))
        assertEquals(
            listOf(TransferAction.Retry, TransferAction.Delete),
            actions(Status.Failed, retryable = true),
        )
    }

    @Test
    fun completedOpenRequiresPublishedUri() {
        assertEquals(listOf(TransferAction.Delete), actions(Status.Completed))
        assertEquals(
            listOf(TransferAction.Open, TransferAction.Delete),
            actions(Status.Completed, savedUri = "content://downloads/file"),
        )
    }

    private fun assertActions(
        status: Status,
        vararg expected: TransferAction,
    ) {
        assertEquals(expected.toList(), actions(status))
    }

    private fun actions(
        status: Status,
        retryable: Boolean = false,
        error: String? = null,
        savedUri: String? = null,
    ): List<TransferAction> =
        availableTransferActions(
            Transfer(
                id = 1,
                direction = Direction.Receive,
                room = "123456-test-room",
                status = status,
                retryable = retryable,
                error = error,
                savedUri = savedUri,
            ),
        )
}
