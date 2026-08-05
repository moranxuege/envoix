package dev.envoix.app

import android.app.Activity
import android.content.ComponentName
import android.content.pm.PackageManager
import android.nfc.NfcAdapter
import android.nfc.NfcManager
import android.nfc.cardemulation.CardEmulation
import android.nfc.cardemulation.HostApduService
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.activity.ComponentActivity
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.util.concurrent.CopyOnWriteArraySet

/**
 * Process-only storage for the current foreground-hosted room invitation.
 *
 * The QR may remain hidden while this snapshot is armed for NFC.
 *
 * Replaced and cleared arrays are wiped best-effort. Nothing in this path is
 * written to preferences, files, intents, notifications, or logs.
 */
internal object NfcInvitationHostStore {
    private var generation = 0L
    private var message: ByteArray? = null
    private val completionListeners = CopyOnWriteArraySet<() -> Unit>()

    fun arm(invitation: String): Boolean {
        val encoded =
            runCatching {
                NfcInvitationNdefCodec
                    .messageFor(invitation)
                    ?.toByteArray()
            }.getOrNull()
        if (encoded == null ||
            encoded.isEmpty() ||
            encoded.size > NfcType4TagProtocol.MAX_NDEF_MESSAGE_BYTES
        ) {
            encoded?.fill(0)
            clear()
            return false
        }

        synchronized(this) {
            message?.fill(0)
            generation += 1
            message = encoded
        }
        NfcInvitationHostSessions.invalidate()
        return true
    }

    fun clear() {
        synchronized(this) {
            message?.fill(0)
            message = null
            generation += 1
        }
        NfcInvitationHostSessions.invalidate()
    }

    @Synchronized
    fun snapshot(): HostedNdefSnapshot? {
        val current = message ?: return null
        return HostedNdefSnapshot(
            generation = generation,
            message = current.copyOf(),
        )
    }

    @Synchronized
    fun isCurrent(candidate: Long): Boolean = message != null && generation == candidate

    fun complete(candidate: Long) {
        val completed =
            synchronized(this) {
                if (message == null || generation != candidate) return@synchronized false
                message?.fill(0)
                message = null
                generation += 1
                true
            }
        if (!completed) return
        NfcInvitationHostSessions.invalidate()
        completionListeners.forEach { listener -> listener() }
    }

    fun addCompletionListener(listener: () -> Unit) {
        completionListeners += listener
    }

    fun removeCompletionListener(listener: () -> Unit) {
        completionListeners -= listener
    }
}

private object NfcInvitationHostSessions {
    private val protocols = CopyOnWriteArraySet<NfcType4TagProtocol>()

    fun register(protocol: NfcType4TagProtocol) {
        protocols += protocol
    }

    fun unregister(protocol: NfcType4TagProtocol) {
        protocols -= protocol
    }

    fun invalidate() {
        protocols.forEach(NfcType4TagProtocol::deactivate)
    }
}

class NfcInvitationHostApduService : HostApduService() {
    private val protocol =
        NfcType4TagProtocol(
            snapshot = NfcInvitationHostStore::snapshot,
            isCurrent = NfcInvitationHostStore::isCurrent,
            onMessageRead = NfcInvitationHostStore::complete,
        )

    override fun onCreate() {
        super.onCreate()
        NfcInvitationHostSessions.register(protocol)
    }

    override fun processCommandApdu(
        commandApdu: ByteArray,
        extras: Bundle?,
    ): ByteArray {
        val response =
            try {
                protocol.process(commandApdu)
            } catch (_: RuntimeException) {
                protocol.deactivate()
                byteArrayOf(0x6f, 0x00)
            }
        if (BuildConfig.DEBUG) {
            Log.d(
                LOG_TAG,
                "${NfcType4TagProtocol.traceCommandShape(commandApdu)} " +
                    NfcType4TagProtocol.traceResponseStatus(response),
            )
        }
        return response
    }

    override fun onDeactivated(reason: Int) {
        if (BuildConfig.DEBUG) {
            Log.d(LOG_TAG, "deactivated reason=$reason")
        }
        protocol.deactivate()
    }

    override fun onDestroy() {
        NfcInvitationHostSessions.unregister(protocol)
        protocol.deactivate()
        super.onDestroy()
    }

    private companion object {
        const val LOG_TAG = "EnvoixNfcHce"
    }
}

internal enum class NfcPhoneHostingStatus {
    Idle,
    Armed,
    RequiresAndroid15,
    NfcUnavailable,
    NfcDisabled,
    HceUnavailable,
    ListenOnlyUnavailable,
    HceActivationFailed,
    InvalidInvitation,
}

internal data class NfcPhoneHostingState(
    val status: NfcPhoneHostingStatus = NfcPhoneHostingStatus.Idle,
) {
    val armed: Boolean
        get() = status == NfcPhoneHostingStatus.Armed
}

internal interface NfcSafeHostingPlatform {
    fun unavailableStatus(): NfcPhoneHostingStatus?

