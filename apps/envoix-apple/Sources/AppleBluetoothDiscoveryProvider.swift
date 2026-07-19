#if os(iOS)
import CoreBluetooth
import Foundation
import OSLog

final class AppleBluetoothDiscoveryProvider: NSObject, NearbyDiscoveryProvider, NearbyRendezvousProvider {
    let source = NearbyDiscoverySource.bluetooth

    private final class OutboundOffer {
        let peerKey: String
        let invite: String
        let requestID: UInt64
        let peripheral: CBPeripheral
        let completion: (String?) -> Void
        var frames: [Data] = []
        var nextFrame = 0
        var characteristic: CBCharacteristic?

        init(
            peerKey: String,
            invite: String,
            requestID: UInt64,
            peripheral: CBPeripheral,
            completion: @escaping (String?) -> Void
        ) {
            self.peerKey = peerKey
            self.invite = invite
            self.requestID = requestID
            self.peripheral = peripheral
            self.completion = completion
        }
    }

    private let identity: LocalNearbyDiscoveryIdentity
    private let logger = Logger(
        subsystem: Bundle.main.bundleIdentifier ?? "com.envoix.app",
        category: "ble-rendezvous"
    )
    private var sink: ((NearbyDiscoveryEvent) -> Void)?
    private var centralManager: CBCentralManager?
    private var peripheralManager: CBPeripheralManager?
    private var writeCharacteristic: CBMutableCharacteristic?
    private var discoveredPeripherals: [String: CBPeripheral] = [:]
    private var inboundAssemblers: [UUID: BleRendezvousProtocol.Assembler] = [:]
    private var outbound: OutboundOffer?
    private var outboundTimeout: DispatchWorkItem?
    private var active = false
    private var scanning = false
    private var advertising = false
    private var advertisingPending = false
    private var rendezvousReady = false
    private var failureDetail: NearbyProviderDetail?

    init(identity: LocalNearbyDiscoveryIdentity) {
        self.identity = identity
    }

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {
        self.sink = sink
        guard !active else {
            emitOperationalStatus()
            return
        }

        emitStatus(.starting, .startingBluetooth)
        switch CBManager.authorization {
        case .denied, .restricted:
            emitStatus(.permissionRequired, .bluetoothAccessRequired)
            return
        case .notDetermined, .allowedAlways:
            break
        @unknown default:
            emitStatus(.temporarilyUnavailable, .bluetoothUnavailable)
            return
        }

        active = true
        failureDetail = nil
        centralManager = CBCentralManager(
            delegate: self,
            queue: .main,
            options: [CBCentralManagerOptionShowPowerAlertKey: false]
        )
        peripheralManager = CBPeripheralManager(
            delegate: self,
            queue: .main,
            options: [CBPeripheralManagerOptionShowPowerAlertKey: false]
        )
    }

    func stop() {
        guard active else {
            sink = nil
            return
        }
        active = false
        completeOutbound(error: "Bluetooth discovery stopped", state: "stopped")
        if scanning {
            centralManager?.stopScan()
        }
        if advertising || advertisingPending {
            peripheralManager?.stopAdvertising()
        }
        peripheralManager?.removeAllServices()
        scanning = false
        advertising = false
        advertisingPending = false
        rendezvousReady = false
        discoveredPeripherals.values.forEach { $0.delegate = nil }
        discoveredPeripherals.removeAll()
        inboundAssemblers.removeAll()
        writeCharacteristic = nil
        centralManager?.delegate = nil
        peripheralManager?.delegate = nil
        centralManager = nil
        peripheralManager = nil
        emitStatus(.stopped, .discoveryStopped)
        sink = nil
    }

