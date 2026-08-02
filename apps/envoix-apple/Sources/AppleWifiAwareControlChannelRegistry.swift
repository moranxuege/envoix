import Foundation

struct AppleWifiAwareControlChannelRegistry<Value> {
    struct Entry {
        let channelID: UUID
        let deviceID: UInt64
        let direction: AppleWifiAwareControlChannelDirection
        let remoteIdentity: LocalNearbyDiscoveryIdentity
        let value: Value
    }

    private var entriesByDevice: [UInt64: [UUID: Entry]] = [:]

    var isEmpty: Bool { entriesByDevice.isEmpty }

    func entries(for deviceID: UInt64) -> [Entry] {
        Array(entriesByDevice[deviceID]?.values ?? [:].values)
    }

    func contains(deviceID: UInt64) -> Bool {
        entriesByDevice[deviceID]?.isEmpty == false
    }

    func selected(
        for deviceID: UInt64,
        preferredRole: AppleWifiAwareControlRole?
    ) -> Entry? {
        let candidates = entries(for: deviceID)
        let preferred = preferredRole.flatMap { role in
            candidates.filter { $0.direction.localRole == role }
                .min { $0.channelID.uuidString < $1.channelID.uuidString }
        }
        return preferred ?? candidates.min {
            $0.channelID.uuidString < $1.channelID.uuidString
        }
    }

    /// Keeps at most one ready channel for each direction. Returning the old
    /// entry lets its owner close it without allowing a late close callback to
    /// remove the replacement.
    @discardableResult
    mutating func register(_ entry: Entry) -> Entry? {
        var deviceEntries = entriesByDevice[entry.deviceID] ?? [:]
        let replaced = deviceEntries.values.first {
            $0.direction == entry.direction && $0.channelID != entry.channelID
        }
        if let replaced {
            deviceEntries.removeValue(forKey: replaced.channelID)
        }
        deviceEntries[entry.channelID] = entry
        entriesByDevice[entry.deviceID] = deviceEntries
        return replaced
    }

    @discardableResult
    mutating func remove(deviceID: UInt64, channelID: UUID) -> Entry? {
        guard var deviceEntries = entriesByDevice[deviceID],
              let removed = deviceEntries.removeValue(forKey: channelID) else {
            return nil
        }
        if deviceEntries.isEmpty {
            entriesByDevice.removeValue(forKey: deviceID)
        } else {
            entriesByDevice[deviceID] = deviceEntries
        }
        return removed
    }

    mutating func retain(deviceIDs: Set<UInt64>) -> [Entry] {
        var removed: [Entry] = []
        let removedDeviceIDs = entriesByDevice.keys.filter {
            !deviceIDs.contains($0)
        }
        for deviceID in removedDeviceIDs {
            if let entries = entriesByDevice.removeValue(forKey: deviceID) {
                removed.append(contentsOf: entries.values)
            }
        }
        return removed
    }

    mutating func removeAll() -> [Entry] {
        let removed = entriesByDevice.values.flatMap(\.values)
        entriesByDevice.removeAll()
        return removed
    }
}