    fun enterListenOnly(): Boolean

    fun resetDiscoveryTechnology()

    fun preferHostService(): Boolean

    fun unsetPreferredHostService()
}

/**
 * Pure lifecycle coordinator: normal Android NFC polling remains untouched
 * until an explicit hosted invitation is supplied. That invitation is never
 * armed until Android has confirmed that this foreground Activity is listening
 * without polling.
 */
internal class NfcSafeHostingSession(
    private val platform: NfcSafeHostingPlatform,
    private val armInvitation: (String) -> Boolean,
    private val clearInvitation: () -> Unit,
) {
    private val _state = MutableStateFlow(NfcPhoneHostingState())
    val state: StateFlow<NfcPhoneHostingState> = _state.asStateFlow()

    private var resumed = false
    private var preferred = false
    private var armedInvitation: String? = null
    private var listenOnly = false
    private var listenOnlyAttemptedThisResume = false

    fun onResume() {
        resumed = true
        listenOnlyAttemptedThisResume = false
    }

    fun setInvitation(invitation: String?) {
        if (resumed &&
            preferred &&
            invitation != null &&
            invitation == armedInvitation &&
            state.value.armed
        ) {
            return
        }
        clearHostedInvitation()
        if (!resumed) {
            publish(NfcPhoneHostingStatus.Idle)
            return
        }
        if (invitation == null) {
            publish(NfcPhoneHostingStatus.Idle)
            return
        }
        if (!listenOnly && !ensureListenOnly()) return
        if (!armInvitation(invitation)) {
            publish(NfcPhoneHostingStatus.InvalidInvitation)
            return
        }
        if (!platform.preferHostService()) {
            clearInvitation()
            publish(NfcPhoneHostingStatus.HceActivationFailed)
            return
        }
        preferred = true
        armedInvitation = invitation
        publish(NfcPhoneHostingStatus.Armed)
    }

    fun onPause() {
        resumed = false
        clearHostedInvitation()
        restoreDiscoveryTechnology()
        publish(NfcPhoneHostingStatus.Idle)
    }

    fun clear() {
        clearHostedInvitation()
        if (!resumed) {
            publish(NfcPhoneHostingStatus.Idle)
        } else if (listenOnly) {
            publish(NfcPhoneHostingStatus.Idle)
        }
    }

    fun leaveConnect() {
        clearHostedInvitation()
        restoreDiscoveryTechnology()
        publish(NfcPhoneHostingStatus.Idle)
    }

    fun close() {
        resumed = false
        clearHostedInvitation()
        restoreDiscoveryTechnology()
        publish(NfcPhoneHostingStatus.Idle)
    }

    private fun ensureListenOnly(): Boolean {
        if (listenOnly) return true
        if (listenOnlyAttemptedThisResume) return false
        platform.unavailableStatus()?.let {
            publish(it)
            return false
        }
        listenOnlyAttemptedThisResume = true
        if (!platform.enterListenOnly()) {
            // The call may have partially reached the NFC service before
            // failing. Remain unarmed and do not risk a polling pulse by
            // resetting discovery while this Activity is still resumed.
            publish(NfcPhoneHostingStatus.ListenOnlyUnavailable)
            return false
        }
        listenOnly = true
        publish(NfcPhoneHostingStatus.Idle)
        return true
    }

    private fun clearHostedInvitation() {
        clearInvitation()
        armedInvitation = null
        if (preferred) {
            platform.unsetPreferredHostService()
            preferred = false
        }
    }

    private fun restoreDiscoveryTechnology() {
        // clear() intentionally keeps listen-only while Connect remains
        // visible. Leaving Connect or pausing resets both Android and this
        // coordinator's cached discovery state.
        if (listenOnly || listenOnlyAttemptedThisResume) {
            platform.resetDiscoveryTechnology()
        }
        listenOnly = false
        listenOnlyAttemptedThisResume = false
    }

    private fun publish(status: NfcPhoneHostingStatus) {
        _state.value = NfcPhoneHostingState(status)
    }
}

/**
 * compileSdk remains pinned to 34. This tiny bridge invokes the public API 35
 * discovery controls reflectively and fails closed if an OEM omits or rejects
 * them.
 */
