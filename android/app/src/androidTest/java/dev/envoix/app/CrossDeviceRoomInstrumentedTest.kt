package dev.envoix.app

import android.content.Context
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.security.MessageDigest

@RunWith(AndroidJUnit4::class)
class CrossDeviceRoomInstrumentedTest {
    @Test
    fun sendAndroidToIosRoom() =
        runBlocking {
            requireEnabled()
            val context = prepareTargetApp()
            val originalSettings = SettingsStore.settings.value
            val baselineIds = TransferRepository.transfers.value.mapTo(mutableSetOf()) { it.id }
            val root = testRoot(context, "send")
            val sendFile = File(root, androidToIosFileName())
            val expectedBytes = androidToIosBytes()
            var transferId: Long? = null

            try {
                SettingsStore.update { it.forCrossDeviceTest() }
                writeRepeatedPayload(sendFile, ANDROID_TO_IOS_PAYLOAD, expectedBytes)
                TransferService.start(
                    context = context,
                    direction = "send",
                    room = androidToIosCode(),
                    path = sendFile.absolutePath,
                    broker = Endpoints.BROKER,
                    relay = Endpoints.RELAY,
                    chunkSize = "",
                    dataStreamWindow = "",
                    candidatesAllow = "",
                    candidatesDeny = "",
                    qrPayload = null,
                )

                val created = awaitNewTransfer(baselineIds, Direction.Send)
                transferId = created.id
                val completed = awaitTerminal(created.id, expectedBytes)
                assertEquals(terminalFailureMessage(completed), Status.Completed, completed.status)
                assertEquals(expectedBytes, completed.bytes)
                assertEquals(sendFile.name, completed.fileName)
                assertRequestedPath(completed)
                marker("Android send completed bytes=$expectedBytes path=${completed.pathType ?: "none"}")
            } finally {
                transferId?.let { TransferService.remove(context, it) }
                SettingsStore.update { originalSettings }
                root.deleteRecursively()
            }
        }

    @Test
    fun receiveIosToAndroidRoom() =
        runBlocking {
            requireEnabled()
            val context = prepareTargetApp()
            val originalSettings = SettingsStore.settings.value
            val baselineIds = TransferRepository.transfers.value.mapTo(mutableSetOf()) { it.id }
            val root = testRoot(context, "receive")
            val expectedBytes = iosToAndroidBytes()
            var transferId: Long? = null
            var publishedUri: Uri? = null

            try {
                SettingsStore.update { it.forCrossDeviceTest() }
                TransferService.start(
                    context = context,
                    direction = "receive",
                    room = iosToAndroidCode(),
                    path = root.absolutePath,
                    broker = Endpoints.BROKER,
                    relay = Endpoints.RELAY,
                    chunkSize = "",
                    dataStreamWindow = "",
                    candidatesAllow = "",
                    candidatesDeny = "",
                    qrPayload = null,
                )

                val created = awaitNewTransfer(baselineIds, Direction.Receive)
                transferId = created.id
                marker("Android room receiver ready")
                val completed = awaitTerminal(created.id, expectedBytes)
                assertEquals(terminalFailureMessage(completed), Status.Completed, completed.status)
                assertEquals(expectedBytes, completed.bytes)
                assertEquals(iosToAndroidFileName(), completed.fileName)
                assertRequestedPath(completed)

                val published = awaitPublished(created.id, expectedBytes)
                assertFalse("received artifact publication failed", published.publishFailed)
                assertFalse("received artifact failed verification", published.publicationInvalid)
                publishedUri = Uri.parse(requireNotNull(published.savedUri))
                assertPublishedPayload(context, publishedUri, IOS_TO_ANDROID_PAYLOAD, expectedBytes)
                marker(
                    "Android receive completed bytes=$expectedBytes " +
                        "path=${published.pathType ?: "none"} sha256=${expectedDigestHex(IOS_TO_ANDROID_PAYLOAD, expectedBytes)}",
                )
            } finally {
                publishedUri?.let { context.contentResolver.delete(it, null, null) }
                transferId?.let { TransferService.remove(context, it) }
                SettingsStore.update { originalSettings }
                root.deleteRecursively()
            }
        }

    private fun requireEnabled() {
        assumeTrue(
            "set -e envoixCrossDevice 1 to run physical cross-device tests",
            argument(ARG_ENABLED) == "1",
        )
    }

    private fun prepareTargetApp(): Context {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val launchIntent =
            requireNotNull(context.packageManager.getLaunchIntentForPackage(context.packageName)) {
                "target app has no launcher activity"
            }
        val component = requireNotNull(launchIntent.component).flattenToShortString()
        val launchOutput =
            ParcelFileDescriptor
                .AutoCloseInputStream(instrumentation.uiAutomation.executeShellCommand("am start -W -n $component"))
                .bufferedReader()
                .use { it.readText() }
        check("Status: ok" in launchOutput || "Warning: Activity not started" in launchOutput) {
            "failed to foreground target app"
        }
        instrumentation.waitForIdleSync()
        return context
    }

