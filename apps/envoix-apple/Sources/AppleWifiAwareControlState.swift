import Foundation

enum AppleWifiAwareControlRole: String, Equatable, Sendable {
    case publisher
    case subscriber

    static func canonical(
        localPeerKey: String,
        remotePeerKey: String
    ) -> AppleWifiAwareControlRole? {
        guard let local = NearbyDiscoveryPeerRegistry.normalizePeerKey(localPeerKey),
              let remote = NearbyDiscoveryPeerRegistry.normalizePeerKey(remotePeerKey),
              local != remote else {
            return nil
        }
        return local < remote ? .subscriber : .publisher
    }
}

enum AppleWifiAwareControlChannelDirection: Equatable, Sendable {
    case inboundPublisher
    case outboundSubscriber

    var localRole: AppleWifiAwareControlRole {
        switch self {
        case .inboundPublisher: .publisher
        case .outboundSubscriber: .subscriber
        }
    }
}

enum AppleWifiAwareRendezvousNetworkingMode: Equatable {
    case automatic
    case publisherOnly
    case subscriberOnly

    var startsBrowser: Bool {
        self != .publisherOnly
    }

    var startsListener: Bool {
        self != .subscriberOnly
    }
}

/// Pure per-device role state. A persisted role is only a preferred bootstrap
/// direction. If that direction cannot produce a ready channel, recovery
/// clears the hint temporarily and authenticated peer keys select a stable,
/// complementary role.
struct AppleWifiAwareControlRoleState: Equatable, Sendable {
    enum Phase: Equatable, Sendable {
        case preferred
        case recovering
        case established
    }

    enum AuthenticationResult: Equatable, Sendable {
        case accepted(role: AppleWifiAwareControlRole, roleChanged: Bool)
        case roleMismatch(role: AppleWifiAwareControlRole, roleChanged: Bool)
        case identityCollision
    }

    private(set) var role: AppleWifiAwareControlRole?
    private(set) var phase: Phase

    init(persistedRole: AppleWifiAwareControlRole?) {
        role = persistedRole
        phase = .preferred
    }

    var startsBrowser: Bool { role != .publisher }
    var startsListener: Bool { role != .subscriber }

    @discardableResult
    mutating func beginRecovery(hasReadyChannel: Bool) -> Bool {
        guard !hasReadyChannel, role != nil else { return false }
        role = nil
        phase = .recovering
        return true
    }

    mutating func authenticate(
        localPeerKey: String,
        remotePeerKey: String,
        direction: AppleWifiAwareControlChannelDirection
    ) -> AuthenticationResult {
        let previousRole = role
        if role == nil {
            guard let canonicalRole = AppleWifiAwareControlRole.canonical(
                localPeerKey: localPeerKey,
                remotePeerKey: remotePeerKey
            ) else {
                return .identityCollision
            }
            role = canonicalRole
        }
        guard let role else { return .identityCollision }
        let roleChanged = role != previousRole
        guard direction.localRole == role else {
            phase = .preferred
            return .roleMismatch(role: role, roleChanged: roleChanged)
        }
        phase = .established
        return .accepted(role: role, roleChanged: roleChanged)
    }

    mutating func channelClosed(hasReadyChannel: Bool) {
        if !hasReadyChannel, phase == .established {
            phase = .preferred
        }
    }
}

/// Tracks one per-device endpoint attempt without capturing a stale endpoint.
struct WifiAwareEndpointAttemptState<Endpoint> {
    struct Token: Hashable, Sendable {
        fileprivate let id: UUID
    }

    private(set) var currentEndpoint: Endpoint?
    private var activeAttempt: Token?

    init(endpoint: Endpoint? = nil) {
        currentEndpoint = endpoint
    }

    var hasActiveAttempt: Bool { activeAttempt != nil }

    mutating func updateEndpoint(_ endpoint: Endpoint?) {
        currentEndpoint = endpoint
    }

    mutating func beginAttempt() -> Token? {
        guard currentEndpoint != nil, activeAttempt == nil else { return nil }
        let token = Token(id: UUID())
        activeAttempt = token
        return token
    }

    @discardableResult
    mutating func finishAttempt(_ token: Token) -> Bool {
        guard activeAttempt == token else { return false }
        activeAttempt = nil
        return true
    }

    mutating func cancelAttempt() {
        activeAttempt = nil
    }
}

/// Limits only unauthenticated/pending inbound handshakes. Tokens are
/// idempotently released so promotion and teardown can safely race.
struct WifiAwareInboundConnectionAdmission {
    struct Token: Hashable, Sendable {
        fileprivate let id: UUID
    }

    static let maximumConcurrentConnections = 8

    private var activeTokens = Set<Token>()
    private var pendingTokensByChannelID: [UUID: Token] = [:]

    var activeConnectionCount: Int { activeTokens.count }
    var pendingConnectionCount: Int { pendingTokensByChannelID.count }

    mutating func acquire() -> Token? {
        guard activeTokens.count < Self.maximumConcurrentConnections else {
            return nil
        }
        let token = Token(id: UUID())
        activeTokens.insert(token)
        return token
    }

    mutating func release(_ token: Token) {
        activeTokens.remove(token)
        pendingTokensByChannelID = pendingTokensByChannelID.filter {
            $0.value != token
        }
    }

    @discardableResult
    mutating func markPending(_ token: Token, for channelID: UUID) -> Bool {
        guard activeTokens.contains(token),
              pendingTokensByChannelID[channelID] == nil,
              !pendingTokensByChannelID.values.contains(token) else {
            return false
        }
        pendingTokensByChannelID[channelID] = token
        return true
    }

    @discardableResult
    mutating func releasePending(for channelID: UUID) -> Bool {
        guard let token = pendingTokensByChannelID.removeValue(
            forKey: channelID
        ) else {
            return false
        }
        activeTokens.remove(token)
        return true
    }

    mutating func reset() {
        activeTokens.removeAll(keepingCapacity: true)
        pendingTokensByChannelID.removeAll(keepingCapacity: true)
    }
}
