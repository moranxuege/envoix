import Foundation

enum NearbyDiscoverySource: String, CaseIterable, Hashable {
    case bluetooth
    case mdns
    case wifiAware

    var logName: String {
        switch self {
        case .bluetooth: return "bluetooth"
        case .mdns: return "mdns"
        case .wifiAware: return "wifi_aware"
        }
    }
}

enum NearbyProviderAvailability: String, Equatable {
    case stopped
    case starting
    case pairingRequired
    case paired
    case ready
    case degraded
    case permissionRequired
    case disabled
    case unsupported
    case temporarilyUnavailable
    case reserved
    case error

    var logName: String {
        switch self {
        case .pairingRequired: return "pairing_required"
        case .permissionRequired: return "permission_required"
        case .temporarilyUnavailable: return "temporarily_unavailable"
        default: return rawValue
        }
    }
}

enum NearbyProviderDetail: Equatable {
    case discoveryStopped
    case startingBluetooth
    case bluetoothAccessRequired
    case bluetoothUnavailable
    case bluetoothOff
    case bluetoothReady
    case bluetoothVisibilityStarting
    case bluetoothScanningOnly
    case bluetoothVisibleOnly
    case startingLocalNetwork
    case localNetworkReady
    case localNetworkScanningOnly
    case localNetworkVisibleOnly
    case localNetworkPermissionOrUnavailable
    case wifiAwareReserved
    case startingWifiAware
    case wifiAwareUnsupported
    case wifiAwareServiceMissing
    case wifiAwareEntitlementMissing
    case wifiAwareTemporarilyUnavailable
    case wifiAwarePairingRequired
    case wifiAwarePairedDevices(Int)
    case wifiAwarePairedDeviceLimitExceeded
}

struct NearbyProviderStatus: Equatable {
    let source: NearbyDiscoverySource
    let availability: NearbyProviderAvailability
    let detail: NearbyProviderDetail
}

/// Native connection coordinates advertised inside the discovery service.
///
/// The endpoint ID alone is not dialable. At least one relay URL or direct
/// socket address must travel with it, and callers freeze this complete value
/// before starting pairing so a later Bonjour update cannot redirect a send.
struct NearbyInviteRoute: Equatable, Hashable {
    static let maximumDirectAddressCount = 4
    static let maximumDirectAddressUTF8Bytes = 128
    // Android's DNS-SD API requires key bytes + value bytes < 255. Keeping the
    // six-byte `irelay` key in that shared budget leaves 248 value bytes.
    static let maximumRelayURLUTF8Bytes = 248

    let endpointID: String
    let relayURL: String?
    let directAddresses: [String]

    init?(
        endpointID: String,
        relayURL: String? = nil,
        directAddresses: [String] = []
    ) {
        guard let endpointID = NearbyDiscoveryPeerRegistry.normalizeInboxEndpointID(
            endpointID
        ) else {
            return nil
        }

        let normalizedRelayURL: String?
        if let relayURL {
            guard let value = Self.normalizeWireValue(
                relayURL,
                maximumUTF8Bytes: Self.maximumRelayURLUTF8Bytes
            ) else {
                return nil
            }
            normalizedRelayURL = value
        } else {
            normalizedRelayURL = nil
        }

        guard directAddresses.count <= Self.maximumDirectAddressCount else {
            return nil
        }
        var seenAddresses = Set<String>()
        var normalizedDirectAddresses: [String] = []
        for address in directAddresses {
            guard let value = Self.normalizeWireValue(
                address,
                maximumUTF8Bytes: Self.maximumDirectAddressUTF8Bytes
            ) else {
                return nil
            }
            if seenAddresses.insert(value).inserted {
                normalizedDirectAddresses.append(value)
            }
        }
        guard normalizedRelayURL != nil || !normalizedDirectAddresses.isEmpty else {
            return nil
        }

        self.endpointID = endpointID
        self.relayURL = normalizedRelayURL
        self.directAddresses = normalizedDirectAddresses
    }

