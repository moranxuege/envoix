import SwiftUI
import Darwin
#if os(macOS)
import AppKit
#elseif os(iOS)
import UIKit
#endif
import CoreImage.CIFilterBuiltins
import EnvoixCore

#if os(macOS)
typealias PlatformImage = NSImage
#elseif os(iOS)
typealias PlatformImage = UIImage
#endif

/// Minimum length of a shared pairing token, matching the core requirement.
let minTokenLength = 12
let defaultRendezvousBroker = "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445"
let defaultRelayURL = "https://envoix.chkxwlyh.us:8444"
let defaultLogServer = "https://rdz.chkxwlyh.us:8460"
let deprecatedLogServers: Set<String> = [
    "http://67.230.187.238:8460",
    "https://envoix.chkxwlyh.us:8460",
    "http://envoix.chkxwlyh.us:8460",
]

struct ActivityActionAvailability: Equatable {
    let canPause: Bool
    let canResume: Bool
    let canCancel: Bool
    let canDelete: Bool
    let isFinalizing: Bool
}

/// Single lifecycle-to-UI action policy. SwiftUI must not infer buttons from
/// presentation state independently of the canonical transfer snapshot.
func activityActionAvailability(for record: FfiTransferActivityRecord) -> ActivityActionAvailability {
    let actions = transferActivityActions(record: record)
    return ActivityActionAvailability(
        canPause: actions.canPause,
        canResume: actions.canResume,
        canCancel: actions.canCancel,
        canDelete: actions.canDelete,
        isFinalizing: actions.isFinalizing
    )
}
let expectedCoreFFIAPIVersion: UInt32 = 2
let appDebugBuildLabel = "Debug build 2026.07.08.19"

/// Generates a short, memorable, easy-to-type pairing token of the form
/// `word-word-NN` (always ≥ `minTokenLength` since each word is ≥4 letters).
func friendlyToken() -> String {
    let words = ["river", "stone", "cloud", "tiger", "maple", "otter", "amber",
                 "comet", "delta", "ember", "flint", "grove", "ivory", "larch",
                 "mango", "ocean", "pearl", "raven", "spark", "topaz", "coral",
                 "hazel", "basil", "willow", "pine", "reef", "surf", "teal"]
    let a = words.randomElement()!
    var b = words.randomElement()!
    while b == a { b = words.randomElement()! }
    return "\(a)-\(b)-\(Int.random(in: 10...99))"
}

/// How two peers find and authenticate each other.
enum PairingMode: Hashable {
    case room    // Android-compatible QR/code, broker-assisted pairing
    case invite  // legacy direct invite link, kept for compatibility
    case token   // compatibility-only shared token; no longer exposed in Apple UI
}

extension String {
    var trimmed: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}

enum RuntimeSettingsProvider {
    static func make(
        concurrentTransfers: Bool,
        language: String,
        serverURL: String,
        relayURL: String,
        configChunkSize: String,
        candidatesAllow: String = "",
        candidatesDeny: String = "",
        speedLimit: Int
    ) throws -> EnvoixRuntimeSettings {
        guard speedLimit >= 0 else {
            throw RuntimeSettingsError("Speed limit cannot be negative.")
        }

        let configPath = try resolveConfigPath(
            chunkSize: configChunkSize,
            candidatesAllow: candidatesAllow,
            candidatesDeny: candidatesDeny
        )

        return EnvoixRuntimeSettings(
            concurrentTransfers: concurrentTransfers,
            language: language,
            serverUrl: serverURL.trimmed,
            relayUrl: relayURL.trimmed,
            configPath: configPath,
            speedLimitMbps: UInt64(speedLimit)
        )
    }
}

func newRoomCode() -> String {
    (try? generateRoomCode()) ?? friendlyToken()
}

struct RuntimeSettingsError: LocalizedError {
    let message: String

    init(_ message: String) {
        self.message = message
    }

    var errorDescription: String? { message }
}

enum AppText {
    static func value(_ english: String, _ simplifiedChinese: String, language: String) -> String {
        language == "zh-Hans" ? simplifiedChinese : english
    }
}

private struct AppLanguageKey: EnvironmentKey {
    static let defaultValue = "en"
}

extension EnvironmentValues {
    var appLanguage: String {
        get { self[AppLanguageKey.self] }
        set { self[AppLanguageKey.self] = newValue }
    }
}

/// A labeled field for entering the shared pairing token, with a one-tap
/// generator (and copy) so users don't have to invent one.
struct TokenField: View {
    @Environment(\.appLanguage) private var language
    @Binding var token: String
    var disabled: Bool

    var body: some View {
        #if os(iOS)
        mobileBody
        #else
        desktopBody
        #endif
    }

