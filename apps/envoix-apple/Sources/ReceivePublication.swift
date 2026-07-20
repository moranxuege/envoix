import Foundation
import Darwin
import EnvoixCore

// Extracted from Support.swift (2026-07-20 split, no behavior change)

/// Returns a completed receive only when the final path still names a regular
/// file with the byte count reported by the transfer core. A completion receipt
/// may legitimately outlive a moved/deleted file, so it must not drive a
/// "Saved file" UI on its own.
func availableCompletedFileURL(path: String, expectedBytes: UInt64) -> URL? {
    let path = path.trimmed
    guard !path.isEmpty else { return nil }
    let url = URL(fileURLWithPath: path)
    guard let values = try? url.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey]),
          values.isRegularFile == true else {
        return nil
    }
    if expectedBytes > 0 {
        guard let fileSize = values.fileSize, fileSize >= 0, UInt64(fileSize) == expectedBytes else {
            return nil
        }
    }
    return url
}

func availableCompletedDirectoryURL(path: String) -> URL? {
    let path = path.trimmed
    guard !path.isEmpty else { return nil }
    let url = URL(fileURLWithPath: path, isDirectory: true)
    guard let values = try? url.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey]),
          values.isDirectory == true,
          values.isSymbolicLink != true else {
        return nil
    }
    return url
}

/// Lists user-visible children without following symbolic links outside the
/// received directory. Directories sort before files to match Files/Finder.
func availableReceivedDirectoryItemURLs(
    directory: URL,
    fileManager: FileManager = .default
) -> [URL] {
    guard availableCompletedDirectoryURL(path: directory.path) != nil,
          let contents = try? fileManager.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: [
                .isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey,
            ],
            options: [.skipsHiddenFiles]
          ) else {
        return []
    }
    let items = contents.compactMap { url -> (url: URL, isDirectory: Bool)? in
        guard let values = try? url.resourceValues(forKeys: [
            .isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey,
        ]), values.isSymbolicLink != true else {
            return nil
        }
        if values.isDirectory == true {
            return (url, true)
        }
        if values.isRegularFile == true {
            return (url, false)
        }
        return nil
    }
    return items.sorted { lhs, rhs in
        if lhs.isDirectory != rhs.isDirectory {
            return lhs.isDirectory
        }
        return lhs.url.lastPathComponent.localizedStandardCompare(
            rhs.url.lastPathComponent
        ) == .orderedAscending
    }.map(\.url)
}

/// Resolves the top-level items that are still available after a completed
/// Manifest receive. Result paths are authoritative because publication may
/// rename a conflicting item.
func availableCompletedManifestItemURLs(record: FfiManifestActivityRecord) -> [URL] {
    let activity = record.activity
    guard activity.direction == .receive,
          activity.state == .completed,
          record.rootCount > 0 else { return [] }
    let destination = URL(fileURLWithPath: activity.completedFilePath, isDirectory: true)
    let successfulStatuses: Set<FfiManifestEntryResultStatus> = [
        .completed, .skippedIdentical, .renamed,
    ]
    let results = Dictionary(
        record.entryResults.map { ($0.entryId, $0) },
        uniquingKeysWith: { first, _ in first }
    )
    return record.entries.compactMap { entry in
        guard isSafeManifestTopLevelName(entry.relativePath),
              let result = results[entry.entryId],
              successfulStatuses.contains(result.status),
              isSafeManifestTopLevelName(result.finalRelativePath) else {
            return nil
        }
        let finalURL = destination.appendingPathComponent(
            result.finalRelativePath,
            isDirectory: entry.kind == .directory
        )
        switch entry.kind {
        case .file:
            return availableCompletedFileURL(path: finalURL.path, expectedBytes: entry.size)
        case .directory:
            return availableCompletedDirectoryURL(path: finalURL.path)
        }
    }
}

