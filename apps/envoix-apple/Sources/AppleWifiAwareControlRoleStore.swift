import Foundation

/// Stores only the local control-plane role for an Apple source-scoped
/// paired-device identifier. The identifier and role are not credentials.
final class AppleWifiAwareControlRoleStore: @unchecked Sendable {
    static let shared = AppleWifiAwareControlRoleStore()

    static let defaultsKey = "envoix.wifiAware.controlRoles.v1"

    private let defaults: UserDefaults
    private let defaultsKey: String
    private let lock = NSLock()

    init(
        defaults: UserDefaults = .standard,
        defaultsKey: String = AppleWifiAwareControlRoleStore.defaultsKey
    ) {
        self.defaults = defaults
        self.defaultsKey = defaultsKey
    }

    func role(for deviceID: UInt64) -> AppleWifiAwareControlRole? {
        lock.lock()
        defer { lock.unlock() }
        return roles()[Self.key(for: deviceID)].flatMap(
            AppleWifiAwareControlRole.init(rawValue:)
        )
    }

    func set(_ role: AppleWifiAwareControlRole, for deviceID: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        var values = roles()
        values[Self.key(for: deviceID)] = role.rawValue
        defaults.set(values, forKey: defaultsKey)
    }

    func setIfAbsent(_ role: AppleWifiAwareControlRole, for deviceID: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        var values = roles()
        let key = Self.key(for: deviceID)
        guard values[key] == nil else { return }
        values[key] = role.rawValue
        defaults.set(values, forKey: defaultsKey)
    }

    func remove(for deviceID: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        var values = roles()
        guard values.removeValue(forKey: Self.key(for: deviceID)) != nil else {
            return
        }
        defaults.set(values, forKey: defaultsKey)
    }

    func retain(deviceIDs: Set<UInt64>) {
        lock.lock()
        defer { lock.unlock() }
        let retainedKeys = Set(deviceIDs.map(Self.key(for:)))
        let current = roles()
        let retained = current.filter { retainedKeys.contains($0.key) }
        if retained != current {
            defaults.set(retained, forKey: defaultsKey)
        }
    }

    @discardableResult
    func setCanonicalRoleIfAbsent(
        localPeerKey: String,
        remotePeerKey: String,
        for deviceID: UInt64
    ) -> AppleWifiAwareControlRole? {
        guard let role = Self.canonicalRole(
            localPeerKey: localPeerKey,
            remotePeerKey: remotePeerKey
        ) else {
            return nil
        }
        setIfAbsent(role, for: deviceID)
        return role
    }

    static func canonicalRole(
        localPeerKey: String,
        remotePeerKey: String
    ) -> AppleWifiAwareControlRole? {
        AppleWifiAwareControlRole.canonical(
            localPeerKey: localPeerKey,
            remotePeerKey: remotePeerKey
        )
    }

    private func roles() -> [String: String] {
        defaults.dictionary(forKey: defaultsKey) as? [String: String] ?? [:]
    }

    private static func key(for deviceID: UInt64) -> String {
        String(format: "%016llx", deviceID)
    }
}
