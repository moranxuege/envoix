package dev.envoix.app

import android.app.ActivityManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.util.Base64
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import dev.envoix.app.ffi.FfiPathPolicy
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.security.MessageDigest
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.atomic.AtomicLong

@RunWith(AndroidJUnit4::class)
class CrossDeviceRoomInstrumentedTest {
    private data class PublishedResult(
        val uri: Uri,
        val bytes: Long,
    )

    @Test
    fun sendAndroidToIosRoom() =
        runBlocking {
            prepareCrossDeviceTest()
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            val root =
                File(context.cacheDir, "envoix-cross-device-android-send").apply {
                    deleteRecursively()
                    mkdirs()
                }
            val sendFile = File(root, androidToIosFileName())
            val expectedBytes = androidToIosBytes()
            val pathPolicy = crossDevicePathPolicy()
            writePayloadFile(sendFile, ANDROID_TO_IOS_PAYLOAD, expectedBytes)
            val multicastLock = multicastLock(context, "envoix-cross-device-android-send")

            try {
                multicastLock.acquire()
                val events = CliEventRecorder()
                val job =
                    launch(Dispatchers.IO) {
                        UniffiTransferRunner
                            .run(
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
                                pathPolicy = pathPolicy,
                            ).collect { events.record(it) }
                    }
                val pauseJob =
                    launch {
                        pauseAndResumeIfRequested(
                            id = ANDROID_TO_IOS_TRANSFER_ID,
                            events = events,
                            expectedBytes = expectedBytes,
                        )
                    }
                val bytes = events.awaitCompletedWithTimeout(crossDeviceTimeoutMs(expectedBytes))
                pauseJob.join()
                job.join()
                assertTrue(
                    "sender completed with unexpected byte count: $bytes",
                    bytes == expectedBytes,
                )
                events.assertPathPolicy(pathPolicy)
            } finally {
                UniffiTransferRunner.remove(ANDROID_TO_IOS_TRANSFER_ID)
                release(multicastLock)
                root.deleteRecursively()
            }
        }

    @Test
    fun receiveIosToAndroidRoom() =
        runBlocking {
            prepareCrossDeviceTest()
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            val root =
                File(context.cacheDir, "envoix-cross-device-android-receive").apply {
                    deleteRecursively()
                    mkdirs()
                }
            val receiveDir = File(root, "received").apply { mkdirs() }
            val multicastLock = multicastLock(context, "envoix-cross-device-android-receive")
            var publishedUri: Uri? = null

            try {
                multicastLock.acquire()
                val expectedBytes = iosToAndroidBytes()
                val pathPolicy = crossDevicePathPolicy()
                val events = CliEventRecorder()
                val job =
                    launch(Dispatchers.IO) {
                        UniffiTransferRunner
                            .run(
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
                                pathPolicy = pathPolicy,
                                publicationRequired = true,
                            ).collect { events.record(it) }
                    }
                val published =
                    publishAndComplete(
                        context = context,
                        id = IOS_TO_ANDROID_TRANSFER_ID,
                        events = events,
                        payload = IOS_TO_ANDROID_PAYLOAD,
                        expectedBytes = expectedBytes,
                    )
                publishedUri = published.uri
                val bytes = published.bytes
                job.join()
                assertTrue(
                    "receiver completed with unexpected byte count: $bytes",
                    bytes == expectedBytes,
                )
                assertPublishedUri(context, publishedUri, IOS_TO_ANDROID_PAYLOAD, expectedBytes)
                events.assertPathPolicy(pathPolicy)
            } finally {
                publishedUri?.let { context.contentResolver.delete(it, null, null) }
                UniffiTransferRunner.remove(IOS_TO_ANDROID_TRANSFER_ID)
                release(multicastLock)
                root.deleteRecursively()
            }
        }