    private static func normalizeWireValue(
        _ value: String,
        maximumUTF8Bytes: Int
    ) -> String? {
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalized.isEmpty,
              normalized.utf8.count <= maximumUTF8Bytes,
              normalized.unicodeScalars.allSatisfy({ scalar in
                  !CharacterSet.whitespacesAndNewlines.contains(scalar)
                      && !CharacterSet.controlCharacters.contains(scalar)
              }) else {
            return nil
        }
        return normalized
    }
}

struct NearbyDiscoveryObservation: Equatable {
    let peerKey: String
    let source: NearbyDiscoverySource
    let seenAtMilliseconds: Int64
    let displayName: String?
    let rssi: Int?
    let inviteRoute: NearbyInviteRoute?
    let nearbyWifiAwareDeviceID: String?

    init(
        peerKey: String,
        source: NearbyDiscoverySource,
        seenAtMilliseconds: Int64,
        displayName: String? = nil,
        rssi: Int? = nil,
        inviteRoute: NearbyInviteRoute? = nil,
        nearbyWifiAwareDeviceID: String? = nil
    ) {
        self.peerKey = peerKey
        self.source = source
        self.seenAtMilliseconds = seenAtMilliseconds
        self.displayName = displayName
        self.rssi = rssi
        self.inviteRoute = inviteRoute
        self.nearbyWifiAwareDeviceID = nearbyWifiAwareDeviceID
    }
}

struct NearbyDiscoveredPeer: Equatable, Identifiable {
    var id: String { peerKey }

    let peerKey: String
    let displayName: String?
    let sources: Set<NearbyDiscoverySource>
    let lastSeenAtMilliseconds: Int64
    let rssi: Int?
    let inviteRoute: NearbyInviteRoute?
    let nearbyWifiAwareDeviceID: String?

    init(
        peerKey: String,
        displayName: String?,
        sources: Set<NearbyDiscoverySource>,
        lastSeenAtMilliseconds: Int64,
        rssi: Int?,
        inviteRoute: NearbyInviteRoute?,
        nearbyWifiAwareDeviceID: String? = nil
    ) {
        self.peerKey = peerKey
        self.displayName = displayName
        self.sources = sources
        self.lastSeenAtMilliseconds = lastSeenAtMilliseconds
        self.rssi = rssi
        self.inviteRoute = inviteRoute
        self.nearbyWifiAwareDeviceID = nearbyWifiAwareDeviceID
    }
}

func nearbyPeerDisplayName(
    _ peer: NearbyDiscoveredPeer,
    among peers: [NearbyDiscoveredPeer],
    fallback: String
) -> String {
    let baseName = normalizedNearbyDisplayName(peer.displayName) ?? fallback
    let normalizedName = baseName.lowercased()
    let duplicateCount = peers.lazy.filter {
        let candidate = normalizedNearbyDisplayName($0.displayName) ?? fallback
        return candidate.lowercased() == normalizedName
    }.count
    guard duplicateCount > 1 else { return baseName }
    return "\(baseName) · \(peer.peerKey.suffix(4).uppercased())"
}

private func normalizedNearbyDisplayName(_ value: String?) -> String? {
    guard let normalized = value?.trimmingCharacters(in: .whitespacesAndNewlines),
          !normalized.isEmpty else {
        return nil
    }
    return normalized
}

/// A device paired by an operating-system discovery provider. The identifier
/// is scoped to that provider and is never treated as an authenticated Envoix
/// identity or a claim that the peer is currently reachable.
struct NearbyPairedDevice: Equatable, Identifiable {
    static let maximumSourceScopedIDLength = 64
    static let maximumSnapshotCount = 256

    var id: String { "\(source.logName):\(sourceScopedID)" }

    let sourceScopedID: String
    let source: NearbyDiscoverySource
    let displayName: String?
    let model: String?

    init?(
        sourceScopedID: String,
        source: NearbyDiscoverySource,
        displayName: String? = nil,
        model: String? = nil
    ) {
        let normalizedID = sourceScopedID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalizedID.isEmpty,
              normalizedID.count <= Self.maximumSourceScopedIDLength,
              normalizedID.unicodeScalars.allSatisfy({
                  !CharacterSet.whitespacesAndNewlines.contains($0) &&
                      !CharacterSet.controlCharacters.contains($0)
              })
        else {
            return nil
        }
        self.sourceScopedID = normalizedID
        self.source = source
        self.displayName = NearbyDiscoveryPeerRegistry.sanitizeDisplayName(displayName)
        self.model = NearbyDiscoveryPeerRegistry.sanitizeDeviceDetail(model)
    }
}