    func offerInvite(
        peerKey: String,
        invite: String,
        completion: @escaping (String?) -> Void
    ) {
        if !Thread.isMainThread {
            DispatchQueue.main.async { [weak self] in
                self?.offerInvite(peerKey: peerKey, invite: invite, completion: completion)
            }
            return
        }
        guard active, rendezvousReady else {
            completion("Experimental Bluetooth pairing is not ready")
            return
        }
        guard let normalizedPeerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(peerKey),
              let peripheral = discoveredPeripherals[normalizedPeerKey] else {
            completion("The selected device is no longer available over Bluetooth")
            return
        }
        guard outbound == nil else {
            completion("Another Bluetooth invitation is already being delivered")
            return
        }
        let requestID = UInt64.random(in: UInt64.min...UInt64.max)
        let offer = OutboundOffer(
            peerKey: normalizedPeerKey,
            invite: invite,
            requestID: requestID,
            peripheral: peripheral,
            completion: completion
        )
        outbound = offer
        peripheral.delegate = self
        logger.info(
            "BLE_RENDEZVOUS direction=outbound state=connecting request_id=\(self.requestIDText(requestID), privacy: .public) auth=none"
        )
        centralManager?.connect(peripheral, options: nil)
        let timeout = DispatchWorkItem { [weak self] in
            self?.completeOutbound(error: "Bluetooth invitation delivery timed out", state: "timeout")
        }
        outboundTimeout = timeout
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.outboundTimeoutSeconds, execute: timeout)
    }

    private func startScanningIfPossible() {
        guard active, !scanning, centralManager?.state == .poweredOn else { return }
        centralManager?.scanForPeripherals(
            withServices: nil,
            options: [CBCentralManagerScanOptionAllowDuplicatesKey: true]
        )
        scanning = true
    }

    private func addRendezvousServiceIfPossible() {
        guard active,
              !rendezvousReady,
              writeCharacteristic == nil,
              peripheralManager?.state == .poweredOn else {
            return
        }
        let characteristic = CBMutableCharacteristic(
            type: CBUUID(nsuuid: BleRendezvousProtocol.writeCharacteristicUUID),
            properties: [.write],
            value: nil,
            permissions: [.writeable]
        )
        let service = CBMutableService(
            type: CBUUID(nsuuid: BleRendezvousProtocol.serviceUUID),
            primary: true
        )
        service.characteristics = [characteristic]
        writeCharacteristic = characteristic
        peripheralManager?.add(service)
    }

    private func startAdvertisingIfPossible() {
        guard active,
              rendezvousReady,
              !advertising,
              !advertisingPending,
              peripheralManager?.state == .poweredOn,
              let uuid = NearbyDiscoveryBluetoothUUID.encode(peerKey: identity.peerKey) else {
            return
        }
        advertisingPending = true
        peripheralManager?.startAdvertising([
            CBAdvertisementDataServiceUUIDsKey: [CBUUID(nsuuid: uuid)],
        ])
    }

    private func handleInboundInvite(_ invite: BleRendezvousInvite) {
        guard active, invite.senderPeerKey != identity.peerKey else { return }
        logger.info(
            "BLE_RENDEZVOUS direction=inbound state=received request_id=\(invite.requestID, privacy: .public) auth=none"
        )
        sink?(.rendezvousOffer(NearbyRendezvousOffer(
            requestID: invite.requestID,
            senderPeerKey: invite.senderPeerKey,
            senderDisplayName: invite.senderDisplayName,
            invite: invite.invite
        )))
    }

    private func writeNextFrame() {
        guard let offer = outbound, let characteristic = offer.characteristic else { return }
        guard offer.nextFrame < offer.frames.count else {
            completeOutbound(error: nil, state: "delivered")
            return
        }
        offer.peripheral.writeValue(
            offer.frames[offer.nextFrame],
            for: characteristic,
            type: .withResponse
        )
    }

    private func completeOutbound(error: String?, state: String) {
        guard let offer = outbound else { return }
        outbound = nil
        outboundTimeout?.cancel()
        outboundTimeout = nil
        logger.info(
            "BLE_RENDEZVOUS direction=outbound state=\(state, privacy: .public) request_id=\(self.requestIDText(offer.requestID), privacy: .public) auth=none"
        )
        offer.completion(error)
        centralManager?.cancelPeripheralConnection(offer.peripheral)
        offer.peripheral.delegate = nil
    }

    private func emitOperationalStatus() {
        guard active else { return }

        let states = [centralManager?.state, peripheralManager?.state].compactMap { $0 }
        if CBManager.authorization == .denied || CBManager.authorization == .restricted
            || states.contains(.unauthorized) {
            emitStatus(.permissionRequired, .bluetoothAccessRequired)
        } else if states.contains(.unsupported) {
            emitStatus(.unsupported, .bluetoothUnavailable)
        } else if states.contains(.poweredOff) {
            emitStatus(.disabled, .bluetoothOff)
        } else if scanning && advertising && rendezvousReady {
            emitStatus(.ready, .bluetoothReady)
        } else if scanning && (advertisingPending || (advertising && !rendezvousReady)) {
            emitStatus(.starting, .bluetoothVisibilityStarting)
        } else if scanning {
            emitStatus(.degraded, failureDetail ?? .bluetoothScanningOnly)
        } else if advertising {
            emitStatus(.degraded, failureDetail ?? .bluetoothVisibleOnly)
        } else if states.contains(.unknown) || states.contains(.resetting) || states.count < 2 {
            emitStatus(.starting, .startingBluetooth)
        } else {
            emitStatus(.temporarilyUnavailable, failureDetail ?? .bluetoothUnavailable)
        }
    }

    private func emitStatus(_ availability: NearbyProviderAvailability, _ detail: NearbyProviderDetail) {
        sink?(.status(NearbyProviderStatus(source: source, availability: availability, detail: detail)))
    }

    private func requestIDText(_ requestID: UInt64) -> String {
        String(format: "%016llx", requestID)
    }

    private static let outboundTimeoutSeconds: TimeInterval = 15
}

