package dev.envoix.app

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import dev.envoix.app.ffi.EnvoixSession
import dev.envoix.app.ffi.FfiTransferActivityRecord
import dev.envoix.app.ffi.FfiTransferEvent
import dev.envoix.app.ffi.FfiTransferFailure
import dev.envoix.app.ffi.TransferObserver
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import java.util.concurrent.ConcurrentLinkedQueue
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class CrossDeviceRoomInstrumentedTest {
    @Test
    fun sendAndroidToIosRoom() = runBlocking {
        assumeTrue("set -e envoixCrossDevice 1 to run cross-device tests", isEnabled())
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "envoix-cross-device-android-send").apply {
            deleteRecursively()
            mkdirs()
        }
        val sendFile = File(root, ANDROID_TO_IOS_FILE_NAME)
        sendFile.writeBytes(ANDROID_TO_IOS_PAYLOAD)

        try {
            val observer = RecordingObserver()
            EnvoixSession().sendRoom(sendFile.absolutePath, ANDROID_TO_IOS_CODE, observer)
            val bytes = observer.awaitCompletedWithTimeout(CROSS_DEVICE_TIMEOUT_MS)
            assertTrue(
                "sender completed with unexpected byte count: $bytes",
                bytes >= ANDROID_TO_IOS_PAYLOAD.size.toULong(),
            )
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun receiveIosToAndroidRoom() = runBlocking {
        assumeTrue("set -e envoixCrossDevice 1 to run cross-device tests", isEnabled())
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "envoix-cross-device-android-receive").apply {
            deleteRecursively()
            mkdirs()
        }
        val receiveDir = File(root, "received").apply { mkdirs() }

        try {
            val observer = RecordingObserver()
            EnvoixSession().receiveRoom(receiveDir.absolutePath, IOS_TO_ANDROID_CODE, observer)
            val bytes = observer.awaitCompletedWithTimeout(CROSS_DEVICE_TIMEOUT_MS)
            assertTrue(
                "receiver completed with unexpected byte count: $bytes",
                bytes >= IOS_TO_ANDROID_PAYLOAD.size.toULong(),
            )
            assertArrayEquals(IOS_TO_ANDROID_PAYLOAD, File(receiveDir, IOS_TO_ANDROID_FILE_NAME).readBytes())
        } finally {
            root.deleteRecursively()
        }
    }

    private fun isEnabled(): Boolean =
        InstrumentationRegistry.getArguments().getString("envoixCrossDevice") == "1"

    private class RecordingObserver : TransferObserver {
        private val completed = CompletableDeferred<ULong>()
        private val failed = CompletableDeferred<String>()
        private val log = ConcurrentLinkedQueue<String>()

        override fun onInviteReady(invite: String) {
            record("invite ready length=${invite.length}")
        }

        override fun onStarted(fileName: String, totalBytes: ULong) {
            record("started fileName=$fileName totalBytes=$totalBytes")
        }

        override fun onProgress(transferred: ULong, total: ULong) {
            record("progress transferred=$transferred total=$total")
        }

        override fun onCompleted(bytes: ULong) {
            record("completed bytes=$bytes")
            completed.complete(bytes)
        }

        override fun onTransferFailed(failure: FfiTransferFailure) {
            val message = failure.diagnosticMessage.ifBlank { failure.userMessageKey }
            record("transfer failed $message")
            failed.complete(message)
        }

        override fun onFailed(reason: String) {
            record("failed $reason")
            failed.complete(reason)
        }

        override fun onTransferEvent(event: FfiTransferEvent) {
            record(
                "event kind=${event.kind} mode=${event.mode} direction=${event.direction} " +
                    "pairing=${event.pairingStep} path=${event.dataPathKind}:${event.dataPathDetail} " +
                    "bytes=${event.bytesTransferred}/${event.totalBytes} token=${tokenLabel(event.token)} " +
                    "peerLen=${event.peerDescriptor.length}",
            )
        }

        override fun onTransferActivity(record: FfiTransferActivityRecord) {
            record("activity state=${record.state} mode=${record.mode} attempt=${record.attemptId} bytes=${record.bytesTransferred}/${record.totalBytes} failure=${record.diagnosticMessage}")
        }

        override fun onStatus(message: String) {
            record("status $message")
        }

        suspend fun awaitCompletedWithTimeout(timeoutMs: Long): ULong =
            try {
                withTimeout(timeoutMs) { awaitCompleted() }
            } catch (error: TimeoutCancellationException) {
                throw AssertionError("timed out waiting for transfer completion\n${dumpLog()}", error)
            }

        private suspend fun awaitCompleted(): ULong {
            while (!completed.isCompleted && !failed.isCompleted) {
                delay(50)
            }
            if (failed.isCompleted) {
                error("transfer failed: ${failed.await()}\n${dumpLog()}")
            }
            return completed.await()
        }

        fun dumpLog(): String = log.joinToString(separator = "\n")

        private fun record(message: String) {
            val line = "[cross-device] Android $message"
            println(line)
            log.add(line)
            while (log.size > 240) {
                log.poll()
            }
        }

        private fun tokenLabel(token: String): String {
            val trimmed = token.trim()
            if (trimmed.isEmpty()) return "<none>"
            val room = trimmed.substringBefore("-")
            return if (room != trimmed && room.isNotBlank()) room else "set(len=${trimmed.length})"
        }
    }

    private companion object {
        const val ANDROID_TO_IOS_CODE = "741203-amber-comet"
        const val IOS_TO_ANDROID_CODE = "741204-azure-river"
        const val ANDROID_TO_IOS_FILE_NAME = "envoix-cross-android-to-ios.txt"
        const val IOS_TO_ANDROID_FILE_NAME = "envoix-cross-ios-to-android.txt"
        val ANDROID_TO_IOS_PAYLOAD = "envoix cross-device android to ios\n".toByteArray()
        val IOS_TO_ANDROID_PAYLOAD = "envoix cross-device ios to android\n".toByteArray()
        const val CROSS_DEVICE_TIMEOUT_MS = 180_000L
    }
}
