package dev.envoix.app

import android.annotation.SuppressLint
import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.aware.AttachCallback
import android.net.wifi.aware.AwarePairingConfig
import android.net.wifi.aware.Characteristics
import android.net.wifi.aware.DiscoverySession
import android.net.wifi.aware.DiscoverySessionCallback
import android.net.wifi.aware.PeerHandle
import android.net.wifi.aware.PublishConfig
import android.net.wifi.aware.PublishDiscoverySession
import android.net.wifi.aware.ServiceDiscoveryInfo
import android.net.wifi.aware.SubscribeConfig
import android.net.wifi.aware.SubscribeDiscoverySession
import android.net.wifi.aware.WifiAwareManager
import android.net.wifi.aware.WifiAwareNetworkInfo
import android.net.wifi.aware.WifiAwareNetworkSpecifier
import android.net.wifi.aware.WifiAwareSession
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.system.OsConstants
import androidx.annotation.RequiresApi
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import org.json.JSONObject
import java.io.Closeable
import java.io.EOFException
import java.io.IOException
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketTimeoutException
import java.security.SecureRandom
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

internal const val ENVOIX_WIFI_AWARE_PROBE_SERVICE = "_envoix-probe._tcp"
internal const val ENVOIX_WIFI_AWARE_TRANSFER_SERVICE = "_envoix-transfer._tcp"

private const val WIFI_AWARE_PROBE_FRAME_LENGTH = 40
private const val WIFI_AWARE_PROBE_NONCE_LENGTH = 32
private const val WIFI_AWARE_NETWORK_TIMEOUT_MS = 30_000
private const val WIFI_AWARE_SOCKET_TIMEOUT_MS = 15_000
private const val WIFI_AWARE_PEER_ALIAS_PREFIX = "envoix-peer-"

internal enum class WifiAwareProbeProtocolFailure {
    INVALID_NONCE_LENGTH,
    INVALID_FRAME_LENGTH,
    INVALID_REQUEST_MAGIC,
    INVALID_RESPONSE_MAGIC,
    NONCE_MISMATCH,
}

internal class WifiAwareProbeProtocolException(
    val failure: WifiAwareProbeProtocolFailure,
) : IllegalArgumentException(failure.name)

internal object WifiAwareProbeWireProtocol {
    const val FRAME_LENGTH = WIFI_AWARE_PROBE_FRAME_LENGTH
    const val NONCE_LENGTH = WIFI_AWARE_PROBE_NONCE_LENGTH

    private val requestMagic = "ENVXWA01".encodeToByteArray()
    private val responseMagic = "ENVXWA02".encodeToByteArray()

    fun makeRequest(nonce: ByteArray): ByteArray {
        if (nonce.size != NONCE_LENGTH) fail(WifiAwareProbeProtocolFailure.INVALID_NONCE_LENGTH)
        return requestMagic + nonce
    }

    fun makeResponse(request: ByteArray): ByteArray {
        if (request.size != FRAME_LENGTH) fail(WifiAwareProbeProtocolFailure.INVALID_FRAME_LENGTH)
        if (!request.startsWith(requestMagic)) fail(WifiAwareProbeProtocolFailure.INVALID_REQUEST_MAGIC)
        return responseMagic + request.copyOfRange(requestMagic.size, request.size)
    }

    fun validateResponse(
        response: ByteArray,
        nonce: ByteArray,
    ) {
        if (nonce.size != NONCE_LENGTH) fail(WifiAwareProbeProtocolFailure.INVALID_NONCE_LENGTH)
        if (response.size != FRAME_LENGTH) fail(WifiAwareProbeProtocolFailure.INVALID_FRAME_LENGTH)
        if (!response.startsWith(responseMagic)) fail(WifiAwareProbeProtocolFailure.INVALID_RESPONSE_MAGIC)
        if (!response.copyOfRange(responseMagic.size, response.size).contentEquals(nonce)) {
            fail(WifiAwareProbeProtocolFailure.NONCE_MISMATCH)
        }
    }

    private fun ByteArray.startsWith(prefix: ByteArray): Boolean = size >= prefix.size && prefix.indices.all { this[it] == prefix[it] }

    private fun fail(failure: WifiAwareProbeProtocolFailure): Nothing = throw WifiAwareProbeProtocolException(failure)
}

internal enum class WifiAwareProbeRole(
    val wireName: String,
) {
    PUBLISHER("publisher"),
    SUBSCRIBER("subscriber"),
}

