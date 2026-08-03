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
    private var lastRateAtNanos: Long? = null
    private var lastRateBytes = observedBytes
    private var lastPublishAtNanos: Long? = null
    private var smoothedBps = 0.0
    private var rateSamples = 0
    private var accumulatedBytes = 0.0
    private var accumulatedNanos = 0L
    private var history = emptyList<Double>()

    @Synchronized
    fun update(
        bytes: Long,
        total: Long,
        nowNanos: Long = System.nanoTime(),
    ): TransferProgressSnapshot? {
        observedBytes = max(observedBytes, bytes.coerceAtLeast(0))
        observedTotal = max(max(observedTotal, total.coerceAtLeast(0)), observedBytes)

        if (lastRateAtNanos == null) {
            lastRateAtNanos = nowNanos
            lastPublishAtNanos = nowNanos
            lastRateBytes = observedBytes
            return snapshot()
        }

        val rateElapsed = nowNanos - checkNotNull(lastRateAtNanos)
        val deltaBytes = (observedBytes - lastRateBytes).coerceAtLeast(0)
        val complete = observedTotal > 0 && observedBytes >= observedTotal
        if (rateElapsed >= RATE_SAMPLE_NANOS || (complete && rateElapsed > 0 && deltaBytes > 0)) {
            val instantaneous =
                deltaBytes.toDouble() *
                    NANOS_PER_SECOND / rateElapsed.toDouble()
            smoothedBps =
                if (rateSamples == 0) {
                    instantaneous
                } else {
                    smoothedBps * (1.0 - RATE_ALPHA) + instantaneous * RATE_ALPHA
                }
            accumulatedBytes += deltaBytes.toDouble()
            accumulatedNanos += rateElapsed
            rateSamples += 1
            if (smoothedBps > 0) {
                history = (history + smoothedBps).takeLast(HISTORY_LIMIT)
            }
            lastRateAtNanos = nowNanos
            lastRateBytes = observedBytes
        }

        val publishElapsed = nowNanos - checkNotNull(lastPublishAtNanos)
        if (!complete && publishElapsed < PUBLISH_INTERVAL_NANOS) return null
        lastPublishAtNanos = nowNanos
        return snapshot()
    }

    private fun snapshot() =
        TransferProgressSnapshot(
            bytes = observedBytes,
            total = observedTotal,
            speedBps = smoothedBps,
            avgBps =
                if (accumulatedNanos > 0) {
                    accumulatedBytes * NANOS_PER_SECOND / accumulatedNanos.toDouble()
                } else {
                    0.0
                },
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
