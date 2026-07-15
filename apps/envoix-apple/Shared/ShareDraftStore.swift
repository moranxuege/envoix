import Foundation

struct ShareDraftDescriptor: Codable, Equatable, Identifiable {
    enum MediaKind: String, Codable {
        case file
        case image
        case video
    }

    static let currentSchemaVersion = 2
    static let legacySchemaVersion = 1

    let schemaVersion: Int
    let id: UUID
    let createdAtMilliseconds: UInt64
    let items: [ShareDraftItemDescriptor]

    var mediaKind: MediaKind {
        items.count == 1 ? items[0].mediaKind : .file
    }

    var contentTypeIdentifier: String {
        items.count == 1 ? items[0].contentTypeIdentifier : "com.envoix.manifest"
    }

    var fileName: String {
        items.count == 1 ? items[0].fileName : "\(items.count) items"
    }

    var byteCount: UInt64 {
        checkedShareDraftByteCount(items) ?? .max
    }

    var stagedRelativePath: String {
        items.count == 1 ? items[0].stagedRelativePath : ""
    }

    init(
        schemaVersion: Int,
        id: UUID,
        createdAtMilliseconds: UInt64,
        items: [ShareDraftItemDescriptor]
    ) {
        self.schemaVersion = schemaVersion
        self.id = id
        self.createdAtMilliseconds = createdAtMilliseconds
        self.items = items
    }

    /// Compatibility initializer retained for existing single-file tests and
    /// for emitting a legacy descriptor fixture.
    init(
        schemaVersion: Int,
        id: UUID,
        mediaKind: MediaKind,
        contentTypeIdentifier: String,
        fileName: String,
        byteCount: UInt64,
        createdAtMilliseconds: UInt64,
        stagedRelativePath: String
    ) {
        self.init(
            schemaVersion: schemaVersion,
            id: id,
            createdAtMilliseconds: createdAtMilliseconds,
            items: [
                ShareDraftItemDescriptor(
                    mediaKind: mediaKind,
                    contentTypeIdentifier: contentTypeIdentifier,
                    fileName: fileName,
                    byteCount: byteCount,
                    stagedRelativePath: stagedRelativePath
                )
            ]
        )
    }

    private enum CodingKeys: String, CodingKey {
        case schemaVersion
        case id
        case createdAtMilliseconds
        case items
        case mediaKind
        case contentTypeIdentifier
        case fileName
        case byteCount
        case stagedRelativePath
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try container.decode(Int.self, forKey: .schemaVersion)
        id = try container.decode(UUID.self, forKey: .id)
        createdAtMilliseconds = try container.decode(UInt64.self, forKey: .createdAtMilliseconds)
        if let decodedItems = try container.decodeIfPresent(
            [ShareDraftItemDescriptor].self,
            forKey: .items
        ) {
            items = decodedItems
        } else {
            guard schemaVersion == Self.legacySchemaVersion else {
                throw DecodingError.dataCorruptedError(
                    forKey: .items,
                    in: container,
                    debugDescription: "Share draft v2 requires an item list."
                )
            }
            items = [
                ShareDraftItemDescriptor(
                    mediaKind: try container.decode(MediaKind.self, forKey: .mediaKind),
                    contentTypeIdentifier: try container.decode(
                        String.self,
                        forKey: .contentTypeIdentifier
                    ),
                    fileName: try container.decode(String.self, forKey: .fileName),
                    byteCount: try container.decode(UInt64.self, forKey: .byteCount),
                    stagedRelativePath: try container.decode(String.self, forKey: .stagedRelativePath)
                )
            ]
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(schemaVersion, forKey: .schemaVersion)
        try container.encode(id, forKey: .id)
        try container.encode(createdAtMilliseconds, forKey: .createdAtMilliseconds)
        if schemaVersion == Self.legacySchemaVersion, items.count == 1, let item = items.first {
            try container.encode(item.mediaKind, forKey: .mediaKind)
            try container.encode(item.contentTypeIdentifier, forKey: .contentTypeIdentifier)
            try container.encode(item.fileName, forKey: .fileName)
            try container.encode(item.byteCount, forKey: .byteCount)
            try container.encode(item.stagedRelativePath, forKey: .stagedRelativePath)
        } else {
            try container.encode(items, forKey: .items)
        }
    }
}

struct ShareDraftItemDescriptor: Codable, Equatable {
    let mediaKind: ShareDraftDescriptor.MediaKind
    let contentTypeIdentifier: String
    let fileName: String
    let byteCount: UInt64
    let stagedRelativePath: String
}

struct ShareDraftStagingItem {
    let sourceURL: URL
    let contentTypeIdentifier: String
    let mediaKind: ShareDraftDescriptor.MediaKind
    let preferredFileName: String?
}

struct ShareDraft: Equatable {
    let descriptor: ShareDraftDescriptor
    let fileURLs: [URL]
}

private func checkedShareDraftByteCount(_ items: [ShareDraftItemDescriptor]) -> UInt64? {
    var total: UInt64 = 0
    for item in items {
        let (next, overflow) = total.addingReportingOverflow(item.byteCount)
        guard !overflow else { return nil }
        total = next
    }
    return total
}

enum ShareDraftLink {
    static func url(for id: UUID) -> URL {
        URL(string: "envoix://share/\(id.uuidString)")!
    }

