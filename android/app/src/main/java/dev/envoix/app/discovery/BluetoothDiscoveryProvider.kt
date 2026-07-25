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
import kotlin.random.Random

internal class BluetoothDiscoveryProvider(
    private val context: Context,
    private val localIdentity: LocalDiscoveryIdentity,
    private val advertiseEnabled: Boolean = true,
) : DiscoveryProvider,
    NearbyRendezvousProvider {
    override val source = DiscoverySource.Bluetooth

    private val handler = Handler(Looper.getMainLooper())
    private val discoveredDevices = mutableMapOf<String, BluetoothDevice>()
    private val inboundAssemblers = mutableMapOf<String, BleRendezvousProtocol.Assembler>()
    private var listener: DiscoveryListener? = null
    private var active = false
    private var scanning = false
    private var advertising = false
    private var advertisingPending = false
    private var rendezvousReady = false
    private var failureDetail: String? = null
    private var gattServer: BluetoothGattServer? = null
    private var outbound: OutboundOffer? = null

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

    private val scanCallback =
        object : ScanCallback() {
            override fun onScanResult(
                callbackType: Int,
                result: ScanResult,
            ) {
                val peerKey =
                    result.scanRecord
                        ?.serviceUuids
                        ?.asSequence()
                        ?.mapNotNull { parcelUuid -> BleDiscoveryUuid.decode(parcelUuid.uuid) }
                        ?.firstOrNull()
                        ?: return
                if (!active || peerKey == localIdentity.peerKey) return
                discoveredDevices[peerKey] = result.device
                listener?.onObservation(
                    DiscoveryObservation(
                        peerKey = peerKey,
                        source = source,
                        seenAtMs = SystemClock.elapsedRealtime(),
                        rssi = result.rssi,
                    ),
                )
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
                if (!active) return
                advertisingPending = false
                advertising = true
                emitOperationalStatus()
            }

            override fun onStartFailure(errorCode: Int) {
                if (!active) return
                advertisingPending = false
                advertising = false
                failureDetail = "Bluetooth advertising failed (code $errorCode)"
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
                    val accepted =
                        active &&
                            rendezvousReady &&
                            characteristic.uuid == BleRendezvousProtocol.WRITE_CHARACTERISTIC_UUID &&
                            !preparedWrite &&
                            offset == 0
                    if (accepted) {
                        val assembler = inboundAssemblers.getOrPut(device.address) { BleRendezvousProtocol.Assembler() }
                        assembler.accept(value)?.let(::handleInboundInvite)
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
                    outbound?.takeIf { it.gatt === gatt }?.mtu =
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
            val settings = ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build()
            try {
                scanner.startScan(listOf(filter), settings, scanCallback)
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

        val serviceUuid = ParcelUuid(checkNotNull(BleDiscoveryUuid.encode(localIdentity.peerKey)))
        adapter.bluetoothLeAdvertiser?.takeIf { advertiseEnabled }?.let { advertiser ->
            val settings =
                AdvertiseSettings
                    .Builder()
                    .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
                    .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_MEDIUM)
                    .setConnectable(true)
                    .build()
            val data =
                AdvertiseData
                    .Builder()
                    .setIncludeDeviceName(false)
                    .setIncludeTxPowerLevel(false)
                    .addServiceUuid(serviceUuid)
                    .build()
            try {
                advertisingPending = true
                advertiser.startAdvertising(settings, data, advertiseCallback)
            } catch (_: SecurityException) {
                advertisingPending = false
                failureDetail = "Bluetooth advertising permission is unavailable"
            } catch (_: IllegalStateException) {
                advertisingPending = false
                failureDetail = "Bluetooth advertising could not start"
            }
        } ?: run {
            if (advertiseEnabled) failureDetail = "Bluetooth advertising is unavailable"
        }
        emitOperationalStatus()
    }

    @SuppressLint("MissingPermission")
    override fun offerInvite(
        peerKey: String,
        invite: String,
        completion: (String?) -> Unit,
    ) {
        val normalizedPeerKey = DiscoveryPeerRegistry.normalizePeerKey(peerKey)
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
        val requestIdText = requestId.toULong().toString(16).padStart(16, '0')
        OpLog.add("BLE_RENDEZVOUS direction=outbound state=connecting request_id=$requestIdText auth=none")
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
            listener = null
            return
        }
        active = false
        completeOutbound("Bluetooth discovery stopped", "stopped")
        val adapter = context.getSystemService(BluetoothManager::class.java)?.adapter
        runCatching { if (scanning) adapter?.bluetoothLeScanner?.stopScan(scanCallback) }
        runCatching { if (advertising || advertisingPending) adapter?.bluetoothLeAdvertiser?.stopAdvertising(advertiseCallback) }
        runCatching { gattServer?.clearServices() }
        runCatching { gattServer?.close() }
        gattServer = null
        scanning = false
        advertising = false
        advertisingPending = false
        rendezvousReady = false
        failureDetail = null
        discoveredDevices.clear()
        inboundAssemblers.clear()
        listener?.onStatus(ProviderStatus(source, ProviderAvailability.Stopped, "Bluetooth discovery stopped"))
        listener = null
    }

    private fun handleInboundInvite(invite: BleRendezvousInvite) {
        if (!active || invite.senderPeerKey == localIdentity.peerKey) return
        OpLog.add(
            "BLE_RENDEZVOUS direction=inbound state=received request_id=${invite.requestId} auth=none",
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
        OpLog.add("BLE_RENDEZVOUS direction=outbound state=$state request_id=$requestId auth=none")
        offer.completion(error)
        offer.gatt?.let(::closeGatt)
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

    private fun emitOperationalStatus() {
        if (!active) return
        val status =
            when {
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
    }
}
