package dev.envoix.app.ui

import dev.envoix.app.Direction
import dev.envoix.app.Status
import dev.envoix.app.Transfer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class RememberedRoomTransferPresentationTest {
    @Test
    fun `latest delivered receive is projected into its remembered room`() {
        val transfers =
            listOf(
                transfer(1, Direction.Receive, Status.Delivered),
                transfer(2, Direction.Send, Status.Delivered),
                transfer(3, Direction.Receive, Status.Transferring),
                transfer(4, Direction.Receive, Status.Delivered),
                transfer(5, Direction.Receive, Status.Delivered),
            )
        val relationships =
            mapOf(
                1L to "room-a",
                2L to "room-a",
                3L to "room-a",
                4L to "room-b",
                5L to "room-a",
            )

        val result = latestDeliveredReceivesByRelationship(transfers, relationships)

        assertEquals(5L, result.getValue("room-a").id)
        assertEquals(4L, result.getValue("room-b").id)
    }

    @Test
    fun `unassociated and unfinished receives are not shown as room results`() {
        val result =
            latestDeliveredReceivesByRelationship(
                transfers =
                    listOf(
                        transfer(1, Direction.Receive, Status.Delivered),
                        transfer(2, Direction.Receive, Status.Verifying),
                    ),
                relationshipByTransferId = mapOf(2L to "room-a"),
            )

        assertFalse(result.containsKey("room-a"))
    }

    private fun transfer(
        id: Long,
        direction: Direction,
        status: Status,
    ) = Transfer(
        id = id,
        direction = direction,
        room = "transfer-room-$id",
        status = status,
    )
}
