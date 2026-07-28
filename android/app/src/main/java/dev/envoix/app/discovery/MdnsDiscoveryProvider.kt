package dev.envoix.app.discovery

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import dev.envoix.app.Native
import dev.envoix.app.NearbyInviteCallback
import dev.envoix.app.OpLog
import org.json.JSONArray
import org.json.JSONObject
import java.net.DatagramSocket
import java.nio.charset.StandardCharsets
import java.util.concurrent.atomic.AtomicLong

@Suppress("DEPRECATION")
internal class MdnsDiscoveryProvider(
    context: Context,
    private val localIdentity: LocalDiscoveryIdentity,
    private val advertiseEnabled: Boolean = true,
    private val relay: String = "",
) : DiscoveryProvider,
    NearbyRendezvousProvider {
    override val source = DiscoverySource.Mdns

    private val nsdManager = context.getSystemService(NsdManager::class.java)
    private val wifiManager = context.applicationContext.getSystemService(WifiManager::class.java)
    private val handler = Handler(Looper.getMainLooper())
    private val resolveQueue = ArrayDeque<NsdServiceInfo>()
    private val resolveFailures = mutableMapOf<String, Int>()
    private val resolvedObservations = mutableMapOf<String, DiscoveryObservation>()
    private val pendingOffers = mutableMapOf<String, (String?) -> Unit>()

    private var listener: DiscoveryListener? = null
    private var active = false
    private var registered = false
    private var registrationRequested = false
    private var registrationSettled = false
    private var discovering = false
    private var discoveryRequested = false
    private var discoverySettled = false
    private var resolving = false
    private var resolvingServiceName: String? = null
    private var inboxSessionId: Long? = null
    private var inboxReady = false
    private var inboxSettled = false
    private var failureDetail: String? = null
    private var serviceSocket: DatagramSocket? = null
    private var multicastLock: WifiManager.MulticastLock? = null
    private var ownServiceName: String? = null

    private val refreshRunnable =
        object : Runnable {
            override fun run() {
                if (!active) return
                val now = SystemClock.elapsedRealtime()
                resolvedObservations.values
                    .map(DiscoveryObservation::peerKey)
                    .distinct()
                    .forEach { peerKey ->
                        currentObservation(peerKey, now)?.let { listener?.onObservation(it) }
                    }
                handler.postDelayed(this, OBSERVATION_REFRESH_MS)
            }
        }

    private val registrationListener =
        object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(serviceInfo: NsdServiceInfo) {
                handler.post {
                    if (!active || !inboxReady) {
                        runCatching { nsdManager?.unregisterService(this) }
                    } else {
                        ownServiceName = serviceInfo.serviceName
                        registered = true
                        registrationSettled = true
                        emitOperationalStatus()
                    }
                }
            }

            override fun onRegistrationFailed(
                serviceInfo: NsdServiceInfo,
                errorCode: Int,
            ) {
                handler.post {
                    if (!active) return@post
                    registrationRequested = false
                    registered = false
                    registrationSettled = true
                    failureDetail = "mDNS visibility failed (code $errorCode)"
                    emitOperationalStatus()
                }
            }

            override fun onServiceUnregistered(serviceInfo: NsdServiceInfo) {
                handler.post {
                    registrationRequested = false
                    registered = false
                    if (active) emitOperationalStatus()
                }
            }

            override fun onUnregistrationFailed(
                serviceInfo: NsdServiceInfo,
                errorCode: Int,
            ) = Unit
        }

    private val discoveryListener =
        object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {
                handler.post {
                    if (!active) {
                        runCatching { nsdManager?.stopServiceDiscovery(this) }
                    } else {
                        discovering = true
                        discoverySettled = true
                        emitOperationalStatus()
                    }
                }
            }

            override fun onServiceFound(serviceInfo: NsdServiceInfo) {
                handler.post {
                    if (!active || serviceInfo.serviceName == ownServiceName) return@post
                    enqueueResolution(serviceInfo)
                    resolveNext()
                }
            }

            override fun onServiceLost(serviceInfo: NsdServiceInfo) {
                handler.post {
                    val removed = resolvedObservations.remove(serviceInfo.serviceName)
                    resolveQueue.removeAll { queued -> queued.serviceName == serviceInfo.serviceName }
                    resolveFailures.remove(serviceInfo.serviceName)
                    removed?.peerKey?.let { peerKey ->
                        currentObservation(peerKey, SystemClock.elapsedRealtime())
                            ?.let { listener?.onObservation(it) }
                    }
                }
            }

            override fun onDiscoveryStopped(serviceType: String) {
                handler.post {
                    discoveryRequested = false
                    if (!active) return@post
                    discovering = false
                    discoverySettled = true
                    resolvedObservations.clear()
                    resolveQueue.clear()
                    resolveFailures.clear()
                    failureDetail = "mDNS scan stopped"
                    emitOperationalStatus()
                }
            }

            override fun onStartDiscoveryFailed(
                serviceType: String,
                errorCode: Int,
            ) {
                handler.post {
                    if (!active) return@post
                    discoveryRequested = false
                    discovering = false
                    discoverySettled = true
                    resolvedObservations.clear()
                    resolveQueue.clear()
                    resolveFailures.clear()
                    failureDetail = "mDNS scan failed (code $errorCode)"
                    emitOperationalStatus()
                }
            }

            override fun onStopDiscoveryFailed(
                serviceType: String,
                errorCode: Int,
            ) = Unit
        }

    override fun start(listener: DiscoveryListener) {
        this.listener = listener
        if (active) {
            emitOperationalStatus()
            return
        }
        listener.onStatus(ProviderStatus(source, ProviderAvailability.Starting, "Starting local-network discovery"))
        if (nsdManager == null) {
            listener.onStatus(ProviderStatus(source, ProviderAvailability.Unsupported, "mDNS is unavailable"))
            return
        }

        try {
            serviceSocket = DatagramSocket(0)
            multicastLock =
                wifiManager
                    ?.createMulticastLock(MULTICAST_LOCK_TAG)
                    ?.apply {
                        setReferenceCounted(false)
                        acquire()
                    }
        } catch (_: Exception) {
            releaseResources()
            listener.onStatus(
                ProviderStatus(source, ProviderAvailability.TemporarilyUnavailable, "Local-network discovery could not start"),
            )
            return
        }

        active = true
        registered = false
        registrationRequested = false
        registrationSettled = !advertiseEnabled
        discovering = false
        discoveryRequested = false
        discoverySettled = false
        inboxReady = false
        inboxSettled = false
        failureDetail = null

        try {
            discoveryRequested = true
            nsdManager.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, discoveryListener)
        } catch (_: Exception) {
            discoveryRequested = false
            discoverySettled = true
            failureDetail = "mDNS scan could not start"
        }
        startInbox()
        handler.postDelayed(refreshRunnable, OBSERVATION_REFRESH_MS)
        emitOperationalStatus()
    }

    override fun stop() {
        if (!active) {
            listener = null
            releaseResources()
            return
        }
        active = false
        handler.removeCallbacks(refreshRunnable)
        val stoppedSessionId = inboxSessionId
        inboxSessionId = null
        stoppedSessionId?.let { id -> runCatching { Native.stopNearbyInviteInbox(id) } }
        completePendingOffers("Local-network discovery stopped")
        if (discoveryRequested) runCatching { nsdManager?.stopServiceDiscovery(discoveryListener) }
        if (registrationRequested) runCatching { nsdManager?.unregisterService(registrationListener) }
        releaseResources()
        listener?.onStatus(ProviderStatus(source, ProviderAvailability.Stopped, "Local-network discovery stopped"))
        listener = null
    }

    override fun offerInvite(
        selection: NearbyPairingSelection,
        invite: String,
        completion: (String?) -> Unit,
    ) {
        val id = inboxSessionId
        val route =
            selection.nearbyInviteRoute?.let {
                NearbyInviteRoute.normalized(
                    endpointId = it.endpointId,
                    relayUrl = it.relayUrl,
                    directAddresses = it.directAddresses,
                )
            }
        if (!active || !inboxReady || id == null) {
            completion("Secure local-network invitation delivery is not ready")
            return
        }
        if (route == null) {
            completion("The selected device does not expose a usable secure local-network route")
            return
        }
        val selectedPeerStillPresent =
            uniqueRouteForPeer(selection.discoveryPeerKey) == route
        if (!selectedPeerStillPresent) {
            completion("The selected device is no longer available on the local network")
            return
        }
        if (pendingOffers.isNotEmpty()) {
            completion("Another local-network invitation is already being delivered")
            return
        }

        val requestId = nextRequestId.getAndIncrement().toULong().toString(16)
        pendingOffers[requestId] = completion
        val response =
            runCatching {
                JSONObject(
                    Native.sendNearbyInvite(
                        id = id,
                        requestId = requestId,
                        routeJson = route.toNativeJson(),
                        invite = invite,
                    ),
                )
            }.getOrElse { error ->
                pendingOffers.remove(requestId)
                completion(error.message ?: "Local-network invitation delivery could not start")
                return
            }
        response.optString("error").takeIf(String::isNotBlank)?.let { error ->
            pendingOffers.remove(requestId)
            completion(error)
            return
        }
        if (!response.optBoolean("queued")) {
            pendingOffers.remove(requestId)
            completion("Local-network invitation delivery was not queued")
        }
    }

    private fun startInbox() {
        val id = nextSessionId.getAndIncrement()
        inboxSessionId = id
        val params =
            JSONObject()
                .put("peer_key", localIdentity.peerKey)
                .put("display_name", localIdentity.displayName)
                .put("relay", relay)
        runCatching {
            Native.startNearbyInviteInbox(
                id,
                params.toString(),
                object : NearbyInviteCallback {
                    override fun onEvent(json: String) {
                        handler.post { handleInboxEvent(id, json) }
                    }
                },
            )
        }.onFailure { error ->
            inboxSessionId = null
            inboxSettled = true
            registrationSettled = true
            failureDetail = error.message ?: "Secure local-network invitations could not start"
        }
    }

    private fun handleInboxEvent(
        id: Long,
        json: String,
    ) {
        if (!active || inboxSessionId != id) return
        val event =
            runCatching { JSONObject(json) }
                .getOrElse {
                    failInbox("Secure local-network invitations returned an invalid event")
                    return
                }
        when (event.optString("event")) {
            "ready" -> {
                val route = event.nearbyInviteRoute()
                if (route == null) {
                    failInbox("Secure local-network invitations returned an unusable route")
                    return
                }
                inboxReady = true
                inboxSettled = true
                if (advertiseEnabled) registerPresence(route)
                emitOperationalStatus()
            }
            "incoming" -> handleIncomingInvite(event)
            "send_result" -> handleSendResult(event)
            "failed" ->
                failInbox(
                    event.optString("message").ifBlank {
                        "Secure local-network invitations failed"
                    },
                )
        }
    }

    private fun handleIncomingInvite(event: JSONObject) {
        val requestId =
            event
                .optString("request_id")
                .trim()
                .takeIf { it.isNotEmpty() && it.length <= MAX_REQUEST_ID_LENGTH }
                ?: return
        val endpointId = normalizeNearbyInboxEndpointId(event.optString("sender_endpoint_id")) ?: return
        val peerKey =
            DiscoveryPeerRegistry.normalizePeerKey(event.optString("sender_peer_key"))
                ?: return
        val knownPeer = resolvedObservations.values.any { it.peerKey == peerKey }
        if (knownPeer && uniqueRouteForPeer(peerKey)?.endpointId != endpointId) {
            OpLog.add("DISCOVERY provider=mdns state=inbox_identity_mismatch")
            return
        }
        val invite = event.optString("invite").trim().takeIf { it.isNotEmpty() } ?: return
        listener?.onRendezvousOffer(
            NearbyRendezvousOffer(
                requestId = requestId,
                senderPeerKey = peerKey,
                senderDisplayName =
                    DiscoveryPeerRegistry.sanitizeDisplayName(
                        event.optString("sender_display_name").takeIf(String::isNotBlank),
                    ),
                invite = invite,
                senderEndpointId = endpointId,
                source = source,
            ),
        )
    }

    private fun handleSendResult(event: JSONObject) {
        val requestId = event.optString("request_id")
        val completion = pendingOffers.remove(requestId) ?: return
        val error =
            event
                .optString("error")
                .takeIf { !event.isNull("error") && it.isNotBlank() }
        completion(error)
    }

    private fun failInbox(message: String) {
        inboxReady = false
        inboxSettled = true
        registrationSettled = true
        failureDetail = message
        if (registrationRequested) {
            registrationRequested = false
            registered = false
            runCatching { nsdManager?.unregisterService(registrationListener) }
        }
        completePendingOffers(message)
        emitOperationalStatus()
    }

    private fun completePendingOffers(error: String) {
        val completions = pendingOffers.values.toList()
        pendingOffers.clear()
        completions.forEach { completion -> completion(error) }
    }

    private fun registerPresence(route: NearbyInviteRoute) {
        if (!active || registrationRequested || registered) return
        try {
            val serviceInfo =
                NsdServiceInfo().apply {
                    serviceName = "Envoix-${localIdentity.peerKey.take(SERVICE_NAME_KEY_LENGTH)}"
                    serviceType = SERVICE_TYPE
                    port = checkNotNull(serviceSocket).localPort
                    setAttribute(TXT_VERSION, PROTOCOL_VERSION)
                    setAttribute(TXT_PEER_KEY, localIdentity.peerKey)
                    setAttribute(TXT_DISPLAY_NAME, localIdentity.displayName)
                    nearbyInviteTxtAttributes(route).forEach { (key, value) ->
                        setAttribute(key, value)
                    }
                }
            registrationRequested = true
            nsdManager?.registerService(serviceInfo, NsdManager.PROTOCOL_DNS_SD, registrationListener)
        } catch (_: Exception) {
            registrationRequested = false
            registrationSettled = true
            failureDetail = "mDNS visibility could not start"
            emitOperationalStatus()
        }
    }

    private fun resolveNext() {
        if (!active || resolving || resolveQueue.isEmpty()) return
        val serviceInfo = resolveQueue.removeFirst()
        resolving = true
        resolvingServiceName = serviceInfo.serviceName
        try {
            nsdManager?.resolveService(
                serviceInfo,
                object : NsdManager.ResolveListener {
                    override fun onServiceResolved(resolved: NsdServiceInfo) {
                        handler.post {
                            resolving = false
                            resolvingServiceName = null
                            resolveFailures.remove(resolved.serviceName)
                            if (active && discovering) acceptResolvedService(resolved)
                            resolveNext()
                        }
                    }

                    override fun onResolveFailed(
                        failed: NsdServiceInfo,
                        errorCode: Int,
                    ) {
                        handler.post {
                            resolving = false
                            resolvingServiceName = null
                            retryResolution(failed, errorCode)
                            resolveNext()
                        }
                    }
                },
            )
        } catch (_: Exception) {
            resolving = false
            resolvingServiceName = null
            retryResolution(serviceInfo, errorCode = null)
            resolveNext()
        }
    }

    private fun enqueueResolution(serviceInfo: NsdServiceInfo) {
        if (resolvingServiceName == serviceInfo.serviceName ||
            resolveQueue.any { it.serviceName == serviceInfo.serviceName }
        ) {
            return
        }
        resolveQueue.addLast(serviceInfo)
    }

    private fun retryResolution(
        serviceInfo: NsdServiceInfo,
        errorCode: Int?,
    ) {
        if (!active || !discovering) return
        val serviceName = serviceInfo.serviceName
        val attempt = (resolveFailures[serviceName] ?: 0) + 1
        resolveFailures[serviceName] = attempt
        if (attempt >= MAX_RESOLVE_ATTEMPTS) {
            OpLog.add(
                "DISCOVERY provider=mdns state=resolve_failed attempts=$attempt" +
                    (errorCode?.let { " code=$it" } ?: ""),
            )
            return
        }
        handler.postDelayed(
            {
                if (active &&
                    discovering &&
                    resolveFailures[serviceName] == attempt
                ) {
                    enqueueResolution(serviceInfo)
                    resolveNext()
                }
            },
            RESOLVE_RETRY_BASE_MS * attempt,
        )
    }

    private fun acceptResolvedService(serviceInfo: NsdServiceInfo) {
        val version = serviceInfo.attributeString(TXT_VERSION) ?: return
        if (version != PROTOCOL_VERSION) return
        val peerKey = DiscoveryPeerRegistry.normalizePeerKey(serviceInfo.attributeString(TXT_PEER_KEY).orEmpty()) ?: return
        if (peerKey == localIdentity.peerKey) return
        val nearbyInviteRoute =
            parseNearbyInviteTxtAttributes { key ->
                serviceInfo.attributeString(key)
            }
        val observation =
            DiscoveryObservation(
                peerKey = peerKey,
                source = source,
                seenAtMs = SystemClock.elapsedRealtime(),
                displayName = serviceInfo.attributeString(TXT_DISPLAY_NAME),
                nearbyInviteRoute = nearbyInviteRoute,
            )
        resolvedObservations[serviceInfo.serviceName] = observation
        currentObservation(peerKey, observation.seenAtMs)?.let { listener?.onObservation(it) }
    }

    private fun currentObservation(
        peerKey: String,
        seenAtMs: Long,
    ): DiscoveryObservation? {
        val matching = resolvedObservations.values.filter { it.peerKey == peerKey }
        val newest = matching.maxByOrNull(DiscoveryObservation::seenAtMs) ?: return null
        return newest.copy(
            seenAtMs = seenAtMs,
            nearbyInviteRoute = uniqueRoute(matching),
        )
    }

    private fun uniqueRouteForPeer(peerKey: String): NearbyInviteRoute? =
        uniqueRoute(resolvedObservations.values.filter { it.peerKey == peerKey })

    private fun uniqueRoute(observations: Collection<DiscoveryObservation>): NearbyInviteRoute? =
        observations
            .mapNotNull(DiscoveryObservation::nearbyInviteRoute)
            .distinct()
            .singleOrNull()

    private fun emitOperationalStatus() {
        if (!active) return
        val status =
            when {
                !registrationSettled || !discoverySettled || !inboxSettled ->
                    ProviderStatus(source, ProviderAvailability.Starting, "Starting local-network discovery")
                inboxReady && (registered || !advertiseEnabled) && discovering ->
                    ProviderStatus(
                        source,
                        ProviderAvailability.Ready,
                        if (advertiseEnabled) {
                            "Scanning and visible on the local network"
                        } else {
                            "Scanning the local network"
                        },
                    )
                registered || discovering || inboxReady ->
                    ProviderStatus(
                        source,
                        ProviderAvailability.Degraded,
                        failureDetail ?: "Local-network discovery is partially available",
                    )
                else ->
                    ProviderStatus(
                        source,
                        ProviderAvailability.TemporarilyUnavailable,
                        failureDetail ?: "Local-network discovery is unavailable",
                    )
            }
        listener?.onStatus(status)
    }

    private fun releaseResources() {
        handler.removeCallbacks(refreshRunnable)
        resolveQueue.clear()
        resolveFailures.clear()
        resolvedObservations.clear()
        resolving = false
        resolvingServiceName = null
        registered = false
        registrationRequested = false
        discovering = false
        discoveryRequested = false
        ownServiceName = null
        inboxReady = false
        inboxSettled = false
        runCatching { serviceSocket?.close() }
        serviceSocket = null
        multicastLock?.let { lock -> runCatching { if (lock.isHeld) lock.release() } }
        multicastLock = null
    }

    private fun NsdServiceInfo.attributeString(key: String): String? =
        attributes[key]
            ?.let { value -> String(value, StandardCharsets.UTF_8) }
            ?.trim()
            ?.ifBlank { null }

    companion object {
        const val SERVICE_TYPE = "_envoix-disc._udp."
        private const val PROTOCOL_VERSION = "1"
        private const val TXT_VERSION = "v"
        private const val TXT_PEER_KEY = "id"
        private const val TXT_DISPLAY_NAME = "name"
        private const val SERVICE_NAME_KEY_LENGTH = 8
        private const val MULTICAST_LOCK_TAG = "envoix-nearby-mdns"
        private const val OBSERVATION_REFRESH_MS = 5_000L
        private const val MAX_RESOLVE_ATTEMPTS = 4
        private const val RESOLVE_RETRY_BASE_MS = 350L
        private const val MAX_REQUEST_ID_LENGTH = 128
        private val nextSessionId = AtomicLong(1L)
        private val nextRequestId = AtomicLong(1L)
    }
}

