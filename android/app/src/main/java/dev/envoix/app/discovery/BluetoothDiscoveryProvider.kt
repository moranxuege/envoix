package dev.envoix.app.discovery

import android.annotation.SuppressLint
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattServer
import android.bluetooth.BluetoothGattServerCallback
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.BluetoothStatusCodes
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import android.os.SystemClock
import dev.envoix.app.OpLog
import java.util.ArrayDeque
import kotlin.random.Random

internal class BluetoothDiscoveryProvider(
    private val context: Context,
    private val localIdentity: LocalDiscoveryIdentity,
    private val advertiseEnabled: Boolean = true,
) : DiscoveryProvider,
    NearbyRendezvousProvider,
    NfcReadinessProvider {
    override val source = DiscoverySource.Bluetooth

    private val handler = Handler(Looper.getMainLooper())
    private val discoveredDevices = linkedMapOf<String, BluetoothDevice>()
    private val resolvedIdentityNames = mutableMapOf<String, String>()
    private val pendingIdentityReads = linkedMapOf<String, IdentityReadRequest>()
    private val identityReadLimiter = IdentityReadAttemptLimiter()
    private val inboundAssemblers = mutableMapOf<String, BleRendezvousProtocol.Assembler>()
    private var listener: DiscoveryListener? = null
    private var active = false
    private var scanning = false
    private var advertising = false
    private var advertisingPending = false
    private var advertisedKind: AdvertisingKind? = null
    private var pendingAdvertisingKind: AdvertisingKind? = null
    private var nfcReadinessOfferId: String? = null
    private var rendezvousReady = false
    private var failureDetail: String? = null
    private var gattServer: BluetoothGattServer? = null
    private var outbound: OutboundOffer? = null
    private var identityRead: IdentityRead? = null

    private data class IdentityReadRequest(
        val peerKey: String,
        val device: BluetoothDevice,
        var rssi: Int?,
    )

    private data class IdentityRead(
        val peerKey: String,
        var rssi: Int?,
        var gatt: BluetoothGatt? = null,
    )

    private data class OutboundOffer(
        val peerKey: String,
        val invite: String,
        val requestId: Long,
        val completion: (String?) -> Unit,
        var gatt: BluetoothGatt? = null,
        var mtu: Int = DEFAULT_GATT_MTU,
        var frames: List<ByteArray> = emptyList(),
        var nextFrame: Int = 0,
        var characteristic: BluetoothGattCharacteristic? = null,
    )

    private enum class AdvertisingKind {
        Presence,
        NfcReadiness,
    }

    private val scanCallback =
        object : ScanCallback() {
            override fun onScanResult(
                callbackType: Int,
                result: ScanResult,
            ) {
                val serviceUuids = result.scanRecord?.serviceUuids.orEmpty()
                val now = SystemClock.elapsedRealtime()
                serviceUuids
                    .asSequence()
                    .mapNotNull { parcelUuid -> NfcReadinessUuid.decode(parcelUuid.uuid) }
                    .firstOrNull()
                    ?.takeIf { offerId -> offerId != nfcReadinessOfferId }
                    ?.let { offerId ->
                        listener?.onNfcReadinessOffer(
                            NfcReadinessOffer(
                                offerId = offerId,
                                seenAtMs = now,
                            ),
                        )
                    }
                val peerKey =
                    serviceUuids
                        .asSequence()
                        .mapNotNull { parcelUuid -> BleDiscoveryUuid.decode(parcelUuid.uuid) }
                        .firstOrNull()
                        ?: return
                if (!active || peerKey == localIdentity.peerKey) return
                val identityUuid = ParcelUuid(checkNotNull(BleDiscoveryUuid.encode(peerKey)))
                val provisionalName =
                    resolvedIdentityNames[peerKey]
                        ?: BleDiscoveryName.decode(
                            serviceData = result.scanRecord?.getServiceData(identityUuid),
                            localName = result.scanRecord?.deviceName,
                        )
                rememberDiscoveredDevice(peerKey, result.device)
                listener?.onObservation(
                    DiscoveryObservation(
                        peerKey = peerKey,
                        source = source,
                        seenAtMs = now,
                        displayName = provisionalName,
                        rssi = result.rssi,
                    ),
                )
                if (peerKey !in resolvedIdentityNames) {
                    enqueueIdentityRead(peerKey, result.device, result.rssi)
                }
            }

            override fun onScanFailed(errorCode: Int) {
                if (!active) return
                scanning = false
                failureDetail = "Bluetooth scan failed (code $errorCode)"
                emitOperationalStatus()
            }
        }

    private val advertiseCallback =
        object : AdvertiseCallback() {
            override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
                if (!active || pendingAdvertisingKind != AdvertisingKind.Presence) return
                advertisingPending = false
                advertising = true
                pendingAdvertisingKind = null
                advertisedKind = AdvertisingKind.Presence
                emitOperationalStatus()
            }

            override fun onStartFailure(errorCode: Int) {
                if (!active || pendingAdvertisingKind != AdvertisingKind.Presence) return
                advertisingPending = false
                advertising = false
                pendingAdvertisingKind = null
                advertisedKind = null
                failureDetail = "Bluetooth advertising failed (code $errorCode)"
                emitOperationalStatus()
            }
        }

    private val nfcReadinessAdvertiseCallback =
        object : AdvertiseCallback() {
            override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
                if (!active || pendingAdvertisingKind != AdvertisingKind.NfcReadiness) return
                advertisingPending = false
                advertising = true
                pendingAdvertisingKind = null
                advertisedKind = AdvertisingKind.NfcReadiness
                emitOperationalStatus()
            }

            override fun onStartFailure(errorCode: Int) {
                if (!active || pendingAdvertisingKind != AdvertisingKind.NfcReadiness) return
                advertisingPending = false
                advertising = false
                pendingAdvertisingKind = null
                advertisedKind = null
                failureDetail = "NFC readiness advertising failed (code $errorCode)"
                emitOperationalStatus()
            }
        }

    private val gattServerCallback =
        object : BluetoothGattServerCallback() {
            override fun onServiceAdded(
                status: Int,
                service: BluetoothGattService,
            ) {
                handler.post {
                    if (!active || service.uuid != BleRendezvousProtocol.SERVICE_UUID) return@post
                    rendezvousReady = status == BluetoothGatt.GATT_SUCCESS
                    if (!rendezvousReady) failureDetail = "Experimental Bluetooth pairing service is unavailable"
                    emitOperationalStatus()
                }
            }

            override fun onConnectionStateChange(
                device: BluetoothDevice,
                status: Int,
                newState: Int,
            ) {
                if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                    handler.post { inboundAssemblers.remove(device.address) }
                }
            }

            override fun onCharacteristicWriteRequest(
                device: BluetoothDevice,
                requestId: Int,
                characteristic: BluetoothGattCharacteristic,
                preparedWrite: Boolean,
                responseNeeded: Boolean,
                offset: Int,
                value: ByteArray,
            ) {
                handler.post {
                    var accepted =
                        active &&
                            rendezvousReady &&
                            characteristic.uuid == BleRendezvousProtocol.WRITE_CHARACTERISTIC_UUID &&
                            !preparedWrite &&
                            offset == 0
                    if (accepted) {
                        val assembler =
                            inboundAssemblers[device.address]
                                ?: BleRendezvousProtocol
                                    .Assembler()
                                    .takeIf { inboundAssemblers.size < MAX_INBOUND_ASSEMBLERS }
                                    ?.also { inboundAssemblers[device.address] = it }
                        accepted = assembler != null
                        assembler?.accept(value)?.let(::handleInboundInvite)
                    }
                    if (responseNeeded) {
                        try {
                            gattServer?.sendResponse(
                                device,
                                requestId,
                                if (accepted) BluetoothGatt.GATT_SUCCESS else BluetoothGatt.GATT_FAILURE,
                                0,
                                null,
                            )
                        } catch (_: SecurityException) {
                            listener?.onStatus(
                                ProviderStatus(
                                    source,
                                    ProviderAvailability.PermissionRequired,
                                    "Bluetooth permission was revoked",
                                ),
                            )
                        }
                    }
                }
            }

            override fun onCharacteristicReadRequest(
                device: BluetoothDevice,
                requestId: Int,
                offset: Int,
                characteristic: BluetoothGattCharacteristic,
            ) {
                handler.post {
                    val payload =
                        if (active &&
                            rendezvousReady &&
                            characteristic.uuid == BleRendezvousProtocol.IDENTITY_CHARACTERISTIC_UUID
                        ) {
                            BleRendezvousProtocol.encodeIdentity(localIdentity)
                        } else {
                            null
                        }
                    val validOffset = payload != null && offset in 0..payload.size
                    try {
                        gattServer?.sendResponse(
                            device,
                            requestId,
                            if (validOffset) BluetoothGatt.GATT_SUCCESS else BluetoothGatt.GATT_FAILURE,
                            offset,
                            if (validOffset) {
                                payload?.let { value -> value.copyOfRange(offset, value.size) }
                            } else {
                                null
                            },
                        )
                    } catch (_: SecurityException) {
                        listener?.onStatus(
                            ProviderStatus(
                                source,
                                ProviderAvailability.PermissionRequired,
                                "Bluetooth permission was revoked",
                            ),
                        )
                    }
                }
            }
        }

    private val identityGattCallback =
        object : BluetoothGattCallback() {
            override fun onConnectionStateChange(
                gatt: BluetoothGatt,
                status: Int,
                newState: Int,
            ) {
                handler.post {
                    val read = identityRead?.takeIf { it.gatt === gatt }
                    if (read == null) {
                        closeGatt(gatt)
                        return@post
                    }
                    if (status != BluetoothGatt.GATT_SUCCESS || newState == BluetoothProfile.STATE_DISCONNECTED) {
                        completeIdentityRead()
                    } else if (newState == BluetoothProfile.STATE_CONNECTED) {
                        val started =
                            runCatching { gatt.discoverServices() }
                                .getOrDefault(false)
                        if (!started) completeIdentityRead()
                    }
                }
            }

            override fun onServicesDiscovered(
                gatt: BluetoothGatt,
                status: Int,
            ) {
                handler.post {
                    val read = identityRead?.takeIf { it.gatt === gatt }
                    if (read == null) {
                        closeGatt(gatt)
                        return@post
                    }
                    val characteristic =
                        if (status == BluetoothGatt.GATT_SUCCESS) {
                            gatt
                                .getService(BleRendezvousProtocol.SERVICE_UUID)
                                ?.getCharacteristic(BleRendezvousProtocol.IDENTITY_CHARACTERISTIC_UUID)
                        } else {
                            null
                        }
                    val started =
                        characteristic?.let { identityCharacteristic ->
                            runCatching { gatt.readCharacteristic(identityCharacteristic) }
                                .getOrDefault(false)
                        } ?: false
                    if (!started) completeIdentityRead()
                }
            }

            @Suppress("DEPRECATION")
            override fun onCharacteristicRead(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                status: Int,
            ) {
                handleIdentityRead(
                    gatt = gatt,
                    characteristic = characteristic,
                    value = characteristic.value,
                    status = status,
                )
            }

            override fun onCharacteristicRead(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                value: ByteArray,
                status: Int,
            ) {
                handleIdentityRead(gatt, characteristic, value, status)
            }
        }

    private val gattClientCallback =
        object : BluetoothGattCallback() {
            override fun onConnectionStateChange(
                gatt: BluetoothGatt,
                status: Int,
                newState: Int,
            ) {
                handler.post {
                    val offer = outbound
                    if (offer?.gatt !== gatt) {
                        closeGatt(gatt)
                        return@post
                    }
                    if (status != BluetoothGatt.GATT_SUCCESS || newState == BluetoothProfile.STATE_DISCONNECTED) {
                        completeOutbound("Bluetooth connection failed", "connection_failed")
                    } else if (newState == BluetoothProfile.STATE_CONNECTED) {
                        val mtuRequestStarted =
                            try {
                                gatt.requestMtu(REQUESTED_GATT_MTU)
                            } catch (_: SecurityException) {
                                completeOutbound("Bluetooth permission was revoked", "missing_permission")
                                return@post
                            }
                        if (!mtuRequestStarted) discoverServices(gatt)
                    }
                }
            }

            override fun onMtuChanged(
                gatt: BluetoothGatt,
                mtu: Int,
                status: Int,
            ) {
                handler.post {
                    val offer = outbound?.takeIf { it.gatt === gatt } ?: return@post
                    offer.mtu =
                        if (status == BluetoothGatt.GATT_SUCCESS) mtu else DEFAULT_GATT_MTU
                    discoverServices(gatt)
                }
            }

            override fun onServicesDiscovered(
                gatt: BluetoothGatt,
                status: Int,
            ) {
                handler.post {
                    val offer = outbound?.takeIf { it.gatt === gatt } ?: return@post
                    val characteristic =
                        if (status == BluetoothGatt.GATT_SUCCESS) {
                            gatt
                                .getService(BleRendezvousProtocol.SERVICE_UUID)
                                ?.getCharacteristic(BleRendezvousProtocol.WRITE_CHARACTERISTIC_UUID)
                        } else {
                            null
                        }
                    if (characteristic == null) {
                        completeOutbound("The device does not expose Envoix Bluetooth pairing", "service_missing")
                        return@post
                    }
                    val maximumFrameBytes =
                        (offer.mtu - GATT_ATTRIBUTE_OVERHEAD).coerceAtLeast(BleRendezvousProtocol.MIN_GATT_WRITE_BYTES)
                    val frames =
                        BleRendezvousProtocol.encodeInvite(
                            identity = localIdentity,
                            invite = offer.invite,
                            requestId = offer.requestId,
                            maximumFrameBytes = maximumFrameBytes,
                        )
                    if (frames == null) {
                        completeOutbound("The Envoix invitation is invalid or too large", "invalid_invite")
                        return@post
                    }
                    offer.frames = frames
                    offer.characteristic = characteristic
                    writeNextFrame()
                }
            }

            override fun onCharacteristicWrite(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                status: Int,
            ) {
                handler.post {
                    val offer = outbound?.takeIf { it.gatt === gatt } ?: return@post
                    if (status != BluetoothGatt.GATT_SUCCESS) {
                        completeOutbound("Bluetooth invitation delivery failed", "write_failed")
                        return@post
                    }
                    offer.nextFrame += 1
                    writeNextFrame()
                }
            }
        }

    @SuppressLint("MissingPermission")
    override fun start(listener: DiscoveryListener) {
        this.listener = listener
        if (active) {
            emitOperationalStatus()
            return
        }
        listener.onStatus(ProviderStatus(source, ProviderAvailability.Starting, "Starting Bluetooth discovery"))
        if (!context.packageManager.hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE)) {
            listener.onStatus(ProviderStatus(source, ProviderAvailability.Unsupported, "Bluetooth LE is unavailable"))
            return
        }
        if (!DiscoveryPermissions.hasBluetoothPermissions(context)) {
            listener.onStatus(
                ProviderStatus(source, ProviderAvailability.PermissionRequired, "Bluetooth access is required"),
            )
            return
        }

        val manager = context.getSystemService(BluetoothManager::class.java)
        val adapter = manager?.adapter
        if (adapter == null) {
            listener.onStatus(ProviderStatus(source, ProviderAvailability.Unsupported, "Bluetooth is unavailable"))
            return
        }
        if (!adapter.isEnabled) {
            listener.onStatus(ProviderStatus(source, ProviderAvailability.Disabled, "Bluetooth is turned off"))
            return
        }

        active = true
        failureDetail = null
        rendezvousReady = !advertiseEnabled
        if (advertiseEnabled) {
            gattServer = runCatching { manager.openGattServer(context, gattServerCallback) }.getOrNull()
            val pairingService =
                BluetoothGattService(
                    BleRendezvousProtocol.SERVICE_UUID,
                    BluetoothGattService.SERVICE_TYPE_PRIMARY,
                ).apply {
                    addCharacteristic(
                        BluetoothGattCharacteristic(
                            BleRendezvousProtocol.WRITE_CHARACTERISTIC_UUID,
                            BluetoothGattCharacteristic.PROPERTY_WRITE,
                            BluetoothGattCharacteristic.PERMISSION_WRITE,
                        ),
                    )
                    addCharacteristic(
                        BluetoothGattCharacteristic(
                            BleRendezvousProtocol.IDENTITY_CHARACTERISTIC_UUID,
                            BluetoothGattCharacteristic.PROPERTY_READ,
                            BluetoothGattCharacteristic.PERMISSION_READ,
                        ),
                    )
                }
            if (gattServer?.addService(pairingService) != true) {
                failureDetail = "Experimental Bluetooth pairing service could not start"
            }
        }

        adapter.bluetoothLeScanner?.let { scanner ->
            val filter =
                ScanFilter
                    .Builder()
                    .setServiceUuid(
                        ParcelUuid(BleDiscoveryUuid.FILTER_BASE_UUID),
                        ParcelUuid(BleDiscoveryUuid.FILTER_MASK_UUID),
                    ).build()
            val nfcReadinessFilter =
                ScanFilter
                    .Builder()
                    .setServiceUuid(
                        ParcelUuid(NfcReadinessUuid.FILTER_BASE_UUID),
                        ParcelUuid(NfcReadinessUuid.FILTER_MASK_UUID),
                    ).build()
            val settings = ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build()
            try {
                scanner.startScan(listOf(filter, nfcReadinessFilter), settings, scanCallback)
                scanning = true
            } catch (_: SecurityException) {
                releaseGattServerAfterStartFailure()
                listener.onStatus(
                    ProviderStatus(source, ProviderAvailability.PermissionRequired, "Bluetooth access is required"),
                )
                return
            } catch (_: IllegalStateException) {
                releaseGattServerAfterStartFailure()
                listener.onStatus(
                    ProviderStatus(source, ProviderAvailability.TemporarilyUnavailable, "Bluetooth scan could not start"),
                )
                return
            }
        } ?: run { failureDetail = "Bluetooth scanning is unavailable" }

        reconcileAdvertising()
        emitOperationalStatus()
    }

    override fun setNfcReadinessOffer(offerId: String?) {
        val normalized = offerId?.let(NfcReadinessUuid::normalizeOfferId)
        if (nfcReadinessOfferId == normalized) return
        nfcReadinessOfferId = normalized
        if (active) reconcileAdvertising()
    }

    @SuppressLint("MissingPermission")
    private fun rememberDiscoveredDevice(
        peerKey: String,
        device: BluetoothDevice,
    ) {
        val wasTracked = discoveredDevices.remove(peerKey) != null
        if (!wasTracked && discoveredDevices.size >= MAX_TRACKED_DEVICES) {
            val evictedPeerKey =
                discoveredDevices.keys.firstOrNull { candidate ->
                    candidate != identityRead?.peerKey && candidate != outbound?.peerKey
                }
            if (evictedPeerKey != null) {
                discoveredDevices.remove(evictedPeerKey)
                pendingIdentityReads.remove(evictedPeerKey)
                resolvedIdentityNames.remove(evictedPeerKey)
            }
        }
        discoveredDevices[peerKey] = device
    }

    @SuppressLint("MissingPermission")
    private fun enqueueIdentityRead(
        peerKey: String,
        device: BluetoothDevice,
        rssi: Int?,
    ) {
        identityRead?.takeIf { it.peerKey == peerKey }?.let {
            it.rssi = rssi
            return
        }
        pendingIdentityReads[peerKey]?.let {
            it.rssi = rssi
            return
        }
        if (pendingIdentityReads.size >= MAX_PENDING_IDENTITY_READS) {
            return
        }
        pendingIdentityReads[peerKey] = IdentityReadRequest(peerKey, device, rssi)
        startNextIdentityRead()
    }

    @SuppressLint("MissingPermission")
    private fun startNextIdentityRead() {
        if (!active || outbound != null || identityRead != null) return
        var request: IdentityReadRequest
        do {
            request =
                pendingIdentityReads.entries.firstOrNull()?.let { (peerKey, request) ->
                    pendingIdentityReads.remove(peerKey)
                    request
                } ?: return
        } while (!identityReadLimiter.tryAcquire(request.peerKey, SystemClock.elapsedRealtime()))
        val read = IdentityRead(request.peerKey, request.rssi)
        identityRead = read
        val gatt =
            runCatching {
                request.device.connectGatt(
                    context,
                    false,
                    identityGattCallback,
                    BluetoothDevice.TRANSPORT_LE,
                )
            }.getOrNull()
        if (gatt == null) {
            identityRead = null
            startNextIdentityRead()
            return
        }
        read.gatt = gatt
        handler.postDelayed(identityReadTimeout, IDENTITY_READ_TIMEOUT_MS)
    }

    @Suppress("DEPRECATION")
    private fun handleIdentityRead(
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
        value: ByteArray?,
        status: Int,
    ) {
        handler.post {
            val read = identityRead?.takeIf { it.gatt === gatt }
            if (read == null) {
                closeGatt(gatt)
                return@post
            }
            val identity =
                if (status == BluetoothGatt.GATT_SUCCESS &&
                    characteristic.uuid == BleRendezvousProtocol.IDENTITY_CHARACTERISTIC_UUID
                ) {
                    value?.let(BleRendezvousProtocol::decodeIdentity)
                } else {
                    null
                }
            if (identity?.peerKey == read.peerKey) {
                resolvedIdentityNames[read.peerKey] = identity.displayName
                listener?.onObservation(
                    DiscoveryObservation(
                        peerKey = read.peerKey,
                        source = source,
                        seenAtMs = SystemClock.elapsedRealtime(),
                        displayName = identity.displayName,
                        rssi = read.rssi,
                    ),
                )
            }
            completeIdentityRead()
        }
    }

    @SuppressLint("MissingPermission")
    private fun completeIdentityRead() {
        val read = identityRead ?: return
        identityRead = null
        handler.removeCallbacks(identityReadTimeout)
        read.gatt?.let(::closeGatt)
        startNextIdentityRead()
    }

    @SuppressLint("MissingPermission")
    private fun clearIdentityReads() {
        handler.removeCallbacks(identityReadTimeout)
        val read = identityRead
        identityRead = null
        read?.gatt?.let(::closeGatt)
        pendingIdentityReads.clear()
        resolvedIdentityNames.clear()
    }

    @SuppressLint("MissingPermission")
    override fun offerInvite(
        selection: NearbyPairingSelection,
        invite: String,
        completion: (String?) -> Unit,
    ) {
        if (!BleRendezvousProtocol.supportsBluetoothVerificationOffer(invite)) {
            completion("Bluetooth accepts only a public device-verification offer")
            return
        }
        val normalizedPeerKey = DiscoveryPeerRegistry.normalizePeerKey(selection.discoveryPeerKey)
        val device = normalizedPeerKey?.let(discoveredDevices::get)
        if (!active || !scanning) {
            completion("Experimental Bluetooth pairing is not ready")
            return
        }
        if (device == null) {
            completion("The selected device is no longer available over Bluetooth")
            return
        }
        if (outbound != null) {
            completion("Another Bluetooth invitation is already being delivered")
            return
        }
        val requestId = Random.nextLong()
        val offer = OutboundOffer(normalizedPeerKey, invite, requestId, completion)
        outbound = offer
        handler.removeCallbacks(identityReadTimeout)
        val currentIdentityRead = identityRead
        identityRead = null
        currentIdentityRead?.gatt?.let(::closeGatt)
        pendingIdentityReads.clear()
        val requestIdText = requestId.toULong().toString(16).padStart(16, '0')
        OpLog.add("BLE_RENDEZVOUS direction=outbound state=connecting request_id=$requestIdText verification=pending payload=public")
        val gatt =
            runCatching {
                device.connectGatt(context, false, gattClientCallback, BluetoothDevice.TRANSPORT_LE)
            }.getOrNull()
        if (gatt == null) {
            completeOutbound("Bluetooth connection could not start", "connection_start_failed")
            return
        }
        offer.gatt = gatt
        handler.postDelayed(outboundTimeout, OUTBOUND_TIMEOUT_MS)
    }

    @SuppressLint("MissingPermission")
    override fun stop() {
        if (!active) {
            clearIdentityReads()
            listener = null
            return
        }
        active = false
        completeOutbound("Bluetooth discovery stopped", "stopped")
        clearIdentityReads()
        val adapter = context.getSystemService(BluetoothManager::class.java)?.adapter
        runCatching { if (scanning) adapter?.bluetoothLeScanner?.stopScan(scanCallback) }
        stopAdvertising(adapter)
        runCatching { gattServer?.clearServices() }
        runCatching { gattServer?.close() }
        gattServer = null
        scanning = false
        rendezvousReady = false
        failureDetail = null
        discoveredDevices.clear()
        inboundAssemblers.clear()
        listener?.onStatus(ProviderStatus(source, ProviderAvailability.Stopped, "Bluetooth discovery stopped"))
        listener = null
    }

    private fun handleInboundInvite(invite: BleRendezvousInvite) {
        if (!active ||
            invite.senderPeerKey == localIdentity.peerKey ||
            !BleRendezvousProtocol.supportsBluetoothVerificationOffer(invite.invite)
        ) {
            return
        }
        OpLog.add(
            "BLE_RENDEZVOUS direction=inbound state=received request_id=${invite.requestId} verification=pending payload=public",
        )
        listener?.onRendezvousOffer(
            NearbyRendezvousOffer(
                requestId = invite.requestId,
                senderPeerKey = invite.senderPeerKey,
                senderDisplayName = invite.senderDisplayName,
                invite = invite.invite,
            ),
        )
    }

    private fun releaseGattServerAfterStartFailure() {
        active = false
        try {
            gattServer?.clearServices()
        } catch (_: SecurityException) {
            // Permission may be revoked while the GATT server is starting.
        }
        try {
            gattServer?.close()
        } catch (_: SecurityException) {
            // The server reference can still be released safely below.
        }
        gattServer = null
        rendezvousReady = false
    }

    @SuppressLint("MissingPermission")
    private fun reconcileAdvertising() {
        if (!active) return
        val target =
            when {
                nfcReadinessOfferId != null -> AdvertisingKind.NfcReadiness
                advertiseEnabled -> AdvertisingKind.Presence
                else -> null
            }
        if (advertisedKind == target || pendingAdvertisingKind == target) return

        val adapter = context.getSystemService(BluetoothManager::class.java)?.adapter
        stopAdvertising(adapter)
        if (target == null) {
            emitOperationalStatus()
            return
        }
        val advertiser = adapter?.bluetoothLeAdvertiser
        if (advertiser == null) {
            failureDetail =
                if (target == AdvertisingKind.NfcReadiness) {
                    "NFC readiness advertising is unavailable"
                } else {
                    "Bluetooth advertising is unavailable"
                }
            emitOperationalStatus()
            return
        }
        val serviceUuid =
            when (target) {
                AdvertisingKind.Presence ->
                    ParcelUuid(checkNotNull(BleDiscoveryUuid.encode(localIdentity.peerKey)))
                AdvertisingKind.NfcReadiness ->
                    ParcelUuid(checkNotNull(NfcReadinessUuid.encode(checkNotNull(nfcReadinessOfferId))))
            }
        val settings =
            AdvertiseSettings
                .Builder()
                .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
                .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_MEDIUM)
                .setConnectable(target == AdvertisingKind.Presence)
                .build()
        val data =
            AdvertiseData
                .Builder()
                .setIncludeDeviceName(false)
                .setIncludeTxPowerLevel(false)
                .addServiceUuid(serviceUuid)
                .build()
        val scanResponse =
            if (target == AdvertisingKind.Presence) {
                BleDiscoveryName.encodeServiceData(localIdentity.displayName)?.let { displayName ->
                    AdvertiseData
                        .Builder()
                        .addServiceData(serviceUuid, displayName)
                        .build()
                }
            } else {
                null
            }
        val callback =
            when (target) {
                AdvertisingKind.Presence -> advertiseCallback
                AdvertisingKind.NfcReadiness -> nfcReadinessAdvertiseCallback
            }
        try {
            advertisingPending = true
            pendingAdvertisingKind = target
            if (scanResponse == null) {
                advertiser.startAdvertising(settings, data, callback)
            } else {
                advertiser.startAdvertising(settings, data, scanResponse, callback)
            }
        } catch (_: SecurityException) {
            advertisingPending = false
            pendingAdvertisingKind = null
            failureDetail = "Bluetooth advertising permission is unavailable"
        } catch (_: IllegalStateException) {
            advertisingPending = false
            pendingAdvertisingKind = null
            failureDetail = "Bluetooth advertising could not start"
        }
    }

    @SuppressLint("MissingPermission")
    private fun stopAdvertising(adapter: android.bluetooth.BluetoothAdapter?) {
        val advertiser = adapter?.bluetoothLeAdvertiser
        val callbacks =
            buildSet {
                advertisedKind?.let(::add)
                pendingAdvertisingKind?.let(::add)
            }
        callbacks.forEach { kind ->
            runCatching {
                advertiser?.stopAdvertising(
                    when (kind) {
                        AdvertisingKind.Presence -> advertiseCallback
                        AdvertisingKind.NfcReadiness -> nfcReadinessAdvertiseCallback
                    },
                )
            }
        }
        advertising = false
        advertisingPending = false
        advertisedKind = null
        pendingAdvertisingKind = null
    }

    @SuppressLint("MissingPermission")
    private fun discoverServices(gatt: BluetoothGatt) {
        if (outbound?.gatt !== gatt || !gatt.discoverServices()) {
            completeOutbound("Envoix Bluetooth service discovery could not start", "service_discovery_failed")
        }
    }

    @Suppress("DEPRECATION")
    @SuppressLint("MissingPermission")
    private fun writeNextFrame() {
        val offer = outbound ?: return
        if (offer.nextFrame >= offer.frames.size) {
            completeOutbound(null, "delivered")
            return
        }
        val gatt = offer.gatt ?: return
        val characteristic = offer.characteristic ?: return
        val value = offer.frames[offer.nextFrame]
        val started =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                gatt.writeCharacteristic(
                    characteristic,
                    value,
                    BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT,
                ) == BluetoothStatusCodes.SUCCESS
            } else {
                characteristic.writeType = BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT
                characteristic.value = value
                gatt.writeCharacteristic(characteristic)
            }
        if (!started) completeOutbound("Bluetooth invitation delivery could not start", "write_start_failed")
    }

    @SuppressLint("MissingPermission")
    private fun completeOutbound(
        error: String?,
        state: String,
    ) {
        val offer = outbound ?: return
        outbound = null
        handler.removeCallbacks(outboundTimeout)
        val requestId =
            offer.requestId
                .toULong()
                .toString(16)
                .padStart(16, '0')
        OpLog.add("BLE_RENDEZVOUS direction=outbound state=$state request_id=$requestId verification=pending payload=public")
        offer.gatt?.let(::closeGatt)
        offer.completion(error)
        startNextIdentityRead()
    }

    @SuppressLint("MissingPermission")
    private fun closeGatt(gatt: BluetoothGatt) {
        runCatching { gatt.disconnect() }
        runCatching { gatt.close() }
    }

    private val outboundTimeout =
        Runnable {
            completeOutbound("Bluetooth invitation delivery timed out", "timeout")
        }

    private val identityReadTimeout =
        Runnable {
            completeIdentityRead()
        }

    private fun emitOperationalStatus() {
        if (!active) return
        val status =
            when {
                scanning &&
                    advertising &&
                    advertisedKind == AdvertisingKind.NfcReadiness ->
                    ProviderStatus(
                        source,
                        ProviderAvailability.Ready,
                        "Scanning and announcing short-lived NFC readiness",
                    )
                scanning && advertising && rendezvousReady ->
                    ProviderStatus(
                        source,
                        ProviderAvailability.Ready,
                        "Scanning, visible, and ready for experimental Bluetooth pairing",
                    )
                scanning && (advertisingPending || (advertising && !rendezvousReady)) ->
                    ProviderStatus(source, ProviderAvailability.Starting, "Bluetooth visibility and pairing are starting")
                scanning ->
                    ProviderStatus(
                        source,
                        ProviderAvailability.Degraded,
                        failureDetail ?: "Scanning only; Bluetooth visibility or pairing is unavailable",
                    )
                advertising ->
                    ProviderStatus(
                        source,
                        ProviderAvailability.Degraded,
                        failureDetail ?: "Visible only; Bluetooth scanning or pairing is unavailable",
                    )
                else ->
                    ProviderStatus(
                        source,
                        ProviderAvailability.TemporarilyUnavailable,
                        failureDetail ?: "Bluetooth discovery is unavailable",
                    )
            }
        listener?.onStatus(status)
    }

    companion object {
        private const val DEFAULT_GATT_MTU = 23
        private const val REQUESTED_GATT_MTU = 517
        private const val GATT_ATTRIBUTE_OVERHEAD = 3
        private const val OUTBOUND_TIMEOUT_MS = 15_000L
        private const val IDENTITY_READ_TIMEOUT_MS = 5_000L
        private const val MAX_PENDING_IDENTITY_READS = 4
        private const val MAX_TRACKED_DEVICES = 64
        private const val MAX_INBOUND_ASSEMBLERS = 16
    }
}