/// Returns a Wi-Fi Aware route only when the snapshot is unambiguous. Paired
/// device names are not stable identities, so multiple devices must never be
/// guessed or matched by display text.
func uniqueNearbyWifiAwareDeviceID(in devices: [NearbyPairedDevice]) -> String? {
    let wifiAwareDevices = devices.filter { $0.source == .wifiAware }
    return wifiAwareDevices.count == 1 ? wifiAwareDevices[0].sourceScopedID : nil
}

/// UI context frozen when a public discovery card is selected. The inbox route
/// and Wi-Fi Aware device ID are untrusted, one-time routing capabilities, not
/// remembered credentials. Freezing them prevents later provider updates from
/// silently changing the selected destination.
struct NearbyPairingSelection: Equatable, Identifiable {
    var id: String { discoveryPeerKey }

    let discoveryPeerKey: String
    let displayName: String?
    let sources: Set<NearbyDiscoverySource>
    let nearbyInviteRoute: NearbyInviteRoute?
    let nearbyWifiAwareDeviceID: String?

    init(
        peer: NearbyDiscoveredPeer,
        nearbyWifiAwareDeviceID: String? = nil
    ) {
        discoveryPeerKey = peer.peerKey
        displayName = peer.displayName
        sources = peer.sources
        nearbyInviteRoute = peer.inviteRoute
        self.nearbyWifiAwareDeviceID = nearbyWifiAwareDeviceID
            ?? peer.nearbyWifiAwareDeviceID
    }

    init(
        discoveryPeerKey: String,
        displayName: String?,
        sources: Set<NearbyDiscoverySource>,
        nearbyInviteRoute: NearbyInviteRoute? = nil,
        nearbyWifiAwareDeviceID: String? = nil
    ) {
        self.discoveryPeerKey = discoveryPeerKey
        self.displayName = displayName
        self.sources = sources
        self.nearbyInviteRoute = nearbyInviteRoute
        self.nearbyWifiAwareDeviceID = nearbyWifiAwareDeviceID
    }
}

struct NearbyRendezvousOffer: Equatable, Identifiable {
    var id: String { requestID }
    var deliveryID: String {
        "\(source.rawValue):\(senderPeerKey):\(requestID.utf8.count):\(requestID)"
    }

    let requestID: String
    let senderPeerKey: String
    let senderDisplayName: String?
    let source: NearbyDiscoverySource
    let senderInboxEndpointID: String?
    let senderWifiAwareDeviceID: String?
    let invite: String

    init(
        requestID: String,
        senderPeerKey: String,
        senderDisplayName: String?,
        source: NearbyDiscoverySource,
        senderInboxEndpointID: String?,
        senderWifiAwareDeviceID: String? = nil,
        invite: String
    ) {
        self.requestID = requestID
        self.senderPeerKey = senderPeerKey
        self.senderDisplayName = senderDisplayName
        self.source = source
        self.senderInboxEndpointID = senderInboxEndpointID
        self.senderWifiAwareDeviceID = senderWifiAwareDeviceID
        self.invite = invite
    }
}

/// A short-lived, secret-free hint that an Android phone has armed its NFC
/// presenter. The Bluetooth peripheral and Envoix peer key are bound by the
/// provider before this value reaches UI policy; neither is authentication.
struct NearbyNFCReadinessOffer: Equatable, Identifiable {
    static let lifetimeMilliseconds: Int64 = 30_000

    let id: String
    let presenterPeerKey: String
    let presenterID: UUID
    let firstSeenAtMilliseconds: Int64

    func isFresh(at nowMilliseconds: Int64) -> Bool {
        nowMilliseconds >= firstSeenAtMilliseconds
            && nowMilliseconds - firstSeenAtMilliseconds
                < Self.lifetimeMilliseconds
    }

    func remainingLifetimeSeconds(at nowMilliseconds: Int64) -> TimeInterval {
        guard isFresh(at: nowMilliseconds) else { return 0 }
        return TimeInterval(
            Self.lifetimeMilliseconds
                - (nowMilliseconds - firstSeenAtMilliseconds)
        ) / 1_000
    }
}