    private var desktopBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(AppText.value("Shared token (same on both devices, \(minTokenLength)+ characters)", "共享口令（两台设备相同，至少 \(minTokenLength) 个字符）", language: language))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
            HStack(spacing: 8) {
                TextField("e.g. envoix-lan-2026", text: $token)
                    .textFieldStyle(.plain)
                    .font(.body.monospaced())
                    .foregroundStyle(Theme.text)
                Button {
                    token = friendlyToken()
                    ToastCenter.shared.show(AppText.value("Token generated", "口令已生成", language: language))
                } label: {
                    Label(AppText.value("Generate", "生成", language: language), systemImage: "wand.and.stars")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                Button {
                    copyWithToast(token, AppText.value("Token copied", "口令已复制", language: language))
                } label: {
                    Label(AppText.value("Copy Token", "复制口令", language: language), systemImage: "doc.on.doc")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                    .disabled(token.trimmed.isEmpty)
            }
            .disabled(disabled)
            .padding(.horizontal, 10)
            .frame(minHeight: 44)
            .background(Theme.surface)
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
            )
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        }
    }

    #if os(iOS)
    private var mobileBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(AppText.value("Shared token", "共享口令", language: language))
                .font(.headline.weight(.semibold))
                .foregroundStyle(Theme.muted)

            TextField("e.g. envoix-lan-2026", text: $token)
                .textFieldStyle(.plain)
                .font(.body.monospaced())
                .foregroundStyle(Theme.text)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .disabled(disabled)
                .padding(.horizontal, 10)
                .frame(minHeight: 44)
                .background(Theme.surface)
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.cardRadius)
                        .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
                )
                .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))

            HStack(spacing: 8) {
                Button {
                    token = friendlyToken()
                    ToastCenter.shared.show(AppText.value("Token generated", "口令已生成", language: language))
                } label: {
                    Label(AppText.value("Generate", "生成", language: language), systemImage: "wand.and.stars")
                        .frame(maxWidth: .infinity, minHeight: 36)
                }
                .disabled(disabled)

                Button {
                    copyWithToast(token, AppText.value("Token copied", "口令已复制", language: language))
                } label: {
                    Label(AppText.value("Copy", "复制", language: language), systemImage: "doc.on.doc")
                        .frame(maxWidth: .infinity, minHeight: 36)
                }
                .disabled(token.trimmed.isEmpty)
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
        }
    }
    #endif
}

struct RoomCodeField: View {
    @Environment(\.appLanguage) private var language
    @Binding var code: String
    var disabled: Bool
    var title = "Room code"
    var placeholder = "135790-amber-comet"
    var canGenerate: Bool = false
    var generateLabel = "Generate"
    var copyLabel = "Copy Code"
    var showsCopyAction = true
    var pasteAction: (() -> Void)?
    var helper: String
    var accessibilityIdentifier = ""

    var body: some View {
        #if os(iOS)
        mobileBody
        #else
        desktopBody
        #endif
    }

