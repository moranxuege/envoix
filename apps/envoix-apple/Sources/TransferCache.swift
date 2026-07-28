import Foundation

struct TransferCacheSummary: Equatable {
    var totalBytes: UInt64 = 0
    var protectedBytes: UInt64 = 0

    var removableBytes: UInt64 {
        totalBytes >= protectedBytes ? totalBytes - protectedBytes : 0
    }

    mutating func include(total: UInt64, protected: UInt64) {
        totalBytes = addingClamped(totalBytes, total)
        protectedBytes = addingClamped(protectedBytes, protected)
    }

    private func addingClamped(_ lhs: UInt64, _ rhs: UInt64) -> UInt64 {
        let (sum, overflow) = lhs.addingReportingOverflow(rhs)
        return overflow ? .max : sum
    }
}

struct TransferCacheStore {
    private let fileManager: FileManager
    private let receiveStagingRoot: URL
    private let includeSharedDrafts: Bool

    init(
        fileManager: FileManager = .default,
        applicationSupportDirectory: URL? = nil,
        includeSharedDrafts: Bool = true
    ) {
        self.fileManager = fileManager
        self.includeSharedDrafts = includeSharedDrafts
        let supportDirectory = applicationSupportDirectory
            ?? fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? fileManager.temporaryDirectory
        receiveStagingRoot = supportDirectory
            .appendingPathComponent("envoix/receive-staging", isDirectory: true)
    }

    func reconcile(
        protectingDraftIDs: Set<UUID>,
        protectingActivityIDs: Set<String>,
        createdBefore: Date
    ) throws {
        try cleanReceiveStaging(
            protectingActivityIDs: protectingActivityIDs,
            createdBefore: createdBefore
        )
        #if os(iOS)
        if includeSharedDrafts {
            try ShareDraftStore.live(fileManager: fileManager).reconcileCache(
                protectingDraftIDs: protectingDraftIDs,
                protectingActivityIDs: protectingActivityIDs,
                createdBefore: createdBefore
            )
        }
        #endif
    }

    func cleanUnprotected(
        protectingDraftIDs: Set<UUID>,
        protectingActivityIDs: Set<String>,
        createdBefore: Date
    ) throws {
        try cleanReceiveStaging(
            protectingActivityIDs: protectingActivityIDs,
            createdBefore: createdBefore
        )
        #if os(iOS)
        if includeSharedDrafts {
            try ShareDraftStore.live(fileManager: fileManager).cleanUnprotected(
                protectingDraftIDs: protectingDraftIDs,
                protectingActivityIDs: protectingActivityIDs,
                createdBefore: createdBefore
            )
        }
        #endif
    }

    func summary(
        protectingDraftIDs: Set<UUID>,
        protectingActivityIDs: Set<String>
    ) throws -> TransferCacheSummary {
        var summary = receiveStagingSummary(protectingActivityIDs: protectingActivityIDs)
        #if os(iOS)
        if includeSharedDrafts {
            let share = try ShareDraftStore.live(fileManager: fileManager).cacheSummary(
                protectingDraftIDs: protectingDraftIDs,
                protectingActivityIDs: protectingActivityIDs
            )
            summary.include(total: share.totalBytes, protected: share.protectedBytes)
        }
        #endif
        return summary
    }

    private func cleanReceiveStaging(
        protectingActivityIDs: Set<String>,
        createdBefore: Date
    ) throws {
        guard fileManager.fileExists(atPath: receiveStagingRoot.path) else { return }
        for directory in try stagingDirectories() {
            guard !protectingActivityIDs.contains(directory.lastPathComponent) else { continue }
            let creationDate = try? directory.resourceValues(forKeys: [.creationDateKey]).creationDate
            guard let creationDate, creationDate <= createdBefore else { continue }
            try? fileManager.removeItem(at: directory)
        }
    }

    private func receiveStagingSummary(
        protectingActivityIDs: Set<String>
    ) -> TransferCacheSummary {
        guard fileManager.fileExists(atPath: receiveStagingRoot.path),
              let directories = try? stagingDirectories() else { return TransferCacheSummary() }
        var summary = TransferCacheSummary()
        for directory in directories {
            let bytes = directoryByteCount(directory)
            summary.include(
                total: bytes,
                protected: protectingActivityIDs.contains(directory.lastPathComponent) ? bytes : 0
            )
        }
        return summary
    }

    private func stagingDirectories() throws -> [URL] {
        try fileManager.contentsOfDirectory(
            at: receiveStagingRoot,
            includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey],
            options: [.skipsHiddenFiles]
        ).filter { url in
            let values = try? url.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
            return values?.isDirectory == true && values?.isSymbolicLink != true
        }
    }

    private func directoryByteCount(_ directory: URL) -> UInt64 {
        guard let enumerator = fileManager.enumerator(
            at: directory,
            includingPropertiesForKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey],
            options: []
        ) else { return 0 }
        var total: UInt64 = 0
        while let url = enumerator.nextObject() as? URL {
            let values = try? url.resourceValues(forKeys: [
                .isRegularFileKey,
                .isSymbolicLinkKey,
                .fileSizeKey,
            ])
            if values?.isSymbolicLink == true {
                enumerator.skipDescendants()
                continue
            }
            guard values?.isRegularFile == true else { continue }
            let bytes = UInt64(max(0, values?.fileSize ?? 0))
            let (sum, overflow) = total.addingReportingOverflow(bytes)
            total = overflow ? .max : sum
        }
        return total
    }
}
