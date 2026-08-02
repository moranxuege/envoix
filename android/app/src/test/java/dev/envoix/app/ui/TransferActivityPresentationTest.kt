package dev.envoix.app.ui

import dev.envoix.app.Direction
import dev.envoix.app.Status
import dev.envoix.app.Transfer
import dev.envoix.app.TransferActivityGroup
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TransferActivityPresentationTest {
    @Test
    fun `activity groups opaque transfer references only by stable presentation id`() {
        val oneTimeGroup = TransferActivityGroup.oneTime("draft-1")
        val rememberedGroup = TransferActivityGroup.remembered("relationship-1")
        val groups =
            groupTransfersForActivity(
                listOf(
                    transfer(
                        id = 1,
                        room = "AZaz09_-AZaz09_-AZaz01",
                        activityGroupId = oneTimeGroup,
                        activityGroupLabel = "Nearby phone",
                    ),
                    transfer(
                        id = 3,
                        room = "AZaz09_-AZaz09_-AZaz03",
                        activityGroupId = rememberedGroup,
                        activityGroupLabel = "Family room",
                    ),
                    transfer(
                        id = 4,
                        room = "AZaz09_-AZaz09_-AZaz04",
                        activityGroupId = oneTimeGroup,
                        activityGroupLabel = "Nearby phone",
                    ),
                ),
            )

        assertEquals(listOf(4L, 1L), groups[0].transfers.map(Transfer::id))
        assertEquals("Nearby phone", groups[0].label)
        assertEquals(listOf(3L), groups[1].transfers.map(Transfer::id))
        assertEquals("Family room", groups[1].label)
    }

    @Test
    fun `duplicate labels do not merge distinct remembered relationships`() {
        val groups =
            groupTransfersForActivity(
                listOf(
                    transfer(
                        id = 1,
                        room = "opaque-reference-1",
                        activityGroupId = TransferActivityGroup.remembered("relationship-1"),
                        activityGroupLabel = "Phone",
                    ),
                    transfer(
                        id = 2,
                        room = "opaque-reference-2",
                        activityGroupId = TransferActivityGroup.remembered("relationship-2"),
                        activityGroupLabel = "Phone",
                    ),
                ),
            )

        assertEquals(2, groups.size)
        assertEquals(listOf("Phone", "Phone"), groups.map(ActivityRoomGroup::label))
        assertNotEquals(groups[0].key, groups[1].key)
    }

    @Test
    fun `activity isolates transfers without a presentation group regardless of room value`() {
        val groups =
            groupTransfersForActivity(
                listOf(
                    transfer(id = 1, room = "same-opaque-reference"),
                    transfer(id = 2, room = "same-opaque-reference"),
                    transfer(id = 3, room = ""),
                    transfer(id = 4, room = "R123456-a1b2-c3d4"),
                ),
            )

        assertEquals(4, groups.size)
        assertEquals(listOf(4L, 3L, 2L, 1L), groups.map { it.transfers.single().id })
        assertTrue(groups.all { it.label == null })
    }

    @Test
    fun `activity ordering uses newest transfer then stable group key`() {
        val groups =
            groupTransfersForActivity(
                listOf(
                    transfer(
                        id = 7,
                        room = "opaque-b",
                        activityGroupId = TransferActivityGroup.oneTime("b"),
                    ),
                    transfer(
                        id = 7,
                        room = "opaque-a",
                        activityGroupId = TransferActivityGroup.oneTime("a"),
                    ),
                ),
            )

        assertEquals(
            listOf("activity:one-time:a", "activity:one-time:b"),
            groups.map(ActivityRoomGroup::key),
        )
    }

    @Test
    fun `room title uses only explicit label and never the transport reference`() {
        assertEquals(
            "Family room",
            activityRoomDisplayName(" Family room ", AppText.ENGLISH),
        )
        assertEquals(
            "一次性房间",
            activityRoomDisplayName(null, AppText.SIMPLIFIED_CHINESE),
        )
        assertEquals(
            "One-time room",
            activityRoomDisplayName(" ", AppText.ENGLISH),
        )
        assertEquals(
            "Direct transfer",
            activityRoomDisplayName(null, AppText.ENGLISH, isDirect = true),
        )
        assertFalse(activityRoomDisplayName(null, AppText.ENGLISH).contains("opaque"))
    }

    @Test
    fun `room card reports only observed data paths in stable order`() {
        val paths =
            activityRoomDataPaths(
                listOf(
                    transfer(id = 1, room = "one", pathAddr = "relay (hidden endpoint)"),
                    transfer(id = 2, room = "two", pathAddr = "direct"),
                    transfer(id = 3, room = "three", pathAddr = "wifi_aware"),
                    transfer(id = 4, room = "four", pathAddr = null),
                ),
                AppText.ENGLISH,
            )

        assertEquals(listOf("Direct", "Relay", "Wi-Fi Aware"), paths)
    }

    @Test
    fun `room metrics aggregate status inventory progress and finite speed`() {
        val metrics =
            activityRoomMetrics(
                listOf(
                    transfer(
                        id = 1,
                        room = "opaque-reference-1",
                        status = Status.Transferring,
                        bytes = 2_000,
                        total = 4_000,
                        avgBps = 200.0,
                        speedHistory = listOf(300.0, 500.0),
                        fileCount = 2,
                    ),
                    transfer(
                        id = 2,
                        room = "opaque-reference-2",
                        status = Status.Delivered,
                        bytes = 6_000,
                        total = 6_000,
                        avgBps = 600.0,
                        directoryCount = 1,
                    ),
                ),
            )

        assertEquals(2, metrics.transferCount)
        assertEquals(1, metrics.activeCount)
        assertEquals(0, metrics.pausedCount)
        assertEquals(1, metrics.deliveredCount)
        assertEquals(2, metrics.fileCount)
        assertEquals(1, metrics.directoryCount)
        assertEquals(8_000L, metrics.bytes)
        assertEquals(10_000L, metrics.total)
        assertEquals(400.0, metrics.currentBps, 0.001)
        assertEquals(400.0, metrics.averageBps, 0.001)
        assertEquals(5.0, requireNotNull(metrics.etaSeconds), 0.001)
        assertEquals(
            "Now 400 B/s · ETA 5s",
            activityRoomPerformanceSummary(metrics, AppText.ENGLISH),
        )
        assertEquals(ActivityRoomStatusKind.Active, activityRoomStatusKind(metrics))
    }

    @Test
    fun `room eta excludes unfinished historical transfers`() {
        val metrics =
            activityRoomMetrics(
                listOf(
                    transfer(
                        id = 1,
                        room = "active",
                        status = Status.Transferring,
                        bytes = 100,
                        total = 1_000,
                        speedHistory = listOf(100.0),
                    ),
                    transfer(
                        id = 2,
                        room = "failed",
                        status = Status.Failed,
                        bytes = 100,
                        total = 1_000,
                    ),
                    transfer(
                        id = 3,
                        room = "paused",
                        status = Status.Paused,
                        bytes = 100,
                        total = 1_000,
                    ),
                ),
            )

        assertEquals(100.0, metrics.currentBps, 0.001)
        assertEquals(9.0, requireNotNull(metrics.etaSeconds), 0.001)
    }

    @Test
    fun `room average throughput is weighted by transferred bytes and elapsed time`() {
        val metrics =
            activityRoomMetrics(
                listOf(
                    transfer(
                        id = 1,
                        room = "one",
                        status = Status.Delivered,
                        bytes = 100,
                        total = 100,
                        avgBps = 100.0,
                    ),
                    transfer(
                        id = 2,
                        room = "two",
                        status = Status.Delivered,
                        bytes = 900,
                        total = 900,
                        avgBps = 300.0,
                    ),
                ),
            )

        assertEquals(250.0, metrics.averageBps, 0.001)
        assertNull(metrics.etaSeconds)
    }

    @Test
    fun `current work wins over historical failure while paused work is not active`() {
        val active =
            activityRoomMetrics(
                listOf(
                    transfer(id = 1, room = "opaque-1", status = Status.Failed),
                    transfer(id = 2, room = "opaque-2", status = Status.Paused),
                    transfer(id = 3, room = "opaque-3", status = Status.Transferring),
                ),
            )
        val paused =
            activityRoomMetrics(
                listOf(
                    transfer(id = 1, room = "opaque-1", status = Status.Failed),
                    transfer(id = 2, room = "opaque-2", status = Status.Paused),
                ),
            )

        assertEquals(1, active.activeCount)
        assertEquals(1, active.pausedCount)
        assertEquals(ActivityRoomStatusKind.Active, activityRoomStatusKind(active))
        assertEquals(0, paused.activeCount)
        assertEquals(ActivityRoomStatusKind.Paused, activityRoomStatusKind(paused))
    }

    @Test
    fun `room metrics reject invalid values and saturate every aggregate`() {
        val metrics =
            activityRoomMetrics(
                listOf(
                    transfer(
                        id = 1,
                        room = "room",
                        status = Status.Transferring,
                        bytes = -10,
                        total = -20,
                        avgBps = Double.NaN,
                        speedHistory = listOf(Double.POSITIVE_INFINITY),
                        fileCount = -1,
                        directoryCount = -1,
                    ),
                    transfer(
                        id = 2,
                        room = "opaque-2",
                        status = Status.Transferring,
                        bytes = Long.MAX_VALUE,
                        total = Long.MAX_VALUE,
                        avgBps = Double.MAX_VALUE,
                        speedHistory = listOf(Double.MAX_VALUE),
                        fileCount = Int.MAX_VALUE,
                        directoryCount = Int.MAX_VALUE,
                    ),
                    transfer(
                        id = 3,
                        room = "opaque-3",
                        status = Status.Transferring,
                        bytes = Long.MAX_VALUE,
                        total = Long.MAX_VALUE,
                        avgBps = Double.MAX_VALUE,
                        speedHistory = listOf(Double.MAX_VALUE),
                        fileCount = Int.MAX_VALUE,
                        directoryCount = Int.MAX_VALUE,
                    ),
                ),
            )

        assertEquals(Long.MAX_VALUE, metrics.bytes)
        assertEquals(Long.MAX_VALUE, metrics.total)
        assertEquals(Int.MAX_VALUE, metrics.fileCount)
        assertEquals(Int.MAX_VALUE, metrics.directoryCount)
        assertTrue(metrics.currentBps.isFinite())
        assertTrue(metrics.averageBps.isFinite())
        assertEquals(Long.MAX_VALUE.toDouble(), metrics.currentBps, 0.0)
        assertEquals(Long.MAX_VALUE.toDouble(), metrics.averageBps, 0.0)
    }

    @Test
    fun `empty group label remains absent instead of falling back to room handle`() {
        val group =
            groupTransfersForActivity(
                listOf(
                    transfer(
                        id = 1,
                        room = "opaque-reference",
                        activityGroupId = TransferActivityGroup.oneTime("draft"),
                        activityGroupLabel = " ",
                    ),
                ),
            ).single()

        assertNull(group.label)
    }

    @Test
    fun `recorded destination remains stable after settings change`() {
        assertEquals(
            "Family archive",
            resolvedSavedDestinationLabel(
                recordedDestinationLabel = "Family archive",
                fallbackDestinationLabel = "New downloads folder",
            ),
        )
    }

    @Test
    fun `legacy activity without recorded destination uses current setting`() {
        assertEquals(
            "Current downloads folder",
            resolvedSavedDestinationLabel(
                recordedDestinationLabel = null,
                fallbackDestinationLabel = "Current downloads folder",
            ),
        )
        assertEquals(
            "Current downloads folder",
            resolvedSavedDestinationLabel(
                recordedDestinationLabel = " ",
                fallbackDestinationLabel = "Current downloads folder",
            ),
        )
    }

    @Test
    fun `delivered activity uses configured destination in both languages`() {
        assertEquals(
            "Saved to Family archive · tap for details",
            savedDestinationSubtitle("Family archive", AppText.ENGLISH),
        )
        assertEquals(
            "已保存到 家庭归档 · 点击查看详情",
            savedDestinationSubtitle("家庭归档", AppText.SIMPLIFIED_CHINESE),
        )
    }

    @Test
    fun `delivered activity falls back to Downloads for an empty destination`() {
        assertEquals(
            "Saved to Downloads · tap for details",
            savedDestinationSubtitle("  ", AppText.ENGLISH),
        )
        assertEquals(
            "Downloads",
            resolvedSavedDestinationLabel(
                recordedDestinationLabel = null,
                fallbackDestinationLabel = " ",
            ),
        )
    }

    private fun transfer(
        id: Long,
        room: String,
        status: Status = Status.Connecting,
        bytes: Long = 0,
        total: Long = 0,
        avgBps: Double = 0.0,
        speedHistory: List<Double> = emptyList(),
        fileCount: Int = 0,
        directoryCount: Int = 0,
        activityGroupId: String? = null,
        activityGroupLabel: String? = null,
        pathAddr: String? = null,
    ) = Transfer(
        id = id,
        direction = Direction.Send,
        room = room,
        activityGroupId = activityGroupId,
        activityGroupLabel = activityGroupLabel,
        pathAddr = pathAddr,
        status = status,
        bytes = bytes,
        total = total,
        avgBps = avgBps,
        speedHistory = speedHistory,
        fileCount = fileCount,
        directoryCount = directoryCount,
    )
}