internal enum class WifiAwareProbePhase(
    val wireName: String,
) {
    IDLE("idle"),
    CHECKING("checking"),
    ATTACHING("attaching"),
    PUBLISHING("publishing"),
    BROWSING("browsing"),
    PAIRING("pairing"),
    REQUESTING_NETWORK("requesting_network"),
    CONNECTING("connecting"),
    EXCHANGING("exchanging"),
    SUCCEEDED("succeeded"),
    FAILED("failed"),
}

internal val WifiAwareProbePhase.isRunning: Boolean
    get() =
        this != WifiAwareProbePhase.IDLE &&
            this != WifiAwareProbePhase.SUCCEEDED &&
            this != WifiAwareProbePhase.FAILED

internal data class WifiAwareProbeSnapshot(
    val phase: WifiAwareProbePhase = WifiAwareProbePhase.IDLE,
    val role: WifiAwareProbeRole? = null,
    val detail: String = "not_started",
    val pairedDeviceCount: Int? = null,
) {
    val diagnosticSummary: String
        get() =
            "phase=${phase.wireName} · role=${role?.wireName ?: "none"} · " +
                "detail=$detail · paired_devices=${pairedDeviceCount ?: "unknown"}"
}

internal class AndroidWifiAwareDiagnosticController(
    context: Context,
) : Closeable {
    private val appContext = context.applicationContext
    private val mainHandler = Handler(Looper.getMainLooper())
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val lock = Any()

    private val _snapshot = MutableStateFlow(WifiAwareProbeSnapshot())
    val snapshot: StateFlow<WifiAwareProbeSnapshot> = _snapshot.asStateFlow()

    private var generation = 0
    private var activeRole: WifiAwareProbeRole? = null
    private var awareSession: WifiAwareSession? = null
    private var discoverySession: DiscoverySession? = null
    private var networkCallback: ConnectivityManager.NetworkCallback? = null
    private var serverSocket: ServerSocket? = null
    private var socket: Socket? = null
    private var ioJob: Job? = null
    private var peerActionStarted = AtomicBoolean(false)
    private var networkActionStarted = AtomicBoolean(false)
    private var operationMode: OperationMode = OperationMode.Probe

    private sealed interface OperationMode {
        data object Probe : OperationMode

        data class Transfer(
            val nativeId: Long,
            val paramsJson: String,
            val pairingToken: String,
            val callback: ManifestV2Callback,
            val nativeStarted: AtomicBoolean = AtomicBoolean(false),
        ) : OperationMode
    }

    fun refresh() {
        val currentGeneration = synchronized(lock) { generation }
        scope.launch {
            val capability = AndroidWifiAwareCapabilityProbe.read(appContext)
            if (!isActive(currentGeneration)) return@launch
            _snapshot.value =
                _snapshot.value.copy(
                    pairedDeviceCount = capability.pairedDeviceCount,
                    detail =
                        if (_snapshot.value.phase == WifiAwareProbePhase.IDLE) {
                            capability.availability.wireName
                        } else {
                            _snapshot.value.detail
                        },
                )
        }
    }

    fun start(role: WifiAwareProbeRole) {
        start(role, OperationMode.Probe)
    }

    fun startTransfer(
        role: WifiAwareProbeRole,
        nativeId: Long,
        paramsJson: String,
        pairingToken: String,
        callback: ManifestV2Callback,
    ) {
        require(pairingToken.isNotBlank()) { "pairingToken must not be blank" }
        start(
            role,
            OperationMode.Transfer(nativeId, paramsJson, pairingToken, callback),
        )
    }

    private fun start(
        role: WifiAwareProbeRole,
        mode: OperationMode,
    ) {
        val currentGeneration =
            synchronized(lock) {
                closeResourcesLocked()
                generation += 1
                activeRole = role
                operationMode = mode
                peerActionStarted = AtomicBoolean(false)
                networkActionStarted = AtomicBoolean(false)
                generation
            }
        update(currentGeneration, WifiAwareProbePhase.CHECKING, role, "capability")

        scope.launch {
            val capability = AndroidWifiAwareCapabilityProbe.read(appContext)
            if (!isActive(currentGeneration)) return@launch
            _snapshot.value = _snapshot.value.copy(pairedDeviceCount = capability.pairedDeviceCount)
            if (
                capability.availability != WifiAwareAvailability.READY &&
                capability.availability != WifiAwareAvailability.PAIRING_REQUIRED
            ) {
                fail(currentGeneration, capability.availability.wireName)
                return@launch
            }
            if (Build.VERSION.SDK_INT < WIFI_AWARE_PAIRING_MIN_API) {
                fail(currentGeneration, "unsupported_os")
                return@launch
            }
            startApi34(currentGeneration, role)
        }
    }

    fun stop() {
        synchronized(lock) {
            generation += 1
            closeResourcesLocked()
            activeRole = null
        }
        _snapshot.value = WifiAwareProbeSnapshot(detail = "stopped")
        log(_snapshot.value)
    }

    override fun close() {
        stop()
        scope.cancel()
    }

    @RequiresApi(WIFI_AWARE_PAIRING_MIN_API)
    private fun startApi34(
        currentGeneration: Int,
        role: WifiAwareProbeRole,
    ) {
        val manager = appContext.getSystemService(WifiAwareManager::class.java)
        if (manager == null || !manager.isAvailable) {
            fail(currentGeneration, "temporarily_unavailable")
            return
        }
        val cipherSuite = preferredPairingCipher(manager.characteristics)
        if (cipherSuite == null) {
            fail(currentGeneration, "pairing_cipher_unavailable")
            return
        }

        update(currentGeneration, WifiAwareProbePhase.ATTACHING, role, "requesting_session")
        try {
            manager.attach(
                object : AttachCallback() {
                    override fun onAttached(session: WifiAwareSession) {
                        if (!isActive(currentGeneration)) {
                            session.close()
                            return
                        }
                        synchronized(lock) { awareSession = session }
                        startDiscovery(currentGeneration, role, session, cipherSuite)
                    }

                    override fun onAttachFailed() {
                        fail(currentGeneration, "attach_failed")
                    }
                },
                mainHandler,
            )
        } catch (error: RuntimeException) {
            fail(currentGeneration, redactedFailure(error))
        }
    }

    @SuppressLint("MissingPermission")
    @RequiresApi(WIFI_AWARE_PAIRING_MIN_API)
    private fun startDiscovery(
        currentGeneration: Int,
        role: WifiAwareProbeRole,
        session: WifiAwareSession,
        cipherSuite: Int,
    ) {
        val pairingConfig =
            AwarePairingConfig
                .Builder()
                .setPairingSetupEnabled(true)
                .setPairingVerificationEnabled(true)
                .setPairingCacheEnabled(true)
                .setBootstrappingMethods(AwarePairingConfig.PAIRING_BOOTSTRAPPING_OPPORTUNISTIC)
                .build()
        val callback = discoveryCallback(currentGeneration, role, cipherSuite)
        val serviceName =
            when (synchronized(lock) { operationMode }) {
                OperationMode.Probe -> ENVOIX_WIFI_AWARE_PROBE_SERVICE
                is OperationMode.Transfer -> ENVOIX_WIFI_AWARE_TRANSFER_SERVICE
            }

        try {
            when (role) {
                WifiAwareProbeRole.PUBLISHER -> {
                    val config =
                        PublishConfig
                            .Builder()
                            .setServiceName(serviceName)
                            .setPairingConfig(pairingConfig)
                            .build()
                    session.publish(config, callback, mainHandler)
                    update(currentGeneration, WifiAwareProbePhase.PUBLISHING, role, "starting")
                }

                WifiAwareProbeRole.SUBSCRIBER -> {
                    val config =
                        SubscribeConfig
                            .Builder()
                            .setServiceName(serviceName)
                            .setPairingConfig(pairingConfig)
                            .build()
                    session.subscribe(config, callback, mainHandler)
                    update(currentGeneration, WifiAwareProbePhase.BROWSING, role, "starting")
                }
            }
        } catch (error: RuntimeException) {
            fail(currentGeneration, redactedFailure(error))
        }
    }

    @RequiresApi(WIFI_AWARE_PAIRING_MIN_API)
    private fun discoveryCallback(
        currentGeneration: Int,
        role: WifiAwareProbeRole,
        cipherSuite: Int,
    ): DiscoverySessionCallback =
        object : DiscoverySessionCallback() {
            override fun onPublishStarted(session: PublishDiscoverySession) {
                retainDiscoverySession(currentGeneration, session)
                update(currentGeneration, WifiAwareProbePhase.PUBLISHING, role, "waiting_for_peer")
            }

            override fun onSubscribeStarted(session: SubscribeDiscoverySession) {
                retainDiscoverySession(currentGeneration, session)
                update(currentGeneration, WifiAwareProbePhase.BROWSING, role, "waiting_for_peer")
            }

            override fun onServiceDiscovered(info: ServiceDiscoveryInfo) {
                if (role != WifiAwareProbeRole.SUBSCRIBER || !isActive(currentGeneration)) return
                handleDiscoveredPeer(
                    currentGeneration,
                    role,
                    info.peerHandle,
                    info.pairedAlias != null,
                    cipherSuite,
                )
            }

            @Deprecated("API 34 supplies ServiceDiscoveryInfo")
            override fun onServiceDiscovered(
                peerHandle: PeerHandle,
                serviceSpecificInfo: ByteArray,
                matchFilter: MutableList<ByteArray>,
            ) {
                if (role != WifiAwareProbeRole.SUBSCRIBER || !isActive(currentGeneration)) return
                handleDiscoveredPeer(currentGeneration, role, peerHandle, false, cipherSuite)
            }

            override fun onPairingSetupRequestReceived(
                peerHandle: PeerHandle,
                requestId: Int,
            ) {
                val session = synchronized(lock) { discoverySession }
                if (!isActive(currentGeneration) || session == null) return
                update(currentGeneration, WifiAwareProbePhase.PAIRING, role, "accepting_request")
                try {
                    session.acceptPairingRequest(
                        requestId,
                        peerHandle,
                        newPeerAlias(),
                        cipherSuite,
                        null,
                    )
                } catch (error: RuntimeException) {
                    fail(currentGeneration, redactedFailure(error))
                }
            }

            override fun onPairingSetupSucceeded(
                peerHandle: PeerHandle,
                alias: String,
            ) {
                handlePairedPeer(currentGeneration, role, peerHandle)
            }

            override fun onPairingVerificationSucceed(
                peerHandle: PeerHandle,
                alias: String,
            ) {
                handlePairedPeer(currentGeneration, role, peerHandle)
            }

            override fun onPairingSetupFailed(peerHandle: PeerHandle) {
                fail(currentGeneration, "pairing_setup_failed")
            }

            override fun onPairingVerificationFailed(peerHandle: PeerHandle) {
                fail(currentGeneration, "pairing_verification_failed")
            }

            override fun onSessionConfigFailed() {
                fail(currentGeneration, "discovery_config_failed")
            }

            override fun onSessionTerminated() {
                if (isActive(currentGeneration)) fail(currentGeneration, "discovery_terminated")
            }
        }

    private fun retainDiscoverySession(
        currentGeneration: Int,
        session: DiscoverySession,
    ) {
        if (!isActive(currentGeneration)) {
            session.close()
            return
        }
        synchronized(lock) { discoverySession = session }
    }

    @RequiresApi(WIFI_AWARE_PAIRING_MIN_API)
    private fun handleDiscoveredPeer(
        currentGeneration: Int,
        role: WifiAwareProbeRole,
        peerHandle: PeerHandle,
        alreadyPaired: Boolean,
        cipherSuite: Int,
    ) {
        if (!peerActionStarted.compareAndSet(false, true)) return
        val session = synchronized(lock) { discoverySession }
        if (session == null) {
            fail(currentGeneration, "discovery_session_missing")
            return
        }

        if (alreadyPaired) {
            handlePairedPeer(currentGeneration, role, peerHandle)
            return
        }

        update(currentGeneration, WifiAwareProbePhase.PAIRING, role, "initiating_request")
        try {
            session.initiatePairingRequest(
                peerHandle,
                newPeerAlias(),
                cipherSuite,
                null,
            )
        } catch (error: RuntimeException) {
            fail(currentGeneration, redactedFailure(error))
        }
    }

    @RequiresApi(WIFI_AWARE_PAIRING_MIN_API)
    private fun handlePairedPeer(
        currentGeneration: Int,
        role: WifiAwareProbeRole,
        peerHandle: PeerHandle,
    ) {
        if (!isActive(currentGeneration)) return
        val session = synchronized(lock) { discoverySession }
        if (session == null) {
            fail(currentGeneration, "discovery_session_missing")
            return
        }
        requestNetwork(currentGeneration, role, session, peerHandle)
    }

    @RequiresApi(WIFI_AWARE_PAIRING_MIN_API)
    private fun requestNetwork(
        currentGeneration: Int,
        role: WifiAwareProbeRole,
        session: DiscoverySession,
        peerHandle: PeerHandle,
    ) {
        if (!networkActionStarted.compareAndSet(false, true)) return
        val connectivity = appContext.getSystemService(ConnectivityManager::class.java)
        if (connectivity == null) {
            fail(currentGeneration, "connectivity_service_missing")
            return
        }

        try {
            val specifierBuilder = WifiAwareNetworkSpecifier.Builder(session, peerHandle)
            if (role == WifiAwareProbeRole.PUBLISHER) {
                val listener = ServerSocket(0)
                synchronized(lock) { serverSocket = listener }
                specifierBuilder
                    .setPort(listener.localPort)
                    .setTransportProtocol(OsConstants.IPPROTO_TCP)
            }
            val request =
                NetworkRequest
                    .Builder()
                    .addTransportType(NetworkCapabilities.TRANSPORT_WIFI_AWARE)
                    .setNetworkSpecifier(specifierBuilder.build())
                    .build()
            val callback = networkCallback(currentGeneration, role, connectivity)
            synchronized(lock) { networkCallback = callback }
            update(
                currentGeneration,
                WifiAwareProbePhase.REQUESTING_NETWORK,
                role,
                "waiting_for_data_path",
            )
            connectivity.requestNetwork(request, callback, WIFI_AWARE_NETWORK_TIMEOUT_MS)
        } catch (error: RuntimeException) {
            fail(currentGeneration, redactedFailure(error))
        } catch (error: IOException) {
            fail(currentGeneration, redactedFailure(error))
        }
    }

    private fun networkCallback(
        currentGeneration: Int,
        role: WifiAwareProbeRole,
        connectivity: ConnectivityManager,
    ): ConnectivityManager.NetworkCallback {
        val exchangeStarted = AtomicBoolean(false)
        return object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                if (!isActive(currentGeneration)) return
                if (role == WifiAwareProbeRole.PUBLISHER && exchangeStarted.compareAndSet(false, true)) {
                    acceptProbe(currentGeneration, role)
                }
            }

            override fun onCapabilitiesChanged(
                network: Network,
                capabilities: NetworkCapabilities,
            ) {
                if (!isActive(currentGeneration)) return
                if (!capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI_AWARE)) {
                    fail(currentGeneration, "wrong_network_transport")
                    return
                }
                if (role != WifiAwareProbeRole.SUBSCRIBER || !exchangeStarted.compareAndSet(false, true)) {
                    return
                }
                val info = capabilities.transportInfo as? WifiAwareNetworkInfo
                val address = info?.peerIpv6Addr
                val port = info?.port ?: 0
                if (address == null || port !in 1..65_535) {
                    exchangeStarted.set(false)
                    return
                }
                sendProbe(currentGeneration, role, network, InetSocketAddress(address, port))
            }

            override fun onUnavailable() {
                fail(currentGeneration, "network_unavailable")
            }

            override fun onLost(network: Network) {
                if (isActive(currentGeneration)) fail(currentGeneration, "network_lost")
            }
        }
    }

    private fun acceptProbe(
        currentGeneration: Int,
        role: WifiAwareProbeRole,
    ) {
        update(currentGeneration, WifiAwareProbePhase.CONNECTING, role, "accepting_socket")
        ioJob =
            scope.launch {
                try {
                    val listener =
                        synchronized(lock) { serverSocket }
                            ?: throw IOException("server socket missing")
                    listener.soTimeout = WIFI_AWARE_SOCKET_TIMEOUT_MS
                    val accepted = listener.accept()
                    accepted.soTimeout =
                        if (isTransferOperation()) 0 else WIFI_AWARE_SOCKET_TIMEOUT_MS
                    synchronized(lock) { socket = accepted }
                    when (val mode = synchronized(lock) { operationMode }) {
                        OperationMode.Probe -> {
                            update(currentGeneration, WifiAwareProbePhase.EXCHANGING, role, "receiving_probe")
                            val request = accepted.getInputStream().readExactly(WIFI_AWARE_PROBE_FRAME_LENGTH)
                            val response = WifiAwareProbeWireProtocol.makeResponse(request)
                            accepted.getOutputStream().apply {
                                write(response)
                                flush()
                            }
                            succeed(currentGeneration, role)
                        }

                        is OperationMode.Transfer -> {
                            startNativeTransfer(currentGeneration, role, accepted, mode)
                        }
                    }
                } catch (error: Exception) {
                    fail(currentGeneration, redactedFailure(error))
                }
            }
    }

    private fun sendProbe(
        currentGeneration: Int,
        role: WifiAwareProbeRole,
        network: Network,
        address: InetSocketAddress,
    ) {
        update(currentGeneration, WifiAwareProbePhase.CONNECTING, role, "opening_socket")
        ioJob =
            scope.launch {
                try {
                    val connected = network.socketFactory.createSocket()
                    connected.soTimeout =
                        if (isTransferOperation()) 0 else WIFI_AWARE_SOCKET_TIMEOUT_MS
                    connected.connect(address, WIFI_AWARE_SOCKET_TIMEOUT_MS)
                    synchronized(lock) { socket = connected }
                    when (val mode = synchronized(lock) { operationMode }) {
                        OperationMode.Probe -> {
                            update(currentGeneration, WifiAwareProbePhase.EXCHANGING, role, "sending_probe")
                            val nonce = ByteArray(WIFI_AWARE_PROBE_NONCE_LENGTH).also(SecureRandom()::nextBytes)
                            connected.getOutputStream().apply {
                                write(WifiAwareProbeWireProtocol.makeRequest(nonce))
                                flush()
                            }
                            val response = connected.getInputStream().readExactly(WIFI_AWARE_PROBE_FRAME_LENGTH)
                            WifiAwareProbeWireProtocol.validateResponse(response, nonce)
                            succeed(currentGeneration, role)
                        }

                        is OperationMode.Transfer -> {
                            startNativeTransfer(currentGeneration, role, connected, mode)
                        }
                    }
                } catch (error: Exception) {
                    fail(currentGeneration, redactedFailure(error))
                }
            }
    }

    private fun startNativeTransfer(
        currentGeneration: Int,
        role: WifiAwareProbeRole,
        connected: Socket,
        mode: OperationMode.Transfer,
    ) {
        update(currentGeneration, WifiAwareProbePhase.EXCHANGING, role, "manifest_v2")
        val transport = AndroidWifiAwareSocketTransport(connected)
        val callback =
            object : ManifestV2Callback {
                override fun onEvent(json: String) {
                    mode.callback.onEvent(json)
                    val state = runCatching { JSONObject(json).optString("state") }.getOrDefault("")
                    when (state) {
                        "completed" -> succeedTransfer(currentGeneration, role)
                        "failed" -> failTransfer(currentGeneration, role)
                    }
                }

                override fun onPlanRequired(requestJson: String): String = mode.callback.onPlanRequired(requestJson)

                override fun onSaveRequired(requestJson: String): String = mode.callback.onSaveRequired(requestJson)

                override fun onRememberedCredential(
                    opaqueCredential: ByteArray,
                    generation: Long,
                ): Boolean = mode.callback.onRememberedCredential(opaqueCredential, generation)
            }
        mode.nativeStarted.set(true)
        Native.startManifestV2NativeSession(
            mode.nativeId,
            mode.paramsJson,
            mode.pairingToken,
            transport,
            callback,
        )
    }

    private fun succeedTransfer(
        currentGeneration: Int,
        role: WifiAwareProbeRole,
    ) {
        complete(
            currentGeneration,
            WifiAwareProbeSnapshot(
                phase = WifiAwareProbePhase.SUCCEEDED,
                role = role,
                detail = "path=wifi_aware · manifest_v2=completed",
                pairedDeviceCount = _snapshot.value.pairedDeviceCount,
            ),
        )
    }

    private fun failTransfer(
        currentGeneration: Int,
        role: WifiAwareProbeRole,
    ) {
        complete(
            currentGeneration,
            WifiAwareProbeSnapshot(
                phase = WifiAwareProbePhase.FAILED,
                role = role,
                detail = "path=wifi_aware · manifest_v2=failed",
                pairedDeviceCount = _snapshot.value.pairedDeviceCount,
            ),
        )
    }

    private fun isTransferOperation(): Boolean = synchronized(lock) { operationMode is OperationMode.Transfer }

    private fun succeed(
        currentGeneration: Int,
        role: WifiAwareProbeRole,
    ) {
        complete(
            currentGeneration,
            WifiAwareProbeSnapshot(
                phase = WifiAwareProbePhase.SUCCEEDED,
                role = role,
                detail = "path=wifi_aware · bytes=$WIFI_AWARE_PROBE_FRAME_LENGTH",
                pairedDeviceCount = _snapshot.value.pairedDeviceCount,
            ),
        )
    }

    private fun fail(
        currentGeneration: Int,
        detail: String,
    ) {
        val (role, mode) = synchronized(lock) { activeRole to operationMode }
        if (mode is OperationMode.Transfer && !mode.nativeStarted.get()) {
            mode.callback.onEvent(
                JSONObject()
                    .put("notice", "manifest_v2")
                    .put("state", "failed")
                    .put("cause", "network_lost")
                    .put("detail", detail)
                    .put("retryable", true)
                    .put("recovery_action", "retry")
                    .toString(),
            )
        }
        complete(
            currentGeneration,
            WifiAwareProbeSnapshot(
                phase = WifiAwareProbePhase.FAILED,
                role = role,
                detail = detail,
                pairedDeviceCount = _snapshot.value.pairedDeviceCount,
            ),
        )
    }

    private fun complete(
        currentGeneration: Int,
        completed: WifiAwareProbeSnapshot,
    ) {
        synchronized(lock) {
            if (currentGeneration != generation) return
            generation += 1
            closeResourcesLocked()
            activeRole = null
        }
        _snapshot.value = completed
        log(completed)
    }

    private fun update(
        currentGeneration: Int,
        phase: WifiAwareProbePhase,
        role: WifiAwareProbeRole,
        detail: String,
    ) {
        if (!isActive(currentGeneration)) return
        val updated =
            WifiAwareProbeSnapshot(
                phase = phase,
                role = role,
                detail = detail,
                pairedDeviceCount = _snapshot.value.pairedDeviceCount,
            )
        _snapshot.value = updated
        log(updated)
    }

    private fun log(snapshot: WifiAwareProbeSnapshot) {
        LogStore.append(
            "wifi_aware_probe phase=${snapshot.phase.wireName} " +
                "role=${snapshot.role?.wireName ?: "none"} detail=${snapshot.detail}",
        )
    }

    private fun isActive(currentGeneration: Int): Boolean = synchronized(lock) { currentGeneration == generation }

    private fun closeResourcesLocked() {
        ioJob?.cancel()
        ioJob = null
        runCatching { socket?.close() }
        socket = null
        runCatching { serverSocket?.close() }
        serverSocket = null
        networkCallback?.let { callback ->
            runCatching {
                appContext
                    .getSystemService(ConnectivityManager::class.java)
                    ?.unregisterNetworkCallback(callback)
            }
        }
        networkCallback = null
        runCatching { discoverySession?.close() }
        discoverySession = null
        runCatching { awareSession?.close() }
        awareSession = null
    }

    @RequiresApi(WIFI_AWARE_PAIRING_MIN_API)
    private fun preferredPairingCipher(characteristics: Characteristics?): Int? {
        val suites = characteristics?.supportedPairingCipherSuites ?: return null
        return when {
            suites and Characteristics.WIFI_AWARE_CIPHER_SUITE_NCS_PK_PASN_256 != 0 ->
                Characteristics.WIFI_AWARE_CIPHER_SUITE_NCS_PK_PASN_256

            suites and Characteristics.WIFI_AWARE_CIPHER_SUITE_NCS_PK_PASN_128 != 0 ->
                Characteristics.WIFI_AWARE_CIPHER_SUITE_NCS_PK_PASN_128

            else -> null
        }
    }

    private fun newPeerAlias(): String = WIFI_AWARE_PEER_ALIAS_PREFIX + UUID.randomUUID().toString().take(8)

    private fun redactedFailure(error: Throwable): String =
        when (error) {
            is SecurityException -> "permission_denied"
            is SocketTimeoutException -> "timeout"
            is EOFException -> "unexpected_eof"
            is WifiAwareProbeProtocolException -> "probe_protocol_error"
            is IOException -> "io_error"
            is IllegalArgumentException -> "invalid_platform_request"
            else -> "unexpected_${error.javaClass.simpleName}"
        }

    private fun java.io.InputStream.readExactly(length: Int): ByteArray {
        val result = ByteArray(length)
        var offset = 0
        while (offset < length) {
            val count = read(result, offset, length - offset)
            if (count < 0) throw EOFException("stream ended before probe frame")
            offset += count
        }
        return result
    }
}