    private var desktopBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title)
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
            HStack(spacing: 8) {
                TextField(placeholder, text: $code)
                    .textFieldStyle(.plain)
                    .font(.body.monospaced())
                    .foregroundStyle(Theme.text)
                    .disabled(disabled)
                    .accessibilityIdentifier(accessibilityIdentifier)
                if canGenerate {
                    Button {
                        code = newRoomCode()
                        ToastCenter.shared.show(AppText.value("Room code generated", "接收码已生成", language: language))
                    } label: {
                        Label(generateLabel, systemImage: "wand.and.stars")
                            .frame(minHeight: 34)
                            .contentShape(Rectangle())
                    }
                    .disabled(disabled)
                }
                Button {
                    copyWithToast(code, AppText.value("Room code copied", "接收码已复制", language: language))
                } label: {
                    Label(copyLabel, systemImage: "doc.on.doc")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(code.trimmed.isEmpty)
            }
            .padding(.horizontal, 10)
            .frame(minHeight: 44)
            .background(Theme.surface)
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
            )
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))

            if !helper.trimmed.isEmpty {
                Text(helper)
                    .font(.body)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    #if os(iOS)
    private var mobileBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title)
                .font(.headline.weight(.semibold))
                .foregroundStyle(Theme.muted)

            HStack(spacing: 8) {
                TextField(placeholder, text: $code)
                    .textFieldStyle(.plain)
                    .font(.body.monospaced())
                    .foregroundStyle(Theme.text)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .disabled(disabled)
                    .accessibilityIdentifier(accessibilityIdentifier)

                if let pasteAction {
                    Button(action: pasteAction) {
                        Label(AppText.value("Paste", "粘贴", language: language), systemImage: "doc.on.clipboard")
                            .font(.subheadline.weight(.semibold))
                            .foregroundStyle(disabled ? Theme.text : Theme.accentStrong)
                            .padding(.horizontal, 12)
                            .frame(minHeight: 36)
                            .background(
                                disabled ? Theme.line : Theme.accentSoft,
                                in: RoundedRectangle(cornerRadius: 8)
                            )
                            .overlay(
                                RoundedRectangle(cornerRadius: 8)
                                    .strokeBorder(Theme.line, lineWidth: 1)
                            )
                    }
                    .buttonStyle(.plain)
                    .disabled(disabled)
                }
            }
            .padding(.horizontal, 10)
            .frame(minHeight: 48)
            .background(Theme.surface)
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
            )
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))

            if canGenerate || showsCopyAction {
                HStack(spacing: 8) {
                    if canGenerate {
                        Button {
                            code = newRoomCode()
                            ToastCenter.shared.show(AppText.value("Room code generated", "接收码已生成", language: language))
                        } label: {
                            Label(generateLabel, systemImage: "wand.and.stars")
                                .frame(maxWidth: .infinity, minHeight: 36)
                        }
                        .disabled(disabled)
                    }

                    if showsCopyAction {
                        Button {
                            copyWithToast(code, AppText.value("Room code copied", "接收码已复制", language: language))
                        } label: {
                            Label(copyLabel == "Copy Code" ? AppText.value("Copy", "复制", language: language) : copyLabel, systemImage: "doc.on.doc")
                                .frame(maxWidth: .infinity, minHeight: 36)
                        }
                        .disabled(code.trimmed.isEmpty)
                    }
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
            }

            if !helper.trimmed.isEmpty {
                Text(helper)
                    .font(.footnote)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .tint(Theme.accentStrong)
    }
    #endif
}

/// Renders a string into a crisp QR code image.
enum QRCode {
    static func image(from string: String) -> PlatformImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(string.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage else { return nil }
        let scaled = output.transformed(by: CGAffineTransform(scaleX: 8, y: 8))
        let context = CIContext()
        guard let cgImage = context.createCGImage(scaled, from: scaled.extent) else { return nil }
        #if os(macOS)
        return NSImage(cgImage: cgImage, size: NSSize(width: scaled.extent.width, height: scaled.extent.height))
        #elseif os(iOS)
        return UIImage(cgImage: cgImage)
        #endif
    }
}

/// Presents an open panel for a single file or directory; returns the choice.
func chooseURL(directory: Bool) -> URL? {
    #if os(macOS)
    let panel = NSOpenPanel()
    panel.canChooseFiles = !directory
    panel.canChooseDirectories = directory
    panel.allowsMultipleSelection = false
    return panel.runModal() == .OK ? panel.url : nil
    #else
    return nil
    #endif
}

func copyToPasteboard(_ text: String) {
    #if os(macOS)
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(text, forType: .string)
    #elseif os(iOS)
    UIPasteboard.general.string = text
    #endif
}

func pasteboardString() -> String? {
    #if os(macOS)
    return NSPasteboard.general.string(forType: .string)
    #elseif os(iOS)
    return UIPasteboard.general.string
    #endif
}

/// Resolves a file from the clipboard, handling both a file copied in Finder
/// (a file-URL on the pasteboard) and a plain-text path (expanding a leading
/// `~`). Returns the URL only if it points to an existing file.
func pastedFileURL() -> URL? {
    #if os(macOS)
    let pb = NSPasteboard.general
    let exists = { FileManager.default.fileExists(atPath: $0) }

    if let urls = pb.readObjects(forClasses: [NSURL.self],
                                 options: [.urlReadingFileURLsOnly: true]) as? [URL],
       let url = urls.first, exists(url.path) {
        return url
    }
    if let raw = pb.string(forType: .string)?.trimmed, !raw.isEmpty {
        let expanded = (raw as NSString).expandingTildeInPath
        if exists(expanded) { return URL(fileURLWithPath: expanded) }
    }
    return nil
    #else
    return nil
    #endif
}

/// Selects the file in Finder (opening its enclosing folder).
func revealInFinder(_ url: URL) {
    #if os(macOS)
    NSWorkspace.shared.activateFileViewerSelecting([url])
    #elseif os(iOS)
    UIApplication.shared.open(url)
    #endif
}

func platformRevealTitle(language: String) -> String {
    #if os(macOS)
    return AppText.value("Reveal in Finder", "在 Finder 中显示", language: language)
    #else
    return AppText.value("Open File", "打开文件", language: language)
    #endif
}

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