/// Deduplicates readiness generations across discovery restarts. An offer ID is
/// bound to the first recent Bluetooth identity that presented it.
struct NearbyNFCReadinessOfferRegistry {
    private static let maximumTrackedOfferCount = 256

    private var trackedOfferIDs = Set<String>()
    private var trackedOfferOrder: [String] = []

    mutating func observe(
        offerID: String,
        presenterPeerKey: String,
        presenterID: UUID,
        at nowMilliseconds: Int64
    ) -> NearbyNFCReadinessOffer? {
        guard nowMilliseconds >= 0,
              let normalizedID =
                  NearbyNFCReadinessBluetoothUUID.normalizeOfferID(offerID),
              let normalizedPeerKey =
                  NearbyDiscoveryPeerRegistry.normalizePeerKey(presenterPeerKey),
              trackedOfferIDs.insert(normalizedID).inserted else {
            return nil
        }
        trackedOfferOrder.append(normalizedID)
        if trackedOfferOrder.count > Self.maximumTrackedOfferCount {
            trackedOfferIDs.remove(trackedOfferOrder.removeFirst())
        }
        return NearbyNFCReadinessOffer(
            id: normalizedID,
            presenterPeerKey: normalizedPeerKey,
            presenterID: presenterID,
            firstSeenAtMilliseconds: nowMilliseconds
        )
    }
}

/// Binds a readiness advertisement to exactly one recently observed Envoix
/// peer on the same Core Bluetooth peripheral. Rotating or ambiguous peer keys
/// fail closed and leave the explicit NFC button as the fallback.
struct NearbyNFCReadinessIdentityRegistry {
    static let bindingLifetimeMilliseconds =
        NearbyDiscoveryPeerRegistry.defaultObservationTTLMilliseconds
    private static let maximumTrackedPresenterCount = 32

    private var observations: [UUID: [String: Int64]] = [:]
    private var presenterOrder: [UUID] = []

    @discardableResult
    mutating func observePresence(
        peerKey: String,
        presenterID: UUID,
        at nowMilliseconds: Int64
    ) -> Bool {
        guard nowMilliseconds >= 0,
              let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(peerKey)
        else {
            return false
        }
        if observations[presenterID] == nil {
            if presenterOrder.count >= Self.maximumTrackedPresenterCount,
               let oldest = presenterOrder.first {
                presenterOrder.removeFirst()
                observations.removeValue(forKey: oldest)
            }
            presenterOrder.append(presenterID)
        }
        var peers = observations[presenterID] ?? [:]
        if let previous = peers[peerKey], previous > nowMilliseconds {
            return false
        }
        peers[peerKey] = nowMilliseconds
        observations[presenterID] = peers
        return true
    }

    mutating func boundPeerKey(
        for presenterID: UUID,
        at nowMilliseconds: Int64
    ) -> String? {
        guard nowMilliseconds >= 0,
              var peers = observations[presenterID] else {
            return nil
        }
        peers = peers.filter { _, seenAtMilliseconds in
            nowMilliseconds >= seenAtMilliseconds
                && nowMilliseconds - seenAtMilliseconds
                    <= Self.bindingLifetimeMilliseconds
        }
        if peers.isEmpty {
            observations.removeValue(forKey: presenterID)
            presenterOrder.removeAll { $0 == presenterID }
            return nil
        }
        observations[presenterID] = peers
        return peers.count == 1 ? peers.keys.first : nil
    }

    mutating func clear() {
        observations.removeAll(keepingCapacity: true)
        presenterOrder.removeAll(keepingCapacity: true)
    }
}

enum NearbyDiscoveryEvent {
    case observation(NearbyDiscoveryObservation)
    case pairedDevices(source: NearbyDiscoverySource, devices: [NearbyPairedDevice])
    case status(NearbyProviderStatus)
    case rendezvousOffer(NearbyRendezvousOffer)
    case nfcPresenterReadiness(
        offerID: String,
        presenterPeerKey: String,
        presenterID: UUID
    )
}

protocol NearbyDiscoveryProvider: AnyObject {
    var source: NearbyDiscoverySource { get }

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void)
    func stop()
}