    @Test
    fun sendAndroidToIosInvite() =
        runBlocking {
            prepareCrossDeviceTest()
            val invite = transferInvite()
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            val root =
                File(context.cacheDir, "envoix-cross-device-android-send-invite").apply {
                    deleteRecursively()
                    mkdirs()
                }
            val sendFile = File(root, androidToIosFileName())
            val expectedBytes = androidToIosBytes()
            writePayloadFile(sendFile, ANDROID_TO_IOS_PAYLOAD, expectedBytes)

            try {
                val events = CliEventRecorder()
                val job =
                    launch(Dispatchers.IO) {
                        UniffiTransferRunner
                            .run(
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
                val pauseJob =
                    launch {
                        pauseAndResumeIfRequested(
                            id = ANDROID_TO_IOS_INVITE_TRANSFER_ID,
                            events = events,
                            expectedBytes = expectedBytes,
                        )
                    }
                val bytes = events.awaitCompletedWithTimeout(crossDeviceTimeoutMs(expectedBytes))
                pauseJob.join()
                job.join()
                assertTrue(
                    "sender completed with unexpected byte count: $bytes",
                    bytes == expectedBytes,
                )
            } finally {
                UniffiTransferRunner.remove(ANDROID_TO_IOS_INVITE_TRANSFER_ID)
                root.deleteRecursively()
            }
        }

    @Test
    fun receiveIosToAndroidInvite() =
        runBlocking {
            prepareCrossDeviceTest()
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            val root =
                File(context.cacheDir, "envoix-cross-device-android-receive-invite").apply {
                    deleteRecursively()
                    mkdirs()
                }
            val receiveDir = File(root, "received").apply { mkdirs() }
            val inviteFile = File(context.cacheDir, ANDROID_RECEIVER_INVITE_FILE_NAME).apply { delete() }
            var publishedUri: Uri? = null

            try {
                val events =
                    CliEventRecorder { invite ->
                        inviteFile.writeText(invite)
                    }
                val expectedBytes = iosToAndroidBytes()
                val job =
                    launch(Dispatchers.IO) {
                        UniffiTransferRunner
                            .run(
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
                                publicationRequired = true,
                            ).collect { events.record(it) }
                    }
                val published =
                    publishAndComplete(
                        context = context,
                        id = IOS_TO_ANDROID_INVITE_TRANSFER_ID,
                        events = events,
                        payload = IOS_TO_ANDROID_PAYLOAD,
                        expectedBytes = expectedBytes,
                    )
                publishedUri = published.uri
                val bytes = published.bytes
                job.join()
                assertTrue(
                    "receiver completed with unexpected byte count: $bytes",
                    bytes == expectedBytes,
                )
                assertPublishedUri(context, publishedUri, IOS_TO_ANDROID_PAYLOAD, expectedBytes)
            } finally {
                publishedUri?.let { context.contentResolver.delete(it, null, null) }
                UniffiTransferRunner.remove(IOS_TO_ANDROID_INVITE_TRANSFER_ID)
                inviteFile.delete()
                root.deleteRecursively()
            }
        }

    private fun isEnabled(): Boolean = InstrumentationRegistry.getArguments().getString("envoixCrossDevice") == "1"

    private fun prepareCrossDeviceTest() {
        assumeTrue("set -e envoixCrossDevice 1 to run cross-device tests", isEnabled())
        bringTargetAppToForeground()
        LogStore.clear()
        val verbose = InstrumentationRegistry.getArguments().getString("envoixVerboseLog") != "0"
        val spec = if (verbose) LOG_VERBOSE else LOG_BASELINE
        NativeBootstrap.setLogLevel(spec)
        Log.i(LOG_TAG, "native log level $spec")
    }

    private fun bringTargetAppToForeground() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val launchIntent =
            requireNotNull(context.packageManager.getLaunchIntentForPackage(context.packageName)) {
                "target app has no launcher activity"
            }.apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
            }
        val component = requireNotNull(launchIntent.component).flattenToShortString()
        val launchOutput =
            ParcelFileDescriptor
                .AutoCloseInputStream(instrumentation.uiAutomation.executeShellCommand("am start -W -n $component"))
                .bufferedReader()
                .use { it.readText() }
        check("Status: ok" in launchOutput || "Warning: Activity not started" in launchOutput) {
            "failed to launch target activity: $launchOutput"
        }

        val deadline = SystemClock.elapsedRealtime() + FOREGROUND_WAIT_MS
        var importance = ActivityManager.RunningAppProcessInfo.IMPORTANCE_GONE
        while (SystemClock.elapsedRealtime() < deadline) {
            val process = ActivityManager.RunningAppProcessInfo()
            ActivityManager.getMyMemoryState(process)
            importance = process.importance
            if (importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND) break
            SystemClock.sleep(FOREGROUND_POLL_MS)
        }
        check(importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND) {
            "cross-device test requires a foreground target process; importance=$importance"
        }
        Log.i(LOG_TAG, "target app foreground importance=$importance")
    }

