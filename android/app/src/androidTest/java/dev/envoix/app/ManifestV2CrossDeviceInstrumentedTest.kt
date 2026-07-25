package dev.envoix.app

import android.content.Context
import android.net.Uri
import android.util.Base64
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.runBlocking
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.security.MessageDigest
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Scenario-driven physical coverage for the JNI Manifest-v2 surface used by
 * the Android app, including its synchronous final-save gate. */
@RunWith(AndroidJUnit4::class)
class ManifestV2CrossDeviceInstrumentedTest {
    @Test
    fun shareSourceFailureDoesNotPoisonNextSelection() {
        requireEnabled()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val testRoot = testRoot(context, "share-recovery")
        val jobStore = File(testRoot, "jobs").apply { check(mkdirs()) }
        val created = checkedResponse(Native.createManifestV2Job(jobStore.path, "never"))
        val jobId = created.getString("job_id")
        var validUri: Uri? = null

        try {
            val missingSource =
                ManifestV2Source(
                    Uri.parse("content://dev.envoix.app.missing/not-readable.txt"),
                    directory = false,
                    displayName = "not-readable.txt",
                )
            val failure =
                runCatching {
                    runBlocking { ManifestV2SourceStager.stage(context, jobId, missingSource) }
                }
            assertTrue("an unreadable shared source must fail staging", failure.isFailure)

            val validFile = File(testRoot, "share-recovery.txt").apply { writeBytes(SHARE_RECOVERY_BYTES) }
            validUri = publishShareSource(context, validFile, validFile.name)
            val validSource = ManifestV2Source(validUri, directory = false, displayName = validFile.name)
            val staged = runBlocking { ManifestV2SourceStager.stage(context, jobId, validSource) }
            val prepared =
                checkedResponse(
                    Native.prepareManifestV2Job(
                        jobStore.path,
                        jobId,
                        ManifestV2SourceStager.rootsJson(validSource, staged, origin = "share"),
                    ),
                )
            assertEquals("ready_to_send", prepared.getString("state"))
            assertEquals(1, prepared.getInt("root_count"))
            assertEquals(1, prepared.getInt("file_count"))
            assertEquals(SHARE_RECOVERY_BYTES.size.toLong(), prepared.getLong("total"))
            marker("Android unreadable Share source recovered with a valid selection")
        } finally {
            validUri?.let { MediaStoreSaver.delete(context, it) }
            File(context.filesDir, "manifest-v2/source-staging/$jobId").deleteRecursively()
            testRoot.deleteRecursively()
        }
    }

    @Test
    fun sendScenarioManifestV2Room() {
        requireEnabled()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val fixture = fixture()
        val testRoot = testRoot(context, "send")
        val sourceDirectory = File(testRoot, "sources").apply { check(mkdirs()) }
        val jobStore = File(testRoot, "jobs").apply { check(mkdirs()) }
        val stateDirectory = File(testRoot, "state").apply { check(mkdirs()) }
        val materialized = fixture.materialize(sourceDirectory)
        val created = checkedResponse(Native.createManifestV2Job(jobStore.path, "never"))
        val jobId = created.getString("job_id")
        val transientUris = mutableListOf<Uri>()

        try {
            val prepared =
                if (fixture.scenario == Scenario.Share) {
                    var snapshot = created
                    fixture.roots.zip(materialized.rootFiles).forEach { (root, file) ->
                        check(!root.directory) { "Share fixtures must be regular files" }
                        val uri = publishShareSource(context, file, root.name)
                        transientUris += uri
                        val source = ManifestV2Source(uri, directory = false, displayName = root.name)
                        val staged = runBlocking { ManifestV2SourceStager.stage(context, jobId, source) }
                        snapshot =
                            checkedResponse(
                                Native.prepareManifestV2Job(
                                    jobStore.path,
                                    jobId,
                                    ManifestV2SourceStager.rootsJson(source, staged, origin = "share"),
                                ),
                            )
                    }
                    snapshot
                } else {
                    checkedResponse(
                        Native.prepareManifestV2Job(
                            jobStore.path,
                            jobId,
                            fixture.rootsJson(materialized.selectedFiles),
                        ),
                    )
                }

            assertEquals("ready_to_send", prepared.getString("state"))
            assertEquals(fixture.roots.size, prepared.getInt("root_count"))
            assertEquals(fixture.fileCount, prepared.getInt("file_count"))
            assertEquals(fixture.directoryCount, prepared.getInt("directory_count"))
            assertEquals(fixture.totalBytes, prepared.getLong("total"))
            assertEquals(0, prepared.getInt("warning_count"))

            val callback = RecordingCallback()
            val sessionId = sessionId()
            try {
                Native.startManifestV2Session(
                    sessionId,
                    startParams(
                        context = context,
                        direction = "send",
                        stateDirectory = stateDirectory,
                        jobStore = jobStore,
                        jobId = jobId,
                    ).toString(),
                    callback,
                )
                callback.awaitTerminal(timeoutMs(fixture.totalBytes))
                callback.assertCompleted()
                assertTrue(callback.states.contains("waiting_for_receiver_save"))
                assertTrue(callback.states.contains("finalizing_delivery"))
                marker("Android send completed scenario=${fixture.scenario.wireName} bytes=${fixture.totalBytes}")
            } finally {
                Native.cancelManifestV2Session(sessionId)
            }
        } finally {
            transientUris.forEach { MediaStoreSaver.delete(context, it) }
            File(context.filesDir, "manifest-v2/source-staging/$jobId").deleteRecursively()
            testRoot.deleteRecursively()
        }
    }