/// Resolves the most useful single completed location for compatibility with
/// existing callers. A multi-root transfer continues to resolve to its
/// destination directory; item-aware UI should use the array helper above.
func availableCompletedManifestURL(record: FfiManifestActivityRecord) -> URL? {
    guard record.activity.direction == .receive,
          record.activity.state == .completed,
          record.rootCount > 0 else { return nil }
    guard record.rootCount == 1 else {
        return availableCompletedDirectoryURL(path: record.activity.completedFilePath)
    }
    let items = availableCompletedManifestItemURLs(record: record)
    return items.count == 1 ? items[0] : nil
}

enum PublicationMaterialization: Equatable {
    case clone
    case copy
}

private struct TransferReceiptReference: Decodable {
    let fileName: String

    private enum CodingKeys: String, CodingKey {
        case fileName = "file_name"
    }
}

/// Removes completion receipts whose directly received file no longer exists.
/// A receipt is valid without its staging file only when native publication is
/// in use; direct destinations must materialize the file again after deletion.
func removeOrphanedDirectReceiveReceipts(
    in directory: URL,
    fileManager: FileManager = .default
) throws {
    let prefix = ".envoix-receipt."
    let suffix = ".json"
    let entries = try fileManager.contentsOfDirectory(
        at: directory,
        includingPropertiesForKeys: [.isRegularFileKey, .isSymbolicLinkKey],
        options: []
    )
    for receiptURL in entries {
        let receiptName = receiptURL.lastPathComponent
        guard receiptName.hasPrefix(prefix), receiptName.hasSuffix(suffix) else { continue }
        let values = try? receiptURL.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
        guard values?.isRegularFile == true, values?.isSymbolicLink != true,
              let data = try? Data(contentsOf: receiptURL),
              let receipt = try? JSONDecoder().decode(TransferReceiptReference.self, from: data),
              isSafeDirectReceiveFileName(receipt.fileName),
              receiptName == "\(prefix)\(receipt.fileName)\(suffix)" else { continue }
        let receivedFile = directory.appendingPathComponent(receipt.fileName, isDirectory: false)
        guard !fileManager.fileExists(atPath: receivedFile.path) else { continue }
        try fileManager.removeItem(at: receiptURL)
    }
}

private func isSafeDirectReceiveFileName(_ name: String) -> Bool {
    !name.isEmpty
        && name != "."
        && name != ".."
        && URL(fileURLWithPath: name).lastPathComponent == name
        && !name.contains("\\")
        && !name.unicodeScalars.contains { CharacterSet.controlCharacters.contains($0) }
}

/// Uses same-volume copy-on-write when the destination supports it. A local
/// APFS publication then allocates metadata instead of rewriting the full
/// payload; FileProvider and cross-volume targets transparently fall back to a
/// normal copy.
@discardableResult
func materializePublishedFile(
    from source: URL,
    to destination: URL,
    fileManager: FileManager = .default
) throws -> PublicationMaterialization {
    let cloneResult = source.path.withCString { sourcePath in
        destination.path.withCString { destinationPath in
            clonefile(sourcePath, destinationPath, 0)
        }
    }
    if cloneResult == 0 {
        return .clone
    }
    try fileManager.copyItem(at: source, to: destination)
    return .copy
}