/// Resolves the most useful completed location for a Manifest receive. A
/// single root opens that item; a multi-root transfer opens its destination.
func availableCompletedManifestURL(record: FfiManifestActivityRecord) -> URL? {
    let activity = record.activity
    guard activity.direction == .receive,
          activity.state == .completed,
          record.rootCount > 0 else { return nil }
    let destination = URL(fileURLWithPath: activity.completedFilePath, isDirectory: true)
    guard record.rootCount == 1 else {
        return availableCompletedDirectoryURL(path: destination.path)
    }

    let successfulStatuses: Set<FfiManifestEntryResultStatus> = [
        .completed, .skippedIdentical, .renamed,
    ]
    let roots = record.entries.filter { isSafeManifestTopLevelName($0.relativePath) }
    guard roots.count == 1,
          let entry = roots.first,
          let result = record.entryResults.first(where: { $0.entryId == entry.entryId }),
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

enum PublicationMaterialization: Equatable {
    case clone
    case copy
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

/// Formats a byte count as a short human-readable string (auto KB/MB/GB).
func byteString(_ bytes: UInt64) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
}

/// Writes a minimal `config.toml` fragment to app storage and returns its path,
/// or returns an empty string when no config overrides are configured.
private let runtimeConfigFileName = "envoix-runtime-config.toml"

func resolveConfigPath(
    chunkSize: String,
    candidatesAllow: String = "",
    candidatesDeny: String = ""
) throws -> String {
    let chunkSize = chunkSize.trimmed
    let allow = configListLines(candidatesAllow)
    let deny = configListLines(candidatesDeny)
    if chunkSize.isEmpty && allow.isEmpty && deny.isEmpty {
        return ""
    }

    let supportDir = FileManager.default.urls(
        for: .applicationSupportDirectory,
        in: .userDomainMask,
    ).first
    guard let supportDir else {
        throw RuntimeSettingsError("Could not locate Application Support directory.")
    }
    let configDir = supportDir.appendingPathComponent("envoix", isDirectory: true)
    try FileManager.default.createDirectory(at: configDir, withIntermediateDirectories: true)
    let configFile = configDir.appendingPathComponent(runtimeConfigFileName)
    var lines: [String] = []
    if !chunkSize.isEmpty {
        lines.append("chunk_size = \"\(tomlEscaped(chunkSize))\"")
    }
    if !allow.isEmpty || !deny.isEmpty {
        lines.append("[candidates]")
        if !allow.isEmpty {
            lines.append("allow = \(tomlArray(allow))")
        }
        if !deny.isEmpty {
            lines.append("deny = \(tomlArray(deny))")
        }
    }
    let contents = lines.joined(separator: "\n") + "\n"
    try contents.write(to: configFile, atomically: true, encoding: .utf8)
    return configFile.path
}

func configListLines(_ text: String) -> [String] {
    text
        .split(whereSeparator: \.isNewline)
        .map { String($0).trimmed }
        .filter { !$0.isEmpty }
}

private func tomlArray(_ values: [String]) -> String {
    "[" + values.map { "\"\(tomlEscaped($0))\"" }.joined(separator: ", ") + "]"
}

private func tomlEscaped(_ value: String) -> String {
    value.replacingOccurrences(of: "\\", with: "\\\\")
        .replacingOccurrences(of: "\"", with: "\\\"")
}

/// Formats a transfer rate, picking the most fitting unit (e.g. "12.3 MB/s").
func rateString(_ bytesPerSec: Double) -> String {
    byteString(UInt64(max(0, bytesPerSec))) + "/s"
}

/// Formats a remaining-time estimate as "ETA 1:20" / "ETA 1:02:03".
func etaString(_ seconds: Double) -> String {
    let s = Int(seconds.rounded())
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60)
    if h > 0 { return String(format: "ETA %d:%02d:%02d", h, m, sec) }
    return String(format: "ETA %d:%02d", m, sec)
}

/// Builds a compact, bounded diagnostic report for a transfer and its snapshots.
struct TransferDiagnostics {
    static let clipboardMaxBytes = 256 * 1024
    static let uploadMaxBytes = RemoteLogUpload.bodyMaxBytes
    private static let headerMaxBytes = 2048
    private static let failureMaxBytes = 48 * 1024
    private static let eventLinesMaxBytes = 80 * 1024
    private static let eventLogMaxBytes = 96 * 1024