    @Test
    fun receiveScenarioManifestV2Room() {
        requireEnabled()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val fixture = fixture()
        val testRoot = testRoot(context, "receive")
        val jobStore = File(testRoot, "jobs").apply { check(mkdirs()) }
        val stateDirectory = File(testRoot, "state").apply { check(mkdirs()) }
        val privateDestination = File(testRoot, "verified").apply { check(mkdirs()) }
        val localDestination = File(testRoot, "published").apply { check(mkdirs()) }
        val publicFolder = "EnvoixMatrix-${crossDeviceRunId()}"
        val sessionId = sessionId()
        val originalSettings = SettingsStore.settings.value
        val publishedUris = mutableListOf<Uri>()
        var collisionUri: Uri? = null

        if (fixture.scenario == Scenario.Collision) {
            val sentinel = File(testRoot, "collision-sentinel").apply { writeBytes(COLLISION_SENTINEL) }
            collisionUri = publishFile(context, sentinel, fixture.roots.single().name, publicFolder)
        }

        val platformWriter = ManifestV2DestinationWriter(context)
        val localPublisher = LocalTestDestinationPublisher(localDestination)
        val callback =
            RecordingCallback(
                plan = { request ->
                    if (fixture.directoryCount > 0) {
                        localPublisher.plan(request)
                    } else {
                        platformWriter.plan(request)
                    }
                },
                save = { request ->
                    if (fixture.directoryCount > 0) {
                        localPublisher.save(request)
                    } else {
                        platformWriter.save(request)
                    }
                },
                offer = { offer ->
                    assertEquals(fixture.roots.size, offer.getInt("root_count"))
                    assertEquals(fixture.fileCount, offer.getInt("file_count"))
                    assertEquals(fixture.directoryCount, offer.getInt("directory_count"))
                    assertEquals(fixture.totalBytes, offer.getLong("total"))
                    val page = checkedResponse(Native.listManifestV2OfferEntries(sessionId, 0, 256))
                    assertEquals(fixture.entryCount, page.getJSONArray("entries").length())
                    assertFalse(page.has("next_offset") && !page.isNull("next_offset"))
                    val decision =
                        JSONObject()
                            .put("target_directory", privateDestination.path)
                            .put("target_allocatable_bytes", privateDestination.usableSpace)
                            .put("exceptional_transfer_approved", false)
                    checkedResponse(Native.continueManifestV2Receive(sessionId, decision.toString()))
                },
            )

        try {
            SettingsStore.update { it.copy(saveTreeUri = "", saveFolder = publicFolder) }
            Native.startManifestV2Session(
                sessionId,
                startParams(
                    context = context,
                    direction = "receive",
                    stateDirectory = stateDirectory,
                    jobStore = jobStore,
                    jobId = null,
                ).toString(),
                callback,
            )
            marker("Android receiver ready scenario=${fixture.scenario.wireName}")
            callback.awaitTerminal(timeoutMs(fixture.totalBytes))
            val completed = callback.assertCompleted()
            assertTrue(callback.states.contains("offer"))
            assertTrue(callback.states.contains("saving"))
            val outcomes =
                (0 until completed.getJSONArray("roots").length())
                    .map { completed.getJSONArray("roots").getJSONObject(it) }
                    .sortedBy { it.getInt("root_id") }
            assertEquals(fixture.roots.size, outcomes.size)
            fixture.roots.zip(outcomes).forEach { (root, outcome) ->
                val uri = Uri.parse(outcome.getString("uri"))
                if (uri.scheme == "file") {
                    root.verifyFile(File(requireNotNull(uri.path)))
                } else {
                    check(!root.directory)
                    assertArrayEquals(
                        root.files
                            .single()
                            .payload
                            .digest(),
                        streamDigest(context, uri),
                    )
                    publishedUris += uri
                }
            }
            assertEquals(
                outcomes.size,
                outcomes
                    .map { it.getString("final_name") }
                    .toSet()
                    .size,
            )
            if (collisionUri != null) {
                assertArrayEquals(COLLISION_SENTINEL, streamBytes(context, collisionUri))
                assertNotEquals(fixture.roots.single().name, outcomes.single().getString("final_name"))
            }
            marker("Android receive saved scenario=${fixture.scenario.wireName} bytes=${fixture.totalBytes}")
        } finally {
            publishedUris.forEach { MediaStoreSaver.delete(context, it) }
            collisionUri?.let { MediaStoreSaver.delete(context, it) }
            callback.completed?.optString("job_id")?.takeIf(String::isNotBlank)?.let { jobId ->
                File(context.filesDir, "manifest-v2/destination-save")
                    .listFiles()
                    ?.filter { it.name.startsWith("$jobId-") }
                    ?.forEach(File::delete)
            }
            Native.cancelManifestV2Session(sessionId)
            SettingsStore.update { originalSettings }
            testRoot.deleteRecursively()
        }
    }