    static func draftID(from url: URL) -> UUID? {
        guard url.scheme?.lowercased() == "envoix",
              url.host?.lowercased() == "share",
              url.user == nil,
              url.password == nil,
              url.port == nil,
              url.query == nil,
              url.fragment == nil else { return nil }
        let components = url.path.split(separator: "/", omittingEmptySubsequences: true)
        guard components.count == 1 else { return nil }
        return UUID(uuidString: String(components[0]))
    }
}

enum ShareDraftStoreError: LocalizedError, Equatable {
    case appGroupUnavailable
    case itemCountExceeded(limit: Int)
    case sourceIsNotRegularFile
    case sourceIsUnreadable
    case insufficientStorage(requiredBytes: UInt64, availableBytes: UInt64?)
    case invalidDraft
    case draftNotFound

    var errorDescription: String? {
        switch self {
        case .appGroupUnavailable:
            return "The Envoix shared container is unavailable."
        case let .itemCountExceeded(limit):
            return "Select between 1 and \(limit) items."
        case .sourceIsNotRegularFile:
            return "Envoix Share accepts regular files. Open folders from the main app instead."
        case .sourceIsUnreadable:
            return "The shared item could not be read. Wait for it to download, then try again."
        case let .insufficientStorage(requiredBytes, availableBytes):
            let required = ByteCountFormatter.string(
                fromByteCount: Int64(clamping: requiredBytes),
                countStyle: .file
            )
            if let availableBytes {
                let available = ByteCountFormatter.string(
                    fromByteCount: Int64(clamping: availableBytes),
                    countStyle: .file
                )
                return "Envoix needs \(required) of temporary space, but only \(available) is available."
            }
            return "Envoix needs \(required) of temporary space, but the device does not have enough free storage."
        case .invalidDraft:
            return "The shared item is incomplete or invalid. Share it again."
        case .draftNotFound:
            return "The shared item is no longer available. Share it again."
        }
    }
}

struct ShareDraftStore {
    static let appGroupIdentifier = "group.com.envoix.app.shared"
    static let defaultTimeToLive: TimeInterval = 24 * 60 * 60
    /// Share intake follows the Manifest protocol's entry-count boundary. This
    /// is a protocol compatibility limit, not a staging-space quota.
    static let maxItemCount = 10_000

    private static let directoryName = "ShareDrafts"
    fileprivate static let descriptorFileName = "draft.json"
    fileprivate static let pendingFileName = "pending.json"
    private static let claimFileName = "claim.json"
    fileprivate static let payloadDirectoryName = "payload"

    fileprivate struct PendingDraft: Codable {
        let id: UUID
    }

    struct Claim: Codable, Equatable {
        let activityID: String?
        let claimedAtMilliseconds: UInt64
    }

    struct CacheSummary: Equatable {
        var totalBytes: UInt64 = 0
        var protectedBytes: UInt64 = 0

        var removableBytes: UInt64 {
            totalBytes >= protectedBytes ? totalBytes - protectedBytes : 0
        }
    }

    private let rootDirectory: URL
    private let fileManager: FileManager
    private let timeToLive: TimeInterval
    private let now: () -> Date
    private let availableCapacity: (URL) -> Int64?

