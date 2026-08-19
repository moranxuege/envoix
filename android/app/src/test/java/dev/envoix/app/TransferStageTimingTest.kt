package dev.envoix.app

import dev.envoix.app.ui.TransferStageTimelineEntry
import dev.envoix.app.ui.formatTransferStageElapsed
import dev.envoix.app.ui.latestTransferStageTimeline
import dev.envoix.app.ui.transferStageTimelineTitleResource
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TransferStageTimingTest {
    @Test
    fun `complete structured sample parses without losing correlation`() {
        val sample =
            TransferStageTimingParser.parse(
                stageWire = "manifest_accepted",
                directionWire = "send",
                attemptId = 7,
                transferId = "job-42",
                elapsedUs = 125_000,
                deltaUs = 25_000,
            )

        assertEquals(
            TransferStageTiming(
                transferId = "job-42",
                direction = Direction.Send,
                attemptId = 7,
                stage = TransferStage.ManifestAccepted,
                elapsedUs = 125_000,
                deltaUs = 25_000,
            ),
            sample,
        )
    }

    @Test
    fun `missing transfer id remains explicitly unbound`() {
        val sample =
            TransferStageTimingParser.parse(
                stageWire = "session_started",
                directionWire = "receive",
                attemptId = 9,
                transferId = null,
                elapsedUs = 10,
                deltaUs = 10,
            )

        assertEquals(null, sample?.transferId)
        assertEquals(Direction.Receive, sample?.direction)
    }

    @Test
    fun `unknown stage and invalid numeric relationships are rejected`() {
        assertNull(
            TransferStageTimingParser.parse(
                stageWire = "made_up_stage",
                directionWire = "send",
                attemptId = 1,
                transferId = null,
                elapsedUs = 20,
                deltaUs = 20,
            ),
        )
        assertNull(
            TransferStageTimingParser.parse(
                stageWire = "first_payload",
                directionWire = "send",
                attemptId = 1,
                transferId = null,
                elapsedUs = 20,
                deltaUs = 21,
            ),
        )
        assertNull(
            TransferStageTimingParser.parse(
                stageWire = "first_payload",
                directionWire = "send",
                attemptId = -1,
                transferId = null,
                elapsedUs = 20,
                deltaUs = 20,
            ),
        )
    }

    @Test
    fun `history rejects duplicate and out of order samples for one attempt`() {
        val first = sample(attemptId = 3, stage = TransferStage.SessionStarted, elapsedUs = 10, deltaUs = 10)
        val accepted =
            TransferStageTimingHistory.append(
                listOf(first),
                sample(
                    attemptId = 3,
                    stage = TransferStage.AuthenticationStarted,
                    elapsedUs = 30,
                    deltaUs = 20,
                ),
            )
        assertTrue(accepted.accepted)

        val duplicate =
            TransferStageTimingHistory.append(
                accepted.samples,
                sample(
                    attemptId = 3,
                    stage = TransferStage.AuthenticationStarted,
                    elapsedUs = 40,
                    deltaUs = 10,
                ),
            )
        assertFalse(duplicate.accepted)
        assertEquals(accepted.samples, duplicate.samples)

        val reversed =
            TransferStageTimingHistory.append(
                accepted.samples,
                sample(
                    attemptId = 3,
                    stage = TransferStage.ConnectionReady,
                    elapsedUs = 40,
                    deltaUs = 10,
                ),
            )
        assertFalse(reversed.accepted)
    }

    @Test
    fun `history retains only the named sample cap`() {
        val appended =
            (0..TransferStageTimingHistory.SAMPLE_CAP).fold(
                TransferStageTimingAppendResult(emptyList(), accepted = true),
            ) { current, attemptId ->
                TransferStageTimingHistory.append(
                    current.samples,
                    sample(
                        attemptId = attemptId.toLong(),
                        stage = TransferStage.SessionStarted,
                        elapsedUs = 1,
                        deltaUs = 1,
                    ),
                )
            }

        assertEquals(TransferStageTimingHistory.SAMPLE_CAP, appended.samples.size)
        assertEquals(1L, appended.samples.first().attemptId)
        assertEquals(TransferStageTimingHistory.SAMPLE_CAP.toLong(), appended.samples.last().attemptId)
    }

    @Test
    fun `timeline keeps only the latest retry and sorts by elapsed time`() {
        val projected =
            latestTransferStageTimeline(
                listOf(
                    sample(1, TransferStage.SessionStarted, elapsedUs = 0, deltaUs = 0),
                    sample(2, TransferStage.FirstPayload, elapsedUs = 250_000, deltaUs = 150_000),
                    sample(2, TransferStage.AuthenticationComplete, elapsedUs = 100_000, deltaUs = 20_000),
                    sample(1, TransferStage.DeliveryComplete, elapsedUs = 900_000, deltaUs = 900_000),
                    sample(2, TransferStage.SessionStarted, elapsedUs = 50_000, deltaUs = 50_000),
                    sample(2, TransferStage.ConnectionReady, elapsedUs = 80_000, deltaUs = 30_000),
                ),
            )

        assertEquals(
            listOf(
                TransferStageTimelineEntry(TransferStage.SessionStarted, elapsedFromSessionUs = 0),
                TransferStageTimelineEntry(TransferStage.ConnectionReady, elapsedFromSessionUs = 30_000),
                TransferStageTimelineEntry(TransferStage.AuthenticationComplete, elapsedFromSessionUs = 50_000),
                TransferStageTimelineEntry(TransferStage.FirstPayload, elapsedFromSessionUs = 200_000),
            ),
            projected,
        )
    }

    @Test
    fun `timeline is empty without samples or a session start baseline`() {
        assertTrue(latestTransferStageTimeline(emptyList()).isEmpty())
        assertTrue(
            latestTransferStageTimeline(
                listOf(
                    sample(4, TransferStage.ConnectionReady, elapsedUs = 20_000, deltaUs = 20_000),
                ),
            ).isEmpty(),
        )
    }

    @Test
    fun `elapsed formatter has stable microsecond millisecond second and minute boundaries`() {
        val cases =
            listOf(
                0L to "0 µs",
                999L to "999 µs",
                1_000L to "1 ms",
                1_100L to "1.1 ms",
                999_999L to "999.9 ms",
                1_000_000L to "1 s",
                1_100_000L to "1.1 s",
                59_999_999L to "59.9 s",
                60_000_000L to "1m 00s",
                61_000_000L to "1m 01s",
            )

        cases.forEach { (elapsedUs, expected) ->
            assertEquals(expected, formatTransferStageElapsed(elapsedUs))
        }
    }

    @Test
    fun `timeline stages map to stable presentation resources`() {
        assertEquals(R.string.remembered_connection_connected, transferStageTimelineTitleResource(TransferStage.ConnectionReady))
        assertEquals(R.string.activity_stage_authenticated, transferStageTimelineTitleResource(TransferStage.AuthenticationComplete))
        assertEquals(R.string.activity_stage_first_byte, transferStageTimelineTitleResource(TransferStage.FirstPayload))
        assertEquals(R.string.activity_stage_payload_complete, transferStageTimelineTitleResource(TransferStage.PayloadComplete))
        assertEquals(R.string.transfer_status_delivered, transferStageTimelineTitleResource(TransferStage.DeliveryComplete))
    }

    private fun sample(
        attemptId: Long,
        stage: TransferStage,
        elapsedUs: Long,
        deltaUs: Long,
    ) = TransferStageTiming(
        transferId = null,
        direction = Direction.Send,
        attemptId = attemptId,
        stage = stage,
        elapsedUs = elapsedUs,
        deltaUs = deltaUs,
    )
}