    static func report(
        for record: FfiTransferActivityRecord,
        eventLog: [String] = [],
        transferEventLines: [String] = [],
        budget: Int = clipboardMaxBytes,
        includeSensitiveFields: Bool = true
    ) -> String {
        var remaining = budget
        var lines: [String] = []

        append(
            section: section("header", headerText(for: record)),
            cap: headerMaxBytes,
            into: &lines,
            remaining: &remaining
        )

        if record.state == .failed || !record.diagnosticMessage.isEmpty || !record.userMessageKey.isEmpty {
            append(
                section: section("failure", failureText(for: record)),
                cap: failureMaxBytes,
                into: &lines,
                remaining: &remaining
            )
        }

        append(
            section: section("activity", activityText(for: record, includeSensitiveFields: includeSensitiveFields)),
            cap: remaining,
            into: &lines,
            remaining: &remaining
        )

        append(
            section: section("transfer_events", transferEventLines.joined(separator: "\n")),
            cap: eventLinesMaxBytes,
            into: &lines,
            remaining: &remaining
        )

        append(
            section: section("activity_log", eventLog.joined(separator: "\n")),
            cap: eventLogMaxBytes,
            into: &lines,
            remaining: &remaining
        )

        return lines.joined(separator: "\n")
    }

    /// Remote reports are larger than clipboard copies but never contain pairing secrets.
    static func remoteReport(
        for record: FfiTransferActivityRecord,
        eventLog: [String] = [],
        transferEventLines: [String] = []
    ) -> String {
        report(
            for: record,
            eventLog: eventLog,
            transferEventLines: transferEventLines,
            budget: uploadMaxBytes,
            includeSensitiveFields: false
        )
    }

    /// App-level report used before a Room exists and for cross-transfer diagnosis.
    static func appReport(
        activities: [FfiTransferActivityRecord],
        eventLines: [String] = []
    ) -> String {
        var remaining = uploadMaxBytes
        var lines: [String] = []
        append(
            section: section("header", [
                "app=envoix-ios",
                "version=\(appVersion)",
                "build=\(appBuild)",
                "generated=\(isoDate())",
                "activity_count=\(activities.count)",
            ].joined(separator: "\n")),
            cap: headerMaxBytes,
            into: &lines,
            remaining: &remaining
        )

        let activitySnapshots = activities.map { record in
            var sections = ["[activity \(record.activityId)]", activityText(for: record, includeSensitiveFields: false)]
            if record.state == .failed || !record.diagnosticMessage.isEmpty || !record.userMessageKey.isEmpty {
                sections.append(failureText(for: record))
            }
            return sections.joined(separator: "\n")
        }.joined(separator: "\n\n")
        append(
            section: section("activities", activitySnapshots),
            cap: remaining,
            into: &lines,
            remaining: &remaining
        )

        append(
            section: section("activity_events", eventLines.joined(separator: "\n")),
            cap: remaining,
            into: &lines,
            remaining: &remaining
        )

        return lines.joined(separator: "\n")
    }

    static func transferEventLine(_ event: FfiTransferEvent) -> String {
        let date = formatTime(event.tsMs)
        var parts = [
            "[\(date)]",
            "\(event.kind)",
            "\(event.direction)",
            "\(event.mode)",
            "\(event.pairingStep)",
        ]
        let file = event.fileName.isEmpty ? "unknown" : event.fileName
        parts.append("file=\(file)")
        parts.append("bytes=\(event.bytesTransferred)/\(event.totalBytes)")
        if !event.dataPathDetail.isEmpty {
            parts.append("path=\(event.dataPathKind) \(event.dataPathDetail)")
        }
        if !event.diagnosticMessage.isEmpty {
            parts.append("message=\(event.diagnosticMessage)")
        }
        return parts.joined(separator: " · ")
    }

    private static func append(
        section: String,
        cap: Int,
        into report: inout [String],
        remaining: inout Int
    ) {
        guard cap > 0 else { return }
        let allowed = min(cap, remaining)
        if allowed <= 0 { return }
        let piece = tail(section, maxBytes: allowed)
        guard !piece.isEmpty else { return }
        if piece.utf8.count > remaining { return }
        report.append(piece)
        remaining -= piece.utf8.count
        if remaining > 0 { remaining -= 1 }
    }

    private static func section(_ title: String, _ body: String) -> String {
        if body.isEmpty { return "[\(title)]\n(empty)" }
        return "[\(title)]\n\(body)"
    }

    private static func headerText(for record: FfiTransferActivityRecord) -> String {
        [
            "app=envoix-ios",
            "version=\(appVersion)",
            "build=\(appBuild)",
            "record_id=\(record.activityId)",
            "attempt_id=\(record.attemptId)",
            "generated=\(isoDate())",
        ].joined(separator: "\n")
    }

    private static func failureText(for record: FfiTransferActivityRecord) -> String {
        [
            "failure_code=\(record.failureCode)",
            "failure_category=\(record.failureCategory)",
            "failure_phase=\(record.failurePhase)",
            "failure_origin=\(record.failureOrigin)",
            "user_message_key=\(record.userMessageKey)",
            "retryable=\(record.retryable)",
            "recovery_action=\(record.recoveryAction)",
            "diagnostic_message=\(record.diagnosticMessage)",
        ].joined(separator: "\n")
    }