    init(
        rootDirectory: URL,
        fileManager: FileManager = .default,
        timeToLive: TimeInterval = Self.defaultTimeToLive,
        now: @escaping () -> Date = Date.init,
        availableCapacity: @escaping (URL) -> Int64? = Self.availableCapacityForImportantUsage
    ) {
        self.rootDirectory = rootDirectory.standardizedFileURL
        self.fileManager = fileManager
        self.timeToLive = timeToLive
        self.now = now
        self.availableCapacity = availableCapacity
    }

    static func live(fileManager: FileManager = .default) throws -> ShareDraftStore {
        guard let container = fileManager.containerURL(
            forSecurityApplicationGroupIdentifier: appGroupIdentifier
        ) else {
            throw ShareDraftStoreError.appGroupUnavailable
        }
        return ShareDraftStore(
            rootDirectory: container.appendingPathComponent(directoryName, isDirectory: true),
            fileManager: fileManager
        )
    }

    @discardableResult
    func stage(
        sourceURL: URL,
        contentTypeIdentifier: String,
        mediaKind: ShareDraftDescriptor.MediaKind,
        preferredFileName: String? = nil
    ) throws -> ShareDraft {
        try stage(items: [
            ShareDraftStagingItem(
                sourceURL: sourceURL,
                contentTypeIdentifier: contentTypeIdentifier,
                mediaKind: mediaKind,
                preferredFileName: preferredFileName
            )
        ])
    }

    @discardableResult
    func stage(items: [ShareDraftStagingItem]) throws -> ShareDraft {
        guard !items.isEmpty, items.count <= Self.maxItemCount else {
            throw ShareDraftStoreError.itemCountExceeded(limit: Self.maxItemCount)
        }
        let staging = try beginStaging(expectedItemCount: items.count)
        do {
            for item in items {
                try staging.append(item)
            }
            return try staging.finalize()
        } catch {
            staging.cancel()
            throw error
        }
    }

    func beginStaging(expectedItemCount: Int) throws -> ShareDraftStagingSession {
        guard (1...Self.maxItemCount).contains(expectedItemCount) else {
            throw ShareDraftStoreError.itemCountExceeded(limit: Self.maxItemCount)
        }
        try prepareRootDirectory()
        try cleanupExpired()
        return try ShareDraftStagingSession(
            rootDirectory: rootDirectory,
            fileManager: fileManager,
            expectedItemCount: expectedItemCount,
            createdAt: now(),
            availableCapacity: availableCapacity
        )
    }

    func load(id: UUID) throws -> ShareDraft {
        let descriptorURL = descriptorURL(for: id)
        guard fileManager.fileExists(atPath: descriptorURL.path) else {
            throw ShareDraftStoreError.draftNotFound
        }

        let descriptor: ShareDraftDescriptor
        do {
            descriptor = try JSONDecoder().decode(
                ShareDraftDescriptor.self,
                from: Data(contentsOf: descriptorURL)
            )
        } catch {
            throw ShareDraftStoreError.invalidDraft
        }
        guard (descriptor.schemaVersion == ShareDraftDescriptor.currentSchemaVersion
                || descriptor.schemaVersion == ShareDraftDescriptor.legacySchemaVersion),
              descriptor.id == id,
              !descriptor.items.isEmpty,
              descriptor.items.count <= Self.maxItemCount,
              checkedShareDraftByteCount(descriptor.items) != nil,
              descriptor.schemaVersion != ShareDraftDescriptor.legacySchemaVersion
                || descriptor.items.count == 1 else {
            throw ShareDraftStoreError.invalidDraft
        }

        var payloadURLs: [URL] = []
        var seenPaths = Set<String>()
        payloadURLs.reserveCapacity(descriptor.items.count)
        for item in descriptor.items {
            let expectedRelativePath = descriptor.schemaVersion == ShareDraftDescriptor.legacySchemaVersion
                ? "\(id.uuidString)/\(item.fileName)"
                : "\(id.uuidString)/\(Self.payloadDirectoryName)/\(item.fileName)"
            guard item.fileName == safeFileName(item.fileName),
                  !item.contentTypeIdentifier.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                  item.stagedRelativePath == expectedRelativePath,
                  seenPaths.insert(normalizedFileName(item.stagedRelativePath)).inserted,
                  let payloadURL = containedURL(for: item.stagedRelativePath) else {
                throw ShareDraftStoreError.invalidDraft
            }

            let values: URLResourceValues
            do {
                values = try payloadURL.resourceValues(forKeys: [
                    .isRegularFileKey,
                    .isSymbolicLinkKey,
                    .fileSizeKey,
                ])
            } catch {
                throw ShareDraftStoreError.invalidDraft
            }
            guard values.isRegularFile == true,
                  values.isSymbolicLink != true,
                  UInt64(max(0, values.fileSize ?? 0)) == item.byteCount else {
                throw ShareDraftStoreError.invalidDraft
            }
            payloadURLs.append(payloadURL)
        }
        return ShareDraft(descriptor: descriptor, fileURLs: payloadURLs)
    }