    private fun transferInvite(): String {
        val encoded = InstrumentationRegistry.getArguments().getString("envoixTransferInviteBase64")
        require(!encoded.isNullOrBlank()) { "missing -e envoixTransferInviteBase64" }
        return String(Base64.decode(encoded, Base64.DEFAULT), Charsets.UTF_8)
    }

    private fun androidToIosBytes(): Long = longArgument(ARG_ANDROID_TO_IOS_BYTES) ?: ANDROID_TO_IOS_PAYLOAD.size.toLong()

    private fun iosToAndroidBytes(): Long = longArgument(ARG_IOS_TO_ANDROID_BYTES) ?: IOS_TO_ANDROID_PAYLOAD.size.toLong()

    private fun androidToIosCode(): String = stringArgument(ARG_ANDROID_TO_IOS_CODE) ?: ANDROID_TO_IOS_CODE

    private fun iosToAndroidCode(): String = stringArgument(ARG_IOS_TO_ANDROID_CODE) ?: IOS_TO_ANDROID_CODE

    private fun crossDeviceRunId(): String {
        val runId = stringArgument(ARG_RUN_ID) ?: "manual"
        require(runId.length <= 80 && runId.all { it.isLetterOrDigit() || it == '-' || it == '_' }) {
            "$ARG_RUN_ID must contain only letters, digits, '-' or '_'"
        }
        return runId
    }

    private fun androidToIosFileName(): String = "envoix-" + crossDeviceRunId() + "-android-to-ios.bin"

    private fun iosToAndroidFileName(): String = "envoix-" + crossDeviceRunId() + "-ios-to-android.bin"

    private fun stringArgument(name: String): String? =
        InstrumentationRegistry
            .getArguments()
            .getString(name)
            ?.trim()
            ?.takeIf { it.isNotEmpty() }

    private fun longArgument(name: String): Long? {
        val raw =
            InstrumentationRegistry
                .getArguments()
                .getString(name)
                ?.trim()
                ?.takeIf { it.isNotEmpty() }
                ?: return null
        val value =
            raw.toLongOrNull()
                ?: error("$name must be a non-negative integer, got $raw")
        require(value >= 0) { "$name must be non-negative, got $value" }
        return value
    }

    private fun crossDevicePathPolicy(): FfiPathPolicy =
        when (stringArgument(ARG_PATH_POLICY)?.lowercase()) {
            null, "auto" -> FfiPathPolicy.AUTO
            "direct", "direct-only" -> FfiPathPolicy.DIRECT_ONLY
            else -> error("$ARG_PATH_POLICY must be auto or direct-only")
        }

    private fun crossDeviceTimeoutMs(expectedBytes: Long): Long {
        longArgument(ARG_TIMEOUT_MS)?.let { return it }
        val scaled = BASE_TIMEOUT_MS + expectedBytes / TIMEOUT_BYTES_PER_MS
        return maxOf(CROSS_DEVICE_TIMEOUT_MS, scaled)
    }

