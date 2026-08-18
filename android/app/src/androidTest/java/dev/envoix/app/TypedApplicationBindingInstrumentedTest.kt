package dev.envoix.app

import androidx.test.ext.junit.runners.AndroidJUnit4
import dev.envoix.app.ffi.FfiApplicationEngine
import dev.envoix.app.ffi.FfiApplicationErrorCode
import dev.envoix.app.ffi.FfiApplicationEvent
import dev.envoix.app.ffi.FfiApplicationEventEnvelope
import dev.envoix.app.ffi.FfiApplicationException
import dev.envoix.app.ffi.FfiApplyOutcome
import dev.envoix.app.ffi.envoixApplicationBindingInfo
import dev.envoix.app.ffi.envoixCoreInfo
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class TypedApplicationBindingInstrumentedTest {
    @Test
    fun typedEventsRebuildSnapshotAndReportGaps() {
        validateApplicationBinding(envoixCoreInfo(), envoixApplicationBindingInfo())
        FfiApplicationEngine().use { engine ->
            val observed =
                FfiApplicationEventEnvelope(
                    contractVersion = EXPECTED_APPLICATION_CONTRACT_VERSION,
                    sequence = 1uL,
                    event =
                        FfiApplicationEvent.DeviceObserved(
                            deviceId = "device_binding_fixture",
                            displayName = "Binding Fixture",
                        ),
                )
            assertEquals(FfiApplyOutcome.APPLIED, engine.apply(observed))
            assertEquals(FfiApplyOutcome.IGNORED_DUPLICATE, engine.apply(observed))

            val snapshot = engine.snapshot()
            assertEquals(1uL, snapshot.lastSequence)
            assertEquals(listOf("device_binding_fixture"), snapshot.devices.map { it.id })

            val gap =
                FfiApplicationEventEnvelope(
                    contractVersion = EXPECTED_APPLICATION_CONTRACT_VERSION,
                    sequence = 3uL,
                    event =
                        FfiApplicationEvent.DeviceObserved(
                            deviceId = "device_gap_fixture",
                            displayName = "Gap Fixture",
                        ),
                )
            val failure = runCatching { engine.apply(gap) }.exceptionOrNull()
            assertTrue(failure is FfiApplicationException.Failed)
            assertEquals(
                FfiApplicationErrorCode.EVENT_GAP,
                (failure as FfiApplicationException.Failed).code,
            )
        }
    }
}
