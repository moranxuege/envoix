import Foundation
import Security

struct RememberedPeerSummary: Equatable, Identifiable, CustomDebugStringConvertible {
    let relationshipID: String
    var label: String
    var generation: UInt64
    var previousGeneration: UInt64?
    let broker: String
    let relay: String

    var id: String { relationshipID }

    var debugDescription: String {
        "RememberedPeerSummary(label: \(label), generation: \(generation))"
    }
}

struct RememberedPeerSessionMaterial: Equatable {
    let summary: RememberedPeerSummary
    let opaqueCredential: Data
}

private struct RememberedPeerRecord: Codable {
    let relationshipID: String
    var label: String
    let credentialReference: String
    var generation: UInt64
    var previousGeneration: UInt64?
    let broker: String
    let relay: String

    var summary: RememberedPeerSummary {
        RememberedPeerSummary(
            relationshipID: relationshipID,
            label: label,
            generation: generation,
            previousGeneration: previousGeneration,
            broker: broker,
            relay: relay
        )
    }
}

struct PendingRememberedPeer {
    let relationshipID: String
    let label: String
    let credentialReference: String
    let broker: String
    let relay: String
}

enum RememberedPeerStoreError: LocalizedError {
    case invalidLabel
    case missingCredential
    case activeTransfer
    case inactiveSession
    case credentialStorageUnavailable
    case credentialInteractionRequired
    case keychain(OSStatus)
    case corruptMetadata

    var errorDescription: String? {
        switch self {
        case .invalidLabel:
            return "Enter a device label."
        case .missingCredential:
            return "This remembered device is unavailable and must be paired again."
        case .activeTransfer:
            return "This remembered device is already in use."
        case .inactiveSession:
            return "This remembered-device session is no longer active."
        case .credentialStorageUnavailable:
            return "Local credential storage is unavailable."
        case .credentialInteractionRequired:
            return "This remembered device must be paired again before Envoix can use it."
        case .keychain:
            return "Protected credential storage is unavailable."
        case .corruptMetadata:
            return "Remembered-device metadata is corrupt."
        }
    }
}

final class RememberedPeerStore: @unchecked Sendable {
    static let shared = RememberedPeerStore()

    private let lock = NSLock()
    private let credentialStore: RememberedCredentialStoring
    private let metadataFileURL: URL?
    private var activeRelationships = Set<String>()

    init(
        credentialStore: RememberedCredentialStoring? = nil,
        metadataFileURL: URL? = nil
    ) {
        self.credentialStore = credentialStore ?? Self.makeDefaultCredentialStore()
        self.metadataFileURL = metadataFileURL
    }

    static func makeDefaultCredentialStore() -> RememberedCredentialStoring {
        #if os(macOS)
        MacOSFileCredentialStore()
        #else
        AppleCredentialStore()
        #endif
    }

    func prepare(label: String, broker: String, relay: String) throws -> PendingRememberedPeer {
        let label = label.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !label.isEmpty, label.count <= 64 else {
            throw RememberedPeerStoreError.invalidLabel
        }
        return PendingRememberedPeer(
            relationshipID: UUID().uuidString,
            label: label,
            credentialReference: UUID().uuidString,
            broker: broker,
            relay: relay
        )
    }

    func peers() throws -> [RememberedPeerSummary] {
        try lock.withEnvoixLock {
            try readMetadata()
                .sorted { $0.label.localizedCaseInsensitiveCompare($1.label) == .orderedAscending }
                .map(\.summary)
        }
    }

    func credential(for peer: RememberedPeerSummary) throws -> Data {
        try sessionMaterial(relationshipID: peer.relationshipID).opaqueCredential
    }

    func sessionMaterial(relationshipID: String) throws -> RememberedPeerSessionMaterial {
        try lock.withEnvoixLock {
            guard let record = try readMetadata().first(where: {
                $0.relationshipID == relationshipID
            }) else {
                throw RememberedPeerStoreError.missingCredential
            }
            return RememberedPeerSessionMaterial(
                summary: record.summary,
                opaqueCredential: try credentialStore.get(record.credentialReference)
            )
        }
    }

    func acquireSession(_ relationshipID: String) throws {
        try lock.withEnvoixLock {
            guard activeRelationships.insert(relationshipID).inserted else {
                throw RememberedPeerStoreError.activeTransfer
            }
        }
    }

    func releaseSession(_ relationshipID: String) {
        _ = lock.withEnvoixLock {
            activeRelationships.remove(relationshipID)
        }
    }