    func discard(id: UUID) throws {
        let directory = rootDirectory.appendingPathComponent(id.uuidString, isDirectory: true)
        if fileManager.fileExists(atPath: directory.path) {
            try fileManager.removeItem(at: directory)
        }
        clearPending(ifMatching: id)
    }

    func pending(preferredID: UUID? = nil) throws -> ShareDraft? {
        if let preferredID {
            return try load(id: preferredID)
        }
        guard fileManager.fileExists(atPath: pendingURL.path) else { return nil }

        let pending: PendingDraft
        do {
            pending = try JSONDecoder().decode(
                PendingDraft.self,
                from: Data(contentsOf: pendingURL)
            )
        } catch {
            try? fileManager.removeItem(at: pendingURL)
            throw ShareDraftStoreError.invalidDraft
        }
        do {
            return try load(id: pending.id)
        } catch {
            try? fileManager.removeItem(at: pendingURL)
            throw error
        }
    }

    func acknowledgePending(id: UUID) {
        clearPending(ifMatching: id)
    }

    func claim(id: UUID, activityID: String? = nil) throws {
        _ = try load(id: id)
        let normalizedActivityID = activityID?.trimmingCharacters(in: .whitespacesAndNewlines)
        let claim = Claim(
            activityID: normalizedActivityID?.isEmpty == false ? normalizedActivityID : nil,
            claimedAtMilliseconds: UInt64(max(0, now().timeIntervalSince1970 * 1_000))
        )
        try JSONEncoder().encode(claim).write(
            to: claimURL(for: id),
            options: [.atomic, .completeFileProtectionUnlessOpen]
        )
    }

    func bindClaim(id: UUID, activityID: String) throws {
        guard !activityID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw ShareDraftStoreError.invalidDraft
        }
        try claim(id: id, activityID: activityID)
    }

    func claimPending(preferredID: UUID? = nil) throws -> ShareDraft? {
        guard let draft = try pending(preferredID: preferredID) else { return nil }
        try claim(id: draft.descriptor.id)
        acknowledgePending(id: draft.descriptor.id)
        return draft
    }

    func cleanupExpired() throws {
        guard fileManager.fileExists(atPath: rootDirectory.path) else { return }
        let cutoff = now().addingTimeInterval(-timeToLive)
        let directories = try fileManager.contentsOfDirectory(
            at: rootDirectory,
            includingPropertiesForKeys: [.isDirectoryKey, .creationDateKey],
            options: [.skipsHiddenFiles]
        )
        for directory in directories {
            let values = try? directory.resourceValues(forKeys: [.isDirectoryKey, .creationDateKey])
            guard values?.isDirectory == true,
                  UUID(uuidString: directory.lastPathComponent) != nil else { continue }
            let descriptorCreationDate = descriptorCreationDate(in: directory)
            let creationDate = descriptorCreationDate ?? values?.creationDate ?? .distantPast
            let claim = claim(in: directory)
            if claim?.activityID?.isEmpty == false {
                continue
            }
            if creationDate < cutoff {
                try? fileManager.removeItem(at: directory)
            }
        }
    }