    private suspend fun awaitNewTransfer(
        baselineIds: Set<Long>,
        direction: Direction,
    ): Transfer =
        withTimeout(crossDeviceTimeoutMs(DEFAULT_PAYLOAD_BYTES)) {
            TransferRepository.transfers
                .map { transfers ->
                    transfers.firstOrNull { it.id !in baselineIds && it.direction == direction }
                }.filterNotNull()
                .first()
        }

    private suspend fun awaitTerminal(
        id: Long,
        expectedBytes: Long,
    ): Transfer =
        withTimeout(crossDeviceTimeoutMs(expectedBytes)) {
            TransferRepository.transfers
                .map { transfers -> transfers.firstOrNull { it.id == id && it.status.isTerminal } }
                .filterNotNull()
                .first()
        }

    private suspend fun awaitPublished(
        id: Long,
        expectedBytes: Long,
    ): Transfer =
        withTimeout(crossDeviceTimeoutMs(expectedBytes)) {
            TransferRepository.transfers
                .map { transfers ->
                    transfers.firstOrNull { transfer ->
                        transfer.id == id && transfer.hasPublicationResult()
                    }
                }.filterNotNull()
                .first()
        }

    private fun Transfer.hasPublicationResult(): Boolean = savedUri != null || publishFailed || publicationInvalid

    private fun Settings.forCrossDeviceTest(): Settings =
        copy(
            useRoom = true,
            useMdns = true,
            saveFolder = CROSS_DEVICE_DOWNLOAD_FOLDER,
            saveTreeUri = "",
        )

    private fun assertRequestedPath(transfer: Transfer) {
        when (argument(ARG_PATH_POLICY)?.lowercase()) {
            null, "", "auto" -> Unit
            "relay", "relay-only" ->
                assertEquals("relay-only transfer used a non-relay path", "relay", transfer.pathType?.lowercase())
            "direct", "direct-only" ->
                assertEquals("direct-only transfer used a non-direct path", "direct", transfer.pathType?.lowercase())
            else -> error("$ARG_PATH_POLICY must be auto, relay-only, or direct-only")
        }
    }

    private fun assertPublishedPayload(
        context: Context,
        uri: Uri,
        payload: ByteArray,
        expectedBytes: Long,
    ) {
        val expectedHash = expectedDigest(payload, expectedBytes)
        val resolver = context.contentResolver
        resolver.openAssetFileDescriptor(uri, "r")?.use { descriptor ->
            if (descriptor.length >= 0) assertEquals(expectedBytes, descriptor.length)
        }
        val actualHash =
            resolver.openInputStream(uri)?.use(::streamDigest)
                ?: error("published test artifact cannot be opened")
        assertArrayEquals("published MediaStore SHA-256 mismatch", expectedHash, actualHash)
    }

    private fun testRoot(
        context: Context,
        suffix: String,
    ): File =
        File(context.cacheDir, "envoix-cross-device-${crossDeviceRunId()}-$suffix").apply {
            deleteRecursively()
            check(mkdirs()) { "failed to create cross-device test directory" }
        }

    private fun writeRepeatedPayload(
        file: File,
        payload: ByteArray,
        expectedBytes: Long,
    ) {
        require(expectedBytes >= 0) { "expected byte count must be non-negative" }
        require(payload.isNotEmpty() || expectedBytes == 0L) { "non-empty transfer requires a payload" }
        val block = repeatedBlock(payload)
        file.outputStream().buffered(HASH_BLOCK_BYTES).use { output ->
            var remaining = expectedBytes
            while (remaining > 0) {
                val count = minOf(remaining, block.size.toLong()).toInt()
                output.write(block, 0, count)
                remaining -= count
            }
        }
    }

    private fun expectedDigest(
        payload: ByteArray,
        expectedBytes: Long,
    ): ByteArray {
        val digest = MessageDigest.getInstance("SHA-256")
        val block = repeatedBlock(payload)
        var remaining = expectedBytes
        while (remaining > 0) {
            val count = minOf(remaining, block.size.toLong()).toInt()
            digest.update(block, 0, count)
            remaining -= count
        }
        return digest.digest()
    }

    private fun expectedDigestHex(
        payload: ByteArray,
        expectedBytes: Long,
    ): String = expectedDigest(payload, expectedBytes).joinToString("") { "%02x".format(it) }

    private fun repeatedBlock(payload: ByteArray): ByteArray {
        if (payload.isEmpty()) return ByteArray(0)
        val repeats = maxOf(1, HASH_BLOCK_BYTES / payload.size)
        return ByteArray(repeats * payload.size).also { block ->
            repeat(repeats) { index -> payload.copyInto(block, index * payload.size) }
        }
    }

