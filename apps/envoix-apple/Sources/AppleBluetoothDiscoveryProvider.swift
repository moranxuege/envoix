#if os(iOS) || os(macOS)
import CoreBluetooth
import Foundation
import OSLog

final class AppleBluetoothIdentityReadAttemptLimiter {
    private struct Attempt {
        let peerKey: String
        let startedAtMilliseconds: Int64
    }

    private let maximumAttempts: Int
    private let windowMilliseconds: Int64
    private let peerBackoffMilliseconds: Int64
    private var attempts: [Attempt] = []

    init(
        maximumAttempts: Int = 16,
        windowMilliseconds: Int64 = 30_000,
        peerBackoffMilliseconds: Int64 = 5_000
    ) {
        precondition(maximumAttempts > 0, "maximum attempts must be positive")
        precondition(windowMilliseconds > 0, "attempt window must be positive")
        precondition(
            (1...windowMilliseconds).contains(peerBackoffMilliseconds),
            "peer backoff must be positive and no greater than the attempt window"
        )
        self.maximumAttempts = maximumAttempts
        self.windowMilliseconds = windowMilliseconds
        self.peerBackoffMilliseconds = peerBackoffMilliseconds
    }

    func tryAcquire(peerKey: String, nowMilliseconds: Int64) -> Bool {
        precondition(nowMilliseconds >= 0, "attempt time must not be negative")
        attempts.removeAll {
            nowMilliseconds - $0.startedAtMilliseconds >= windowMilliseconds
        }
        guard attempts.count < maximumAttempts,
              !attempts.contains(where: {
                  $0.peerKey == peerKey
                      && nowMilliseconds - $0.startedAtMilliseconds < peerBackoffMilliseconds
              }) else {
            return false
        }
        attempts.append(Attempt(
            peerKey: peerKey,
            startedAtMilliseconds: nowMilliseconds
        ))
        return true
    }
}