protocol NearbySystemPairingQuiescing: AnyObject {
    func waitUntilStopped() async
}

protocol NearbyAdvertisingConfigurable: AnyObject {
    func setAdvertisingEnabled(_ enabled: Bool)
}

protocol NearbyIdentityConfigurable: AnyObject {
    func setIdentity(_ identity: LocalNearbyDiscoveryIdentity)
}

protocol NearbyRendezvousAdmissionConfigurable: AnyObject {
    func setRendezvousOfferAdmission(
        _ admission: @escaping @MainActor (NearbyRendezvousOffer) -> Bool
    )
}

protocol NearbyRendezvousProvider: NearbyDiscoveryProvider {
    func canOfferInvite(to selection: NearbyPairingSelection) -> Bool

    func offerInvite(
        to selection: NearbyPairingSelection,
        invite: String,
        completion: @escaping (_ error: String?) -> Void
    )
}

struct LocalNearbyDiscoveryIdentity: Equatable {
    let peerKey: String
    let displayName: String
}

enum NearbyDiscoveryIdentityFactory {
    static func create(
        displayName: String,
        randomValue: () -> UInt64 = { UInt64.random(in: UInt64.min...UInt64.max) }
    ) -> LocalNearbyDiscoveryIdentity {
        return LocalNearbyDiscoveryIdentity(
            peerKey: String(format: "%016llx", randomValue()),
            displayName: NearbyDiscoveryPeerRegistry.sanitizeDisplayName(displayName) ?? "Apple device"
        )
    }
}

final class NearbyDiscoveryPeerRegistry {
    static let defaultObservationTTLMilliseconds: Int64 = 20_000
    static let maximumDisplayNameLength = 48
    static let maximumDeviceDetailLength = 96
    static let maximumInboxEndpointIDLength = 80
    static let peerKeyHexLength = 16
    static let maximumPeerCount = 64

    private let observationTTLMilliseconds: Int64
    private var observations: [String: [NearbyDiscoverySource: NearbyDiscoveryObservation]] = [:]
    private var peerOrdinals: [String: Int] = [:]
    private var nextPeerOrdinal = 0

    init(observationTTLMilliseconds: Int64 = NearbyDiscoveryPeerRegistry.defaultObservationTTLMilliseconds) {
        precondition(observationTTLMilliseconds > 0, "observation TTL must be positive")
        self.observationTTLMilliseconds = observationTTLMilliseconds
    }

    @discardableResult
    func upsert(_ observation: NearbyDiscoveryObservation) -> Bool {
        guard
            let peerKey = Self.normalizePeerKey(observation.peerKey),
            observation.seenAtMilliseconds >= 0
        else {
            return false
        }
        guard observations[peerKey] != nil || observations.count < Self.maximumPeerCount else {
            return false
        }

        var bySource = observations[peerKey] ?? [:]
        let previous = bySource[observation.source]
        if let previous,
           previous.seenAtMilliseconds > observation.seenAtMilliseconds {
            return false
        }
        let normalized = NearbyDiscoveryObservation(
            peerKey: peerKey,
            source: observation.source,
            seenAtMilliseconds: observation.seenAtMilliseconds,
            displayName: Self.sanitizeDisplayName(observation.displayName)
                ?? previous?.displayName,
            rssi: observation.rssi,
            inviteRoute: observation.inviteRoute,
            nearbyWifiAwareDeviceID: observation.source == .wifiAware
                ? Self.normalizeWifiAwareDeviceID(observation.nearbyWifiAwareDeviceID)
                    ?? previous?.nearbyWifiAwareDeviceID
                : nil
        )
        if observations[peerKey] == nil {
            peerOrdinals[peerKey] = nextPeerOrdinal
            nextPeerOrdinal += 1
        }
        bySource[observation.source] = normalized
        observations[peerKey] = bySource
        return true
    }

    func clear() {
        observations.removeAll()
        peerOrdinals.removeAll()
        nextPeerOrdinal = 0
    }