    private class RecordingCallback(
        private val plan: ((String) -> String)? = null,
        private val save: ((String) -> String)? = null,
        private val offer: ((JSONObject) -> Unit)? = null,
    ) : ManifestV2Callback {
        private val terminal = CountDownLatch(1)
        val states: MutableList<String> = Collections.synchronizedList(mutableListOf())

        @Volatile
        var completed: JSONObject? = null

        @Volatile
        private var failure: String? = null

        override fun onEvent(json: String) {
            val event = JSONObject(json)
            val state = event.optString("state")
            if (state.isNotEmpty()) states += state
            when (state) {
                "offer" -> offer?.invoke(event)
                "completed" -> {
                    completed = event
                    terminal.countDown()
                }
                "failed" -> {
                    failure = "${event.optString("cause")}: ${event.optString("detail")}".trim()
                    terminal.countDown()
                }
            }
            event.optString("message").takeIf(String::isNotBlank)?.let(::marker)
        }

        override fun onSaveRequired(requestJson: String): String =
            requireNotNull(save) { "sender unexpectedly requested a destination save" }(requestJson)

        override fun onPlanRequired(requestJson: String): String =
            requireNotNull(plan) { "sender unexpectedly requested a destination plan" }(requestJson)

        override fun onRememberedCredential(
            opaqueCredential: ByteArray,
            generation: Long,
        ): Boolean = false

        fun awaitTerminal(timeoutMs: Long) {
            assertTrue(
                "Manifest v2 physical transfer timed out after ${timeoutMs}ms",
                terminal.await(timeoutMs, TimeUnit.MILLISECONDS),
            )
        }

        fun assertCompleted(): JSONObject {
            assertTrue("Manifest v2 physical transfer failed: ${failure ?: "unknown"}", failure == null)
            return requireNotNull(completed) { "Manifest v2 transfer ended without completion" }
        }
    }

