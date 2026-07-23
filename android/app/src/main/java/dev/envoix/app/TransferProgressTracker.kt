package dev.envoix.app

import kotlin.math.max

data class TransferProgressSnapshot(
    val bytes: Long,
    val total: Long,
    val speedBps: Double,
    val avgBps: Double,
    val speedHistory: List<Double>,
)

/**
 * Monotonic, rate-limited progress projection for Compose. Native callbacks
 * may arrive much faster than a screen should recompose.
 */
class TransferProgressTracker(
    private val initialBytes: Long = 0,
) {
    private var observedBytes = initialBytes.coerceAtLeast(0)
    private var observedTotal = observedBytes
    private var startedAtNanos: Long? = null
    private var lastRateAtNanos: Long? = null
    private var lastRateBytes = observedBytes
    private var lastPublishAtNanos: Long? = null
    private var smoothedBps = 0.0
    private var history = emptyList<Double>()

    @Synchronized
    fun update(
        bytes: Long,
        total: Long,
        nowNanos: Long = System.nanoTime(),
    ): TransferProgressSnapshot? {
        observedBytes = max(observedBytes, bytes.coerceAtLeast(0))
        observedTotal = max(max(observedTotal, total.coerceAtLeast(0)), observedBytes)

        if (startedAtNanos == null) {
            startedAtNanos = nowNanos
            lastRateAtNanos = nowNanos
            lastPublishAtNanos = nowNanos
            lastRateBytes = observedBytes
            return snapshot(avgBps = 0.0)
        }

        val rateElapsed = nowNanos - checkNotNull(lastRateAtNanos)
        if (rateElapsed >= RATE_SAMPLE_NANOS) {
            val instantaneous =
                (observedBytes - lastRateBytes).coerceAtLeast(0).toDouble() *
                    NANOS_PER_SECOND / rateElapsed.toDouble()
            smoothedBps =
                if (history.isEmpty()) {
                    instantaneous
                } else {
                    smoothedBps * (1.0 - RATE_ALPHA) + instantaneous * RATE_ALPHA
                }
            if (smoothedBps > 0) {
                history = (history + smoothedBps).takeLast(HISTORY_LIMIT)
            }
            lastRateAtNanos = nowNanos
            lastRateBytes = observedBytes
        }

        val publishElapsed = nowNanos - checkNotNull(lastPublishAtNanos)
        val complete = observedTotal > 0 && observedBytes >= observedTotal
        if (!complete && publishElapsed < PUBLISH_INTERVAL_NANOS) return null
        lastPublishAtNanos = nowNanos

        val totalElapsed = nowNanos - checkNotNull(startedAtNanos)
        val average =
            if (totalElapsed > 0) {
                (observedBytes - initialBytes.coerceAtLeast(0)).coerceAtLeast(0).toDouble() *
                    NANOS_PER_SECOND / totalElapsed.toDouble()
            } else {
                0.0
            }
        return snapshot(avgBps = average)
    }

    private fun snapshot(avgBps: Double) =
        TransferProgressSnapshot(
            bytes = observedBytes,
            total = observedTotal,
            speedBps = smoothedBps,
            avgBps = avgBps,
            speedHistory = history,
        )

    private companion object {
        const val NANOS_PER_SECOND = 1_000_000_000.0
        const val RATE_SAMPLE_NANOS = 100_000_000L
        const val PUBLISH_INTERVAL_NANOS = 200_000_000L
        const val RATE_ALPHA = 0.3
        const val HISTORY_LIMIT = 24
    }
}
