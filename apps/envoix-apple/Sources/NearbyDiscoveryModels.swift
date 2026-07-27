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

    init(
        peerKey: String,
        source: NearbyDiscoverySource,
        seenAtMilliseconds: Int64,
        displayName: String? = nil,
        rssi: Int? = nil,
        inviteRoute: NearbyInviteRoute? = nil
    ) {
        self.peerKey = peerKey
        self.source = source
        self.seenAtMilliseconds = seenAtMilliseconds
        self.displayName = displayName
        self.rssi = rssi
        self.inviteRoute = inviteRoute
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
}

/// UI context frozen when a public discovery card is selected. The inbox route
/// is an untrusted routing capability, not a remembered credential; freezing
/// the complete route prevents a later mDNS record swap from silently changing
/// the selected destination.
struct NearbyPairingSelection: Equatable, Identifiable {
    var id: String { discoveryPeerKey }

    let discoveryPeerKey: String
    let displayName: String?
    let sources: Set<NearbyDiscoverySource>
    let nearbyInviteRoute: NearbyInviteRoute?

    init(peer: NearbyDiscoveredPeer) {
        discoveryPeerKey = peer.peerKey
        displayName = peer.displayName
        sources = peer.sources
        nearbyInviteRoute = peer.inviteRoute
    }

    init(
        discoveryPeerKey: String,
        displayName: String?,
        sources: Set<NearbyDiscoverySource>,
        nearbyInviteRoute: NearbyInviteRoute? = nil
    ) {
        self.discoveryPeerKey = discoveryPeerKey
        self.displayName = displayName
        self.sources = sources
        self.nearbyInviteRoute = nearbyInviteRoute
    }
}

struct NearbyRendezvousOffer: Equatable, Identifiable {
    var id: String { requestID }

    let requestID: String
    let senderPeerKey: String
    let senderDisplayName: String?
    let source: NearbyDiscoverySource
    let senderInboxEndpointID: String?
    let invite: String
}

enum NearbyDiscoveryEvent {
    case observation(NearbyDiscoveryObservation)
    case status(NearbyProviderStatus)
    case rendezvousOffer(NearbyRendezvousOffer)
}

protocol NearbyDiscoveryProvider: AnyObject {
    var source: NearbyDiscoverySource { get }

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void)
    func stop()
}

protocol NearbyAdvertisingConfigurable: AnyObject {
    func setAdvertisingEnabled(_ enabled: Bool)
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
    static let maximumInboxEndpointIDLength = 80
    static let peerKeyHexLength = 16

    private let observationTTLMilliseconds: Int64
    private var observations: [String: [NearbyDiscoverySource: NearbyDiscoveryObservation]] = [:]

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

        let normalized = NearbyDiscoveryObservation(
            peerKey: peerKey,
            source: observation.source,
            seenAtMilliseconds: observation.seenAtMilliseconds,
            displayName: Self.sanitizeDisplayName(observation.displayName),
            rssi: observation.rssi,
            inviteRoute: observation.inviteRoute
        )
        var bySource = observations[peerKey] ?? [:]
        if let previous = bySource[observation.source],
           previous.seenAtMilliseconds > normalized.seenAtMilliseconds {
            return false
        }
        bySource[observation.source] = normalized
        observations[peerKey] = bySource
        return true
    }

    func clear() {
        observations.removeAll()
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
            } else {
                observations[peerKey] = bySource
            }
        }

        return observations.map { peerKey, bySource in
            let values = Array(bySource.values)
            return NearbyDiscoveredPeer(
                peerKey: peerKey,
                displayName: Self.latest(values, value: \NearbyDiscoveryObservation.displayName),
                sources: Set(bySource.keys),
                lastSeenAtMilliseconds: values.map(\.seenAtMilliseconds).max() ?? 0,
                rssi: Self.latest(values, value: \NearbyDiscoveryObservation.rssi),
                inviteRoute: Self.latest(values, value: \NearbyDiscoveryObservation.inviteRoute)
            )
        }
        .sorted {
            if $0.lastSeenAtMilliseconds != $1.lastSeenAtMilliseconds {
                return $0.lastSeenAtMilliseconds > $1.lastSeenAtMilliseconds
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
