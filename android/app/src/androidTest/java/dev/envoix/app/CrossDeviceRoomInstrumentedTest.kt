package dev.envoix.app

import android.content.Context
import android.net.wifi.WifiManager
import android.util.Base64
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import java.util.concurrent.ConcurrentLinkedQueue
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.io.RandomAccessFile

@RunWith(AndroidJUnit4::class)
class CrossDeviceRoomInstrumentedTest {
    @Test
    fun sendAndroidToIosRoom() = runBlocking {
        prepareCrossDeviceTest()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "envoix-cross-device-android-send").apply {
            deleteRecursively()
            mkdirs()
        }
        val sendFile = File(root, ANDROID_TO_IOS_FILE_NAME)
        val expectedBytes = androidToIosBytes()
        writePayloadFile(sendFile, ANDROID_TO_IOS_PAYLOAD, expectedBytes)
        val multicastLock = multicastLock(context, "envoix-cross-device-android-send")

        try {
            multicastLock.acquire()
            val events = CliEventRecorder()
            val job = launch(Dispatchers.IO) {
                UniffiTransferRunner.run(
                    id = ANDROID_TO_IOS_TRANSFER_ID,
                    direction = "send",
                    code = androidToIosCode(),
                    broker = Endpoints.BROKER,
                    relay = Endpoints.RELAY,
                    path = sendFile.absolutePath,
                    configPath = "",
                    qrPayload = null,
                    transferInvite = null,
                    internetAvailable = true,
                    useRoom = true,
                    useMdns = true,
                ).collect { events.record(it) }
            }
            val bytes = events.awaitCompletedWithTimeout(crossDeviceTimeoutMs(expectedBytes))
            job.join()
            assertTrue(
                "sender completed with unexpected byte count: $bytes",
                bytes >= expectedBytes,
            )
        } finally {
            release(multicastLock)
            root.deleteRecursively()
        }
    }

    @Test
    fun receiveIosToAndroidRoom() = runBlocking {
        prepareCrossDeviceTest()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "envoix-cross-device-android-receive").apply {
            deleteRecursively()
            mkdirs()
        }
        val receiveDir = File(root, "received").apply { mkdirs() }
        val multicastLock = multicastLock(context, "envoix-cross-device-android-receive")

        try {
            multicastLock.acquire()
            val expectedBytes = iosToAndroidBytes()
            val events = CliEventRecorder()
            val job = launch(Dispatchers.IO) {
                UniffiTransferRunner.run(
                    id = IOS_TO_ANDROID_TRANSFER_ID,
                    direction = "receive",
                    code = iosToAndroidCode(),
                    broker = Endpoints.BROKER,
                    relay = Endpoints.RELAY,
                    path = receiveDir.absolutePath,
                    configPath = "",
                    qrPayload = null,
                    transferInvite = null,
                    internetAvailable = true,
                    useRoom = true,
                    useMdns = true,
                ).collect { events.record(it) }
            }
            val bytes = events.awaitCompletedWithTimeout(crossDeviceTimeoutMs(expectedBytes))
            job.join()
            assertTrue(
                "receiver completed with unexpected byte count: $bytes",
                bytes >= expectedBytes,
            )
            assertReceivedFile(receiveDir, IOS_TO_ANDROID_FILE_NAME, IOS_TO_ANDROID_PAYLOAD, expectedBytes)
        } finally {
            release(multicastLock)
            root.deleteRecursively()
        }
    }

    @Test
    fun sendAndroidToIosInvite() = runBlocking {
        prepareCrossDeviceTest()
        val invite = transferInvite()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "envoix-cross-device-android-send-invite").apply {
            deleteRecursively()
            mkdirs()
        }
        val sendFile = File(root, ANDROID_TO_IOS_FILE_NAME)
        val expectedBytes = androidToIosBytes()
        writePayloadFile(sendFile, ANDROID_TO_IOS_PAYLOAD, expectedBytes)

        try {
            val events = CliEventRecorder()
            val job = launch(Dispatchers.IO) {
                UniffiTransferRunner.run(
                    id = ANDROID_TO_IOS_INVITE_TRANSFER_ID,
                    direction = "send",
                    code = "invite-direct",
                    broker = Endpoints.BROKER,
                    relay = Endpoints.RELAY,
                    path = sendFile.absolutePath,
                    configPath = "",
                    qrPayload = null,
                    transferInvite = invite,
                    internetAvailable = true,
                    useRoom = false,
                    useMdns = false,
                ).collect { events.record(it) }
            }
            val bytes = events.awaitCompletedWithTimeout(crossDeviceTimeoutMs(expectedBytes))
            job.join()
            assertTrue(
                "sender completed with unexpected byte count: $bytes",
                bytes >= expectedBytes,
            )
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun receiveIosToAndroidInvite() = runBlocking {
        prepareCrossDeviceTest()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "envoix-cross-device-android-receive-invite").apply {
            deleteRecursively()
            mkdirs()
        }
        val receiveDir = File(root, "received").apply { mkdirs() }
        val inviteFile = File(context.cacheDir, ANDROID_RECEIVER_INVITE_FILE_NAME).apply { delete() }

        try {
            val events = CliEventRecorder { invite ->
                inviteFile.writeText(invite)
            }
            val expectedBytes = iosToAndroidBytes()
            val job = launch(Dispatchers.IO) {
                UniffiTransferRunner.run(
                    id = IOS_TO_ANDROID_INVITE_TRANSFER_ID,
                    direction = "receive",
                    code = "invite-direct",
                    broker = Endpoints.BROKER,
                    relay = Endpoints.RELAY,
                    path = receiveDir.absolutePath,
                    configPath = "",
                    qrPayload = "pending-invite",
                    transferInvite = null,
                    internetAvailable = true,
                    useRoom = false,
                    useMdns = false,
                ).collect { events.record(it) }
            }
            val bytes = events.awaitCompletedWithTimeout(crossDeviceTimeoutMs(expectedBytes))
            job.join()
            assertTrue(
                "receiver completed with unexpected byte count: $bytes",
                bytes >= expectedBytes,
            )
            assertReceivedFile(receiveDir, IOS_TO_ANDROID_FILE_NAME, IOS_TO_ANDROID_PAYLOAD, expectedBytes)
        } finally {
            inviteFile.delete()
            root.deleteRecursively()
        }
    }

    private fun isEnabled(): Boolean =
        InstrumentationRegistry.getArguments().getString("envoixCrossDevice") == "1"

    private fun prepareCrossDeviceTest() {
        assumeTrue("set -e envoixCrossDevice 1 to run cross-device tests", isEnabled())
        LogStore.clear()
        val verbose = InstrumentationRegistry.getArguments().getString("envoixVerboseLog") != "0"
        val spec = if (verbose) LOG_VERBOSE else LOG_BASELINE
        NativeBootstrap.setLogLevel(spec)
        Log.i(LOG_TAG, "native log level $spec")
    }

    private fun transferInvite(): String {
        val encoded = InstrumentationRegistry.getArguments().getString("envoixTransferInviteBase64")
        require(!encoded.isNullOrBlank()) { "missing -e envoixTransferInviteBase64" }
        return String(Base64.decode(encoded, Base64.DEFAULT), Charsets.UTF_8)
    }

    private fun androidToIosBytes(): Long =
        longArgument(ARG_ANDROID_TO_IOS_BYTES) ?: ANDROID_TO_IOS_PAYLOAD.size.toLong()

    private fun iosToAndroidBytes(): Long =
        longArgument(ARG_IOS_TO_ANDROID_BYTES) ?: IOS_TO_ANDROID_PAYLOAD.size.toLong()

    private fun androidToIosCode(): String =
        stringArgument(ARG_ANDROID_TO_IOS_CODE) ?: ANDROID_TO_IOS_CODE

    private fun iosToAndroidCode(): String =
        stringArgument(ARG_IOS_TO_ANDROID_CODE) ?: IOS_TO_ANDROID_CODE

    private fun stringArgument(name: String): String? =
        InstrumentationRegistry.getArguments().getString(name)?.trim()
            ?.takeIf { it.isNotEmpty() }

    private fun longArgument(name: String): Long? {
        val raw = InstrumentationRegistry.getArguments().getString(name)?.trim()
            ?.takeIf { it.isNotEmpty() }
            ?: return null
        val value = raw.toLongOrNull()
            ?: error("$name must be a non-negative integer, got $raw")
        require(value >= 0) { "$name must be non-negative, got $value" }
        return value
    }

    private fun crossDeviceTimeoutMs(expectedBytes: Long): Long {
        longArgument(ARG_TIMEOUT_MS)?.let { return it }
        val scaled = BASE_TIMEOUT_MS + expectedBytes / TIMEOUT_BYTES_PER_MS
        return maxOf(CROSS_DEVICE_TIMEOUT_MS, scaled)
    }

    private fun writePayloadFile(file: File, payload: ByteArray, expectedBytes: Long) {
        require(expectedBytes >= 0) { "expectedBytes must be non-negative" }
        if (expectedBytes == payload.size.toLong()) {
            file.writeBytes(payload)
            return
        }
        RandomAccessFile(file, "rw").use { handle ->
            handle.setLength(expectedBytes)
            if (expectedBytes == 0L) {
                return
            }
            val prefixBytes = minOf(payload.size.toLong(), expectedBytes).toInt()
            handle.seek(0)
            handle.write(payload, 0, prefixBytes)
            if (expectedBytes > payload.size.toLong()) {
                handle.seek(expectedBytes - 1)
                handle.write(payload[((expectedBytes - 1) % payload.size).toInt()].toInt())
            }
        }
    }

    private fun assertReceivedFile(
        receiveDir: File,
        fileName: String,
        payload: ByteArray,
        expectedBytes: Long,
    ) {
        val received = File(receiveDir, fileName)
        assertTrue("received file does not exist: ${received.absolutePath}", received.isFile)
        assertTrue(
            "received file has unexpected size: ${received.length()}",
            received.length() == expectedBytes,
        )
        if (expectedBytes == payload.size.toLong()) {
            assertArrayEquals(payload, received.readBytes())
        }
    }

    private fun multicastLock(context: Context, tag: String): WifiManager.MulticastLock =
        (context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager)
            .createMulticastLock(tag)
            .apply { setReferenceCounted(false) }

    private fun release(lock: WifiManager.MulticastLock) {
        runCatching {
            if (lock.isHeld) lock.release()
        }
    }

    private class CliEventRecorder(
        private val onInviteReady: (String) -> Unit = {},
    ) {
        private val completed = CompletableDeferred<Long>()
        private val failed = CompletableDeferred<String>()
        private val log = ConcurrentLinkedQueue<String>()

        fun record(event: CliEvent) {
            when (event) {
                is CliEvent.InviteReady -> {
                    onInviteReady(event.invite)
                    record("invite ready length=${event.invite.length}")
                }
                CliEvent.Binding -> record("binding")
                CliEvent.Connecting -> record("connecting")
                is CliEvent.Connected -> record("connected path=${event.pathType}:${event.addr}")
                is CliEvent.Started -> record("started fileName=${event.fileName} totalBytes=${event.totalBytes}")
                is CliEvent.Progress -> record("progress transferred=${event.bytesTransferred} total=${event.totalBytes}")
                is CliEvent.Completed -> {
                    record("completed bytes=${event.bytesTransferred}")
                    completed.complete(event.bytesTransferred)
                }
                is CliEvent.Failed -> {
                    record("failed ${event.error}")
                    failed.complete(event.error)
                }
                is CliEvent.CoreStatus -> record("status ${event.message}")
                is CliEvent.Exit -> {
                    record("exit ${event.code}")
                    if (event.code != 0) failed.complete("exit ${event.code}")
                }
            }
        }

        suspend fun awaitCompletedWithTimeout(timeoutMs: Long): Long =
            try {
                withTimeout(timeoutMs) { awaitCompleted() }
            } catch (error: TimeoutCancellationException) {
                throw AssertionError("timed out waiting for transfer completion\n${dumpLog()}", error)
            }

        private suspend fun awaitCompleted(): Long {
            while (!completed.isCompleted && !failed.isCompleted) {
                delay(50)
            }
            if (failed.isCompleted) {
                error("transfer failed: ${failed.await()}\n${dumpLog()}")
            }
            return completed.await()
        }

        fun dumpLog(): String = buildString {
            append(log.joinToString(separator = "\n"))
            val coreLog = LogStore.dump()
                .lines()
                .takeLast(CORE_LOG_TAIL_LINES)
                .joinToString(separator = "\n")
            if (coreLog.isNotBlank()) {
                append("\n\n=== Android core log tail ===\n")
                append(coreLog)
            }
        }

        private fun record(message: String) {
            val line = "[cross-device] Android $message"
            println(line)
            Log.i(LOG_TAG, line)
            log.add(line)
            while (log.size > 240) {
                log.poll()
            }
        }
    }

    private companion object {
        const val ANDROID_TO_IOS_TRANSFER_ID = 92_101L
        const val IOS_TO_ANDROID_TRANSFER_ID = 92_102L
        const val ANDROID_TO_IOS_INVITE_TRANSFER_ID = 92_201L
        const val IOS_TO_ANDROID_INVITE_TRANSFER_ID = 92_202L
        const val ANDROID_RECEIVER_INVITE_FILE_NAME = "envoix-cross-device-ios-to-android.invite"
        const val ANDROID_TO_IOS_CODE = "741203-amber-comet"
        const val IOS_TO_ANDROID_CODE = "741204-azure-river"
        const val ANDROID_TO_IOS_FILE_NAME = "envoix-cross-android-to-ios.txt"
        const val IOS_TO_ANDROID_FILE_NAME = "envoix-cross-ios-to-android.txt"
        val ANDROID_TO_IOS_PAYLOAD = "envoix cross-device android to ios\n".toByteArray()
        val IOS_TO_ANDROID_PAYLOAD = "envoix cross-device ios to android\n".toByteArray()
        const val ARG_ANDROID_TO_IOS_CODE = "envoixAndroidToIosCode"
        const val ARG_IOS_TO_ANDROID_CODE = "envoixIosToAndroidCode"
        const val ARG_ANDROID_TO_IOS_BYTES = "envoixAndroidToIosBytes"
        const val ARG_IOS_TO_ANDROID_BYTES = "envoixIosToAndroidBytes"
        const val ARG_TIMEOUT_MS = "envoixCrossDeviceTimeoutMs"
        const val CROSS_DEVICE_TIMEOUT_MS = 180_000L
        const val BASE_TIMEOUT_MS = 180_000L
        const val TIMEOUT_BYTES_PER_MS = 2_048L
        const val CORE_LOG_TAIL_LINES = 400
        const val LOG_TAG = "EnvoixCrossDevice"
        const val LOG_BASELINE = "envoix=debug,iroh=info,warn"
        const val LOG_VERBOSE = "envoix=trace,iroh=debug,warn"
    }
}