    func peers(nowMilliseconds: Int64) -> [NearbyDiscoveredPeer] {
        precondition(nowMilliseconds >= 0, "current time must not be negative")

        for peerKey in Array(observations.keys) {
            guard var bySource = observations[peerKey] else { continue }
            bySource = bySource.filter { _, observation in
                nowMilliseconds - observation.seenAtMilliseconds <= observationTTLMilliseconds
            }
            if bySource.isEmpty {
                observations.removeValue(forKey: peerKey)
                peerOrdinals.removeValue(forKey: peerKey)
            } else {
                observations[peerKey] = bySource
            }
        }

        return observations.map { peerKey, bySource in
            let values = Array(bySource.values)
            return NearbyDiscoveredPeer(
                peerKey: peerKey,
                displayName: Self.preferredDisplayName(in: bySource)
                    ?? Self.latest(values, value: \NearbyDiscoveryObservation.displayName),
                sources: Set(bySource.keys),
                lastSeenAtMilliseconds: values.map(\.seenAtMilliseconds).max() ?? 0,
                rssi: Self.latest(values, value: \NearbyDiscoveryObservation.rssi),
                inviteRoute: Self.latest(values, value: \NearbyDiscoveryObservation.inviteRoute),
                nearbyWifiAwareDeviceID: bySource[.wifiAware]?.nearbyWifiAwareDeviceID
            )
        }
        .sorted {
            let leftOrdinal = peerOrdinals[$0.peerKey] ?? Int.max
            let rightOrdinal = peerOrdinals[$1.peerKey] ?? Int.max
            if leftOrdinal != rightOrdinal {
                return leftOrdinal < rightOrdinal
            }
            return $0.peerKey < $1.peerKey
        }
    }

    static func normalizePeerKey(_ value: String) -> String? {
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard normalized.count == peerKeyHexLength,
              normalized.unicodeScalars.allSatisfy({ scalar in
                  (48...57).contains(scalar.value) || (97...102).contains(scalar.value)
              }) else {
            return nil
        }
        return normalized
    }

    static func sanitizeDisplayName(_ value: String?) -> String? {
        sanitizeText(value, maximumLength: maximumDisplayNameLength)
    }

    static func sanitizeDeviceDetail(_ value: String?) -> String? {
        sanitizeText(value, maximumLength: maximumDeviceDetailLength)
    }

    static func normalizeInboxEndpointID(_ value: String?) -> String? {
        guard let value else { return nil }
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalized.isEmpty,
              normalized.count <= maximumInboxEndpointIDLength,
              normalized.unicodeScalars.allSatisfy({ scalar in
                  (48...57).contains(scalar.value)
                      || (65...90).contains(scalar.value)
                      || (97...122).contains(scalar.value)
              }) else {
            return nil
        }
        return normalized
    }

    private static func normalizeWifiAwareDeviceID(_ value: String?) -> String? {
        guard let value else { return nil }
        return NearbyPairedDevice(
            sourceScopedID: value,
            source: .wifiAware
        )?.sourceScopedID
    }

    private static func sanitizeText(_ value: String?, maximumLength: Int) -> String? {
        guard let value else { return nil }
        let normalized = value.split(whereSeparator: \Character.isWhitespace).joined(separator: " ")
        let bounded = String(normalized.prefix(maximumLength))
        return bounded.isEmpty ? nil : bounded
    }

    private static func latest<Value>(
        _ observations: [NearbyDiscoveryObservation],
        value: KeyPath<NearbyDiscoveryObservation, Value?>
    ) -> Value? {
        observations
            .compactMap { observation in
                observation[keyPath: value].map { (observation.seenAtMilliseconds, $0) }
            }
            .max { $0.0 < $1.0 }?
            .1
    }

    private static func preferredDisplayName(
        in observations: [NearbyDiscoverySource: NearbyDiscoveryObservation]
    ) -> String? {
        [.mdns, .wifiAware, .bluetooth]
            .compactMap { observations[$0]?.displayName }
            .first
    }
}

enum NearbyDiscoveryBluetoothUUID {
    static let baseUUIDString = "d5f3a2d8-8f4a-4b33-0000-000000000000"
    private static let fixedPrefix = "d5f3a2d8-8f4a-4b33-"

    static func encode(peerKey: String) -> UUID? {
        guard let key = NearbyDiscoveryPeerRegistry.normalizePeerKey(peerKey) else { return nil }
        let splitIndex = key.index(key.startIndex, offsetBy: 4)
        return UUID(uuidString: fixedPrefix + key[..<splitIndex] + "-" + key[splitIndex...])
    }

