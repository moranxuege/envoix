#if os(iOS) || os(macOS)
import Combine
import Foundation
#if os(iOS)
import UIKit
#endif

enum NearbyVisibilityMode: String, CaseIterable, Equatable {
    case hidden
    case everyoneTenMinutes
    case whileAppOpen
}

@MainActor
final class NearbyPresencePreferences: ObservableObject {
    static let visibilityDuration: TimeInterval = 10 * 60

    @Published private(set) var displayName: String
    @Published private(set) var visibility: NearbyVisibilityMode
    @Published private(set) var visibilityExpiresAt: Date?

    private enum Key {
        static let displayName = "envoix.nearby.display-name"
        static let visibility = "envoix.nearby.visibility"
        static let visibilityExpiresAt = "envoix.nearby.visibility-expires-at"
    }

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard, now: Date = Date()) {
        self.defaults = defaults
        let storedName = defaults.string(forKey: Key.displayName)
#if os(iOS)
        let platformDisplayName: String? = UIDevice.current.model
        let fallbackDisplayName = "Apple device"
#else
        let platformDisplayName = Host.current().localizedName
        let fallbackDisplayName = "Mac"
#endif
        displayName = NearbyDiscoveryPeerRegistry.sanitizeDisplayName(storedName)
            ?? NearbyDiscoveryPeerRegistry.sanitizeDisplayName(platformDisplayName)
            ?? fallbackDisplayName

        let storedVisibility = defaults.object(forKey: Key.visibility)
        let resolvedVisibility: NearbyVisibilityMode
        if let storedVisibility = storedVisibility as? String {
            resolvedVisibility = NearbyVisibilityMode(rawValue: storedVisibility) ?? .hidden
        } else if storedVisibility != nil {
            resolvedVisibility = .hidden
        } else {
            resolvedVisibility = .whileAppOpen
        }
        let storedExpiry = defaults.object(forKey: Key.visibilityExpiresAt) as? Date
        if resolvedVisibility == .everyoneTenMinutes,
           let storedExpiry,
           storedExpiry > now {
            visibility = resolvedVisibility
            visibilityExpiresAt = storedExpiry
        } else {
            visibility = resolvedVisibility == .whileAppOpen ? .whileAppOpen : .hidden
            visibilityExpiresAt = nil
        }
    }

    @discardableResult
    func updateDisplayName(_ value: String) -> Bool {
        guard let sanitized = NearbyDiscoveryPeerRegistry.sanitizeDisplayName(value) else {
            return false
        }
        displayName = sanitized
        defaults.set(sanitized, forKey: Key.displayName)
        return true
    }

    func setVisibility(_ value: NearbyVisibilityMode, now: Date = Date()) {
        visibility = value
        visibilityExpiresAt = value == .everyoneTenMinutes
            ? now.addingTimeInterval(Self.visibilityDuration)
            : nil
        persistVisibility()
    }

    @discardableResult
    func expireIfNeeded(now: Date = Date()) -> Bool {
        guard visibility == .everyoneTenMinutes,
              let visibilityExpiresAt,
              now >= visibilityExpiresAt else {
            return false
        }
        visibility = .hidden
        self.visibilityExpiresAt = nil
        persistVisibility()
        return true
    }

    func isAdvertising(sceneIsActive: Bool, now: Date = Date()) -> Bool {
        guard sceneIsActive else { return false }
        switch visibility {
        case .hidden:
            return false
        case .everyoneTenMinutes:
            return visibilityExpiresAt.map { now < $0 } ?? false
        case .whileAppOpen:
            return true
        }
    }

    private func persistVisibility() {
        defaults.set(visibility.rawValue, forKey: Key.visibility)
        if let visibilityExpiresAt {
            defaults.set(visibilityExpiresAt, forKey: Key.visibilityExpiresAt)
        } else {
            defaults.removeObject(forKey: Key.visibilityExpiresAt)
        }
    }
}
#endif