    private fun streamDigest(input: java.io.InputStream): ByteArray {
        val digest = MessageDigest.getInstance("SHA-256")
        val buffer = ByteArray(HASH_BLOCK_BYTES)
        while (true) {
            val count = input.read(buffer)
            if (count < 0) break
            if (count > 0) digest.update(buffer, 0, count)
        }
        return digest.digest()
    }

    private fun crossDeviceRunId(): String {
        val runId = argument(ARG_RUN_ID) ?: "manual"
        require(runId.length <= MAX_RUN_ID_LENGTH && runId.all { it.isLetterOrDigit() || it == '-' || it == '_' }) {
            "$ARG_RUN_ID must contain only letters, digits, '-' or '_'"
        }
        return runId
    }

    private fun androidToIosCode(): String = argument(ARG_ANDROID_TO_IOS_CODE) ?: DEFAULT_ANDROID_TO_IOS_CODE

    private fun iosToAndroidCode(): String = argument(ARG_IOS_TO_ANDROID_CODE) ?: DEFAULT_IOS_TO_ANDROID_CODE

    private fun androidToIosFileName(): String = "envoix-${crossDeviceRunId()}-android-to-ios.bin"

    private fun iosToAndroidFileName(): String = "envoix-${crossDeviceRunId()}-ios-to-android.bin"

    private fun androidToIosBytes(): Long = longArgument(ARG_ANDROID_TO_IOS_BYTES) ?: ANDROID_TO_IOS_PAYLOAD.size.toLong()

    private fun iosToAndroidBytes(): Long = longArgument(ARG_IOS_TO_ANDROID_BYTES) ?: IOS_TO_ANDROID_PAYLOAD.size.toLong()

    private fun crossDeviceTimeoutMs(expectedBytes: Long): Long =
        longArgument(ARG_TIMEOUT_MS)
            ?: maxOf(DEFAULT_TIMEOUT_MS, DEFAULT_TIMEOUT_MS + expectedBytes / TIMEOUT_BYTES_PER_MS)

    private fun argument(name: String): String? =
        InstrumentationRegistry
            .getArguments()
            .getString(name)
            ?.trim()
            ?.takeIf { it.isNotEmpty() }

    private fun longArgument(name: String): Long? {
        val raw = argument(name) ?: return null
        val value = raw.toLongOrNull() ?: error("$name must be a non-negative integer")
        require(value >= 0) { "$name must be non-negative" }
        return value
    }

    private fun terminalFailureMessage(transfer: Transfer): String =
        buildString {
            append("transfer ended as ${transfer.status.wire}; reason=${transfer.error ?: "none"}")
            val logTail = transfer.log.takeLast(FAILURE_LOG_TAIL_LINES)
            if (logTail.isNotEmpty()) append("\n").append(logTail.joinToString("\n"))
        }

    private fun marker(message: String) {
        val line = "[cross-device] $message"
        Log.i(LOG_TAG, line)
        println(line)
    }

    private companion object {
        const val LOG_TAG = "EnvoixCrossDevice"
        const val ARG_ENABLED = "envoixCrossDevice"
        const val ARG_RUN_ID = "envoixCrossDeviceRunId"
        const val ARG_TIMEOUT_MS = "envoixCrossDeviceTimeoutMs"
        const val ARG_PATH_POLICY = "envoixCrossDevicePathPolicy"
        const val ARG_ANDROID_TO_IOS_CODE = "envoixAndroidToIosCode"
        const val ARG_IOS_TO_ANDROID_CODE = "envoixIosToAndroidCode"
        const val ARG_ANDROID_TO_IOS_BYTES = "envoixAndroidToIosBytes"
        const val ARG_IOS_TO_ANDROID_BYTES = "envoixIosToAndroidBytes"
        const val DEFAULT_ANDROID_TO_IOS_CODE = "741203-amber-comet"
        const val DEFAULT_IOS_TO_ANDROID_CODE = "741204-azure-river"
        const val DEFAULT_TIMEOUT_MS = 180_000L
        const val TIMEOUT_BYTES_PER_MS = 2_048L
        const val MAX_RUN_ID_LENGTH = 80
        const val FAILURE_LOG_TAIL_LINES = 80
        const val HASH_BLOCK_BYTES = 1024 * 1024
        const val CROSS_DEVICE_DOWNLOAD_FOLDER = "EnvoixCrossDeviceTests"
        const val DEFAULT_PAYLOAD_BYTES = 64L
        val ANDROID_TO_IOS_PAYLOAD = "envoix cross-device android to ios\n".toByteArray()
        val IOS_TO_ANDROID_PAYLOAD = "envoix cross-device ios to android\n".toByteArray()
    }
}
