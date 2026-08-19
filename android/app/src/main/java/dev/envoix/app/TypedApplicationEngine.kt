package dev.envoix.app

import dev.envoix.app.ffi.FfiApplicationBindingInfo
import dev.envoix.app.ffi.FfiApplicationCommandEnvelope
import dev.envoix.app.ffi.FfiApplicationEffectEnvelope
import dev.envoix.app.ffi.FfiApplicationEngine
import dev.envoix.app.ffi.FfiApplicationEngineInterface
import dev.envoix.app.ffi.FfiApplicationEventEnvelope
import dev.envoix.app.ffi.FfiApplicationSnapshot
import dev.envoix.app.ffi.FfiApplyOutcome
import dev.envoix.app.ffi.FfiCoreInfo
import dev.envoix.app.ffi.envoixApplicationBindingInfo
import dev.envoix.app.ffi.envoixCoreInfo
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.Closeable
import java.util.concurrent.atomic.AtomicBoolean

internal const val EXPECTED_FFI_API_VERSION: UInt = 20u
internal const val EXPECTED_APPLICATION_BINDING_VERSION: UInt = 1u
internal const val EXPECTED_APPLICATION_CONTRACT_VERSION: UShort = 6u
private const val TYPED_APPLICATION_CAPABILITY = "typed_application_contract_v6"

internal class IncompatibleApplicationBinding(
    message: String,
) : IllegalStateException(message)

internal fun validateApplicationBinding(
    core: FfiCoreInfo,
    binding: FfiApplicationBindingInfo,
) {
    if (core.ffiApiVersion != EXPECTED_FFI_API_VERSION ||
        binding.bindingVersion != EXPECTED_APPLICATION_BINDING_VERSION ||
        binding.contractVersion != EXPECTED_APPLICATION_CONTRACT_VERSION ||
        TYPED_APPLICATION_CAPABILITY !in core.capabilities
    ) {
        throw IncompatibleApplicationBinding(
            "Unsupported Envoix binding: FFI ${core.ffiApiVersion}, " +
                "binding ${binding.bindingVersion}, contract ${binding.contractVersion}",
        )
    }
}

/**
 * The only coroutine owner for one in-process application Engine handle.
 *
 * Product transitions remain synchronous and serialized in Rust. Cancellation
 * is checked before entering that short critical section; long-running transfer
 * work continues to use its explicit native cancellation token.
 */
internal class TypedApplicationEngine private constructor(
    private val engine: FfiApplicationEngineInterface,
    private val release: () -> Unit,
    private val dispatcher: CoroutineDispatcher,
) : Closeable {
    private val calls = Mutex()
    private val closed = AtomicBoolean(false)

    suspend fun snapshot(): FfiApplicationSnapshot = invoke { engine.snapshot() }

    suspend fun apply(envelope: FfiApplicationEventEnvelope): FfiApplyOutcome = invoke { engine.apply(envelope) }

    suspend fun decide(envelope: FfiApplicationCommandEnvelope): FfiApplicationEffectEnvelope = invoke { engine.decide(envelope) }

    override fun close() {
        if (closed.compareAndSet(false, true)) {
            release()
        }
    }

    private suspend fun <T> invoke(operation: () -> T): T =
        withContext(dispatcher) {
            ensureActive()
            calls.withLock {
                ensureActive()
                check(!closed.get()) { "application Engine is closed" }
                operation()
            }
        }

    companion object {
        fun open(dispatcher: CoroutineDispatcher = Dispatchers.Default): TypedApplicationEngine {
            val core = envoixCoreInfo()
            val binding = envoixApplicationBindingInfo()
            validateApplicationBinding(core, binding)
            val engine = FfiApplicationEngine()
            return TypedApplicationEngine(engine, engine::close, dispatcher)
        }

        internal fun forTesting(
            engine: FfiApplicationEngineInterface,
            release: () -> Unit = {},
            dispatcher: CoroutineDispatcher = Dispatchers.Default,
        ): TypedApplicationEngine = TypedApplicationEngine(engine, release, dispatcher)
    }
}