extension AppleBluetoothDiscoveryProvider: CBCentralManagerDelegate {
    func centralManagerDidUpdateState(_ central: CBCentralManager) {
        guard active else { return }
        if central.state == .poweredOn {
            startScanningIfPossible()
        } else {
            scanning = false
        }
        emitOperationalStatus()
    }

    func centralManager(
        _ central: CBCentralManager,
        didDiscover peripheral: CBPeripheral,
        advertisementData: [String: Any],
        rssi RSSI: NSNumber
    ) {
        guard active else { return }
        let serviceUUIDs = [
            CBAdvertisementDataServiceUUIDsKey,
            CBAdvertisementDataOverflowServiceUUIDsKey,
        ].flatMap { key in
            advertisementData[key] as? [CBUUID] ?? []
        }
        guard let peerKey = serviceUUIDs.lazy.compactMap({ serviceUUID in
            UUID(uuidString: serviceUUID.uuidString).flatMap { uuid in
                NearbyDiscoveryBluetoothUUID.decode(uuid)
            }
        }).first,
        peerKey != identity.peerKey else {
            return
        }
        discoveredPeripherals[peerKey] = peripheral
        let rssi = RSSI.intValue == 127 ? nil : RSSI.intValue
        sink?(.observation(NearbyDiscoveryObservation(
            peerKey: peerKey,
            source: source,
            seenAtMilliseconds: Int64(ProcessInfo.processInfo.systemUptime * 1_000),
            rssi: rssi
        )))
    }

    func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        guard outbound?.peripheral == peripheral else {
            central.cancelPeripheralConnection(peripheral)
            return
        }
        peripheral.discoverServices([CBUUID(nsuuid: BleRendezvousProtocol.serviceUUID)])
    }

    func centralManager(
        _ central: CBCentralManager,
        didFailToConnect peripheral: CBPeripheral,
        error: Error?
    ) {
        guard outbound?.peripheral == peripheral else { return }
        completeOutbound(error: "Bluetooth connection failed", state: "connection_failed")
    }

    func centralManager(
        _ central: CBCentralManager,
        didDisconnectPeripheral peripheral: CBPeripheral,
        error: Error?
    ) {
        guard outbound?.peripheral == peripheral else { return }
        completeOutbound(error: "Bluetooth connection ended before delivery", state: "disconnected")
    }
}