    func create(
        _ pending: PendingRememberedPeer,
        opaqueCredential: Data,
        generation: UInt64
    ) throws {
        try lock.withEnvoixLock {
            guard activeRelationships.contains(pending.relationshipID) else {
                throw RememberedPeerStoreError.inactiveSession
            }
            try credentialStore.put(pending.credentialReference, opaqueCredential)
            do {
                var peers = try readMetadata()
                peers.removeAll { $0.relationshipID == pending.relationshipID }
                peers.append(RememberedPeerRecord(
                    relationshipID: pending.relationshipID,
                    label: pending.label,
                    credentialReference: pending.credentialReference,
                    generation: generation,
                    previousGeneration: nil,
                    broker: pending.broker,
                    relay: pending.relay
                ))
                try writeMetadata(peers)
            } catch {
                try? credentialStore.delete(pending.credentialReference)
                throw error
            }
        }
    }

    func rotate(
        relationshipID: String,
        opaqueCredential: Data,
        generation: UInt64
    ) throws {
        try lock.withEnvoixLock {
            guard activeRelationships.contains(relationshipID) else {
                throw RememberedPeerStoreError.inactiveSession
            }
            var peers = try readMetadata()
            guard let index = peers.firstIndex(where: { $0.relationshipID == relationshipID }) else {
                throw RememberedPeerStoreError.missingCredential
            }
            let oldGeneration = peers[index].generation
            guard generation >= oldGeneration else {
                throw RememberedPeerStoreError.corruptMetadata
            }
            if generation == oldGeneration {
                return
            }
            try credentialStore.put(peers[index].credentialReference, opaqueCredential)
            peers[index].previousGeneration = oldGeneration
            peers[index].generation = generation
            try writeMetadata(peers)
        }
    }

    func delete(_ peer: RememberedPeerSummary) throws {
        try lock.withEnvoixLock {
            guard !activeRelationships.contains(peer.relationshipID) else {
                throw RememberedPeerStoreError.activeTransfer
            }
            var peers = try readMetadata()
            guard let record = peers.first(where: {
                $0.relationshipID == peer.relationshipID
            }) else {
                return
            }
            try credentialStore.delete(record.credentialReference)
            peers.removeAll { $0.relationshipID == peer.relationshipID }
            try writeMetadata(peers)
        }
    }

    func delete(relationshipID: String) {
        guard let peer = try? peers().first(where: { $0.relationshipID == relationshipID }) else {
            return
        }
        try? delete(peer)
    }

    private func readMetadata() throws -> [RememberedPeerRecord] {
        let url = try metadataURL()
        guard FileManager.default.fileExists(atPath: url.path) else { return [] }
        do {
            return try JSONDecoder().decode(
                [RememberedPeerRecord].self,
                from: Data(contentsOf: url)
            )
        } catch {
            throw RememberedPeerStoreError.corruptMetadata
        }
    }

    private func writeMetadata(_ peers: [RememberedPeerRecord]) throws {
        let data = try JSONEncoder().encode(peers)
        try data.write(to: metadataURL(), options: [.atomic, .completeFileProtection])
    }

    private func metadataURL() throws -> URL {
        if let metadataFileURL {
            let directory = metadataFileURL.deletingLastPathComponent()
            try FileManager.default.createDirectory(
                at: directory,
                withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700]
            )
            return metadataFileURL
        }
        guard let support = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else {
            throw RememberedPeerStoreError.corruptMetadata
        }
        let directory = support.appendingPathComponent("envoix/relationships", isDirectory: true)
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        return directory.appendingPathComponent("remembered-peers-v1.json")
    }
}

final class RememberPersistenceContext: @unchecked Sendable {
    private enum Target {
        case pending(PendingRememberedPeer)
        case relationship(String)
    }

    private let lock = NSLock()
    private let target: Target
    private let relationshipLease: String?
    private var createdRelationshipID: String?

    init(pending: PendingRememberedPeer) throws {
        try RememberedPeerStore.shared.acquireSession(pending.relationshipID)
        target = .pending(pending)
        relationshipLease = pending.relationshipID
    }

    init(peer: RememberedPeerSummary) throws {
        try RememberedPeerStore.shared.acquireSession(peer.relationshipID)
        target = .relationship(peer.relationshipID)
        relationshipLease = peer.relationshipID
    }

    deinit {
        if let relationshipLease {
            RememberedPeerStore.shared.releaseSession(relationshipLease)
        }
    }

    func persist(_ opaqueCredential: Data, generation: UInt64) -> Bool {
        lock.withEnvoixLock {
            do {
                if let createdRelationshipID {
                    try RememberedPeerStore.shared.rotate(
                        relationshipID: createdRelationshipID,
                        opaqueCredential: opaqueCredential,
                        generation: generation
                    )
                } else {
                    switch target {
                    case let .pending(pending):
                        try RememberedPeerStore.shared.create(
                            pending,
                            opaqueCredential: opaqueCredential,
                            generation: generation
                        )
                        createdRelationshipID = pending.relationshipID
                    case let .relationship(relationshipID):
                        try RememberedPeerStore.shared.rotate(
                            relationshipID: relationshipID,
                            opaqueCredential: opaqueCredential,
                            generation: generation
                        )
                    }
                }
                return true
            } catch {
                return false
            }
        }
    }
}