    private static func activityText(
        for record: FfiTransferActivityRecord,
        includeSensitiveFields: Bool
    ) -> String {
        [
            "activity_id=\(record.activityId)",
            "attempt_id=\(record.attemptId)",
            "state=\(record.state)",
            "direction=\(record.direction)",
            "mode=\(record.mode)",
            "created_at=\(formatTime(record.createdAtMs))",
            "updated_at=\(formatTime(record.updatedAtMs))",
            "started_at=\(formatTime(record.startedAtMs))",
            "completed_at=\(formatTime(record.completedAtMs))",
            "transfer_id=\(record.transferId)",
            "file_name=\(record.fileName)",
            "bytes=\(record.bytesTransferred)/\(record.totalBytes)",
            "resumed_bytes=\(record.bytesResumed)",
            "invite=\(sensitiveValue(record.invite, include: includeSensitiveFields))",
            "token=\(sensitiveValue(record.token, include: includeSensitiveFields))",
            "peer=\(sensitiveValue(record.peerDescriptor, include: includeSensitiveFields))",
            "data_path=\(record.dataPathKind) \(record.dataPathDetail)",
            "limits=\(record.limits)",
            "completed_file_path=\(sensitiveValue(record.completedFilePath, include: includeSensitiveFields))",
        ].joined(separator: "\n")
    }

    private static func sensitiveValue(_ value: String, include: Bool) -> String {
        guard !value.isEmpty else { return "" }
        return include ? value : "[redacted]"
    }

    private static var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "dev"
    }

    private static var appBuild: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "dev"
    }

    private static func formatTime(_ ms: UInt64) -> String {
        guard ms > 0 else { return "0" }
        let date = Date(timeIntervalSince1970: TimeInterval(ms) / 1000)
        let formatter = ISO8601DateFormatter()
        return formatter.string(from: date)
    }

    private static func isoDate() -> String {
        ISO8601DateFormatter().string(from: Date())
    }

    private static func tail(_ text: String, maxBytes: Int) -> String {
        let bytes = Array(text.utf8)
        if bytes.count <= maxBytes { return text }
        let head = "[… trimmed — last \(maxBytes / 1024) KB]\n"
        let headBytes = Array(head.utf8)
        let remaining = max(0, maxBytes - headBytes.count)
        if remaining == 0 { return head }
        let tailBytes = bytes.suffix(remaining)
        return head + String(decoding: tailBytes, as: UTF8.self)
    }
}

/// Shared status / progress section used by both the send and receive views.
struct TransferStatusView: View {
    @Environment(\.appLanguage) private var language
    @AppStorage("envoix.developerMode") private var developerMode = false
    @AppStorage("envoix.verboseLog") private var verboseLog = false
    @ObservedObject var viewModel: TransferViewModel

    var body: some View {
        if showsStatus {
            statusCard
        }
    }

    private var showsStatus: Bool {
        switch viewModel.phase {
        case .idle: return !viewModel.statusText.isEmpty
        default: return true
        }
    }