extension AppleBluetoothDiscoveryProvider: CBPeripheralDelegate {
    func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        guard let offer = outbound, offer.peripheral == peripheral, error == nil,
              let service = peripheral.services?.first(where: {
                  $0.uuid == CBUUID(nsuuid: BleRendezvousProtocol.serviceUUID)
              }) else {
            completeOutbound(error: "The device does not expose Envoix Bluetooth pairing", state: "service_missing")
            return
        }
        peripheral.discoverCharacteristics(
            [CBUUID(nsuuid: BleRendezvousProtocol.writeCharacteristicUUID)],
            for: service
        )
    }

    func peripheral(
        _ peripheral: CBPeripheral,
        didDiscoverCharacteristicsFor service: CBService,
        error: Error?
    ) {
        guard let offer = outbound, offer.peripheral == peripheral, error == nil,
              let characteristic = service.characteristics?.first(where: {
                  $0.uuid == CBUUID(nsuuid: BleRendezvousProtocol.writeCharacteristicUUID)
              }) else {
            completeOutbound(error: "The Envoix Bluetooth write channel is unavailable", state: "characteristic_missing")
            return
        }
        let maximumFrameBytes = max(
            peripheral.maximumWriteValueLength(for: .withResponse),
            BleRendezvousProtocol.minimumGATTWriteBytes
        )
        guard let frames = BleRendezvousProtocol.encodeInvite(
            identity: identity,
            invite: offer.invite,
            requestID: offer.requestID,
            maximumFrameBytes: maximumFrameBytes
        ) else {
            completeOutbound(error: "The Envoix invitation is invalid or too large", state: "invalid_invite")
            return
        }
        offer.frames = frames
        offer.characteristic = characteristic
        writeNextFrame()
    }

    func peripheral(
        _ peripheral: CBPeripheral,
        didWriteValueFor characteristic: CBCharacteristic,
        error: Error?
    ) {
        guard let offer = outbound, offer.peripheral == peripheral else { return }
        guard error == nil else {
            completeOutbound(error: "Bluetooth invitation delivery failed", state: "write_failed")
            return
        }
        offer.nextFrame += 1
        writeNextFrame()
    }
}

extension AppleBluetoothDiscoveryProvider: CBPeripheralManagerDelegate {
    func peripheralManagerDidUpdateState(_ peripheral: CBPeripheralManager) {
        guard active else { return }
        if peripheral.state == .poweredOn {
            addRendezvousServiceIfPossible()
        } else {
            advertising = false
            advertisingPending = false
            rendezvousReady = false
            writeCharacteristic = nil
        }
        emitOperationalStatus()
    }

    func peripheralManager(_ peripheral: CBPeripheralManager, didAdd service: CBService, error: Error?) {
        guard active, service.uuid == CBUUID(nsuuid: BleRendezvousProtocol.serviceUUID) else { return }
        rendezvousReady = error == nil
        if error != nil {
            failureDetail = .bluetoothUnavailable
            writeCharacteristic = nil
        } else {
            startAdvertisingIfPossible()
        }
        emitOperationalStatus()
    }

    func peripheralManagerDidStartAdvertising(_ peripheral: CBPeripheralManager, error: Error?) {
        guard active else { return }
        advertisingPending = false
        advertising = error == nil
        emitOperationalStatus()
    }

    func peripheralManager(
        _ peripheral: CBPeripheralManager,
        didReceiveWrite requests: [CBATTRequest]
    ) {
        guard active else { return }
        for request in requests {
            guard request.characteristic.uuid == CBUUID(nsuuid: BleRendezvousProtocol.writeCharacteristicUUID),
                  request.offset == 0,
                  let value = request.value else {
                peripheral.respond(to: request, withResult: .requestNotSupported)
                continue
            }
            let assembler = inboundAssemblers[request.central.identifier] ?? BleRendezvousProtocol.Assembler()
            inboundAssemblers[request.central.identifier] = assembler
            if let invite = assembler.accept(value) {
                handleInboundInvite(invite)
            }
            peripheral.respond(to: request, withResult: .success)
        }
    }
}
#endif
