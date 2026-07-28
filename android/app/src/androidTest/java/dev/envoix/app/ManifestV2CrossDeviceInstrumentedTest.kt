package dev.envoix.app

import android.app.Application
import android.content.Context
import android.net.Uri
import android.os.Build
import android.provider.OpenableColumns
import android.util.Base64
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.mapNotNull
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
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
        val evidence = endpointEvidence(context, fixture, "sender")
        runWithEndpointEvidence(evidence) {
            val testRoot = testRoot(context, "send")
            val sourceDirectory = File(testRoot, "sources").apply { check(mkdirs()) }
            val jobStore = File(testRoot, "jobs").apply { check(mkdirs()) }
            val stateDirectory = File(testRoot, "state").apply { check(mkdirs()) }
            val transientUris = mutableListOf<Uri>()
            var jobId: String? = null
            try {
                val materialized = fixture.materialize(sourceDirectory)
                evidence.recordSource(materialized.rootFiles)
                val created = checkedResponse(Native.createManifestV2Job(jobStore.path, "never"))
                val createdJobId = created.getString("job_id")
                jobId = createdJobId
                evidence.jobId = createdJobId
                val prepared =
                    if (fixture.scenario == Scenario.Share) {
                        var snapshot = created
                        fixture.roots.zip(materialized.rootFiles).forEach { (root, file) ->
                            check(!root.directory) { "Share fixtures must be regular files" }
                            val uri = publishShareSource(context, file, root.name)
                            transientUris += uri
                            val source = ManifestV2Source(uri, directory = false, displayName = root.name)
                            val staged = runBlocking { ManifestV2SourceStager.stage(context, createdJobId, source) }
                            snapshot =
                                checkedResponse(
                                    Native.prepareManifestV2Job(
                                        jobStore.path,
                                        createdJobId,
                                        ManifestV2SourceStager.rootsJson(source, staged, origin = "share"),
                                    ),
                                )
                        }
                        snapshot
                    } else {
                        checkedResponse(
                            Native.prepareManifestV2Job(
                                jobStore.path,
                                createdJobId,
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

                val invitation =
                    checkedResponse(
                        Native.parseInviteForRole(
                            scenarioCode(),
                            "send",
                        ),
                    )
                val callback = RecordingCallback(evidence = evidence)
                val sessionId = sessionId()
                try {
                    Native.startManifestV2Session(
                        sessionId,
                        startParams(
                            context = context,
                            direction = "send",
                            stateDirectory = stateDirectory,
                            jobStore = jobStore,
                            jobId = createdJobId,
                            invitationReference = invitation.getString("reference"),
                        ).toString(),
                        callback,
                    )
                    callback.awaitTerminal(timeoutMs(fixture.totalBytes))
                    callback.assertCompleted()
                    assertTrue(callback.states.contains("waiting_for_receiver_save"))
                    assertTrue(callback.states.contains("finalizing_delivery"))
                    evidence.deliveryProof = true
                    marker("Android send completed scenario=${fixture.scenario.wireName} bytes=${fixture.totalBytes}")
                } finally {
                    Native.cancelManifestV2Session(sessionId)
                }
            } finally {
                transientUris.forEach { check(MediaStoreSaver.delete(context, it)) }
                jobId?.let {
                    check(File(context.filesDir, "manifest-v2/source-staging/$it").deleteRecursively())
                }
                check(testRoot.deleteRecursively())
                evidence.cleanupCompleted = true
            }
        }
    }

    @Test
    fun receiveScenarioManifestV2Room() {
        requireEnabled()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val fixture = fixture()
        val evidence = endpointEvidence(context, fixture, "receiver")
        runWithEndpointEvidence(evidence) {
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
            var callback: RecordingCallback? = null
            try {
                val invitation =
                    checkedResponse(
                        Native.generateInvite(
                            "receive",
                            Endpoints.BROKER,
                            Endpoints.RELAY,
                        ),
                    )
                marker("invitation=${invitation.getString("code")}")

                if (fixture.scenario == Scenario.Collision) {
                    val sentinel = File(testRoot, "collision-sentinel").apply { writeBytes(COLLISION_SENTINEL) }
                    collisionUri = publishFile(context, sentinel, fixture.roots.single().name, publicFolder)
                }

                val platformWriter = ManifestV2DestinationWriter(context)
                val localPublisher = LocalTestDestinationPublisher(localDestination)
                val endpointCallback =
                    RecordingCallback(
                        evidence = evidence,
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
                callback = endpointCallback

                SettingsStore.update { it.copy(saveTreeUri = "", saveFolder = publicFolder) }
                Native.startManifestV2Session(
                    sessionId,
                    startParams(
                        context = context,
                        direction = "receive",
                        stateDirectory = stateDirectory,
                        jobStore = jobStore,
                        jobId = null,
                        invitationReference = invitation.getString("reference"),
                    ).toString(),
                    endpointCallback,
                )
                marker("Android receiver ready scenario=${fixture.scenario.wireName}")
                endpointCallback.awaitTerminal(timeoutMs(fixture.totalBytes))
                val completed = endpointCallback.assertCompleted()
                evidence.jobId = completed.optString("job_id").takeIf(String::isNotBlank)
                assertTrue(endpointCallback.states.contains("offer"))
                assertTrue(endpointCallback.states.contains("saving"))
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
                evidence.recordDestination(outcomes)
                evidence.deliveryProof = true
                marker("Android receive saved scenario=${fixture.scenario.wireName} bytes=${fixture.totalBytes}")
            } finally {
                publishedUris.forEach { check(MediaStoreSaver.delete(context, it)) }
                collisionUri?.let { check(MediaStoreSaver.delete(context, it)) }
                callback
                    ?.completed
                    ?.optString("job_id")
                    ?.takeIf(String::isNotBlank)
                    ?.let { jobId ->
                        File(context.filesDir, "manifest-v2/destination-save")
                            .listFiles()
                            ?.filter { it.name.startsWith("$jobId-") }
                            ?.forEach { check(it.delete()) }
                    }
                Native.cancelManifestV2Session(sessionId)
                SettingsStore.update { originalSettings }
                check(testRoot.deleteRecursively())
                evidence.cleanupCompleted = true
            }
        }
    }

    @Test
    fun sendScenarioProductActivityRoom() {
        requireEnabled()
        requireProductActivityRun()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val fixture = fixture()
        require(fixture.directoryCount == 0) {
            "Android product-Activity directory publication requires a provisioned SAF tree"
        }
        val evidence = endpointEvidence(context, fixture, "sender", productActivity = true)
        runWithEndpointEvidence(evidence) {
            val testRoot = testRoot(context, "product-send")
            val sourceDirectory = File(testRoot, "sources").apply { check(mkdirs()) }
            val transientUris = mutableListOf<Uri>()
            var model: TransferViewModel? = null
            var transferId: Long? = null
            var jobId: String? = null
            try {
                val materialized = fixture.materialize(sourceDirectory)
                evidence.recordSource(materialized.rootFiles)
                val jobStore = TransferService.jobStoreDirectory(context)
                val created = checkedResponse(Native.createManifestV2Job(jobStore.path, "never"))
                val createdJobId = created.getString("job_id")
                jobId = createdJobId
                evidence.jobId = createdJobId
                var prepared = created
                fixture.roots.zip(materialized.rootFiles).forEach { (root, file) ->
                    check(!root.directory)
                    val uri = publishShareSource(context, file, root.name)
                    transientUris += uri
                    val source = ManifestV2Source(uri, directory = false, displayName = root.name)
                    val staged = runBlocking { ManifestV2SourceStager.stage(context, createdJobId, source) }
                    prepared =
                        checkedResponse(
                            Native.prepareManifestV2Job(
                                jobStore.path,
                                createdJobId,
                                ManifestV2SourceStager.rootsJson(source, staged),
                            ),
                        )
                }
                assertEquals("ready_to_send", prepared.getString("state"))
                assertEquals(fixture.roots.size, prepared.getInt("root_count"))
                assertEquals(fixture.fileCount, prepared.getInt("file_count"))
                assertEquals(fixture.totalBytes, prepared.getLong("total"))

                val invitation = checkedResponse(Native.parseInviteForRole(scenarioCode(), "send"))
                val productModel =
                    TransferViewModel(context.applicationContext as Application).also { model = it }
                val id =
                    productModel
                        .startSend(
                            invitation.getString("reference"),
                            createdJobId,
                            Endpoints.BROKER,
                            Endpoints.RELAY,
                            qrPayload = null,
                        ).also { transferId = it }
                val completed = awaitProductTransfer(id, evidence, timeoutMs(fixture.totalBytes))
                assertEquals(Status.Delivered, completed.status)
                assertEquals(createdJobId, completed.jobId)
                assertEquals(fixture.totalBytes, completed.total)
                evidence.deliveryProof = true
                marker(
                    "Android product sender delivered " +
                        "scenario=${fixture.scenario.wireName} bytes=${fixture.totalBytes}",
                )
            } finally {
                if (model != null && transferId != null) {
                    cleanupProductTransfer(requireNotNull(model), requireNotNull(transferId))
                }
                transientUris.forEach { check(MediaStoreSaver.delete(context, it)) }
                jobId?.let { deleteManifestJobArtifacts(context.filesDir, it) }
                check(testRoot.deleteRecursively())
                evidence.cleanupCompleted = true
            }
        }
    }

    @Test
    fun receiveScenarioProductActivityRoom() {
        requireEnabled()
        requireProductActivityRun()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val fixture = fixture()
        require(fixture.directoryCount == 0) {
            "Android product-Activity directory publication requires a provisioned SAF tree"
        }
        val evidence = endpointEvidence(context, fixture, "receiver", productActivity = true)
        runWithEndpointEvidence(evidence) {
            val originalSettings = SettingsStore.settings.value
            val publicFolder = "EnvoixMatrix-${crossDeviceRunId()}"
            val publishedUris = mutableListOf<Uri>()
            var model: TransferViewModel? = null
            var transferId: Long? = null
            try {
                SettingsStore.update { it.copy(saveTreeUri = "", saveFolder = publicFolder) }
                val invitation =
                    checkedResponse(
                        Native.generateInvite(
                            "receive",
                            Endpoints.BROKER,
                            Endpoints.RELAY,
                        ),
                    )
                marker("invitation=${invitation.getString("code")}")
                val productModel =
                    TransferViewModel(context.applicationContext as Application).also { model = it }
                val id =
                    productModel
                        .startReceive(
                            invitation.getString("reference"),
                            Endpoints.BROKER,
                            Endpoints.RELAY,
                            qrPayload = invitation.getString("code"),
                            destinationCopyApproved = true,
                        ).also { transferId = it }
                var readyPublished = false
                val completed =
                    awaitProductTransfer(
                        id,
                        evidence,
                        timeoutMs(fixture.totalBytes),
                    ) { transfer ->
                        if (!readyPublished && transfer.status == Status.WaitingForPeer) {
                            readyPublished = true
                            marker("Android receiver ready scenario=${fixture.scenario.wireName}")
                        }
                    }
                assertTrue("product receiver never reached its native readiness phase", readyPublished)
                assertEquals(Status.Delivered, completed.status)
                assertEquals(fixture.roots.size, completed.savedUris.size)
                fixture.roots.zip(completed.savedUris).forEach { (root, encodedUri) ->
                    val uri = Uri.parse(encodedUri)
                    assertEquals(root.name, displayName(context, uri))
                    assertArrayEquals(
                        root.files
                            .single()
                            .payload
                            .digest(),
                        streamDigest(context, uri),
                    )
                    publishedUris += uri
                }
                evidence.recordProductDestination(completed.savedUris.map(Uri::parse))
                evidence.jobId = completed.jobId
                evidence.deliveryProof = true
                marker(
                    "Android product receiver published " +
                        "scenario=${fixture.scenario.wireName} bytes=${fixture.totalBytes}",
                )
            } finally {
                if (model != null && transferId != null) {
                    cleanupProductTransfer(requireNotNull(model), requireNotNull(transferId))
                }
                publishedUris.forEach { check(MediaStoreSaver.delete(context, it)) }
                SettingsStore.update { originalSettings }
                evidence.cleanupCompleted = true
            }
        }
    }

    private class RecordingCallback(
        private val evidence: AndroidMatrixEndpointEvidence,
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
            evidence.recordEvent(event)
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
        invitationReference: String,
    ): JSONObject =
        JSONObject()
            .put("direction", direction)
            .put("mode", "invitation")
            .put("room", invitationReference)
            .put("invitation_ref", invitationReference)
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

    private fun displayName(
        context: Context,
        uri: Uri,
    ): String =
        requireNotNull(
            context.contentResolver
                .query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                ?.use { cursor ->
                    if (cursor.moveToFirst()) cursor.getString(0) else null
                },
        ) { "published URI has no display name" }

    private fun awaitProductTransfer(
        id: Long,
        evidence: AndroidMatrixEndpointEvidence,
        timeoutMs: Long,
        onUpdate: (Transfer) -> Unit = {},
    ): Transfer =
        runBlocking {
            withTimeout(timeoutMs) {
                TransferRepository.transfers
                    .mapNotNull { transfers ->
                        transfers
                            .firstOrNull { it.id == id }
                            ?.also {
                                evidence.recordActivity(it)
                                onUpdate(it)
                            }
                    }.first { it.status.isTerminal }
            }
        }

    private fun cleanupProductTransfer(
        model: TransferViewModel,
        id: Long,
    ) {
        val current = TransferRepository.transfers.value.firstOrNull { it.id == id }
        if (current != null && !current.status.isTerminal) {
            model.cancel(id)
            runBlocking {
                withTimeout(PRODUCT_CLEANUP_TIMEOUT_MS) {
                    TransferRepository.transfers
                        .mapNotNull { transfers -> transfers.firstOrNull { it.id == id } }
                        .first { it.status.isTerminal }
                }
            }
        }
        if (TransferRepository.transfers.value.any { it.id == id }) {
            model.remove(id)
            runBlocking {
                withTimeout(PRODUCT_CLEANUP_TIMEOUT_MS) {
                    TransferRepository.transfers.first { transfers ->
                        transfers.none { it.id == id }
                    }
                }
            }
        }
    }

    private fun requireEnabled() {
        assumeTrue(
            "set -e $ARG_ENABLED 1 to run physical cross-device tests",
            argument(ARG_ENABLED) == "1",
        )
    }

    private fun requireProductActivityRun() {
        require(argument(ARG_BUILD_VARIANT) == "release_equivalent") {
            "product-Activity matrix tests require a release-equivalent build"
        }
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
        require(
            value.length <= MAX_RUN_ID_LENGTH &&
                value.all { it.isLetterOrDigit() || it == '-' || it == '_' || it == '.' },
        )
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

    private fun endpointEvidence(
        context: Context,
        fixture: Fixture,
        role: String,
        productActivity: Boolean = false,
    ): AndroidMatrixEndpointEvidence =
        AndroidMatrixEndpointEvidence(
            context = context,
            fixture = fixture,
            runId = crossDeviceRunId(),
            caseId = crossDeviceCaseId(),
            repetition = crossDeviceRepetition(),
            role = role,
            buildVariant = argument(ARG_BUILD_VARIANT) ?: "debug",
            testLayer = if (productActivity) "l2_physical" else "l1_native",
            driver = if (productActivity) "product_activity" else "direct_jni",
        )

    private fun runWithEndpointEvidence(
        evidence: AndroidMatrixEndpointEvidence,
        block: () -> Unit,
    ) {
        var testFailure: Throwable? = null
        try {
            block()
            evidence.complete()
        } catch (error: Throwable) {
            testFailure = error
            evidence.fail()
            throw error
        } finally {
            try {
                evidence.write()
            } catch (writeFailure: Throwable) {
                testFailure?.addSuppressed(writeFailure) ?: throw writeFailure
            }
        }
    }

    private fun crossDeviceCaseId(): String {
        val value = argument(ARG_CASE_ID) ?: "manual"
        require(value.length <= MAX_IDENTIFIER_LENGTH && value.matches(LOWER_IDENTIFIER))
        return value
    }

    private fun crossDeviceRepetition(): Int {
        val value = argument(ARG_REPETITION) ?: "1"
        return requireNotNull(value.toIntOrNull()) { "$ARG_REPETITION must be an integer" }
            .also { require(it in 1..10) }
    }

    private fun sessionId(): Long = System.nanoTime().and(Long.MAX_VALUE)

    private fun checkedResponse(raw: String): JSONObject =
        JSONObject(raw).also { value ->
            assertFalse(value.optString("error", "native operation failed"), value.has("error"))
        }

    private companion object {
        const val LOG_TAG = "EnvoixCrossDevice"
        const val ARG_ENABLED = "envoixCrossDevice"
        const val ARG_RUN_ID = "envoixCrossDeviceRunId"
        const val ARG_CASE_ID = "envoixCrossDeviceCaseId"
        const val ARG_REPETITION = "envoixCrossDeviceRepetition"
        const val ARG_BUILD_VARIANT = "envoixCrossDeviceBuildVariant"
        const val ARG_TIMEOUT_MS = "envoixCrossDeviceTimeoutMs"
        const val ARG_SCENARIO = "envoixCrossDeviceScenario"
        const val ARG_CODE = "envoixCrossDeviceCode"
        const val ARG_LARGE_BYTES = "envoixCrossDeviceLargeBytes"
        const val DEFAULT_CODE = "741203-ambe-come"
        const val DEFAULT_TIMEOUT_MS = 180_000L
        const val PRODUCT_CLEANUP_TIMEOUT_MS = 15_000L
        const val TIMEOUT_BYTES_PER_MS = 2_048L
        const val DEFAULT_LARGE_BYTES = 128L * 1_024 * 1_024
        const val MAX_RUN_ID_LENGTH = 96
        const val MAX_IDENTIFIER_LENGTH = 96
        const val HASH_BLOCK_BYTES = 1024 * 1024
        const val SHA256 = "SHA-256"
        val LOWER_IDENTIFIER = Regex("[a-z0-9][a-z0-9_.-]{0,95}")
        val COLLISION_SENTINEL = "pre-existing destination must remain unchanged\n".toByteArray()
        val SHARE_RECOVERY_BYTES = "valid source after an unreadable Share item\n".toByteArray()

        fun marker(message: String) {
            val line = "[cross-device] $message"
            Log.i(LOG_TAG, line)
            println(line)
        }
    }
}

private class AndroidMatrixEndpointEvidence(
    private val context: Context,
    private val fixture: Fixture,
    private val runId: String,
    private val caseId: String,
    private val repetition: Int,
    private val role: String,
    private val buildVariant: String,
    private val testLayer: String,
    private val driver: String,
) {
    private val startedAt = System.currentTimeMillis()
    private val phases = mutableListOf<String>()
    private var selectedPath: String? = null
    private var nativeFailureCode: String? = null
    private var nativeFailurePhase: String? = null
    private var nativeRecoveryAction: String? = null
    private var sourceSummary: JSONObject? = null
    private var destinationSummary: JSONObject? = null
    private var terminalState: String? = null
    private var activityId: String? = null
    private var attemptCount = 1

    var jobId: String? = null
    var deliveryProof: Boolean = false
    var cleanupCompleted: Boolean = false

    init {
        require(role == "sender" || role == "receiver")
        require(buildVariant == "debug" || buildVariant == "release_equivalent")
        require(
            (testLayer == "l1_native" && driver == "direct_jni") ||
                (testLayer == "l2_physical" && driver == "product_activity"),
        )
    }

    @Synchronized
    fun recordEvent(event: JSONObject) {
        if (event.optString("kind") == "path") {
            event
                .optString("path_kind")
                .takeIf { it in PATH_KINDS }
                ?.let { selectedPath = it }
        }
        val state = event.optString("state")
        if (state !in PHASES) return
        if (state == "failed") {
            nativeFailurePhase = phases.lastOrNull()
            nativeFailureCode =
                event
                    .optString("cause")
                    .takeIf(::stableFailureCode)
                    ?: "native_failure"
            nativeRecoveryAction =
                event
                    .optString("recovery_action")
                    .takeIf { it in RECOVERY_ACTIONS }
                    ?: "none"
        }
        if (phases.lastOrNull() != state) phases += state
    }

    @Synchronized
    fun recordActivity(transfer: Transfer) {
        activityId = transfer.id.toString()
        attemptCount = maxOf(attemptCount, transfer.attempt)
        transfer.jobId?.takeIf(String::isNotBlank)?.let { jobId = it }
        transfer.pathAddr?.takeIf { it in PATH_KINDS }?.let { selectedPath = it }
        val phase =
            when (transfer.status) {
                Status.Preparing -> null
                Status.WaitingForPeer -> "waiting_for_peer"
                Status.Pairing -> "pairing"
                Status.Connecting -> "connecting"
                Status.AwaitingDecision -> "offer"
                Status.Transferring -> "transferring"
                Status.Verifying -> "verifying"
                Status.Saving -> "saving"
                Status.WaitingForReceiverSave -> "waiting_for_receiver_save"
                Status.FinalizingDelivery -> "finalizing_delivery"
                Status.Delivered -> "completed"
                Status.Failed, Status.Canceled -> "failed"
                Status.Paused -> null
            }
        if (phase == "failed") {
            nativeFailurePhase = phases.lastOrNull()
            nativeFailureCode =
                transfer.failureCause
                    ?.takeIf(::stableFailureCode)
                    ?: if (transfer.status == Status.Canceled) {
                        "user_canceled"
                    } else {
                        "product_activity_failed"
                    }
            nativeRecoveryAction = transfer.recoveryAction.wire
        }
        if (phase != null && phases.lastOrNull() != phase) phases += phase
    }

    fun recordSource(roots: List<File>) {
        check(role == "sender")
        sourceSummary = summaryFromFiles(roots, publication = null)
    }

    fun recordDestination(outcomes: List<JSONObject>) {
        check(role == "receiver")
        check(outcomes.size == fixture.roots.size)
        val entries = mutableListOf<AndroidMatrixEntry>()
        val mechanisms = mutableSetOf<String>()
        fixture.roots.zip(outcomes).forEach { (_, outcome) ->
            val finalName = outcome.getString("final_name")
            val uri = Uri.parse(outcome.getString("uri"))
            if (uri.scheme == "file") {
                mechanisms += "test_local_directory"
                entries += entriesFromFile(File(requireNotNull(uri.path)), finalName)
            } else {
                mechanisms += "media_store"
                val inspected = MediaStoreSaver.inspect(context, uri).getOrThrow()
                entries +=
                    AndroidMatrixEntry(
                        relativePath = finalName,
                        kind = "file",
                        plaintextBytes = inspected.size,
                        sha256 = inspected.sha256,
                    )
            }
        }
        val mechanism = mechanisms.singleOrNull() ?: "mixed"
        destinationSummary =
            endpointSummary(
                rootCount = outcomes.size,
                entries = entries,
                publication =
                    JSONObject()
                        .put("mechanism", mechanism)
                        .put("committed", true),
            )
    }

    fun recordProductDestination(uris: List<Uri>) {
        check(role == "receiver")
        check(uris.size == fixture.roots.size)
        val entries =
            uris.map { uri ->
                val inspected = MediaStoreSaver.inspect(context, uri).getOrThrow()
                AndroidMatrixEntry(
                    relativePath = publishedDisplayName(uri),
                    kind = "file",
                    plaintextBytes = inspected.size,
                    sha256 = inspected.sha256,
                )
            }
        destinationSummary =
            endpointSummary(
                rootCount = uris.size,
                entries = entries,
                publication =
                    JSONObject()
                        .put("mechanism", "media_store")
                        .put("committed", true),
            )
    }

    @Synchronized
    fun complete() {
        check(deliveryProof) { "endpoint did not record delivery proof" }
        check(cleanupCompleted) { "endpoint did not complete test-owned cleanup" }
        if (role == "sender") check(sourceSummary != null)
        if (role == "receiver") check(destinationSummary != null)
        if (driver == "product_activity") {
            check(buildVariant == "release_equivalent")
            check(activityId != null) { "product endpoint did not record its Activity ID" }
            check(selectedPath != null) { "product endpoint did not record its selected path" }
        }
        terminalState = "completed"
        if (phases.lastOrNull() != "completed") phases += "completed"
    }

    @Synchronized
    fun fail() {
        terminalState = "failed"
        if (phases.lastOrNull() != "failed") phases += "failed"
    }

    @Synchronized
    fun write() {
        val finishedAt = System.currentTimeMillis()
        val terminal = requireNotNull(terminalState)
        val failure =
            if (terminal == "failed") {
                JSONObject()
                    .put("code", nativeFailureCode ?: "endpoint_assertion_failed")
                    .put(
                        "phase",
                        nativeFailurePhase
                            ?: if (!cleanupCompleted) {
                                "cleanup"
                            } else if (phases.size == 1) {
                                "setup"
                            } else {
                                "driver_validation"
                            },
                    ).put("recovery_action", nativeRecoveryAction ?: "none")
            } else {
                null
            }
        val capabilities =
            JSONArray()
                .put("manifest_v2")
                .put(
                    when (destinationSummary?.optJSONObject("publication")?.optString("mechanism")) {
                        "media_store" -> "media_store_publication"
                        "storage_access_framework" -> "storage_access_framework_publication"
                        "test_local_directory" -> "test_local_directory_publication"
                        "mixed" -> "mixed_test_publication"
                        else -> "source_fixture"
                    },
                )
        val result =
            JSONObject()
                .put("schema_version", 1)
                .put("run_id", runId)
                .put("case_id", caseId)
                .put("repetition", repetition)
                .put("role", role)
                .put("platform", "android")
                .put("test_layer", testLayer)
                .put("driver", driver)
                .put("build_variant", buildVariant)
                .put("app_version", BuildConfig.VERSION_NAME)
                .put("core_version", JSONObject.NULL)
                .put("protocol_version", 2)
                .put("device_model", Build.MODEL)
                .put("os_version", "Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})")
                .put("capabilities", capabilities)
                .put("activity_id", activityId ?: JSONObject.NULL)
                .put("job_id", jobId ?: JSONObject.NULL)
                .put("started_at", startedAt)
                .put("finished_at", finishedAt)
                .put("terminal_state", terminal)
                .put("ordered_phases", JSONArray(phases))
                .put("attempt_count", attemptCount)
                .put("selected_path", selectedPath ?: JSONObject.NULL)
                .put("path_reason", JSONObject.NULL)
                .put("source_summary", sourceSummary ?: JSONObject.NULL)
                .put("destination_summary", destinationSummary ?: JSONObject.NULL)
                .put("delivery_proof", deliveryProof && terminal == "completed")
                .put("failure", failure ?: JSONObject.NULL)
                .put(
                    "cleanup",
                    JSONObject()
                        .put("test_owned", true)
                        .put("completed", cleanupCompleted),
                ).put(
                    "metrics",
                    JSONObject()
                        .put("plaintext_bytes", fixture.totalBytes)
                        .put("elapsed_ms", finishedAt - startedAt),
                )
        val directory = File(context.filesDir, "envoix-matrix/$runId/$caseId")
        check(directory.mkdirs() || directory.isDirectory)
        val output = File(directory, "$role.json")
        val temporary = File(directory, ".$role.json.tmp")
        temporary.writeText(result.toString())
        if (output.exists()) check(output.delete())
        check(temporary.renameTo(output)) { "could not publish Android endpoint evidence" }
    }

    private fun publishedDisplayName(uri: Uri): String =
        requireNotNull(
            context.contentResolver
                .query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                ?.use { cursor ->
                    if (cursor.moveToFirst()) cursor.getString(0) else null
                },
        ) { "published URI has no display name" }

    private fun summaryFromFiles(
        roots: List<File>,
        publication: JSONObject?,
    ): JSONObject {
        check(roots.size == fixture.roots.size)
        return endpointSummary(
            rootCount = roots.size,
            entries =
                roots.flatMapIndexed { index, root ->
                    entriesFromFile(root, fixture.roots[index].name)
                },
            publication = publication,
        )
    }

    private fun entriesFromFile(
        root: File,
        relativeRoot: String,
    ): List<AndroidMatrixEntry> {
        check(root.exists())
        if (root.isFile) {
            return listOf(
                AndroidMatrixEntry(
                    relativePath = relativeRoot,
                    kind = "file",
                    plaintextBytes = root.length(),
                    sha256 = root.inputStream().use(::sha256).hex(),
                ),
            )
        }
        return root
            .walkTopDown()
            .map { file ->
                val suffix =
                    file
                        .relativeTo(root)
                        .invariantSeparatorsPath
                        .takeIf(String::isNotEmpty)
                val relativePath = listOfNotNull(relativeRoot, suffix).joinToString("/")
                if (file.isDirectory) {
                    AndroidMatrixEntry(relativePath, "directory", 0, null)
                } else {
                    AndroidMatrixEntry(
                        relativePath,
                        "file",
                        file.length(),
                        file.inputStream().use(::sha256).hex(),
                    )
                }
            }.toList()
    }

    private fun endpointSummary(
        rootCount: Int,
        entries: List<AndroidMatrixEntry>,
        publication: JSONObject?,
    ): JSONObject {
        val sorted = entries.sortedBy(AndroidMatrixEntry::relativePath)
        val canonical =
            sorted.joinToString(separator = "") { entry ->
                "${entry.kind}\u0000${entry.relativePath}\u0000${entry.plaintextBytes}" +
                    "\u0000${entry.sha256 ?: "-"}\n"
            }
        return JSONObject()
            .put("root_count", rootCount)
            .put("file_count", sorted.count { it.kind == "file" })
            .put("directory_count", sorted.count { it.kind == "directory" })
            .put("plaintext_bytes", sorted.filter { it.kind == "file" }.sumOf { it.plaintextBytes })
            .put("manifest_digest", JSONObject.NULL)
            .put(
                "tree_digest",
                MessageDigest
                    .getInstance("SHA-256")
                    .digest(canonical.toByteArray())
                    .hex(),
            ).put("entries", JSONArray(sorted.map(AndroidMatrixEntry::toJson)))
            .put("publication", publication ?: JSONObject.NULL)
    }

    private companion object {
        val PHASES =
            setOf(
                "waiting_for_peer",
                "pairing",
                "connecting",
                "offer",
                "transferring",
                "verifying",
                "saving",
                "waiting_for_receiver_save",
                "finalizing_delivery",
                "completed",
                "failed",
            )
        val PATH_KINDS = setOf("direct", "relay", "wifi_aware", "other")
        val RECOVERY_ACTIONS = setOf("none", "retry", "resume", "re_pair", "open_settings", "choose_folder")
        val FAILURE_CODE = Regex("[a-z0-9][a-z0-9_-]{0,63}")

        fun stableFailureCode(value: String): Boolean = value.matches(FAILURE_CODE)
    }
}

private data class AndroidMatrixEntry(
    val relativePath: String,
    val kind: String,
    val plaintextBytes: Long,
    val sha256: String?,
) {
    fun toJson(): JSONObject =
        JSONObject()
            .put("relative_path", relativePath)
            .put("kind", kind)
            .put("plaintext_bytes", plaintextBytes)
            .put("sha256", sha256 ?: JSONObject.NULL)
            .put("disposition", "completed")
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

private fun ByteArray.hex(): String = joinToString(separator = "") { "%02x".format(it) }