protocol RememberedCredentialStoring {
    func put(_ reference: String, _ credential: Data) throws
    func get(_ reference: String) throws -> Data
    func delete(_ reference: String) throws
}

#if os(macOS)
/// Stores opaque remembered-room credentials in the current user's Application
/// Support directory so ad-hoc development builds never request Keychain access.
final class MacOSFileCredentialStore: RememberedCredentialStoring {
    private static let directoryPermissions = 0o700
    private static let credentialPermissions = 0o600
    private static let credentialDirectoryName = "credentials-v1"

    private let configuredDirectoryURL: URL?

    init(directoryURL: URL? = nil) {
        configuredDirectoryURL = directoryURL
    }

    func put(_ reference: String, _ credential: Data) throws {
        let url = try credentialURL(reference)
        do {
            try credential.write(to: url, options: [.atomic, .completeFileProtection])
            try FileManager.default.setAttributes(
                [.posixPermissions: Self.credentialPermissions],
                ofItemAtPath: url.path
            )
        } catch {
            throw RememberedPeerStoreError.credentialStorageUnavailable
        }
    }

    func get(_ reference: String) throws -> Data {
        let url = try credentialURL(reference)
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw RememberedPeerStoreError.missingCredential
        }
        do {
            return try Data(contentsOf: url)
        } catch {
            throw RememberedPeerStoreError.credentialStorageUnavailable
        }
    }

    func delete(_ reference: String) throws {
        let url = try credentialURL(reference)
        guard FileManager.default.fileExists(atPath: url.path) else { return }
        do {
            try FileManager.default.removeItem(at: url)
        } catch {
            throw RememberedPeerStoreError.credentialStorageUnavailable
        }
    }

    private func credentialURL(_ reference: String) throws -> URL {
        guard UUID(uuidString: reference) != nil else {
            throw RememberedPeerStoreError.corruptMetadata
        }
        return try directoryURL().appendingPathComponent(reference, isDirectory: false)
    }

    private func directoryURL() throws -> URL {
        let directory: URL
        if let configuredDirectoryURL {
            directory = configuredDirectoryURL
        } else {
            guard let support = FileManager.default.urls(
                for: .applicationSupportDirectory,
                in: .userDomainMask
            ).first else {
                throw RememberedPeerStoreError.credentialStorageUnavailable
            }
            directory = support
                .appendingPathComponent("envoix/relationships", isDirectory: true)
                .appendingPathComponent(Self.credentialDirectoryName, isDirectory: true)
        }
        do {
            try FileManager.default.createDirectory(
                at: directory,
                withIntermediateDirectories: true,
                attributes: [.posixPermissions: Self.directoryPermissions]
            )
            try FileManager.default.setAttributes(
                [.posixPermissions: Self.directoryPermissions],
                ofItemAtPath: directory.path
            )
            return directory
        } catch {
            throw RememberedPeerStoreError.credentialStorageUnavailable
        }
    }
}
#else
final class AppleCredentialStore: RememberedCredentialStoring {
    private let service = "com.envoix.remembered-credential.v1"

    func put(_ reference: String, _ credential: Data) throws {
        let query = baseQuery(reference)
        let update: [CFString: Any] = [
            kSecValueData: credential,
            kSecAttrAccessible: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
        ]
        let updateStatus = SecItemUpdate(query as CFDictionary, update as CFDictionary)
        if updateStatus == errSecSuccess { return }
        guard updateStatus == errSecItemNotFound else {
            throw RememberedPeerStoreError.keychain(updateStatus)
        }
        var item = query
        update.forEach { item[$0] = $1 }
        let addStatus = SecItemAdd(item as CFDictionary, nil)
        guard addStatus == errSecSuccess else {
            throw RememberedPeerStoreError.keychain(addStatus)
        }
    }

    func get(_ reference: String) throws -> Data {
        var query = baseQuery(reference)
        query[kSecReturnData] = true
        query[kSecMatchLimit] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound {
            throw RememberedPeerStoreError.missingCredential
        }
        guard status == errSecSuccess, let data = result as? Data else {
            throw RememberedPeerStoreError.keychain(status)
        }
        return data
    }

    func delete(_ reference: String) throws {
        let status = SecItemDelete(baseQuery(reference) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw RememberedPeerStoreError.keychain(status)
        }
    }

    private func baseQuery(_ reference: String) -> [CFString: Any] {
        [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: reference,
            kSecAttrSynchronizable: false,
        ]
    }
}
#endif

extension NSLock {
    func withEnvoixLock<T>(_ body: () throws -> T) rethrows -> T {
        lock()
        defer { unlock() }
        return try body()
    }
}