    private suspend fun pauseAndResumeIfRequested(
        id: Long,
        events: CliEventRecorder,
        expectedBytes: Long,
    ) {
        val pauseAfter = longArgument(ARG_PAUSE_AFTER_BYTES) ?: return
        require(pauseAfter in 1 until expectedBytes) {
            "$ARG_PAUSE_AFTER_BYTES must be between 1 and expectedBytes - 1"
        }
        val timeoutMs = crossDeviceTimeoutMs(expectedBytes)
        events.awaitProgressAtLeast(pauseAfter, timeoutMs)
        assertTrue("canonical pause request was rejected", UniffiTransferRunner.pause(id))
        events.awaitPausedWithTimeout(timeoutMs)
        delay(longArgument(ARG_PAUSE_DURATION_MS) ?: DEFAULT_PAUSE_DURATION_MS)
        assertTrue("canonical resume request was rejected", UniffiTransferRunner.resume(id))
    }

    private fun writePayloadFile(
        file: File,
        payload: ByteArray,
        expectedBytes: Long,
    ) {
        require(expectedBytes >= 0) { "expectedBytes must be non-negative" }
        require(payload.isNotEmpty() || expectedBytes == 0L) {
            "payload must not be empty for a non-empty transfer"
        }
        val block = repeatedPayloadBlock(payload)
        file.outputStream().buffered(HASH_BLOCK_BYTES).use { output ->
            var remaining = expectedBytes
            while (remaining > 0) {
                val count = minOf(remaining, block.size.toLong()).toInt()
                output.write(block, 0, count)
                remaining -= count
            }
        }
    }

    private suspend fun publishAndComplete(
        context: Context,
        id: Long,
        events: CliEventRecorder,
        payload: ByteArray,
        expectedBytes: Long,
    ): PublishedResult {
        val timeoutMs = crossDeviceTimeoutMs(expectedBytes)
        val publishing = events.awaitPublishingWithTimeout(timeoutMs)
        assertEquals(iosToAndroidFileName(), publishing.fileName)
        assertTrue(
            "publishing snapshot has unexpected byte count: " + publishing.bytesTransferred,
            publishing.bytesTransferred == expectedBytes,
        )
        val staged = File(publishing.stagedPath)
        assertReceivedFile(
            staged.parentFile ?: error("staging file has no parent"),
            staged.name,
            payload,
            expectedBytes,
        )
        val uri =
            withContext(Dispatchers.IO) {
                MediaStoreSaver.saveReceived(
                    context = context,
                    source = staged,
                    displayName = publishing.fileName,
                    treeUri = "",
                    folder = CROSS_DEVICE_DOWNLOAD_FOLDER,
                )
            }
        assertNotNull("MediaStore publication failed", uri)
        val publishedUri = requireNotNull(uri)
        assertTrue(
            "canonical core rejected publication success",
            UniffiTransferRunner.publicationSucceeded(id, publishedUri.toString()),
        )
        val bytes = events.awaitCompletedWithTimeout(timeoutMs)
        val completed = requireNotNull(UniffiTransferRunner.activity(id))
        assertEquals(publishedUri.toString(), completed.completedFilePath)
        assertTrue(
            "verified staging file was not removed after canonical completion",
            staged.delete(),
        )
        return PublishedResult(publishedUri, bytes)
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
        assertArrayEquals(
            "received file SHA-256 does not match the deterministic payload",
            repeatedPayloadSha256(payload, expectedBytes),
            fileSha256(received),
        )
        if (expectedBytes == payload.size.toLong()) {
            assertArrayEquals(payload, received.readBytes())
        }
    }

    private fun assertPublishedUri(
        context: Context,
        uri: Uri?,
        payload: ByteArray,
        expectedBytes: Long,
    ) {
        val publishedUri = requireNotNull(uri)
        val resolver = context.contentResolver
        val actualBytes =
            resolver.openAssetFileDescriptor(publishedUri, "r")?.use { descriptor ->
                descriptor.length.takeIf { it >= 0 }
            }
        if (actualBytes != null) {
            assertEquals("published MediaStore file has unexpected size", expectedBytes, actualBytes)
        }
        val actualHash =
            resolver.openInputStream(publishedUri)?.use(::streamSha256)
                ?: error("published MediaStore file cannot be opened: $publishedUri")
        val expectedHash = repeatedPayloadSha256(payload, expectedBytes)
        assertArrayEquals("published MediaStore SHA-256 mismatch", expectedHash, actualHash)
        val evidence = actualHash.joinToString(separator = "") { "%02x".format(it) }
        Log.i(
            LOG_TAG,
            "[cross-device] evidence uri=" + publishedUri + " size=" + expectedBytes + " sha256=" + evidence,
        )
    }