    /// Removes orphaned activity-bound drafts immediately and applies the TTL
    /// to unbound drafts. Active, paused, and retryable activities are supplied
    /// by the host app and are never removed here.
    func reconcileCache(
        protectingDraftIDs: Set<UUID>,
        protectingActivityIDs: Set<String>,
        createdBefore: Date = .distantFuture
    ) throws {
        guard fileManager.fileExists(atPath: rootDirectory.path) else { return }
        let cutoff = now().addingTimeInterval(-timeToLive)
        for directory in try draftDirectories() {
            guard let id = UUID(uuidString: directory.lastPathComponent) else { continue }
            let claim = claim(in: directory)
            let isProtected = protectingDraftIDs.contains(id)
                || claim?.activityID.map(protectingActivityIDs.contains) == true
            guard !isProtected else { continue }
            guard let descriptorDate = descriptorCreationDate(in: directory),
                  descriptorDate <= createdBefore else { continue }

            if claim?.activityID?.isEmpty == false {
                try? fileManager.removeItem(at: directory)
                clearPending(ifMatching: id)
                continue
            }
            let values = try? directory.resourceValues(forKeys: [.creationDateKey])
            let creationDate = values?.creationDate ?? descriptorDate
            if creationDate < cutoff {
                try? fileManager.removeItem(at: directory)
                clearPending(ifMatching: id)
            }
        }
    }

    /// Manual cleanup removes every draft that is not required by a protected
    /// transfer. It intentionally also clears an unclaimed pending share.
    func cleanUnprotected(
        protectingDraftIDs: Set<UUID>,
        protectingActivityIDs: Set<String>,
        createdBefore: Date = .distantFuture
    ) throws {
        guard fileManager.fileExists(atPath: rootDirectory.path) else { return }
        for directory in try draftDirectories() {
            guard let id = UUID(uuidString: directory.lastPathComponent) else { continue }
            let claim = claim(in: directory)
            let isProtected = protectingDraftIDs.contains(id)
                || claim?.activityID.map(protectingActivityIDs.contains) == true
            guard !isProtected else { continue }
            guard let creationDate = descriptorCreationDate(in: directory),
                  creationDate <= createdBefore else { continue }
            try? fileManager.removeItem(at: directory)
            clearPending(ifMatching: id)
        }
    }

    func cacheSummary(
        protectingDraftIDs: Set<UUID>,
        protectingActivityIDs: Set<String>
    ) throws -> CacheSummary {
        guard fileManager.fileExists(atPath: rootDirectory.path) else { return CacheSummary() }
        var summary = CacheSummary()
        for directory in try draftDirectories() {
            guard let id = UUID(uuidString: directory.lastPathComponent) else { continue }
            let bytes = directoryByteCount(directory)
            summary.totalBytes = addingClamped(summary.totalBytes, bytes)
            let claim = claim(in: directory)
            if protectingDraftIDs.contains(id)
                || claim?.activityID.map(protectingActivityIDs.contains) == true {
                summary.protectedBytes = addingClamped(summary.protectedBytes, bytes)
            }
        }
        return summary
    }

    func claimedDraftsByActivityID() throws -> [String: UUID] {
        guard fileManager.fileExists(atPath: rootDirectory.path) else { return [:] }
        var claims: [String: UUID] = [:]
        for directory in try draftDirectories() {
            guard let id = UUID(uuidString: directory.lastPathComponent),
                  let activityID = claim(in: directory)?.activityID,
                  !activityID.isEmpty else { continue }
            claims[activityID] = id
        }
        return claims
    }

    private func prepareRootDirectory() throws {
        try fileManager.createDirectory(at: rootDirectory, withIntermediateDirectories: true)
    }

    private func draftDirectories() throws -> [URL] {
        try fileManager.contentsOfDirectory(
            at: rootDirectory,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        ).filter { directory in
            guard UUID(uuidString: directory.lastPathComponent) != nil else { return false }
            return (try? directory.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true
        }
    }

    private func directoryByteCount(_ directory: URL) -> UInt64 {
        var total: UInt64 = 0
        guard let enumerator = fileManager.enumerator(
            at: directory,
            includingPropertiesForKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey],
            options: []
        ) else { return 0 }
        while let fileURL = enumerator.nextObject() as? URL {
            let values = try? fileURL.resourceValues(forKeys: [
                .isRegularFileKey,
                .isSymbolicLinkKey,
                .fileSizeKey,
            ])
            if values?.isSymbolicLink == true {
                enumerator.skipDescendants()
                continue
            }
            guard values?.isRegularFile == true else { continue }
            total = addingClamped(total, UInt64(max(0, values?.fileSize ?? 0)))
        }
        return total
    }

