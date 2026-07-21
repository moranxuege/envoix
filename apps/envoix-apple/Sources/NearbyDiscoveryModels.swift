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

struct NearbyDiscoveryObservation: Equatable {
    let peerKey: String
    let source: NearbyDiscoverySource
    let seenAtMilliseconds: Int64
    let displayName: String?
    let rssi: Int?
    let endpoint: String?

    init(
        peerKey: String,
        source: NearbyDiscoverySource,
        seenAtMilliseconds: Int64,
        displayName: String? = nil,
        rssi: Int? = nil,
        endpoint: String? = nil
    ) {
        self.peerKey = peerKey
        self.source = source
        self.seenAtMilliseconds = seenAtMilliseconds
        self.displayName = displayName
        self.rssi = rssi
        self.endpoint = endpoint
    }
}

struct NearbyDiscoveredPeer: Equatable, Identifiable {
    var id: String { peerKey }

    let peerKey: String
    let displayName: String?
    let sources: Set<NearbyDiscoverySource>
    let lastSeenAtMilliseconds: Int64
    let rssi: Int?
    let endpoint: String?
}

/// UI context carried from public discovery into the authenticated pairing
/// flow. It intentionally excludes endpoint and credential material: selecting
/// a discovery card never authorizes a connection by itself.
struct NearbyPairingSelection: Equatable, Identifiable {
    var id: String { discoveryPeerKey }

    let discoveryPeerKey: String
    let displayName: String?
    let sources: Set<NearbyDiscoverySource>

    init(peer: NearbyDiscoveredPeer) {
        discoveryPeerKey = peer.peerKey
        displayName = peer.displayName
        sources = peer.sources
    }

    init(
        discoveryPeerKey: String,
        displayName: String?,
        sources: Set<NearbyDiscoverySource>
    ) {
        self.discoveryPeerKey = discoveryPeerKey
        self.displayName = displayName
        self.sources = sources
    }
}

struct NearbyRendezvousOffer: Equatable, Identifiable {
    var id: String { requestID }

    let requestID: String
    let senderPeerKey: String
    let senderDisplayName: String?
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

protocol NearbyRendezvousProvider: AnyObject {
    func offerInvite(
        peerKey: String,
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
    static let maximumEndpointLength = 96
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
            endpoint: Self.sanitizeText(observation.endpoint, maximumLength: Self.maximumEndpointLength)
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
                endpoint: Self.latest(values, value: \NearbyDiscoveryObservation.endpoint)
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

    init?(dictionary: [String: String]) {
        guard dictionary["v"] == Self.protocolVersion,
              let rawPeerKey = dictionary["id"],
              let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(rawPeerKey) else {
            return nil
        }
        self.peerKey = peerKey
        self.displayName = NearbyDiscoveryPeerRegistry.sanitizeDisplayName(dictionary["name"])
    }

    init(identity: LocalNearbyDiscoveryIdentity) {
        peerKey = identity.peerKey
        displayName = NearbyDiscoveryPeerRegistry.sanitizeDisplayName(identity.displayName)
    }

    var dictionary: [String: String] {
        var value = ["v": Self.protocolVersion, "id": peerKey]
        if let displayName {
            value["name"] = displayName
        }
        return value
    }
}