    private fun repeatedPayloadSha256(
        payload: ByteArray,
        expectedBytes: Long,
    ): ByteArray {
        val digest = MessageDigest.getInstance("SHA-256")
        val block = repeatedPayloadBlock(payload)
        var remaining = expectedBytes
        while (remaining > 0) {
            val count = minOf(remaining, block.size.toLong()).toInt()
            digest.update(block, 0, count)
            remaining -= count
        }
        return digest.digest()
    }

    private fun fileSha256(file: File): ByteArray = file.inputStream().buffered(HASH_BLOCK_BYTES).use(::streamSha256)

    private fun streamSha256(input: java.io.InputStream): ByteArray {
        val digest = MessageDigest.getInstance("SHA-256")
        val buffer = ByteArray(HASH_BLOCK_BYTES)
        while (true) {
            val count = input.read(buffer)
            if (count < 0) break
            if (count > 0) digest.update(buffer, 0, count)
        }
        return digest.digest()
    }

    private fun repeatedPayloadBlock(payload: ByteArray): ByteArray {
        if (payload.isEmpty()) return ByteArray(0)
        val repeats = maxOf(1, HASH_BLOCK_BYTES / payload.size)
        return ByteArray(repeats * payload.size).also { block ->
            repeat(repeats) { index ->
                payload.copyInto(block, destinationOffset = index * payload.size)
            }
        }
    }

    private fun multicastLock(
        context: Context,
        tag: String,
    ): WifiManager.MulticastLock =
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
        private val publishing = CompletableDeferred<CliEvent.Publishing>()
        private val paused = CompletableDeferred<Unit>()
        private val failed = CompletableDeferred<String>()
        private val log = ConcurrentLinkedQueue<String>()
        private val paths = ConcurrentLinkedQueue<String>()
        private val latestProgress = AtomicLong()