    private fun startParams(
        context: Context,
        direction: String,
        stateDirectory: File,
        jobStore: File,
        jobId: String?,
    ): JSONObject =
        JSONObject()
            .put("direction", direction)
            .put("room", scenarioCode())
            .put("broker", Endpoints.BROKER)
            .put("relay", Endpoints.RELAY)
            .put("state_directory", stateDirectory.path)
            .put("job_store_directory", jobStore.path)
            .put("job_id", jobId ?: JSONObject.NULL)
            .put("use_room", true)
            .put("use_mdns", false)
            .also { check(context.packageName == "dev.envoix.app") }

    private fun publishShareSource(
        context: Context,
        source: File,
        name: String,
    ): Uri = publishFile(context, source, name, "EnvoixMatrixSources-${crossDeviceRunId()}")

    private fun publishFile(
        context: Context,
        source: File,
        name: String,
        folder: String,
    ): Uri {
        val reserved = requireNotNull(MediaStoreSaver.reserve(context, name, "", folder))
        MediaStoreSaver.copyInto(context, source, reserved).getOrThrow()
        return MediaStoreSaver.commit(context, reserved).getOrThrow().uri
    }

    private fun streamBytes(
        context: Context,
        uri: Uri,
    ): ByteArray = requireNotNull(context.contentResolver.openInputStream(uri)).use { it.readBytes() }

    private fun streamDigest(
        context: Context,
        uri: Uri,
    ): ByteArray = requireNotNull(context.contentResolver.openInputStream(uri)).use(::streamDigest)

    private fun streamDigest(input: java.io.InputStream): ByteArray {
        val digest = MessageDigest.getInstance(SHA256)
        val buffer = ByteArray(HASH_BLOCK_BYTES)
        while (true) {
            val count = input.read(buffer)
            if (count < 0) break
            if (count > 0) digest.update(buffer, 0, count)
        }
        return digest.digest()
    }

    private fun requireEnabled() {
        assumeTrue(
            "set -e $ARG_ENABLED 1 to run physical cross-device tests",
            argument(ARG_ENABLED) == "1",
        )
    }

    private fun fixture(): Fixture =
        Fixture.make(
            scenario =
                requireNotNull(Scenario.fromWireName(argument(ARG_SCENARIO) ?: Scenario.SingleFile.wireName)) {
                    "unknown transfer scenario"
                },
            runId = crossDeviceRunId(),
            largeBytes = longArgument(ARG_LARGE_BYTES) ?: DEFAULT_LARGE_BYTES,
        )

    private fun testRoot(
        context: Context,
        suffix: String,
    ): File =
        File(context.cacheDir, "envoix-manifest-v2-${crossDeviceRunId()}-$suffix").apply {
            deleteRecursively()
            check(mkdirs())
        }

    private fun crossDeviceRunId(): String {
        val value = argument(ARG_RUN_ID) ?: "manual"
        require(value.length <= MAX_RUN_ID_LENGTH && value.all { it.isLetterOrDigit() || it == '-' || it == '_' })
        return value
    }

    private fun scenarioCode(): String = argument(ARG_CODE) ?: DEFAULT_CODE

    private fun argument(name: String): String? =
        InstrumentationRegistry
            .getArguments()
            .getString(name)
            ?.trim()
            ?.takeIf(String::isNotEmpty)

    private fun longArgument(name: String): Long? {
        val raw = argument(name) ?: return null
        return requireNotNull(raw.toLongOrNull()) { "$name must be a non-negative integer" }
            .also { require(it >= 0) }
    }

    private fun timeoutMs(expectedBytes: Long): Long =
        longArgument(ARG_TIMEOUT_MS) ?: maxOf(DEFAULT_TIMEOUT_MS, DEFAULT_TIMEOUT_MS + expectedBytes / TIMEOUT_BYTES_PER_MS)

    private fun sessionId(): Long = System.nanoTime().and(Long.MAX_VALUE)

    private fun checkedResponse(raw: String): JSONObject =
        JSONObject(raw).also { value ->
            assertFalse(value.optString("error", "native operation failed"), value.has("error"))
        }

