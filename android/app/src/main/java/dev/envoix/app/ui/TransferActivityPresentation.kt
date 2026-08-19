package dev.envoix.app.ui

import dev.envoix.app.Direction
import dev.envoix.app.TransferStage
import dev.envoix.app.TransferStageTiming

internal data class TransferActivityPresentationEnvironment(
    val defaultDestinationLabel: String,
    val developerMode: Boolean,
    val canUploadDiagnostics: Boolean,
)

internal fun waitingTransferSubtitle(
    direction: Direction,
    itemTitle: String,
    destinationLabel: String,
    language: String,
): String =
    if (direction == Direction.Send) {
        AppText.value(
            "Sending $itemTitle",
            "准备发送 $itemTitle",
            language,
        )
    } else {
        val destination = destinationLabel.trim().ifEmpty { "Downloads" }
        AppText.value(
            "Saving to $destination",
            "将保存到 $destination",
            language,
        )
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

internal fun transferStageTimelineTitle(
    stage: TransferStage,
    language: String,
): String =
    when (stage) {
        TransferStage.SessionStarted -> AppText.value("Started", "已开始", language)
        TransferStage.ConnectionReady -> AppText.value("Connected", "已连接", language)
        TransferStage.AuthenticationStarted -> AppText.value("Authenticating", "正在认证", language)
        TransferStage.AuthenticationComplete -> AppText.value("Authenticated", "已认证", language)
        TransferStage.ManifestOffer -> AppText.value("File list ready", "文件清单已就绪", language)
        TransferStage.ManifestAccepted -> AppText.value("File list accepted", "文件清单已接受", language)
        TransferStage.FirstPayload -> AppText.value("First byte", "首字节", language)
        TransferStage.PayloadComplete -> AppText.value("Payload complete", "数据传输完成", language)
        TransferStage.DeliveryComplete -> AppText.value("Delivered", "已送达", language)
        TransferStage.Canceled -> AppText.value("Canceled", "已取消", language)
        TransferStage.Failed -> AppText.value("Failed", "失败", language)
    }

internal fun savedDestinationSubtitle(
    destinationLabel: String,
    language: String,
): String {
    val destination = destinationLabel.trim().ifEmpty { "Downloads" }
    return AppText.value(
        "Saved to $destination · tap for details",
        "已保存到 $destination · 点击查看详情",
        language,
    )
}

internal fun resolvedSavedDestinationLabel(
    recordedDestinationLabel: String?,
    fallbackDestinationLabel: String,
): String =
    recordedDestinationLabel
        ?.trim()
        ?.takeIf(String::isNotEmpty)
        ?: fallbackDestinationLabel.trim().ifEmpty { "Downloads" }