    private var statusCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: iconName)
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(tint)
                    .frame(width: 30, height: 30)
                    .background(tint.opacity(0.10), in: Circle())

                VStack(alignment: .leading, spacing: 4) {
                    Text(titleText)
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.text)
                        .lineLimit(2)

                    if let detailText {
                        Text(detailText)
                            .font(.body)
                            .foregroundStyle(Theme.muted)
                            .lineLimit(3)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }

                Spacer(minLength: 8)
            }

            switch viewModel.phase {
            case .idle, .waiting, .canceled, .failed:
                EmptyView()
            case .paused:
                if viewModel.total > 0 {
                    transferProgressLine
                }
                if let path = currentDataPathText {
                    pathLine(path)
                }
            case .transferring:
                ProgressBar(value: viewModel.progressFraction)
                transferProgressLine
                if let path = currentDataPathText {
                    pathLine(path)
                }
            case .completed(let bytes):
                Text(byteString(bytes))
                    .font(.body.monospacedDigit())
                    .foregroundStyle(Theme.muted)
                if let url = viewModel.completedFileURL {
                    completedFileControls(url)
                }
            }

            if let stepText {
                Text(stepText)
                    .font(.callout.monospaced())
                    .foregroundStyle(Theme.muted)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            if developerMode && !viewModel.eventLog.isEmpty {
                Divider().overlay(Theme.line)
                logsCard
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(backgroundTint)
        .overlay(
            RoundedRectangle(cornerRadius: Theme.cardRadius)
                .strokeBorder(tint.opacity(borderOpacity), lineWidth: 0.9)
        )
        .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
    }

    private var transferProgressLine: some View {
        HStack(spacing: 6) {
            Text("\(byteString(viewModel.transferred)) / \(byteString(viewModel.total))")
            if viewModel.bytesPerSec > 0 {
                Text("·")
                Text(rateString(viewModel.bytesPerSec))
            }
            if let eta = viewModel.etaSeconds {
                Text("·")
                Text(etaString(eta))
            }
        }
        .font(.body.monospacedDigit())
        .foregroundStyle(Theme.muted)
    }

    private func pathLine(_ path: String) -> some View {
        HStack(spacing: 6) {
            Text(AppText.value("Path", "链路", language: language))
                .fontWeight(.semibold)
            Text("·")
            Text(path)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .font(.body.monospacedDigit())
        .foregroundStyle(Theme.muted)
    }

    private var currentDataPathText: String? {
        guard let record = viewModel.transferActivity, record.dataPathKind != .none else { return nil }
        let pathKind: String
        switch record.dataPathKind {
        case .direct:
            pathKind = AppText.value("Direct", "直连", language: language)
        case .relay:
            pathKind = AppText.value("Relay", "中继", language: language)
        case .other:
            pathKind = AppText.value("Path", "路径", language: language)
        case .none:
            return nil
        }
        guard developerMode, !record.dataPathDetail.isEmpty else { return pathKind }
        return "\(pathKind) · \(record.dataPathDetail)"
    }

    @ViewBuilder private var logsCard: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(AppText.value("Activity log", "活动日志", language: language))
                    .font(.callout.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Spacer(minLength: 8)
                if verboseLog {
                    Text(AppText.value("Verbose", "详细", language: language))
                        .font(.caption.monospaced())
                        .foregroundStyle(Theme.muted)
                }
                Button {
                    copyToPasteboard(viewModel.eventLog.joined(separator: "\n"))
                    ToastCenter.shared.show(AppText.value("Log copied", "日志已复制", language: language))
                } label: {
                    Label(AppText.value("Copy", "复制", language: language), systemImage: "doc.on.doc")
                        .labelStyle(.iconOnly)
                        .frame(width: 30, height: 30)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }

            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(viewModel.eventLog, id: \.self) { line in
                        Text(line)
                            .font(.caption.monospaced())
                            .foregroundStyle(Theme.muted)
                            .lineLimit(2)
                            .textSelection(.enabled)
                    }
                }
            }
            .frame(maxHeight: 180)
        }
    }

    private var titleText: String {
        switch viewModel.phase {
        case .idle:
            return AppText.value("Status", "状态", language: language)
        case .waiting:
            return AppText.value("Waiting for the other device", "正在等待另一台设备", language: language)
        case .transferring:
            return viewModel.fileName.isEmpty ? AppText.value("Transferring", "正在传输", language: language) : viewModel.fileName
        case .paused:
            return AppText.value("Transfer paused", "传输已暂停", language: language)
        case .completed:
            return AppText.value("Transfer completed", "传输完成", language: language)
        case .canceled:
            return AppText.value("Transfer canceled", "传输已取消", language: language)
        case .failed(let reason):
            return failureText(reason: reason).title
        }
    }

    private var detailText: String? {
        switch viewModel.phase {
        case .idle:
            return viewModel.statusText.isEmpty ? nil : viewModel.statusText
        case .waiting:
            return viewModel.statusText.isEmpty
                ? AppText.value("Keep this window open until the peer connects.", "请保持此窗口打开，直到对方连接。", language: language)
                : viewModel.statusText
        case .transferring:
            return AppText.value("Keep both devices awake until the transfer finishes.", "请保持两台设备唤醒，直到传输完成。", language: language)
        case .paused:
            return AppText.value("Resume or delete this transfer from Activity.", "请在活动页继续或删除此传输。", language: language)
        case .completed:
            return viewModel.statusText.isEmpty ? AppText.value("The file is ready.", "文件已准备好。", language: language) : viewModel.statusText
        case .canceled:
            return AppText.value("Ready to start another transfer.", "可以开始新的传输。", language: language)
        case .failed(let reason):
            return failureText(reason: reason).detail
        }
    }

    private var stepText: String? {
        let text = viewModel.statusText.trimmed
        guard !text.isEmpty else { return nil }
        if case .failed = viewModel.phase {
            return AppText.value("Last step: \(text)", "上一步：\(text)", language: language)
        }
        return nil
    }

    private var iconName: String {
        switch viewModel.phase {
        case .idle: return "info.circle"
        case .waiting: return "antenna.radiowaves.left.and.right"
        case .transferring: return "arrow.up.arrow.down.circle"
        case .paused: return "pause.circle"
        case .completed: return "checkmark.circle.fill"
        case .canceled: return "xmark.circle"
        case .failed: return "exclamationmark.triangle.fill"
        }
    }

    private var tint: Color {
        switch viewModel.phase {
        case .idle: return Theme.muted
        case .waiting, .transferring, .paused: return Theme.warning
        case .completed: return Theme.success
        case .canceled: return Theme.muted
        case .failed: return Theme.danger
        }
    }

    private var backgroundTint: Color {
        switch viewModel.phase {
        case .failed: return Theme.dangerSoft.opacity(0.55)
        case .waiting, .transferring, .paused: return Theme.warning.opacity(0.06)
        case .completed: return Theme.success.opacity(0.06)
        case .idle, .canceled: return Theme.surface
        }
    }

    private var borderOpacity: Double {
        switch viewModel.phase {
        case .idle: return 0.25
        default: return 0.35
        }
    }

    private func failureText(reason: String) -> (title: String, detail: String) {
        if let failure = viewModel.failure {
            return structuredFailureText(failure)
        }
        return fallbackFailureText(reason)
    }

    private func structuredFailureText(_ failure: FfiTransferFailure) -> (title: String, detail: String) {
        let title: String
        switch failure.code {
        case .userCanceled, .peerCanceled:
            title = AppText.value("Transfer canceled", "传输已取消", language: language)
        case .networkLost, .peerUnreachable, .timeout:
            title = AppText.value("Connection failed", "连接失败", language: language)
        case .authenticationFailed:
            title = AppText.value("Pairing failed", "配对失败", language: language)
        case .permissionDenied:
            title = AppText.value("Permission needed", "需要权限", language: language)
        case .diskFull:
            title = AppText.value("Not enough space", "空间不足", language: language)
        case .hashMismatch:
            title = AppText.value("Verification failed", "校验失败", language: language)
        case .protocolError:
            title = AppText.value("Protocol mismatch", "协议不匹配", language: language)
        case .destinationConflict:
            title = AppText.value("Destination conflict", "目标位置冲突", language: language)
        case .unsupportedFeature:
            title = AppText.value("Update required", "需要更新", language: language)
        case .internalError, .unknown:
            title = AppText.value("Transfer failed", "传输失败", language: language)
        }
        return (title, friendlyFailure(failure, language: language))
    }

    private func fallbackFailureText(_ reason: String) -> (title: String, detail: String) {
        let cleanReason = reason.trimmed
        let lower = cleanReason.lowercased()
        if lower.contains("mdns") && lower.contains("peers discovered") {
            return (
                AppText.value("No device found on the local network", "未在局域网发现设备", language: language),
                AppText.value("Make sure the other device is receiving with the same token and both devices are on the same network.", "请确认另一台设备正在使用相同口令接收，并且两台设备在同一网络中。", language: language)
            )
        }
        if cleanReason.isEmpty {
            return (
                AppText.value("Transfer failed", "传输失败", language: language),
                AppText.value("Try again, or switch pairing method if discovery keeps failing.", "请重试；如果一直无法发现设备，请切换配对方式。", language: language)
            )
        }
        return (AppText.value("Transfer failed", "传输失败", language: language), cleanReason)
    }

    /// Reveal the received file. iOS hides the raw container path unless
    /// developer mode is enabled because it is not a user-facing location.
    @ViewBuilder private func completedFileControls(_ url: URL) -> some View {
        HStack {
            Button(platformRevealTitle(language: language)) { revealInFinder(url) }
            #if os(macOS)
            copyPathButton(url)
            #elseif os(iOS)
            ShareLink(item: url) {
                Label(AppText.value("Share", "分享", language: language), systemImage: "square.and.arrow.up")
            }
            if developerMode {
                copyPathButton(url)
            }
            #endif
        }
        #if os(macOS)
        Text(url.path)
            .font(.body.monospaced())
            .foregroundStyle(Theme.muted)
            .textSelection(.enabled)
            .lineLimit(1)
            .truncationMode(.middle)
        #elseif os(iOS)
        Text(AppText.value("Saved as \(url.lastPathComponent)", "已保存为 \(url.lastPathComponent)", language: language))
            .font(.body)
            .foregroundStyle(Theme.muted)
            .lineLimit(1)
            .truncationMode(.middle)
        if developerMode {
            Text(url.path)
                .font(.body.monospaced())
                .foregroundStyle(Theme.muted)
                .textSelection(.enabled)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        #endif
    }

    private func copyPathButton(_ url: URL) -> some View {
        Button(AppText.value("Copy Path", "复制路径", language: language)) {
            copyWithToast(url.path, AppText.value("Path copied", "路径已复制", language: language))
        }
    }
}