    private companion object {
        const val LOG_TAG = "EnvoixCrossDevice"
        const val ARG_ENABLED = "envoixCrossDevice"
        const val ARG_RUN_ID = "envoixCrossDeviceRunId"
        const val ARG_TIMEOUT_MS = "envoixCrossDeviceTimeoutMs"
        const val ARG_SCENARIO = "envoixCrossDeviceScenario"
        const val ARG_CODE = "envoixCrossDeviceCode"
        const val ARG_LARGE_BYTES = "envoixCrossDeviceLargeBytes"
        const val DEFAULT_CODE = "741203-amber-comet"
        const val DEFAULT_TIMEOUT_MS = 180_000L
        const val TIMEOUT_BYTES_PER_MS = 2_048L
        const val DEFAULT_LARGE_BYTES = 128L * 1_024 * 1_024
        const val MAX_RUN_ID_LENGTH = 72
        const val HASH_BLOCK_BYTES = 1024 * 1024
        const val SHA256 = "SHA-256"
        val COLLISION_SENTINEL = "pre-existing destination must remain unchanged\n".toByteArray()
        val SHARE_RECOVERY_BYTES = "valid source after an unreadable Share item\n".toByteArray()

        fun marker(message: String) {
            val line = "[cross-device] $message"
            Log.i(LOG_TAG, line)
            println(line)
        }
    }
}

private enum class Scenario(
    val wireName: String,
) {
    SingleFile("single_file"),
    MultipleFiles("multiple_files"),
    Folder("folder"),
    MultipleFolders("multiple_folders"),
    Image("image"),
    Share("share"),
    LargeFile("large_file"),
    Collision("collision"),
    Overlap("overlap"),
    UnicodeAndEmpty("unicode_empty"),
    SameNameRoots("same_name_roots"),
    ;

    companion object {
        fun fromWireName(value: String): Scenario? = entries.firstOrNull { it.wireName == value }
    }
}

