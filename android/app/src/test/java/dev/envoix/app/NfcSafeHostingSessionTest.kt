package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NfcSafeHostingSessionTest {
    @Test
    fun `foreground without an explicit invitation leaves NFC polling untouched`() {
        val events = mutableListOf<String>()
        val session =
            NfcSafeHostingSession(
                platform = FakeSafeHostingPlatform(events = events),
                armInvitation = { error("an absent invitation must not be armed") },
                clearInvitation = { events += "clear" },
            )

        session.onResume()
        assertTrue(events.isEmpty())

        session.setInvitation(null)
        assertEquals(listOf("clear"), events)
        assertEquals(NfcPhoneHostingStatus.Idle, session.state.value.status)

        events.clear()
        session.onPause()
        assertEquals(listOf("clear"), events)
    }

    @Test
    fun `safe host arms only after listen-only mode succeeds`() {
        val events = mutableListOf<String>()
        val platform = FakeSafeHostingPlatform(events = events)
        val session =
            NfcSafeHostingSession(
                platform = platform,
                armInvitation = {
                    events += "arm"
                    true
                },
                clearInvitation = { events += "clear" },
            )

        session.onResume()
        session.setInvitation("envoix://room/redacted")

        assertEquals(
            listOf("clear", "listen-only", "arm", "prefer"),
            events,
        )
        assertTrue(session.state.value.armed)

        events.clear()
        session.setInvitation("envoix://room/redacted")

        assertTrue(events.isEmpty())
        assertTrue(session.state.value.armed)

        events.clear()
        session.onPause()

        assertEquals(listOf("clear", "unset", "reset"), events)
        assertEquals(NfcPhoneHostingStatus.Idle, session.state.value.status)
    }

    @Test
    fun `listen-only failure stays unarmed without resetting while resumed`() {
        val events = mutableListOf<String>()
        var armed = false
        val platform =
            FakeSafeHostingPlatform(
                events = events,
                enterListenOnlyResult = false,
            )
        val session =
            NfcSafeHostingSession(
                platform = platform,
                armInvitation = {
                    armed = true
                    true
                },
                clearInvitation = { events += "clear" },
            )

        session.onResume()
        session.setInvitation("envoix://room/redacted")

        assertFalse(armed)
        assertEquals(
            listOf("clear", "listen-only"),
            events,
        )
        assertEquals(
            NfcPhoneHostingStatus.ListenOnlyUnavailable,
            session.state.value.status,
        )

        events.clear()
        session.onPause()
        assertEquals(listOf("clear", "reset"), events)
    }

    @Test
    fun `unsupported platform is exposed without attempting polling controls`() {
        val events = mutableListOf<String>()
        var armed = false
        val platform =
            FakeSafeHostingPlatform(
                events = events,
                unavailable = NfcPhoneHostingStatus.RequiresAndroid15,
            )
        val session =
            NfcSafeHostingSession(
                platform = platform,
                armInvitation = {
                    armed = true
                    true
                },
                clearInvitation = { events += "clear" },
            )

        session.onResume()
        session.setInvitation("envoix://room/redacted")

        assertFalse(armed)
        assertEquals(listOf("clear"), events)
        assertEquals(
            NfcPhoneHostingStatus.RequiresAndroid15,
            session.state.value.status,
        )
    }

    @Test
    fun `enabling NFC allows another share attempt in the same resume`() {
        val events = mutableListOf<String>()
        var unavailable: NfcPhoneHostingStatus? = NfcPhoneHostingStatus.NfcDisabled
        val platform =
            object : NfcSafeHostingPlatform {
                override fun unavailableStatus(): NfcPhoneHostingStatus? = unavailable

                override fun enterListenOnly(): Boolean {
                    events += "listen-only"
                    return true
                }

                override fun resetDiscoveryTechnology() {
                    events += "reset"
                }

                override fun preferHostService(): Boolean {
                    events += "prefer"
                    return true
                }

                override fun unsetPreferredHostService() {
                    events += "unset"
                }
            }
        val session =
            NfcSafeHostingSession(
                platform = platform,
                armInvitation = {
                    events += "arm"
                    true
                },
                clearInvitation = { events += "clear" },
            )
        session.onResume()

        session.setInvitation("envoix://room/first")

        assertEquals(listOf("clear"), events)
        assertEquals(NfcPhoneHostingStatus.NfcDisabled, session.state.value.status)

        unavailable = null
        events.clear()
        session.setInvitation("envoix://room/second")

        assertEquals(listOf("clear", "listen-only", "arm", "prefer"), events)
        assertTrue(session.state.value.armed)
    }

    @Test
    fun `failed HCE preference clears invitation but keeps listen-only`() {
        val events = mutableListOf<String>()
        val platform =
            FakeSafeHostingPlatform(
                events = events,
                preferResult = false,
            )
        val session =
            NfcSafeHostingSession(
                platform = platform,
                armInvitation = {
                    events += "arm"
                    true
                },
                clearInvitation = { events += "clear" },
            )

        session.onResume()
        session.setInvitation("envoix://room/redacted")

        assertEquals(
            listOf(
                "clear",
                "listen-only",
                "arm",
                "prefer",
                "clear",
            ),
            events,
        )
        assertEquals(
            NfcPhoneHostingStatus.HceActivationFailed,
            session.state.value.status,
        )
    }

    @Test
    fun `clearing HCE while resumed keeps polling disabled until pause`() {
        val events = mutableListOf<String>()
        val session =
            NfcSafeHostingSession(
                platform = FakeSafeHostingPlatform(events = events),
                armInvitation = {
                    events += "arm"
                    true
                },
                clearInvitation = { events += "clear" },
            )
        session.onResume()
        session.setInvitation("envoix://room/redacted")
        events.clear()

        // This is the connection/Stop waiting path while phones may still
        // physically touch.
        session.setInvitation(null)

        assertEquals(listOf("clear", "unset"), events)
        assertEquals(NfcPhoneHostingStatus.Idle, session.state.value.status)

        events.clear()
        session.onPause()

        assertEquals(listOf("clear", "reset"), events)
    }

    @Test
    fun `leaving Connect resets cached listen-only state before another presentation`() {
        val events = mutableListOf<String>()
        var listenOnlyAvailable = true
        var armCalls = 0
        val platform =
            object : NfcSafeHostingPlatform {
                override fun unavailableStatus(): NfcPhoneHostingStatus? = null

                override fun enterListenOnly(): Boolean {
                    events += "listen-only"
                    return listenOnlyAvailable
                }

                override fun resetDiscoveryTechnology() {
                    events += "reset"
                }

                override fun preferHostService(): Boolean {
                    events += "prefer"
                    return true
                }

                override fun unsetPreferredHostService() {
                    events += "unset"
                }
            }
        val session =
            NfcSafeHostingSession(
                platform = platform,
                armInvitation = {
                    armCalls += 1
                    events += "arm"
                    true
                },
                clearInvitation = { events += "clear" },
            )
        session.onResume()
        session.setInvitation("envoix://room/first")
        assertTrue(session.state.value.armed)
        events.clear()

        session.leaveConnect()

        assertEquals(listOf("clear", "unset", "reset"), events)
        assertEquals(NfcPhoneHostingStatus.Idle, session.state.value.status)

        listenOnlyAvailable = false
        events.clear()
        session.setInvitation("envoix://room/second")

        assertEquals(listOf("clear", "listen-only"), events)
        assertEquals(1, armCalls)
        assertEquals(
            NfcPhoneHostingStatus.ListenOnlyUnavailable,
            session.state.value.status,
        )
    }

    @Test
    fun `API 35 bridge uses polling-disabled and listen-keep flags`() {
        var flags: Pair<Int, Int>? = null
        var resetCalls = 0
        val bridge =
            NfcDiscoveryTechnologyBridge(
                apiLevel = 35,
                setTechnology = { poll, listen -> flags = poll to listen },
                resetTechnology = { resetCalls += 1 },
            )

        assertTrue(bridge.enterListenOnly())
        bridge.reset()

        assertEquals(
            NfcDiscoveryTechnologyBridge.POLLING_DISABLED to
                NfcDiscoveryTechnologyBridge.KEEP_CURRENT_LISTEN_TECHNOLOGIES,
            flags,
        )
        assertEquals(1, resetCalls)
    }

    @Test
    fun `pre API 35 bridge never changes discovery technology`() {
        var setCalls = 0
        var resetCalls = 0
        val bridge =
            NfcDiscoveryTechnologyBridge(
                apiLevel = 34,
                setTechnology = { _, _ -> setCalls += 1 },
                resetTechnology = { resetCalls += 1 },
            )

        assertFalse(bridge.enterListenOnly())
        bridge.reset()

        assertEquals(0, setCalls)
        assertEquals(0, resetCalls)
    }

    @Test
    fun `reflection bridge reports invocation failure instead of arming`() {
        val bridge =
            NfcDiscoveryTechnologyBridge(
                apiLevel = 36,
                setTechnology = { _, _ -> error("OEM rejected discovery control") },
                resetTechnology = {},
            )

        assertFalse(bridge.enterListenOnly())
    }
}

private class FakeSafeHostingPlatform(
    private val events: MutableList<String>,
    private val unavailable: NfcPhoneHostingStatus? = null,
    private val enterListenOnlyResult: Boolean = true,
    private val preferResult: Boolean = true,
) : NfcSafeHostingPlatform {
    override fun unavailableStatus(): NfcPhoneHostingStatus? = unavailable

    override fun enterListenOnly(): Boolean {
        events += "listen-only"
        return enterListenOnlyResult
    }

    override fun resetDiscoveryTechnology() {
        events += "reset"
    }

    override fun preferHostService(): Boolean {
        events += "prefer"
        return preferResult
    }

    override fun unsetPreferredHostService() {
        events += "unset"
    }
}