    private func addingClamped(_ lhs: UInt64, _ rhs: UInt64) -> UInt64 {
        let (sum, overflow) = lhs.addingReportingOverflow(rhs)
        return overflow ? .max : sum
    }

    private func descriptorCreationDate(in directory: URL) -> Date? {
        let descriptorURL = directory.appendingPathComponent(Self.descriptorFileName)
        guard let data = try? Data(contentsOf: descriptorURL),
              let descriptor = try? JSONDecoder().decode(ShareDraftDescriptor.self, from: data) else {
            return nil
        }
        return Date(timeIntervalSince1970: TimeInterval(descriptor.createdAtMilliseconds) / 1_000)
    }

    private func descriptorURL(for id: UUID) -> URL {
        rootDirectory
            .appendingPathComponent(id.uuidString, isDirectory: true)
            .appendingPathComponent(Self.descriptorFileName, isDirectory: false)
    }

    private func claimURL(for id: UUID) -> URL {
        rootDirectory
            .appendingPathComponent(id.uuidString, isDirectory: true)
            .appendingPathComponent(Self.claimFileName, isDirectory: false)
    }

    private func claim(in directory: URL) -> Claim? {
        let url = directory.appendingPathComponent(Self.claimFileName, isDirectory: false)
        guard let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(Claim.self, from: data)
    }

    private var pendingURL: URL {
        rootDirectory.appendingPathComponent(Self.pendingFileName, isDirectory: false)
    }

    private func clearPending(ifMatching id: UUID) {
        guard let data = try? Data(contentsOf: pendingURL),
              let pending = try? JSONDecoder().decode(PendingDraft.self, from: data),
              pending.id == id else { return }
        try? fileManager.removeItem(at: pendingURL)
    }

    private func containedURL(for relativePath: String) -> URL? {
        guard !relativePath.isEmpty, !relativePath.hasPrefix("/") else { return nil }
        let candidate = rootDirectory.appendingPathComponent(relativePath).standardizedFileURL
        let rootPath = rootDirectory.standardizedFileURL.path + "/"
        guard candidate.path.hasPrefix(rootPath) else { return nil }
        return candidate
    }

    private func safeFileName(_ candidate: String) -> String {
        let trimmed = candidate.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed != ".", trimmed != ".." else {
            return "Shared Item"
        }
        return trimmed.replacingOccurrences(of: "/", with: "-")
    }

    private func normalizedFileName(_ value: String) -> String {
        value.precomposedStringWithCanonicalMapping.lowercased()
    }

    private static func availableCapacityForImportantUsage(at url: URL) -> Int64? {
        try? url.resourceValues(forKeys: [
            .volumeAvailableCapacityForImportantUsageKey,
        ]).volumeAvailableCapacityForImportantUsage
    }
}

/// Incremental staging keeps each `NSItemProvider` file representation alive
/// only for the duration of one direct copy into the App Group. This avoids the
/// previous scratch-directory copy followed by a second App Group copy.
final class ShareDraftStagingSession {
    private enum State {
        case active
        case cancelled
        case finalized
    }

    private let lock = NSLock()
    private let rootDirectory: URL
    private let draftDirectory: URL
    private let payloadDirectory: URL
    private let fileManager: FileManager
    private let expectedItemCount: Int
    private let createdAtMilliseconds: UInt64
    private let availableCapacity: (URL) -> Int64?
    private(set) var id: UUID

    private var state = State.active
    private var operationInFlight = false
    private var usedNames = Set<String>()
    private var descriptors: [ShareDraftItemDescriptor] = []
    private var payloadURLs: [URL] = []

    init(
        rootDirectory: URL,
        fileManager: FileManager,
        expectedItemCount: Int,
        createdAt: Date,
        availableCapacity: @escaping (URL) -> Int64?
    ) throws {
        let id = UUID()
        self.id = id
        self.rootDirectory = rootDirectory
        self.fileManager = fileManager
        self.expectedItemCount = expectedItemCount
        self.createdAtMilliseconds = UInt64(max(0, createdAt.timeIntervalSince1970 * 1_000))
        self.availableCapacity = availableCapacity
        draftDirectory = rootDirectory.appendingPathComponent(id.uuidString, isDirectory: true)
        payloadDirectory = draftDirectory.appendingPathComponent(
            ShareDraftStore.payloadDirectoryName,
            isDirectory: true
        )
        do {
            try fileManager.createDirectory(at: draftDirectory, withIntermediateDirectories: false)
            try fileManager.createDirectory(at: payloadDirectory, withIntermediateDirectories: false)
        } catch {
            try? fileManager.removeItem(at: draftDirectory)
            throw error
        }
    }