internal class NfcDiscoveryTechnologyBridge(
    private val apiLevel: Int,
    private val setTechnology: (pollTechnology: Int, listenTechnology: Int) -> Unit,
    private val resetTechnology: () -> Unit,
) {
    fun enterListenOnly(): Boolean {
        if (apiLevel < SAFE_LISTEN_ONLY_API) return false
        return runCatching {
            setTechnology(POLLING_DISABLED, KEEP_CURRENT_LISTEN_TECHNOLOGIES)
        }.isSuccess
    }

    fun reset() {
        if (apiLevel < SAFE_LISTEN_ONLY_API) return
        runCatching(resetTechnology)
    }

    companion object {
        internal const val SAFE_LISTEN_ONLY_API = 35
        internal const val POLLING_DISABLED = 0
        internal const val KEEP_CURRENT_LISTEN_TECHNOLOGIES = Int.MIN_VALUE

        fun forActivity(
            activity: Activity,
            adapter: NfcAdapter,
        ): NfcDiscoveryTechnologyBridge =
            NfcDiscoveryTechnologyBridge(
                apiLevel = Build.VERSION.SDK_INT,
                setTechnology = { pollTechnology, listenTechnology ->
                    val method =
                        NfcAdapter::class.java.getMethod(
                            "setDiscoveryTechnology",
                            Activity::class.java,
                            Int::class.javaPrimitiveType,
                            Int::class.javaPrimitiveType,
                        )
                    method.invoke(adapter, activity, pollTechnology, listenTechnology)
                },
                resetTechnology = {
                    val method =
                        NfcAdapter::class.java.getMethod(
                            "resetDiscoveryTechnology",
                            Activity::class.java,
                        )
                    method.invoke(adapter, activity)
                },
            )
    }
}

private class AndroidNfcSafeHostingPlatform(
    private val activity: ComponentActivity,
) : NfcSafeHostingPlatform {
    private val adapter =
        activity
            .getSystemService(NfcManager::class.java)
            ?.defaultAdapter
    private val cardEmulation =
        if (activity.packageManager.hasSystemFeature(
                PackageManager.FEATURE_NFC_HOST_CARD_EMULATION,
            )
        ) {
            adapter?.let { runCatching { CardEmulation.getInstance(it) }.getOrNull() }
        } else {
            null
        }
    private val service = ComponentName(activity, NfcInvitationHostApduService::class.java)
    private val discoveryTechnology =
        adapter?.let { NfcDiscoveryTechnologyBridge.forActivity(activity, it) }

    override fun unavailableStatus(): NfcPhoneHostingStatus? =
        when {
            Build.VERSION.SDK_INT < NfcDiscoveryTechnologyBridge.SAFE_LISTEN_ONLY_API ->
                NfcPhoneHostingStatus.RequiresAndroid15
            adapter == null -> NfcPhoneHostingStatus.NfcUnavailable
            runCatching { adapter?.isEnabled == true }.getOrDefault(false).not() ->
                NfcPhoneHostingStatus.NfcDisabled
            cardEmulation == null -> NfcPhoneHostingStatus.HceUnavailable
            else -> null
        }

    override fun enterListenOnly(): Boolean = discoveryTechnology?.enterListenOnly() == true

    override fun resetDiscoveryTechnology() {
        discoveryTechnology?.reset()
    }

    override fun preferHostService(): Boolean =
        cardEmulation?.let { manager ->
            runCatching {
                manager.setPreferredService(activity, service)
            }.getOrDefault(false)
        } == true

    override fun unsetPreferredHostService() {
        cardEmulation?.let { manager ->
            runCatching { manager.unsetPreferredService(activity) }
        }
    }
}

internal class NfcInvitationHostController(
    private val activity: ComponentActivity,
) {
    private val handler = Handler(Looper.getMainLooper())
    private var presentationActive = false
    private val timeout = Runnable(::cancelPresentation)
    private val completionListener: () -> Unit = {
        handler.post(::cancelPresentation)
    }
    private val session =
        NfcSafeHostingSession(
            platform = AndroidNfcSafeHostingPlatform(activity),
            armInvitation = NfcInvitationHostStore::arm,
            clearInvitation = NfcInvitationHostStore::clear,
        )

    val state: StateFlow<NfcPhoneHostingState> = session.state

    init {
        NfcInvitationHostStore.addCompletionListener(completionListener)
    }

    fun beginPresentation(invitation: String?) {
        cancelPresentation()
        presentationActive = true
        handler.postDelayed(timeout, PRESENTATION_LEASE_MS)
        session.setInvitation(invitation)
    }

    fun setInvitation(invitation: String?) {
        if (!presentationActive) {
            session.setInvitation(null)
        } else {
            // A null value can be the normal, short-lived state while the room
            // host request is producing its invitation. Keep the explicit
            // presentation lease alive so the next non-null update can arm HCE.
            session.setInvitation(invitation)
        }
    }

    fun cancelPresentation() {
        presentationActive = false
        handler.removeCallbacks(timeout)
        session.clear()
    }

    fun onResume() = session.onResume()

    fun onPause() {
        presentationActive = false
        handler.removeCallbacks(timeout)
        session.onPause()
    }

    fun leaveConnect() {
        presentationActive = false
        handler.removeCallbacks(timeout)
        session.leaveConnect()
    }

    fun clear() = cancelPresentation()

    fun close() {
        presentationActive = false
        handler.removeCallbacks(timeout)
        NfcInvitationHostStore.removeCompletionListener(completionListener)
        session.close()
    }

    companion object {
        internal const val PRESENTATION_LEASE_MS = 120_000L
    }
}