private data class Fixture(
    val scenario: Scenario,
    val roots: List<FixtureRoot>,
    val overlappingSelection: Boolean = false,
) {
    val fileCount: Int = roots.sumOf { it.files.size }
    val directoryCount: Int = roots.sumOf { if (it.directory) 1 + it.directories.size else 0 }
    val entryCount: Int = fileCount + directoryCount
    val totalBytes: Long = roots.flatMap(FixtureRoot::files).sumOf { it.payload.size }

    fun materialize(parent: File): MaterializedFixture {
        val roots =
            roots.mapIndexed { index, root ->
                val selectionDirectory = File(parent, "selection-$index").apply { check(mkdirs()) }
                root.materialize(selectionDirectory)
            }
        val selected = roots.toMutableList()
        if (overlappingSelection) {
            val child =
                this.roots
                    .first()
                    .files
                    .first()
                    .path
                    .fold(roots.first(), ::File)
            selected += child
        }
        return MaterializedFixture(roots, selected)
    }

    fun rootsJson(selected: List<File>): String =
        JSONArray()
            .apply {
                selected.forEach { file ->
                    put(
                        JSONObject()
                            .put("path", file.path)
                            .put("requested_name", file.name)
                            .put("origin", "file_provider")
                            .put("issues", JSONArray()),
                    )
                }
            }.toString()

    companion object {
        fun make(
            scenario: Scenario,
            runId: String,
            largeBytes: Long,
        ): Fixture {
            fun text(
                path: String,
                value: String,
            ) = FixtureFile(path.split('/'), Payload.Data(value.toByteArray()))

            fun file(
                name: String,
                bytes: ByteArray,
            ) = FixtureRoot(name, false, emptyList(), listOf(FixtureFile(emptyList(), Payload.Data(bytes))))

            val roots =
                when (scenario) {
                    Scenario.SingleFile -> listOf(file("single-$runId.txt", "single file fixture $runId\n".toByteArray()))
                    Scenario.MultipleFiles ->
                        listOf(
                            file("alpha-$runId.txt", "alpha\n".toByteArray()),
                            file("beta-$runId.bin", ByteArray(257) { (it % 251).toByte() }),
                            file("空 白-$runId.txt", "多文件内容\n".toByteArray()),
                        )
                    Scenario.Folder ->
                        listOf(
                            FixtureRoot(
                                "Folder-$runId",
                                true,
                                listOf(listOf("Empty"), listOf("Nested"), listOf("Nested", "深层")),
                                listOf(
                                    text("alpha.txt", "folder alpha\n"),
                                    text("Nested/beta.bin", "folder beta\n"),
                                    FixtureFile(listOf("Nested", "深层", "zero.dat"), Payload.Data(ByteArray(0))),
                                ),
                            ),
                        )
                    Scenario.MultipleFolders ->
                        listOf(
                            FixtureRoot(
                                "First-$runId",
                                true,
                                listOf(listOf("Nested")),
                                listOf(text("one.txt", "first root\n"), text("Nested/two.txt", "nested root\n")),
                            ),
                            FixtureRoot(
                                "Second-$runId",
                                true,
                                listOf(listOf("Empty")),
                                listOf(FixtureFile(listOf("photo.png"), Payload.Data(PNG_DATA))),
                            ),
                        )
                    Scenario.Image -> listOf(file("photo-$runId.png", PNG_DATA))
                    Scenario.Share ->
                        listOf(
                            file("shared-note-$runId.txt", "shared through platform provider\n".toByteArray()),
                            file("shared-photo-$runId.png", PNG_DATA),
                        )
                    Scenario.LargeFile ->
                        listOf(
                            FixtureRoot(
                                "large-$runId.bin",
                                false,
                                emptyList(),
                                listOf(
                                    FixtureFile(
                                        emptyList(),
                                        Payload.Repeated("large Manifest v2 fixture $runId\n".toByteArray(), largeBytes),
                                    ),
                                ),
                            ),
                        )
                    Scenario.Collision -> listOf(file("Photo.png", PNG_DATA))
                    Scenario.Overlap ->
                        listOf(
                            FixtureRoot(
                                "Overlap-$runId",
                                true,
                                listOf(listOf("Empty"), listOf("Nested")),
                                listOf(text("inside.txt", "selected twice but sent once\n"), text("Nested/deep.bin", "deep\n")),
                            ),
                        )
                    Scenario.UnicodeAndEmpty ->
                        listOf(
                            FixtureRoot(
                                "资料-$runId",
                                true,
                                listOf(listOf("空目录"), listOf("子 目录")),
                                listOf(
                                    text("résumé.txt", "naïve café\n"),
                                    FixtureFile(listOf("子 目录", "照片 ①.png"), Payload.Data(PNG_DATA)),
                                    FixtureFile(listOf("零字节.dat"), Payload.Data(ByteArray(0))),
                                ),
                            ),
                        )
                    Scenario.SameNameRoots ->
                        listOf(
                            file("duplicate.txt", "first duplicate root\n".toByteArray()),
                            file("duplicate.txt", "second duplicate root\n".toByteArray()),
                        )
                }
            return Fixture(scenario, roots, overlappingSelection = scenario == Scenario.Overlap)
        }

        private val PNG_DATA =
            Base64.decode(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
                Base64.DEFAULT,
            )
    }
}

private data class MaterializedFixture(
    val rootFiles: List<File>,
    val selectedFiles: List<File>,
)

private data class FixtureRoot(
    val name: String,
    val directory: Boolean,
    val directories: List<List<String>>,
    val files: List<FixtureFile>,
) {
    fun materialize(parent: File): File {
        val root = File(parent, name)
        if (directory) check(root.mkdir())
        directories.sortedBy(List<String>::size).forEach { components ->
            check(components.fold(root, ::File).mkdirs())
        }
        files.forEach { file ->
            val target = file.path.fold(root, ::File)
            if (directory) target.parentFile?.mkdirs()
            file.payload.write(target)
        }
        return root
    }

    fun verifyFile(root: File) {
        assertTrue(root.exists())
        assertEquals(directory, root.isDirectory)
        if (directory) {
            val expected =
                (directories.map { it.joinToString("/") } + files.map { it.path.joinToString("/") }).toSet()
            val actual =
                root
                    .walkTopDown()
                    .drop(1)
                    .map { it.relativeTo(root).invariantSeparatorsPath }
                    .toSet()
            assertEquals(expected, actual)
        }
        files.forEach { file ->
            val target = file.path.fold(root, ::File)
            assertArrayEquals(file.payload.digest(), target.inputStream().use(::sha256))
        }
    }
}