    static func decode(_ uuid: UUID?) -> String? {
        guard let uuid else { return nil }
        let value = uuid.uuidString.lowercased()
        guard value.hasPrefix(fixedPrefix) else { return nil }
        let suffix = String(value.dropFirst(fixedPrefix.count)).replacingOccurrences(of: "-", with: "")
        return NearbyDiscoveryPeerRegistry.normalizePeerKey(suffix)
    }
}

/// Secret-free Android presenter readiness. The fixed high 64 bits identify
/// readiness-v1; the low 64 bits are a fresh, random, nonzero offer ID.
enum NearbyNFCReadinessBluetoothUUID {
    static let baseUUIDString = "d5f3a2d8-8f4a-4b34-0000-000000000000"
    private static let fixedPrefix = "d5f3a2d8-8f4a-4b34-"
    private static let offerIDLength = 16

    static func encode(offerID: String) -> UUID? {
        guard let normalizedID = normalizeOfferID(offerID) else { return nil }
        let splitIndex = normalizedID.index(
            normalizedID.startIndex,
            offsetBy: 4
        )
        return UUID(
            uuidString:
                fixedPrefix
                + normalizedID[..<splitIndex]
                + "-"
                + normalizedID[splitIndex...]
        )
    }

    static func decode(_ uuid: UUID?) -> String? {
        guard let uuid else { return nil }
        let value = uuid.uuidString.lowercased()
        guard value.hasPrefix(fixedPrefix) else { return nil }
        return normalizeOfferID(
            String(value.dropFirst(fixedPrefix.count))
                .replacingOccurrences(of: "-", with: "")
        )
    }

    static func normalizeOfferID(_ value: String) -> String? {
        guard value.count == offerIDLength,
              value != "0000000000000000",
              value.utf8.allSatisfy({
                  (0x30...0x39).contains($0)
                      || (0x61...0x66).contains($0)
              }) else {
            return nil
        }
        return value
    }
}

struct NearbyDiscoveryBonjourRecord: Equatable {
    static let serviceType = "_envoix-disc._udp"
    static let wireServiceType = "_envoix-disc._udp."
    private static let protocolVersion = "1"
    let peerKey: String
    let displayName: String?
    let inviteRoute: NearbyInviteRoute?

    init?(dictionary: [String: String]) {
        guard dictionary["v"] == Self.protocolVersion,
              let rawPeerKey = dictionary["id"],
              let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(rawPeerKey) else {
            return nil
        }
        self.peerKey = peerKey
        self.displayName = NearbyDiscoveryPeerRegistry.sanitizeDisplayName(dictionary["name"])
        if let endpointID = dictionary["ibox"] {
            self.inviteRoute = NearbyInviteRoute(
                endpointID: endpointID,
                relayURL: dictionary["irelay"],
                directAddresses: (0..<NearbyInviteRoute.maximumDirectAddressCount)
                    .compactMap { dictionary["iaddr\($0)"] }
            )
        } else {
            self.inviteRoute = nil
        }
    }

    init(identity: LocalNearbyDiscoveryIdentity, inviteRoute: NearbyInviteRoute? = nil) {
        peerKey = identity.peerKey
        displayName = NearbyDiscoveryPeerRegistry.sanitizeDisplayName(identity.displayName)
        self.inviteRoute = inviteRoute
    }

    var dictionary: [String: String] {
        var value = ["v": Self.protocolVersion, "id": peerKey]
        if let displayName {
            value["name"] = displayName
        }
        if let inviteRoute {
            value["ibox"] = inviteRoute.endpointID
            if let relayURL = inviteRoute.relayURL {
                value["irelay"] = relayURL
            }
            for (index, address) in inviteRoute.directAddresses.enumerated() {
                value["iaddr\(index)"] = address
            }
        }
        return value
    }

    static func consistentInviteRoute(
        in records: [NearbyDiscoveryBonjourRecord]
    ) -> NearbyInviteRoute? {
        guard let first = records.first?.inviteRoute else { return nil }
        return records.allSatisfy { $0.inviteRoute == first } ? first : nil
    }
}