internal class IdentityReadAttemptLimiter(
    private val maxAttempts: Int = DEFAULT_MAX_ATTEMPTS,
    private val windowMs: Long = DEFAULT_WINDOW_MS,
    private val peerBackoffMs: Long = DEFAULT_PEER_BACKOFF_MS,
) {
    private data class Attempt(
        val peerKey: String,
        val startedAtMs: Long,
    )

    private val attempts = ArrayDeque<Attempt>()

    init {
        require(maxAttempts > 0) { "maxAttempts must be positive" }
        require(windowMs > 0) { "windowMs must be positive" }
        require(peerBackoffMs in 1..windowMs) { "peerBackoffMs must be positive and no greater than windowMs" }
    }

    fun tryAcquire(
        peerKey: String,
        nowMs: Long,
    ): Boolean {
        require(nowMs >= 0) { "nowMs must not be negative" }
        prune(nowMs)
        if (attempts.size >= maxAttempts) return false
        if (attempts.any { it.peerKey == peerKey && nowMs - it.startedAtMs < peerBackoffMs }) return false
        attempts.addLast(Attempt(peerKey, nowMs))
        return true
    }

    private fun prune(nowMs: Long) {
        while (attempts.isNotEmpty() && nowMs - attempts.first.startedAtMs >= windowMs) {
            attempts.removeFirst()
        }
    }

    private companion object {
        const val DEFAULT_MAX_ATTEMPTS = 16
        const val DEFAULT_WINDOW_MS = 30_000L
        const val DEFAULT_PEER_BACKOFF_MS = 5_000L
    }
}
