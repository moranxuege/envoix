package dev.envoix.app.discovery

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import java.net.DatagramSocket
import java.nio.charset.StandardCharsets

@Suppress("DEPRECATION")
internal class MdnsDiscoveryProvider(
    context: Context,
    private val localIdentity: LocalDiscoveryIdentity,
) : DiscoveryProvider {
    override val source = DiscoverySource.Mdns

    private val nsdManager = context.getSystemService(NsdManager::class.java)
    private val wifiManager = context.applicationContext.getSystemService(WifiManager::class.java)
    private val handler = Handler(Looper.getMainLooper())
    private val resolveQueue = ArrayDeque<NsdServiceInfo>()
    private val resolvedObservations = mutableMapOf<String, DiscoveryObservation>()

    private var listener: DiscoveryListener? = null
    private var active = false
    private var registered = false
    private var registrationRequested = false
    private var registrationSettled = false
    private var discovering = false
    private var discoveryRequested = false
    private var discoverySettled = false
    private var resolving = false
    private var failureDetail: String? = null
    private var serviceSocket: DatagramSocket? = null
    private var multicastLock: WifiManager.MulticastLock? = null
    private var ownServiceName: String? = null

    private val refreshRunnable =
        object : Runnable {
            override fun run() {
                if (!active) return
                val now = SystemClock.elapsedRealtime()
                resolvedObservations.values.forEach { observation ->
                    listener?.onObservation(observation.copy(seenAtMs = now))
                }
                handler.postDelayed(this, OBSERVATION_REFRESH_MS)
            }
        }

    private val registrationListener =
        object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(serviceInfo: NsdServiceInfo) {
                handler.post {
                    if (!active) {
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
                handler.post { registrationRequested = false }
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
                    resolveQueue.addLast(serviceInfo)
                    resolveNext()
                }
            }

            override fun onServiceLost(serviceInfo: NsdServiceInfo) {
                handler.post {
                    resolvedObservations.remove(serviceInfo.serviceName)
                    resolveQueue.removeAll { queued -> queued.serviceName == serviceInfo.serviceName }
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
        registrationSettled = false
        discovering = false
        discoveryRequested = false
        discoverySettled = false
        failureDetail = null

        val serviceInfo =
            NsdServiceInfo().apply {
                serviceName = "Envoix-${localIdentity.peerKey.take(SERVICE_NAME_KEY_LENGTH)}"
                serviceType = SERVICE_TYPE
                port = checkNotNull(serviceSocket).localPort
                setAttribute(TXT_VERSION, PROTOCOL_VERSION)
                setAttribute(TXT_PEER_KEY, localIdentity.peerKey)
                setAttribute(TXT_DISPLAY_NAME, localIdentity.displayName)
            }

        try {
            registrationRequested = true
            nsdManager.registerService(serviceInfo, NsdManager.PROTOCOL_DNS_SD, registrationListener)
        } catch (_: Exception) {
            registrationRequested = false
            registrationSettled = true
            failureDetail = "mDNS visibility could not start"
        }
        try {
            discoveryRequested = true
            nsdManager.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, discoveryListener)
        } catch (_: Exception) {
            discoveryRequested = false
            discoverySettled = true
            failureDetail = "mDNS scan could not start"
        }
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
        if (discoveryRequested) runCatching { nsdManager?.stopServiceDiscovery(discoveryListener) }
        if (registrationRequested) runCatching { nsdManager?.unregisterService(registrationListener) }
        releaseResources()
        listener?.onStatus(ProviderStatus(source, ProviderAvailability.Stopped, "Local-network discovery stopped"))
        listener = null
    }

    private fun resolveNext() {
        if (!active || resolving || resolveQueue.isEmpty()) return
        val serviceInfo = resolveQueue.removeFirst()
        resolving = true
        try {
            nsdManager?.resolveService(
                serviceInfo,
                object : NsdManager.ResolveListener {
                    override fun onServiceResolved(resolved: NsdServiceInfo) {
                        handler.post {
                            resolving = false
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
                            resolveNext()
                        }
                    }
                },
            )
        } catch (_: Exception) {
            resolving = false
            resolveNext()
        }
    }

    private fun acceptResolvedService(serviceInfo: NsdServiceInfo) {
        val version = serviceInfo.attributeString(TXT_VERSION) ?: return
        if (version != PROTOCOL_VERSION) return
        val peerKey = DiscoveryPeerRegistry.normalizePeerKey(serviceInfo.attributeString(TXT_PEER_KEY).orEmpty()) ?: return
        if (peerKey == localIdentity.peerKey) return
        val observation =
            DiscoveryObservation(
                peerKey = peerKey,
                source = source,
                seenAtMs = SystemClock.elapsedRealtime(),
                displayName = serviceInfo.attributeString(TXT_DISPLAY_NAME),
            )
        resolvedObservations[serviceInfo.serviceName] = observation
        listener?.onObservation(observation)
    }

    private fun emitOperationalStatus() {
        if (!active) return
        val status =
            when {
                !registrationSettled || !discoverySettled ->
                    ProviderStatus(source, ProviderAvailability.Starting, "Starting local-network discovery")
                registered && discovering ->
                    ProviderStatus(source, ProviderAvailability.Ready, "Scanning and visible on the local network")
                registered || discovering ->
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
        resolvedObservations.clear()
        resolving = false
        registered = false
        registrationRequested = false
        discovering = false
        discoveryRequested = false
        ownServiceName = null
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
    }
}