private data class FixtureFile(
    val path: List<String>,
    val payload: Payload,
)

private sealed interface Payload {
    val size: Long

    fun forEachChunk(consumer: (ByteArray, Int) -> Unit)

    fun write(file: File) {
        file.outputStream().buffered(HASH_BLOCK_BYTES).use { output ->
            forEachChunk { block, count -> output.write(block, 0, count) }
        }
    }

    fun digest(): ByteArray {
        val digest = MessageDigest.getInstance(SHA256)
        forEachChunk { block, count -> digest.update(block, 0, count) }
        return digest.digest()
    }

    data class Data(
        val bytes: ByteArray,
    ) : Payload {
        override val size: Long = bytes.size.toLong()

        override fun forEachChunk(consumer: (ByteArray, Int) -> Unit) = consumer(bytes, bytes.size)
    }

    data class Repeated(
        val pattern: ByteArray,
        override val size: Long,
    ) : Payload {
        override fun forEachChunk(consumer: (ByteArray, Int) -> Unit) {
            require(pattern.isNotEmpty() || size == 0L)
            val repeats = maxOf(1, HASH_BLOCK_BYTES / pattern.size)
            val block = ByteArray(repeats * pattern.size)
            repeat(repeats) { pattern.copyInto(block, it * pattern.size) }
            var remaining = size
            while (remaining > 0) {
                val count = minOf(remaining, block.size.toLong()).toInt()
                consumer(block, count)
                remaining -= count
            }
        }
    }

    companion object {
        const val HASH_BLOCK_BYTES = 1024 * 1024
        const val SHA256 = "SHA-256"
    }
}

private class LocalTestDestinationPublisher(
    private val destination: File,
) {
    private val plannedNames = mutableMapOf<Int, String>()

    fun plan(requestJson: String): String {
        val roots = JSONObject(requestJson).getJSONArray("roots")
        val reply = JSONArray()
        for (index in 0 until roots.length()) {
            val root = roots.getJSONObject(index)
            val name =
                allocateName(
                    root.getString("requested_name"),
                    root.getString("kind") == "file",
                    plannedNames.values.toSet(),
                )
            plannedNames[root.getInt("root_id")] = name
            reply.put(
                JSONObject()
                    .put("root_id", root.getInt("root_id"))
                    .put("planned_name", name),
            )
        }
        return JSONObject().put("roots", reply).toString()
    }

    fun save(requestJson: String): String {
        val roots = JSONObject(requestJson).getJSONArray("roots")
        val outcomes = JSONArray()
        for (index in 0 until roots.length()) {
            val root = roots.getJSONObject(index)
            val source = File(root.getString("local_path"))
            val finalName = root.getString("planned_name")
            check(plannedNames[root.getInt("root_id")] == finalName)
            val target = File(destination, finalName)
            check(source.copyRecursively(target, overwrite = false))
            outcomes.put(
                JSONObject()
                    .put("root_id", root.getInt("root_id"))
                    .put("final_name", finalName)
                    .put("uri", Uri.fromFile(target).toString()),
            )
        }
        return JSONObject().put("roots", outcomes).toString()
    }

    private fun allocateName(
        requestedName: String,
        preserveExtension: Boolean,
        reserved: Set<String>,
    ): String =
        (0 until 10_000)
            .map { suffix ->
                if (suffix == 0) requestedName else manifestV2KeepBothName(requestedName, suffix, preserveExtension)
            }.first { it !in reserved && !File(destination, it).exists() }
}

private fun sha256(input: java.io.InputStream): ByteArray {
    val digest = MessageDigest.getInstance("SHA-256")
    val buffer = ByteArray(1024 * 1024)
    while (true) {
        val count = input.read(buffer)
        if (count < 0) break
        if (count > 0) digest.update(buffer, 0, count)
    }
    return digest.digest()
}
