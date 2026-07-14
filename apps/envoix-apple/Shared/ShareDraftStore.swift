import Foundation

struct ShareDraftDescriptor: Codable, Equatable, Identifiable {
    enum MediaKind: String, Codable {
        case file
        case image
        case video
    }

    static let currentSchemaVersion = 1

    let schemaVersion: Int
    let id: UUID
    let mediaKind: MediaKind
    let contentTypeIdentifier: String
    let fileName: String
    let byteCount: UInt64
    let createdAtMilliseconds: UInt64
    let stagedRelativePath: String
}

struct ShareDraft: Equatable {
    let descriptor: ShareDraftDescriptor
    let fileURL: URL
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
    case sourceIsNotRegularFile
    case sourceIsUnreadable
    case quotaExceeded(limitBytes: UInt64)
    case invalidDraft
    case draftNotFound

    var errorDescription: String? {
        switch self {
        case .appGroupUnavailable:
            return "The Envoix shared container is unavailable."
        case .sourceIsNotRegularFile:
            return "Envoix can currently share one file at a time. Folders require Manifest support."
        case .sourceIsUnreadable:
            return "The shared item could not be read. Wait for it to download, then try again."
        case let .quotaExceeded(limitBytes):
            return "The item exceeds Envoix's temporary sharing limit of \(ByteCountFormatter.string(fromByteCount: Int64(limitBytes), countStyle: .file))."
        case .invalidDraft:
            return "The shared item is incomplete or invalid. Share it again."
        case .draftNotFound:
            return "The shared item is no longer available. Share it again."
        }
    }
}

struct ShareDraftStore {
    static let appGroupIdentifier = "group.com.envoix.app.shared"
    static let defaultQuotaBytes: UInt64 = 4 * 1_024 * 1_024 * 1_024
    static let defaultTimeToLive: TimeInterval = 24 * 60 * 60

    private static let directoryName = "ShareDrafts"
    private static let descriptorFileName = "draft.json"
    private static let pendingFileName = "pending.json"

    private struct PendingDraft: Codable {
        let id: UUID
    }

    private let rootDirectory: URL
    private let fileManager: FileManager
    private let quotaBytes: UInt64
    private let timeToLive: TimeInterval
    private let now: () -> Date

    init(
        rootDirectory: URL,
        fileManager: FileManager = .default,
        quotaBytes: UInt64 = Self.defaultQuotaBytes,
        timeToLive: TimeInterval = Self.defaultTimeToLive,
        now: @escaping () -> Date = Date.init
    ) {
        self.rootDirectory = rootDirectory.standardizedFileURL
        self.fileManager = fileManager
        self.quotaBytes = quotaBytes
        self.timeToLive = timeToLive
        self.now = now
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
        try prepareRootDirectory()
        try cleanupExpired()

        let sourceValues = try sourceURL.resourceValues(forKeys: [
            .isRegularFileKey,
            .isSymbolicLinkKey,
            .fileSizeKey,
        ])
        guard sourceValues.isRegularFile == true, sourceValues.isSymbolicLink != true else {
            throw ShareDraftStoreError.sourceIsNotRegularFile
        }
        guard fileManager.isReadableFile(atPath: sourceURL.path) else {
            throw ShareDraftStoreError.sourceIsUnreadable
        }

        let byteCount = UInt64(max(0, sourceValues.fileSize ?? 0))
        let occupiedBytes = try stagedByteCount()
        guard byteCount <= quotaBytes, occupiedBytes <= quotaBytes - byteCount else {
            throw ShareDraftStoreError.quotaExceeded(limitBytes: quotaBytes)
        }

        let id = UUID()
        let draftDirectory = rootDirectory.appendingPathComponent(id.uuidString, isDirectory: true)
        let fileName = safeFileName(preferredFileName ?? sourceURL.lastPathComponent)
        let payloadURL = draftDirectory.appendingPathComponent(fileName, isDirectory: false)
        let relativePath = "\(id.uuidString)/\(fileName)"
        let createdAtMilliseconds = UInt64(max(0, now().timeIntervalSince1970 * 1_000))
        let descriptor = ShareDraftDescriptor(
            schemaVersion: ShareDraftDescriptor.currentSchemaVersion,
            id: id,
            mediaKind: mediaKind,
            contentTypeIdentifier: contentTypeIdentifier,
            fileName: fileName,
            byteCount: byteCount,
            createdAtMilliseconds: createdAtMilliseconds,
            stagedRelativePath: relativePath
        )

        do {
            try fileManager.createDirectory(at: draftDirectory, withIntermediateDirectories: false)
            try fileManager.copyItem(at: sourceURL, to: payloadURL)
            try fileManager.setAttributes(
                [.protectionKey: FileProtectionType.completeUnlessOpen],
                ofItemAtPath: payloadURL.path
            )
            let copiedValues = try payloadURL.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
            guard copiedValues.isRegularFile == true,
                  UInt64(max(0, copiedValues.fileSize ?? 0)) == byteCount else {
                throw ShareDraftStoreError.invalidDraft
            }
            try encode(descriptor).write(
                to: descriptorURL(for: id),
                options: [.atomic, .completeFileProtectionUnlessOpen]
            )
            try JSONEncoder().encode(PendingDraft(id: id)).write(
                to: pendingURL,
                options: [.atomic, .completeFileProtectionUnlessOpen]
            )
            return ShareDraft(descriptor: descriptor, fileURL: payloadURL)
        } catch {
            try? fileManager.removeItem(at: draftDirectory)
            throw error
        }
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
        guard descriptor.schemaVersion == ShareDraftDescriptor.currentSchemaVersion,
              descriptor.id == id,
              descriptor.fileName == safeFileName(descriptor.fileName),
              descriptor.stagedRelativePath == "\(id.uuidString)/\(descriptor.fileName)",
              let payloadURL = containedURL(for: descriptor.stagedRelativePath) else {
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
              UInt64(max(0, values.fileSize ?? 0)) == descriptor.byteCount else {
            throw ShareDraftStoreError.invalidDraft
        }
        return ShareDraft(descriptor: descriptor, fileURL: payloadURL)
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

    func claimPending(preferredID: UUID? = nil) throws -> ShareDraft? {
        guard let draft = try pending(preferredID: preferredID) else { return nil }
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
            guard values?.isDirectory == true else { continue }
            let descriptorCreationDate = descriptorCreationDate(in: directory)
            let creationDate = descriptorCreationDate ?? values?.creationDate ?? .distantPast
            if creationDate < cutoff {
                try? fileManager.removeItem(at: directory)
            }
        }
    }

    private func prepareRootDirectory() throws {
        try fileManager.createDirectory(at: rootDirectory, withIntermediateDirectories: true)
    }

    private func stagedByteCount() throws -> UInt64 {
        guard fileManager.fileExists(atPath: rootDirectory.path) else { return 0 }
        let directories = try fileManager.contentsOfDirectory(
            at: rootDirectory,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        )
        return directories.reduce(into: UInt64(0)) { total, directory in
            guard let id = UUID(uuidString: directory.lastPathComponent),
                  let draft = try? load(id: id) else { return }
            total += draft.descriptor.byteCount
        }
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

    private func encode(_ descriptor: ShareDraftDescriptor) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        return try encoder.encode(descriptor)
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