/// Publishes one already-verified staging file into a user-selected Files
/// directory. The destination becomes visible only after clone/copy completion
/// and a size check. The verified source remains available for retries.
func publishReceivedFile(
    from source: URL,
    to destinationDirectory: URL,
    expectedBytes: UInt64
) throws -> URL {
    guard availableCompletedFileURL(path: source.path, expectedBytes: expectedBytes) != nil else {
        throw RuntimeSettingsError("The verified staging file is missing or has an unexpected size.")
    }
    let fileManager = FileManager.default
    try fileManager.createDirectory(at: destinationDirectory, withIntermediateDirectories: true)
    let finalURL = destinationDirectory.appendingPathComponent(source.lastPathComponent)
    if fileManager.fileExists(atPath: finalURL.path) {
        guard availableCompletedFileURL(path: finalURL.path, expectedBytes: expectedBytes) != nil,
              try filesHaveEqualContents(source, finalURL) else {
            throw RuntimeSettingsError("A different file with the same name already exists in the selected folder.")
        }
        return finalURL
    }
    let temporaryURL = destinationDirectory.appendingPathComponent(
        ".envoix-publish-\(UUID().uuidString).part"
    )
    defer { try? fileManager.removeItem(at: temporaryURL) }
    try materializePublishedFile(from: source, to: temporaryURL, fileManager: fileManager)
    guard availableCompletedFileURL(path: temporaryURL.path, expectedBytes: expectedBytes) != nil else {
        throw RuntimeSettingsError("The copied file did not match the verified size.")
    }
    try fileManager.moveItem(at: temporaryURL, to: finalURL)
    return finalURL
}

/// Publishes every verified top-level Manifest root. Existing identical roots
/// make retries idempotent; a conflicting root fails before any new copy starts.
func publishReceivedManifest(
    from sourceRoot: URL,
    to destinationDirectory: URL,
    record: FfiManifestActivityRecord
) throws -> URL {
    let fileManager = FileManager.default
    let successfulStatuses: Set<FfiManifestEntryResultStatus> = [
        .completed, .skippedIdentical, .renamed,
    ]
    let results = Dictionary(
        record.entryResults.map { ($0.entryId, $0) },
        uniquingKeysWith: { first, _ in first }
    )
    let roots = try record.entries.filter { !$0.relativePath.contains("/") }.map { entry in
        guard let result = results[entry.entryId],
              successfulStatuses.contains(result.status),
              isSafeManifestTopLevelName(entry.relativePath),
              isSafeManifestTopLevelName(result.finalRelativePath) else {
            throw RuntimeSettingsError("The verified Manifest is missing a safe top-level result.")
        }
        return (
            entry,
            sourceRoot.appendingPathComponent(result.finalRelativePath),
            destinationDirectory.appendingPathComponent(result.finalRelativePath)
        )
    }
    guard roots.count == Int(record.rootCount), !roots.isEmpty else {
        throw RuntimeSettingsError("The verified Manifest root count is inconsistent.")
    }

    try fileManager.createDirectory(at: destinationDirectory, withIntermediateDirectories: true)
    for (entry, source, destination) in roots where fileManager.fileExists(atPath: destination.path) {
        guard try publishedItemMatches(entry: entry, source: source, destination: destination) else {
            throw RuntimeSettingsError(
                "A different item named \(destination.lastPathComponent) already exists in the selected folder."
            )
        }
    }

    for (entry, source, destination) in roots where !fileManager.fileExists(atPath: destination.path) {
        switch entry.kind {
        case .file:
            _ = try publishReceivedFile(
                from: source,
                to: destinationDirectory,
                expectedBytes: entry.size
            )
        case .directory:
            try publishReceivedDirectory(from: source, to: destination)
        }
    }
    return destinationDirectory
}

private func isSafeManifestTopLevelName(_ name: String) -> Bool {
    !name.isEmpty
        && name != "."
        && name != ".."
        && !name.contains("/")
        && !name.contains("\\")
        && !name.unicodeScalars.contains { CharacterSet.controlCharacters.contains($0) }
}

private func publishReceivedDirectory(from source: URL, to destination: URL) throws {
    let fileManager = FileManager.default
    let sourceValues = try source.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
    guard sourceValues.isDirectory == true, sourceValues.isSymbolicLink != true else {
        throw RuntimeSettingsError("A verified Manifest directory is missing or invalid.")
    }
    let temporaryURL = destination.deletingLastPathComponent().appendingPathComponent(
        ".envoix-publish-\(UUID().uuidString).part",
        isDirectory: true
    )
    defer { try? fileManager.removeItem(at: temporaryURL) }
    try fileManager.copyItem(at: source, to: temporaryURL)
    guard try directoryTreesHaveEqualContents(source, temporaryURL) else {
        throw RuntimeSettingsError("The copied directory did not match the verified staging data.")
    }
    try fileManager.moveItem(at: temporaryURL, to: destination)
}