        fun record(event: CliEvent) {
            when (event) {
                is CliEvent.InviteReady -> {
                    onInviteReady(event.invite)
                    record("invite ready length=${event.invite.length}")
                }
                CliEvent.Binding -> record("binding")
                CliEvent.Connecting -> record("connecting")
                is CliEvent.Connected -> {
                    paths.add(event.pathType)
                    record("connected path=${event.pathType}:${event.addr}")
                }
                is CliEvent.Started -> record("started fileName=${event.fileName} totalBytes=${event.totalBytes}")
                is CliEvent.Progress -> {
                    latestProgress.accumulateAndGet(event.bytesTransferred, ::maxOf)
                    record("progress transferred=${event.bytesTransferred} total=${event.totalBytes}")
                }
                is CliEvent.Publishing -> {
                    record(
                        "publishing fileName=" + event.fileName +
                            " path=" + event.stagedPath +
                            " bytes=" + event.bytesTransferred,
                    )
                    publishing.complete(event)
                }
                CliEvent.Verifying -> record("verifying")
                CliEvent.Confirming -> record("confirming")
                CliEvent.Paused -> {
                    record("paused")
                    paused.complete(Unit)
                }
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

        suspend fun awaitPublishingWithTimeout(timeoutMs: Long): CliEvent.Publishing =
            try {
                withTimeout(timeoutMs) {
                    while (!publishing.isCompleted && !failed.isCompleted) {
                        delay(50)
                    }
                    if (failed.isCompleted) {
                        error("transfer failed before publication: " + failed.await() + "\n" + dumpLog())
                    }
                    publishing.await()
                }
            } catch (error: TimeoutCancellationException) {
                throw AssertionError("timed out waiting for transfer publication\n" + dumpLog(), error)
            }

        suspend fun awaitProgressAtLeast(
            bytes: Long,
            timeoutMs: Long,
        ) {
            try {
                withTimeout(timeoutMs) {
                    while (latestProgress.get() < bytes && !failed.isCompleted && !completed.isCompleted) {
                        delay(25)
                    }
                    if (failed.isCompleted) {
                        error("transfer failed before pause threshold: " + failed.await() + "\n" + dumpLog())
                    }
                    check(latestProgress.get() >= bytes) {
                        "transfer completed before pause threshold; progress=" + latestProgress.get()
                    }
                }
            } catch (error: TimeoutCancellationException) {
                throw AssertionError("timed out waiting for pause threshold\n" + dumpLog(), error)
            }
        }

        suspend fun awaitPausedWithTimeout(timeoutMs: Long) {
            try {
                withTimeout(timeoutMs) {
                    while (!paused.isCompleted && !failed.isCompleted) {
                        delay(25)
                    }
                    if (failed.isCompleted) {
                        error("transfer failed while pausing: " + failed.await() + "\n" + dumpLog())
                    }
                    paused.await()
                }
            } catch (error: TimeoutCancellationException) {
                throw AssertionError("timed out waiting for Paused snapshot\n" + dumpLog(), error)
            }
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

        fun dumpLog(): String =
            buildString {
                append(log.joinToString(separator = "\n"))
                val coreLog =
                    LogStore
                        .dump()
                        .lines()
                        .takeLast(CORE_LOG_TAIL_LINES)
                        .joinToString(separator = "\n")
                if (coreLog.isNotBlank()) {
                    append("\n\n=== Android core log tail ===\n")
                    append(coreLog)
                }
            }

        fun assertPathPolicy(policy: FfiPathPolicy) {
            if (policy != FfiPathPolicy.DIRECT_ONLY) return
            assertTrue("direct-only transfer did not report a direct path: $paths", paths.contains("direct"))
            assertTrue("direct-only transfer reported a relay path: $paths", !paths.contains("relay"))
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
        val ANDROID_TO_IOS_PAYLOAD = "envoix cross-device android to ios\n".toByteArray()
        val IOS_TO_ANDROID_PAYLOAD = "envoix cross-device ios to android\n".toByteArray()
        const val ARG_ANDROID_TO_IOS_CODE = "envoixAndroidToIosCode"
        const val ARG_IOS_TO_ANDROID_CODE = "envoixIosToAndroidCode"
        const val ARG_ANDROID_TO_IOS_BYTES = "envoixAndroidToIosBytes"
        const val ARG_IOS_TO_ANDROID_BYTES = "envoixIosToAndroidBytes"
        const val ARG_TIMEOUT_MS = "envoixCrossDeviceTimeoutMs"
        const val ARG_PATH_POLICY = "envoixCrossDevicePathPolicy"
        const val ARG_RUN_ID = "envoixCrossDeviceRunId"
        const val ARG_PAUSE_AFTER_BYTES = "envoixCrossDevicePauseAfterBytes"
        const val ARG_PAUSE_DURATION_MS = "envoixCrossDevicePauseDurationMs"
        const val CROSS_DEVICE_TIMEOUT_MS = 180_000L
        const val BASE_TIMEOUT_MS = 180_000L
        const val TIMEOUT_BYTES_PER_MS = 2_048L
        const val CORE_LOG_TAIL_LINES = 400
        const val HASH_BLOCK_BYTES = 1024 * 1024
        const val CROSS_DEVICE_DOWNLOAD_FOLDER = "EnvoixCrossDeviceTests"
        const val DEFAULT_PAUSE_DURATION_MS = 2_000L
        const val FOREGROUND_WAIT_MS = 10_000L
        const val FOREGROUND_POLL_MS = 100L
        const val LOG_TAG = "EnvoixCrossDevice"
        const val LOG_BASELINE = "envoix=debug,iroh=info,warn"
        const val LOG_VERBOSE = "envoix=trace,iroh=debug,warn"
    }
}
