package dev.envoix.app.ui

import androidx.annotation.StringRes
import dev.envoix.app.ConnectionPathKind
import dev.envoix.app.Direction
import dev.envoix.app.R
import dev.envoix.app.TransferStage
import dev.envoix.app.TransferStageTiming

internal data class TransferActivityPresentationEnvironment(
    val defaultDestinationLabel: String,
    val developerMode: Boolean,
    val canUploadDiagnostics: Boolean,
)

@StringRes
internal fun waitingTransferSubtitleResource(direction: Direction): Int =
    if (direction == Direction.Send) {
        R.string.activity_sending_item
    } else {
        R.string.activity_saving_to
    }

internal data class TransferStageTimelineEntry(
    val stage: TransferStage,
    val elapsedFromSessionUs: Long,
)

internal fun latestTransferStageTimeline(samples: List<TransferStageTiming>): List<TransferStageTimelineEntry> {
    val latestAttemptId = samples.maxOfOrNull(TransferStageTiming::attemptId) ?: return emptyList()
    val latestAttempt = samples.filter { it.attemptId == latestAttemptId }
    val sessionStartedAt =
        latestAttempt
            .filter { it.stage == TransferStage.SessionStarted }
            .minOfOrNull(TransferStageTiming::elapsedUs)
            ?: return emptyList()
    return latestAttempt
        .asSequence()
        .filter { it.elapsedUs >= sessionStartedAt }
        .sortedWith(compareBy<TransferStageTiming> { it.elapsedUs }.thenBy { it.stage.order })
        .distinctBy(TransferStageTiming::stage)
        .map {
            TransferStageTimelineEntry(
                stage = it.stage,
                elapsedFromSessionUs = it.elapsedUs - sessionStartedAt,
            )
        }.toList()
}

internal fun formatTransferStageElapsed(elapsedUs: Long): String {
    val safeElapsedUs = elapsedUs.coerceAtLeast(0L)
    return when {
        safeElapsedUs < 1_000L -> "$safeElapsedUs µs"
        safeElapsedUs < 1_000_000L -> "${formatTenths(safeElapsedUs / 100L)} ms"
        safeElapsedUs < 60_000_000L -> "${formatTenths(safeElapsedUs / 100_000L)} s"
        else -> {
            val totalSeconds = safeElapsedUs / 1_000_000L
            val minutes = totalSeconds / 60L
            val seconds = totalSeconds % 60L
            "${minutes}m ${seconds.toString().padStart(2, '0')}s"
        }
    }
}

private fun formatTenths(value: Long): String =
    if (value % 10L == 0L) {
        (value / 10L).toString()
    } else {
        "${value / 10L}.${value % 10L}"
    }

@StringRes
internal fun transferStageTimelineTitleResource(stage: TransferStage): Int =
    when (stage) {
        TransferStage.SessionStarted -> R.string.activity_stage_started
        TransferStage.ConnectionReady -> R.string.remembered_connection_connected
        TransferStage.AuthenticationStarted -> R.string.activity_stage_authenticating
        TransferStage.AuthenticationComplete -> R.string.activity_stage_authenticated
        TransferStage.ManifestOffer -> R.string.activity_stage_file_list_ready
        TransferStage.ManifestAccepted -> R.string.activity_stage_file_list_accepted
        TransferStage.FirstPayload -> R.string.activity_stage_first_byte
        TransferStage.PayloadComplete -> R.string.activity_stage_payload_complete
        TransferStage.DeliveryComplete -> R.string.transfer_status_delivered
        TransferStage.Canceled -> R.string.transfer_status_canceled
        TransferStage.Failed -> R.string.transfer_status_failed
    }

@StringRes
internal fun connectionPathLabelResource(kind: ConnectionPathKind): Int =
    when (kind) {
        ConnectionPathKind.Direct -> R.string.connection_path_direct
        ConnectionPathKind.DirectIpv4 -> R.string.connection_path_direct_ipv4
        ConnectionPathKind.DirectIpv6 -> R.string.connection_path_direct_ipv6
        ConnectionPathKind.Relay -> R.string.connection_path_relay
        ConnectionPathKind.WifiAware -> R.string.hub_wifi_aware
        ConnectionPathKind.Other -> R.string.connection_path_other
    }

internal fun resolvedSavedDestinationLabel(
    recordedDestinationLabel: String?,
    fallbackDestinationLabel: String,
): String =
    recordedDestinationLabel
        ?.trim()
        ?.takeIf(String::isNotEmpty)
        ?: fallbackDestinationLabel.trim().ifEmpty { "Downloads" }
