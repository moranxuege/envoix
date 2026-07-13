package dev.envoix.app

import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToIndex
import dev.envoix.app.ui.EnvoixTestTags
import dev.envoix.app.ui.EnvoixTheme
import dev.envoix.app.ui.HomeScreen
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

class HomeScreenActivityInstrumentedTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun activityActionsMatchCanonicalLifecycle() {
        val transfers =
            listOf(
                transfer(4, Status.Transferring),
                transfer(3, Status.Paused),
                transfer(2, Status.Completed),
                transfer(1, Status.Failed, retryable = true),
            )

        compose.setContent {
            EnvoixTheme {
                HomeScreen(
                    transfers = transfers,
                    onReceive = { _, _, _, _ -> },
                    onSend = { _, _, _, _, _, _ -> },
                    onPauseResume = {},
                    onCancel = {},
                    onRemove = {},
                    onOpenLogs = {},
                    onOpen = {},
                )
            }
        }

        compose.onNodeWithTag(EnvoixTestTags.ACTIVITY_TAB).performClick()

        assertAction(4, TransferAction.Pause, exists = true)
        assertAction(4, TransferAction.Cancel, exists = true)
        assertAction(4, TransferAction.Resume, exists = false)
        assertAction(4, TransferAction.Delete, exists = false)

        assertAction(3, TransferAction.Resume, exists = true)
        assertAction(3, TransferAction.Cancel, exists = true)
        assertAction(3, TransferAction.Pause, exists = false)
        assertAction(3, TransferAction.Delete, exists = false)

        compose.onNodeWithTag(EnvoixTestTags.ACTIVITY_LIST).performScrollToIndex(3)

        assertAction(2, TransferAction.Delete, exists = true)
        assertAction(2, TransferAction.Pause, exists = false)
        assertAction(2, TransferAction.Resume, exists = false)
        assertAction(2, TransferAction.Cancel, exists = false)

        assertAction(1, TransferAction.Retry, exists = true)
        assertAction(1, TransferAction.Delete, exists = true)
        assertAction(1, TransferAction.Cancel, exists = false)
    }

    private fun assertAction(
        id: Long,
        action: TransferAction,
        exists: Boolean,
    ) {
        val actual =
            compose
                .onAllNodesWithTag(EnvoixTestTags.activityAction(id, action.name))
                .fetchSemanticsNodes()
                .isNotEmpty()
        assertEquals("$action visibility for activity $id", exists, actual)
    }

    private fun transfer(
        id: Long,
        status: Status,
        retryable: Boolean = false,
    ) = Transfer(
        id = id,
        direction = if (id % 2L == 0L) Direction.Send else Direction.Receive,
        room = "123456-ui-test",
        fileName = "activity-$id-long-file-name.mp4",
        pathType = "relay",
        pathAddr = "https://envoix.chkxwlyh.us:8444/",
        bytes = 41_000_000,
        total = 92_000_000,
        speedBps = 12_400_000.0,
        avgBps = 10_800_000.0,
        status = status,
        retryable = retryable,
    )
}