private func publishedItemMatches(
    entry: FfiPreparedManifestEntry,
    source: URL,
    destination: URL
) throws -> Bool {
    switch entry.kind {
    case .file:
        guard availableCompletedFileURL(path: destination.path, expectedBytes: entry.size) != nil else {
            return false
        }
        return try filesHaveEqualContents(source, destination)
    case .directory:
        return try directoryTreesHaveEqualContents(source, destination)
    }
}

private func directoryTreesHaveEqualContents(_ lhs: URL, _ rhs: URL) throws -> Bool {
    let leftItems = try directoryInventory(lhs)
    let rightItems = try directoryInventory(rhs)
    guard leftItems.keys == rightItems.keys else { return false }
    for path in leftItems.keys {
        guard let left = leftItems[path], let right = rightItems[path], left.kind == right.kind else {
            return false
        }
        if left.kind == .file {
            guard left.size == right.size,
                  try filesHaveEqualContents(
                    lhs.appendingPathComponent(path),
                    rhs.appendingPathComponent(path)
                  ) else { return false }
        }
    }
    return true
}

private enum PublishedItemKind: Equatable {
    case file
    case directory
}

private func directoryInventory(_ root: URL) throws -> [String: (kind: PublishedItemKind, size: Int)] {
    let keys: Set<URLResourceKey> = [
        .isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey,
    ]
    let rootValues = try root.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
    guard rootValues.isDirectory == true, rootValues.isSymbolicLink != true else {
        throw RuntimeSettingsError("A Manifest directory is missing or invalid.")
    }
    let resolvedRoot = root.resolvingSymlinksInPath().standardizedFileURL
    let rootPrefix = resolvedRoot.path + "/"
    guard let enumerator = FileManager.default.enumerator(
        at: resolvedRoot,
        includingPropertiesForKeys: Array(keys),
        options: []
    ) else {
        throw RuntimeSettingsError("Could not inspect a Manifest directory.")
    }
    var inventory: [String: (kind: PublishedItemKind, size: Int)] = [:]
    while let url = enumerator.nextObject() as? URL {
        let values = try url.resourceValues(forKeys: keys)
        guard values.isSymbolicLink != true else {
            throw RuntimeSettingsError("Symbolic links are not supported in a Manifest directory.")
        }
        let resolvedURL = url.resolvingSymlinksInPath().standardizedFileURL
        guard resolvedURL.path.hasPrefix(rootPrefix) else {
            throw RuntimeSettingsError("A Manifest directory entry escaped its root.")
        }
        let relativePath = String(resolvedURL.path.dropFirst(rootPrefix.count))
        if values.isDirectory == true {
            inventory[relativePath] = (.directory, 0)
        } else if values.isRegularFile == true {
            inventory[relativePath] = (.file, values.fileSize ?? -1)
        } else {
            throw RuntimeSettingsError("A Manifest directory contains an unsupported item.")
        }
    }
    return inventory
}

private func filesHaveEqualContents(_ lhs: URL, _ rhs: URL) throws -> Bool {
    let left = try FileHandle(forReadingFrom: lhs)
    let right = try FileHandle(forReadingFrom: rhs)
    defer {
        try? left.close()
        try? right.close()
    }
    let chunkSize = 1024 * 1024
    while true {
        let leftChunk = try left.read(upToCount: chunkSize) ?? Data()
        let rightChunk = try right.read(upToCount: chunkSize) ?? Data()
        guard leftChunk == rightChunk else { return false }
        if leftChunk.isEmpty { return true }
    }
}