    deinit {
        cancel()
    }

    func append(_ item: ShareDraftStagingItem) throws {
        try beginOperation()
        do {
            guard !item.contentTypeIdentifier.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                throw ShareDraftStoreError.invalidDraft
            }
            let sourceValues: URLResourceValues
            do {
                sourceValues = try item.sourceURL.resourceValues(forKeys: [
                    .isRegularFileKey,
                    .isSymbolicLinkKey,
                    .fileSizeKey,
                ])
            } catch {
                throw ShareDraftStoreError.sourceIsUnreadable
            }
            guard sourceValues.isRegularFile == true, sourceValues.isSymbolicLink != true else {
                throw ShareDraftStoreError.sourceIsNotRegularFile
            }
            guard fileManager.isReadableFile(atPath: item.sourceURL.path) else {
                throw ShareDraftStoreError.sourceIsUnreadable
            }

            let byteCount = UInt64(max(0, sourceValues.fileSize ?? 0))
            if let available = availableCapacity(payloadDirectory),
               available >= 0,
               UInt64(available) < byteCount {
                throw ShareDraftStoreError.insufficientStorage(
                    requiredBytes: byteCount,
                    availableBytes: UInt64(available)
                )
            }

            let fileName = uniqueFileName(
                item.preferredFileName ?? item.sourceURL.lastPathComponent
            )
            let payloadURL = payloadDirectory.appendingPathComponent(fileName, isDirectory: false)
            do {
                try fileManager.copyItem(at: item.sourceURL, to: payloadURL)
            } catch {
                throw normalizedCopyError(error, requiredBytes: byteCount)
            }
            try fileManager.setAttributes(
                [.protectionKey: FileProtectionType.completeUnlessOpen],
                ofItemAtPath: payloadURL.path
            )
            let copiedValues = try payloadURL.resourceValues(forKeys: [
                .isRegularFileKey,
                .isSymbolicLinkKey,
                .fileSizeKey,
            ])
            guard copiedValues.isRegularFile == true,
                  copiedValues.isSymbolicLink != true,
                  UInt64(max(0, copiedValues.fileSize ?? 0)) == byteCount else {
                throw ShareDraftStoreError.invalidDraft
            }

            try finishAppend(
                descriptor: ShareDraftItemDescriptor(
                    mediaKind: item.mediaKind,
                    contentTypeIdentifier: item.contentTypeIdentifier,
                    fileName: fileName,
                    byteCount: byteCount,
                    stagedRelativePath: "\(id.uuidString)/\(ShareDraftStore.payloadDirectoryName)/\(fileName)"
                ),
                payloadURL: payloadURL
            )
        } catch {
            failOperation()
            throw error
        }
    }

    func finalize() throws -> ShareDraft {
        try beginFinalization()
        let descriptor = ShareDraftDescriptor(
            schemaVersion: ShareDraftDescriptor.currentSchemaVersion,
            id: id,
            createdAtMilliseconds: createdAtMilliseconds,
            items: descriptors
        )
        do {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.sortedKeys]
            try encoder.encode(descriptor).write(
                to: draftDirectory.appendingPathComponent(ShareDraftStore.descriptorFileName),
                options: [.atomic, .completeFileProtectionUnlessOpen]
            )
            try JSONEncoder().encode(ShareDraftStore.PendingDraft(id: id)).write(
                to: rootDirectory.appendingPathComponent(ShareDraftStore.pendingFileName),
                options: [.atomic, .completeFileProtectionUnlessOpen]
            )

            lock.lock()
            let wasCancelled = state == .cancelled
            if !wasCancelled {
                state = .finalized
            }
            operationInFlight = false
            lock.unlock()
            if wasCancelled {
                try? fileManager.removeItem(at: draftDirectory)
                clearPendingIfMatching()
                throw CancellationError()
            }
            return ShareDraft(descriptor: descriptor, fileURLs: payloadURLs)
        } catch {
            failOperation()
            throw error
        }
    }

    func cancel() {
        lock.lock()
        guard state == .active else {
            lock.unlock()
            return
        }
        state = .cancelled
        let removeNow = !operationInFlight
        lock.unlock()
        if removeNow {
            try? fileManager.removeItem(at: draftDirectory)
            clearPendingIfMatching()
        }
    }

    private func beginOperation() throws {
        lock.lock()
        defer { lock.unlock() }
        guard state == .active,
              !operationInFlight,
              descriptors.count < expectedItemCount else {
            throw CancellationError()
        }
        operationInFlight = true
    }

    private func finishAppend(
        descriptor: ShareDraftItemDescriptor,
        payloadURL: URL
    ) throws {
        lock.lock()
        let wasCancelled = state == .cancelled
        if !wasCancelled {
            descriptors.append(descriptor)
            payloadURLs.append(payloadURL)
        }
        operationInFlight = false
        lock.unlock()
        if wasCancelled {
            try? fileManager.removeItem(at: draftDirectory)
            clearPendingIfMatching()
            throw CancellationError()
        }
    }

    private func beginFinalization() throws {
        lock.lock()
        defer { lock.unlock() }
        guard state == .active,
              !operationInFlight,
              descriptors.count == expectedItemCount else {
            throw ShareDraftStoreError.invalidDraft
        }
        operationInFlight = true
    }

    private func failOperation() {
        lock.lock()
        if state == .active {
            state = .cancelled
        }
        operationInFlight = false
        lock.unlock()
        try? fileManager.removeItem(at: draftDirectory)
        clearPendingIfMatching()
    }

    private func uniqueFileName(_ candidate: String) -> String {
        let safeName = safeFileName(candidate)
        if usedNames.insert(normalizedFileName(safeName)).inserted {
            return safeName
        }
        let path = safeName as NSString
        let pathExtension = path.pathExtension
        let stem = path.deletingPathExtension
        for suffix in 2...(ShareDraftStore.maxItemCount + 1) {
            let fileName = pathExtension.isEmpty
                ? "\(stem) (\(suffix))"
                : "\(stem) (\(suffix)).\(pathExtension)"
            if usedNames.insert(normalizedFileName(fileName)).inserted {
                return fileName
            }
        }
        return "\(UUID().uuidString)-\(safeName)"
    }

    private func safeFileName(_ candidate: String) -> String {
        let trimmed = candidate.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed != ".", trimmed != ".." else {
            return "Shared Item"
        }
        return trimmed.replacingOccurrences(of: "/", with: "-")
    }

    private func normalizedFileName(_ value: String) -> String {
        value.precomposedStringWithCanonicalMapping.lowercased()
    }

    private func normalizedCopyError(_ error: Error, requiredBytes: UInt64) -> Error {
        let nsError = error as NSError
        if nsError.domain == NSCocoaErrorDomain,
           nsError.code == CocoaError.fileWriteOutOfSpace.rawValue {
            let available = availableCapacity(payloadDirectory).flatMap { value in
                value >= 0 ? UInt64(value) : nil
            }
            return ShareDraftStoreError.insufficientStorage(
                requiredBytes: requiredBytes,
                availableBytes: available
            )
        }
        return error
    }

    private func clearPendingIfMatching() {
        let pendingURL = rootDirectory.appendingPathComponent(ShareDraftStore.pendingFileName)
        guard let data = try? Data(contentsOf: pendingURL),
              let pending = try? JSONDecoder().decode(
                ShareDraftStore.PendingDraft.self,
                from: data
              ),
              pending.id == id else { return }
        try? fileManager.removeItem(at: pendingURL)
    }
}

/// Resolves the race between a background provider callback and a user closing
/// the extension. A draft accepted before cancellation is returned for cleanup;
/// a draft produced afterward is rejected so its callback can discard it.
final class ShareDraftImportGate {
    private let lock = NSLock()
    private var isCancelled = false
    private var acceptedDraftID: UUID?

    func accept(_ id: UUID) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard !isCancelled else { return false }
        acceptedDraftID = id
        return true
    }

    func cancel() -> UUID? {
        lock.lock()
        defer { lock.unlock() }
        isCancelled = true
        defer { acceptedDraftID = nil }
        return acceptedDraftID
    }
}
