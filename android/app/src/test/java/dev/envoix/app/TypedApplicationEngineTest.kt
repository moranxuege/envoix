package dev.envoix.app

import dev.envoix.app.ffi.FfiApplicationBindingInfo
import dev.envoix.app.ffi.FfiApplicationCommandEnvelope
import dev.envoix.app.ffi.FfiApplicationEffect
import dev.envoix.app.ffi.FfiApplicationEffectEnvelope
import dev.envoix.app.ffi.FfiApplicationEngineInterface
import dev.envoix.app.ffi.FfiApplicationEventEnvelope
import dev.envoix.app.ffi.FfiApplicationSnapshot
import dev.envoix.app.ffi.FfiApplyOutcome
import dev.envoix.app.ffi.FfiCoreInfo
import dev.envoix.app.ffi.FfiPlatformCapabilities
import dev.envoix.app.ffi.FfiPreparedRelationship
import dev.envoix.app.ffi.FfiRememberedRelationship
import dev.envoix.app.ffi.FfiRememberedRelationshipMaterial
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicInteger

@OptIn(ExperimentalCoroutinesApi::class)
class TypedApplicationEngineTest {
    @Test
    fun `binding negotiation rejects every mismatched version`() {
        val core = coreInfo()
        val binding = bindingInfo()
        validateApplicationBinding(core, binding)

        assertThrows(IncompatibleApplicationBinding::class.java) {
            validateApplicationBinding(core.copy(ffiApiVersion = 15u), binding)
        }
        assertThrows(IncompatibleApplicationBinding::class.java) {
            validateApplicationBinding(core, binding.copy(bindingVersion = 2u))
        }
        assertThrows(IncompatibleApplicationBinding::class.java) {
            validateApplicationBinding(core, binding.copy(contractVersion = 5u))
        }
        assertThrows(IncompatibleApplicationBinding::class.java) {
            validateApplicationBinding(core.copy(capabilities = emptyList()), binding)
        }
        assertThrows(IncompatibleApplicationBinding::class.java) {
            validateApplicationBinding(
                core.copy(capabilities = listOf("persistent_application_engine_v1")),
                binding,
            )
        }
        assertThrows(IncompatibleApplicationBinding::class.java) {
            validateApplicationBinding(
                core.copy(capabilities = listOf("typed_application_contract_v6")),
                binding,
            )
        }
    }

    @Test
    fun `concurrent callers enter the generated Engine one at a time`() =
        runBlocking {
            val active = AtomicInteger()
            val maximum = AtomicInteger()
            val fake =
                FakeEngine {
                    val count = active.incrementAndGet()
                    maximum.accumulateAndGet(count) { current, next -> maxOf(current, next) }
                    Thread.sleep(20)
                    active.decrementAndGet()
                }
            Executors.newFixedThreadPool(2).asCoroutineDispatcher().use { dispatcher ->
                val engine = TypedApplicationEngine.forTesting(fake, dispatcher = dispatcher)
                listOf(async { engine.snapshot() }, async { engine.snapshot() }).awaitAll()
                engine.close()
            }

            assertEquals(1, maximum.get())
            assertEquals(2, fake.snapshotCalls.get())
        }

    @Test
    fun `cancellation before dispatch never enters FFI`() =
        runTest {
            val dispatcher = StandardTestDispatcher(testScheduler)
            val fake = FakeEngine()
            val engine = TypedApplicationEngine.forTesting(fake, dispatcher = dispatcher)
            val operation = async { engine.snapshot() }

            operation.cancel()
            runCurrent()

            assertEquals(0, fake.snapshotCalls.get())
            engine.close()
        }

    @Test
    fun `close is idempotent and rejects later calls`() =
        runTest {
            val releases = AtomicInteger()
            val engine =
                TypedApplicationEngine.forTesting(
                    FakeEngine(),
                    release = { releases.incrementAndGet() },
                    dispatcher = StandardTestDispatcher(testScheduler),
                )

            engine.close()
            engine.close()

            assertEquals(1, releases.get())
            assertTrue(runCatching { engine.snapshot() }.exceptionOrNull() is IllegalStateException)
        }

    private fun coreInfo() =
        FfiCoreInfo(
            ffiApiVersion = EXPECTED_FFI_API_VERSION,
            coreVersion = "test",
            capabilities =
                listOf(
                    "typed_application_contract_v6",
                    "persistent_application_engine_v1",
                ),
        )

    private fun bindingInfo() =
        FfiApplicationBindingInfo(
            bindingVersion = EXPECTED_APPLICATION_BINDING_VERSION,
            contractVersion = EXPECTED_APPLICATION_CONTRACT_VERSION,
        )
}

private class FakeEngine(
    private val beforeSnapshot: () -> Unit = {},
) : FfiApplicationEngineInterface {
    val snapshotCalls = AtomicInteger()

    override fun snapshot(): FfiApplicationSnapshot {
        beforeSnapshot()
        snapshotCalls.incrementAndGet()
        return FfiApplicationSnapshot(
            contractVersion = EXPECTED_APPLICATION_CONTRACT_VERSION,
            lastSequence = 0uL,
            capabilities = FfiPlatformCapabilities(emptyList()),
            devices = emptyList(),
            relationships = emptyList(),
            rooms = emptyList(),
            transfers = emptyList(),
        )
    }

    override fun apply(envelope: FfiApplicationEventEnvelope): FfiApplyOutcome = FfiApplyOutcome.APPLIED

    override fun decide(envelope: FfiApplicationCommandEnvelope): FfiApplicationEffectEnvelope =
        FfiApplicationEffectEnvelope(
            contractVersion = EXPECTED_APPLICATION_CONTRACT_VERSION,
            commandId = envelope.commandId,
            effect = FfiApplicationEffect.CreateRoom,
        )

    override fun prepareRelationship(
        label: String,
        broker: String,
        relay: String,
    ): FfiPreparedRelationship = error("not used")

    override fun discardPreparedRelationship(relationshipId: String) = Unit

    override fun commitRelationship(
        relationshipId: String,
        opaqueCredential: ByteArray,
        generation: ULong,
    ): FfiRememberedRelationship = error("not used")

    override fun relationships(): List<FfiRememberedRelationship> = emptyList()

    override fun loadRelationship(relationshipId: String): FfiRememberedRelationshipMaterial? = null

    override fun rotateRelationship(
        relationshipId: String,
        opaqueCredential: ByteArray,
        generation: ULong,
    ): FfiRememberedRelationship = error("not used")

    override fun renameRelationship(
        relationshipId: String,
        label: String,
    ): FfiRememberedRelationship = error("not used")

    override fun revokeRelationship(relationshipId: String): FfiRememberedRelationship = error("not used")
}