internal const val TXT_INBOX_ENDPOINT = "ibox"
internal const val TXT_INBOX_RELAY = "irelay"
internal const val TXT_INBOX_ADDRESS_PREFIX = "iaddr"

internal fun nearbyInviteTxtAttributes(route: NearbyInviteRoute): Map<String, String> =
    buildMap {
        put(TXT_INBOX_ENDPOINT, route.endpointId)
        route.relayUrl?.let { put(TXT_INBOX_RELAY, it) }
        route.directAddresses.forEachIndexed { index, address ->
            put("$TXT_INBOX_ADDRESS_PREFIX$index", address)
        }
    }

internal fun parseNearbyInviteTxtAttributes(attribute: (String) -> String?): NearbyInviteRoute? =
    NearbyInviteRoute.normalized(
        endpointId = attribute(TXT_INBOX_ENDPOINT),
        relayUrl = attribute(TXT_INBOX_RELAY),
        directAddresses =
            (0 until MAX_NEARBY_DIRECT_ADDRESSES)
                .mapNotNull { index -> attribute("$TXT_INBOX_ADDRESS_PREFIX$index") },
    )

private fun JSONObject.nearbyInviteRoute(): NearbyInviteRoute? {
    val addresses = optJSONArray("direct_addresses") ?: JSONArray()
    return NearbyInviteRoute.normalized(
        endpointId = optString("endpoint_id"),
        relayUrl = if (isNull("relay_url")) null else optString("relay_url"),
        directAddresses =
            (0 until minOf(addresses.length(), MAX_NEARBY_DIRECT_ADDRESSES))
                .mapNotNull { index -> addresses.optString(index).takeIf(String::isNotBlank) },
    )
}

private fun NearbyInviteRoute.toNativeJson(): String =
    JSONObject()
        .put("endpoint_id", endpointId)
        .put("relay_url", relayUrl ?: JSONObject.NULL)
        .put("direct_addresses", JSONArray(directAddresses))
        .toString()