final class AppleBluetoothDiscoveryProvider: NSObject, NearbyRendezvousProvider,
    NearbyAdvertisingConfigurable, NearbyIdentityConfigurable {
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
        var connectionStarted = false
        var waitingForCleanDisconnect = false

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

    private final class IdentityRead {
        let peerKey: String
        let peripheral: CBPeripheral
        var waitingForCleanDisconnect = false

        init(peerKey: String, peripheral: CBPeripheral) {
            self.peerKey = peerKey
            self.peripheral = peripheral
        }
    }

    private var identity: LocalNearbyDiscoveryIdentity
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
    private var inboundAssemblerOrder: [UUID] = []
    private var outbound: OutboundOffer?
    private var outboundTimeout: DispatchWorkItem?
    private var pendingIdentityReads: [IdentityRead] = []
    private var activeIdentityRead: IdentityRead?
    private var identityReadTimeout: DispatchWorkItem?
    private let identityReadAttemptLimiter = AppleBluetoothIdentityReadAttemptLimiter()
    private var nfcReadinessIdentities = NearbyNFCReadinessIdentityRegistry()
    private var resolvedDisplayNames: [String: String] = [:]
    private var lastRSSIByPeerKey: [String: Int] = [:]
    private var trackedPeerOrder: [String] = []
    private var active = false
    private var scanning = false
    private var advertising = false
    private var advertisingPending = false
    private var rendezvousReady = false
    private var failureDetail: NearbyProviderDetail?
    private var advertisingEnabled = false

    init(identity: LocalNearbyDiscoveryIdentity) {
        self.identity = identity
    }

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {
        if !Thread.isMainThread {
            DispatchQueue.main.sync {
                self.start(sink: sink)
            }
            return
        }
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
        if advertisingEnabled {
            peripheralManager = CBPeripheralManager(
                delegate: self,
                queue: .main,
                options: [CBPeripheralManagerOptionShowPowerAlertKey: false]
            )
        }
    }

    func stop() {
        if !Thread.isMainThread {
            DispatchQueue.main.sync {
                self.stop()
            }
            return
        }
        guard active else {
            sink = nil
            return
        }
        active = false
        completeOutbound(error: "Bluetooth discovery stopped", state: "stopped")
        cancelIdentityReads(clearCache: true)
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
        nfcReadinessIdentities.clear()
        trackedPeerOrder.removeAll()
        inboundAssemblers.removeAll()
        inboundAssemblerOrder.removeAll()
        writeCharacteristic = nil
        centralManager?.delegate = nil
        peripheralManager?.delegate = nil
        centralManager = nil
        peripheralManager = nil
        emitStatus(.stopped, .discoveryStopped)
        sink = nil
    }

    func offerInvite(
        to selection: NearbyPairingSelection,
        invite: String,
        completion: @escaping (String?) -> Void
    ) {
        if !Thread.isMainThread {
            DispatchQueue.main.async { [weak self] in
                self?.offerInvite(to: selection, invite: invite, completion: completion)
            }
            return
        }
        guard BleRendezvousProtocol.isSupportedBluetoothVerificationOffer(invite) else {
            completion("Bluetooth accepts only a public device-verification offer")
            return
        }
        guard active, centralManager?.state == .poweredOn else {
            completion("Experimental Bluetooth pairing is not ready")
            return
        }
        guard let normalizedPeerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(
                  selection.discoveryPeerKey
              ),
              let peripheral = discoveredPeripherals[normalizedPeerKey] else {
            completion("The selected device is no longer available over Bluetooth")
            return
        }
        guard outbound == nil else {
            completion("Another Bluetooth invitation is already being delivered")
            return
        }
        cancelIdentityReads(clearCache: false)
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
            "BLE_RENDEZVOUS direction=outbound state=connecting request_id=\(self.requestIDText(requestID), privacy: .public) verification=pending payload=public"
        )
        let timeout = DispatchWorkItem { [weak self] in
            self?.completeOutbound(error: "Bluetooth invitation delivery timed out", state: "timeout")
        }
        outboundTimeout = timeout
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.outboundTimeoutSeconds, execute: timeout)
        beginOutboundConnection(offer)
    }

    func canOfferInvite(to selection: NearbyPairingSelection) -> Bool {
        if !Thread.isMainThread {
            return DispatchQueue.main.sync {
                self.canOfferInvite(to: selection)
            }
        }
        guard active,
              centralManager?.state == .poweredOn,
              let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(
                  selection.discoveryPeerKey
              ) else {
            return false
        }
        return discoveredPeripherals[peerKey] != nil
    }

    func setAdvertisingEnabled(_ enabled: Bool) {
        if !Thread.isMainThread {
            DispatchQueue.main.sync {
                self.setAdvertisingEnabled(enabled)
            }
            return
        }
        precondition(!active, "Advertising policy must be configured before discovery starts")
        advertisingEnabled = enabled
    }

    func setIdentity(_ identity: LocalNearbyDiscoveryIdentity) {
        if !Thread.isMainThread {
            DispatchQueue.main.sync {
                self.setIdentity(identity)
            }
            return
        }
        precondition(!active, "Identity must be configured before discovery starts")
        self.identity = identity
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
              advertisingEnabled,
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
        guard let identityValue = BleRendezvousProtocol.encodeIdentity(identity: identity) else {
            failureDetail = .bluetoothUnavailable
            emitOperationalStatus()
            return
        }
        let identityCharacteristic = CBMutableCharacteristic(
            type: CBUUID(nsuuid: BleRendezvousProtocol.identityCharacteristicUUID),
            properties: [.read],
            value: identityValue,
            permissions: [.readable]
        )
        let service = CBMutableService(
            type: CBUUID(nsuuid: BleRendezvousProtocol.serviceUUID),
            primary: true
        )
        service.characteristics = [characteristic, identityCharacteristic]
        writeCharacteristic = characteristic
        peripheralManager?.add(service)
    }

    private func startAdvertisingIfPossible() {
        guard active,
              advertisingEnabled,
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
            CBAdvertisementDataLocalNameKey: identity.displayName,
        ])
    }

    private func handleInboundInvite(_ invite: BleRendezvousInvite) {
        guard active,
              invite.senderPeerKey != identity.peerKey,
              BleRendezvousProtocol.isSupportedBluetoothVerificationOffer(invite.invite) else {
            return
        }
        logger.info(
            "BLE_RENDEZVOUS direction=inbound state=received request_id=\(invite.requestID, privacy: .public) verification=pending payload=public"
        )
        sink?(.rendezvousOffer(NearbyRendezvousOffer(
            requestID: invite.requestID,
            senderPeerKey: invite.senderPeerKey,
            senderDisplayName: invite.senderDisplayName,
            source: source,
            senderInboxEndpointID: nil,
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

    private func beginOutboundConnection(_ offer: OutboundOffer) {
        guard outbound === offer else { return }
        switch offer.peripheral.state {
        case .disconnected:
            offer.connectionStarted = true
            offer.waitingForCleanDisconnect = false
            centralManager?.connect(offer.peripheral, options: nil)
        case .connected, .connecting:
            offer.waitingForCleanDisconnect = true
            centralManager?.cancelPeripheralConnection(offer.peripheral)
        case .disconnecting:
            offer.waitingForCleanDisconnect = true
        @unknown default:
            completeOutbound(error: "Bluetooth connection is unavailable", state: "connection_failed")
        }
    }

    private func enqueueIdentityRead(peerKey: String, peripheral: CBPeripheral) {
        guard active,
              outbound == nil,
              resolvedDisplayNames[peerKey] == nil,
              activeIdentityRead?.peerKey != peerKey,
              !pendingIdentityReads.contains(where: { $0.peerKey == peerKey }),
              pendingIdentityReads.count < Self.maximumPendingIdentityReads else {
            return
        }
        pendingIdentityReads.append(IdentityRead(peerKey: peerKey, peripheral: peripheral))
        startNextIdentityRead()
    }

    private func trackPeerIfPossible(_ peerKey: String) -> Bool {
        if discoveredPeripherals[peerKey] != nil {
            return true
        }
        if discoveredPeripherals.count >= Self.maximumTrackedPeers {
            let protectedPeerKeys = Set([
                activeIdentityRead?.peerKey,
                outbound?.peerKey,
            ].compactMap { $0 })
            guard let evictedPeerKey = trackedPeerOrder.first(where: {
                !protectedPeerKeys.contains($0)
            }) else {
                return false
            }
            trackedPeerOrder.removeAll { $0 == evictedPeerKey }
            discoveredPeripherals.removeValue(forKey: evictedPeerKey)
            resolvedDisplayNames.removeValue(forKey: evictedPeerKey)
            lastRSSIByPeerKey.removeValue(forKey: evictedPeerKey)
            pendingIdentityReads.removeAll { $0.peerKey == evictedPeerKey }
        }
        trackedPeerOrder.append(peerKey)
        return true
    }

    private func startNextIdentityRead() {
        guard active,
              outbound == nil,
              activeIdentityRead == nil,
              centralManager?.state == .poweredOn,
              !pendingIdentityReads.isEmpty else {
            return
        }
        var read: IdentityRead
        repeat {
            guard !pendingIdentityReads.isEmpty else { return }
            read = pendingIdentityReads.removeFirst()
        } while !identityReadAttemptLimiter.tryAcquire(
            peerKey: read.peerKey,
            nowMilliseconds: Int64(ProcessInfo.processInfo.systemUptime * 1_000)
        )
        activeIdentityRead = read
        read.peripheral.delegate = self

        let timeout = DispatchWorkItem { [weak self] in
            self?.completeIdentityRead(displayName: nil)
        }
        identityReadTimeout = timeout
        DispatchQueue.main.asyncAfter(
            deadline: .now() + Self.identityReadTimeoutSeconds,
            execute: timeout
        )

        switch read.peripheral.state {
        case .disconnected:
            centralManager?.connect(read.peripheral, options: nil)
        case .connected:
            read.peripheral.discoverServices([CBUUID(nsuuid: BleRendezvousProtocol.serviceUUID)])
        case .connecting:
            break
        case .disconnecting:
            read.waitingForCleanDisconnect = true
        @unknown default:
            completeIdentityRead(displayName: nil)
        }
    }

    private func completeIdentityRead(displayName: String?) {
        guard let read = activeIdentityRead else { return }
        activeIdentityRead = nil
        identityReadTimeout?.cancel()
        identityReadTimeout = nil

        if read.peripheral.state != .disconnected {
            centralManager?.cancelPeripheralConnection(read.peripheral)
        }
        read.peripheral.delegate = nil
        if let displayName {
            resolvedDisplayNames[read.peerKey] = displayName
            sink?(.observation(NearbyDiscoveryObservation(
                peerKey: read.peerKey,
                source: source,
                seenAtMilliseconds: Int64(ProcessInfo.processInfo.systemUptime * 1_000),
                displayName: displayName,
                rssi: lastRSSIByPeerKey[read.peerKey]
            )))
        }
        DispatchQueue.main.async { [weak self] in
            self?.startNextIdentityRead()
        }
    }

    private func cancelIdentityReads(clearCache: Bool) {
        pendingIdentityReads.removeAll()
        identityReadTimeout?.cancel()
        identityReadTimeout = nil
        if let read = activeIdentityRead {
            activeIdentityRead = nil
            if read.peripheral.state != .disconnected {
                centralManager?.cancelPeripheralConnection(read.peripheral)
            }
            read.peripheral.delegate = nil
        }
        if clearCache {
            resolvedDisplayNames.removeAll()
            lastRSSIByPeerKey.removeAll()
        }
    }

    private func completeOutbound(error: String?, state: String) {
        guard let offer = outbound else { return }
        outbound = nil
        outboundTimeout?.cancel()
        outboundTimeout = nil
        centralManager?.cancelPeripheralConnection(offer.peripheral)
        offer.peripheral.delegate = nil
        logger.info(
            "BLE_RENDEZVOUS direction=outbound state=\(state, privacy: .public) request_id=\(self.requestIDText(offer.requestID), privacy: .public) verification=pending payload=public"
        )
        offer.completion(error)
    }

    private func emitOperationalStatus() {
        guard active else { return }

        let states = [centralManager?.state, advertisingEnabled ? peripheralManager?.state : nil]
            .compactMap { $0 }
        if CBManager.authorization == .denied || CBManager.authorization == .restricted
            || states.contains(.unauthorized) {
            emitStatus(.permissionRequired, .bluetoothAccessRequired)
        } else if states.contains(.unsupported) {
            emitStatus(.unsupported, .bluetoothUnavailable)
        } else if states.contains(.poweredOff) {
            emitStatus(.disabled, .bluetoothOff)
        } else if scanning && !advertisingEnabled {
            emitStatus(.ready, .bluetoothScanningOnly)
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
    private static let identityReadTimeoutSeconds: TimeInterval = 5
    private static let maximumPendingIdentityReads = 8
    private static let maximumTrackedPeers = 32
    private static let maximumInboundAssemblers = 8
}

extension AppleBluetoothDiscoveryProvider: CBCentralManagerDelegate {
    func centralManagerDidUpdateState(_ central: CBCentralManager) {
        guard active else { return }
        if central.state == .poweredOn {
            startScanningIfPossible()
            startNextIdentityRead()
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
        let nowMilliseconds = Int64(
            ProcessInfo.processInfo.systemUptime * 1_000
        )
        if let offerID = serviceUUIDs.lazy.compactMap({ serviceUUID in
            UUID(uuidString: serviceUUID.uuidString).flatMap {
                NearbyNFCReadinessBluetoothUUID.decode($0)
            }
        }).first,
           let presenterPeerKey = nfcReadinessIdentities.boundPeerKey(
               for: peripheral.identifier,
               at: nowMilliseconds
           ) {
            sink?(.nfcPresenterReadiness(
                offerID: offerID,
                presenterPeerKey: presenterPeerKey,
                presenterID: peripheral.identifier
            ))
        }
        guard let peerKey = serviceUUIDs.lazy.compactMap({ serviceUUID in
            UUID(uuidString: serviceUUID.uuidString).flatMap { uuid in
                NearbyDiscoveryBluetoothUUID.decode(uuid)
            }
        }).first,
        peerKey != identity.peerKey else {
            return
        }
        guard trackPeerIfPossible(peerKey) else { return }
        discoveredPeripherals[peerKey] = peripheral
        nfcReadinessIdentities.observePresence(
            peerKey: peerKey,
            presenterID: peripheral.identifier,
            at: nowMilliseconds
        )
        let rssi = RSSI.intValue == 127 ? nil : RSSI.intValue
        lastRSSIByPeerKey[peerKey] = rssi
        let presenceUUID = NearbyDiscoveryBluetoothUUID.encode(peerKey: peerKey)
            .map(CBUUID.init(nsuuid:))
        let serviceData = presenceUUID.flatMap {
            (advertisementData[CBAdvertisementDataServiceDataKey] as? [CBUUID: Data])?[$0]
        }
        let provisionalName = BleRendezvousProtocol.decodeProvisionalDisplayName(
            serviceData: serviceData,
            localName: advertisementData[CBAdvertisementDataLocalNameKey] as? String
        )
        sink?(.observation(NearbyDiscoveryObservation(
            peerKey: peerKey,
            source: source,
            seenAtMilliseconds: nowMilliseconds,
            displayName: resolvedDisplayNames[peerKey] ?? provisionalName,
            rssi: rssi
        )))
        enqueueIdentityRead(peerKey: peerKey, peripheral: peripheral)
    }

    func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        if let offer = outbound, offer.peripheral == peripheral {
            guard offer.connectionStarted, !offer.waitingForCleanDisconnect else {
                central.cancelPeripheralConnection(peripheral)
                return
            }
            peripheral.discoverServices([CBUUID(nsuuid: BleRendezvousProtocol.serviceUUID)])
            return
        }
        if activeIdentityRead?.peripheral == peripheral {
            peripheral.discoverServices([CBUUID(nsuuid: BleRendezvousProtocol.serviceUUID)])
        } else {
            central.cancelPeripheralConnection(peripheral)
        }
    }

    func centralManager(
        _ central: CBCentralManager,
        didFailToConnect peripheral: CBPeripheral,
        error: Error?
    ) {
        if let offer = outbound, offer.peripheral == peripheral {
            if offer.waitingForCleanDisconnect {
                beginOutboundConnection(offer)
            } else {
                completeOutbound(error: "Bluetooth connection failed", state: "connection_failed")
            }
        } else if let read = activeIdentityRead, read.peripheral == peripheral {
            if read.waitingForCleanDisconnect {
                read.waitingForCleanDisconnect = false
                central.connect(peripheral, options: nil)
            } else {
                completeIdentityRead(displayName: nil)
            }
        }
    }

    func centralManager(
        _ central: CBCentralManager,
        didDisconnectPeripheral peripheral: CBPeripheral,
        error: Error?
    ) {
        if let offer = outbound, offer.peripheral == peripheral {
            if offer.waitingForCleanDisconnect {
                beginOutboundConnection(offer)
            } else {
                completeOutbound(error: "Bluetooth connection ended before delivery", state: "disconnected")
            }
        } else if let read = activeIdentityRead, read.peripheral == peripheral {
            if read.waitingForCleanDisconnect {
                read.waitingForCleanDisconnect = false
                central.connect(peripheral, options: nil)
            } else {
                completeIdentityRead(displayName: nil)
            }
        }
    }
}

extension AppleBluetoothDiscoveryProvider: CBPeripheralDelegate {
    func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        if activeIdentityRead?.peripheral == peripheral {
            guard error == nil,
                  let service = peripheral.services?.first(where: {
                      $0.uuid == CBUUID(nsuuid: BleRendezvousProtocol.serviceUUID)
                  }) else {
                completeIdentityRead(displayName: nil)
                return
            }
            peripheral.discoverCharacteristics(
                [CBUUID(nsuuid: BleRendezvousProtocol.identityCharacteristicUUID)],
                for: service
            )
            return
        }
        guard let offer = outbound,
              offer.peripheral == peripheral,
              offer.connectionStarted,
              !offer.waitingForCleanDisconnect else {
            return
        }
        guard error == nil,
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
        if activeIdentityRead?.peripheral == peripheral {
            guard error == nil,
                  let characteristic = service.characteristics?.first(where: {
                      $0.uuid == CBUUID(nsuuid: BleRendezvousProtocol.identityCharacteristicUUID)
                  }) else {
                completeIdentityRead(displayName: nil)
                return
            }
            peripheral.readValue(for: characteristic)
            return
        }
        guard let offer = outbound,
              offer.peripheral == peripheral,
              offer.connectionStarted,
              !offer.waitingForCleanDisconnect else {
            return
        }
        guard error == nil,
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
        guard let offer = outbound,
              offer.peripheral == peripheral,
              offer.characteristic === characteristic else {
            return
        }
        guard error == nil else {
            completeOutbound(error: "Bluetooth invitation delivery failed", state: "write_failed")
            return
        }
        offer.nextFrame += 1
        writeNextFrame()
    }

    func peripheral(
        _ peripheral: CBPeripheral,
        didUpdateValueFor characteristic: CBCharacteristic,
        error: Error?
    ) {
        guard let read = activeIdentityRead,
              read.peripheral == peripheral,
              characteristic.uuid == CBUUID(nsuuid: BleRendezvousProtocol.identityCharacteristicUUID),
              error == nil,
              let value = characteristic.value,
              let remoteIdentity = BleRendezvousProtocol.decodeIdentity(value),
              remoteIdentity.peerKey == read.peerKey else {
            if activeIdentityRead?.peripheral == peripheral {
                completeIdentityRead(displayName: nil)
            }
            return
        }
        completeIdentityRead(displayName: remoteIdentity.displayName)
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
            let centralID = request.central.identifier
            let assembler: BleRendezvousProtocol.Assembler
            if let existing = inboundAssemblers[centralID] {
                assembler = existing
            } else {
                if inboundAssemblers.count >= Self.maximumInboundAssemblers,
                   let oldestCentralID = inboundAssemblerOrder.first {
                    inboundAssemblers.removeValue(forKey: oldestCentralID)
                    inboundAssemblerOrder.removeFirst()
                }
                assembler = BleRendezvousProtocol.Assembler()
                inboundAssemblers[centralID] = assembler
                inboundAssemblerOrder.append(centralID)
            }
            if let invite = assembler.accept(value) {
                inboundAssemblers.removeValue(forKey: centralID)
                inboundAssemblerOrder.removeAll { $0 == centralID }
                handleInboundInvite(invite)
            } else if !assembler.isAssembling {
                inboundAssemblers.removeValue(forKey: centralID)
                inboundAssemblerOrder.removeAll { $0 == centralID }
            }
            peripheral.respond(to: request, withResult: .success)
        }
    }
}
#endif
