import SwiftUI
#if os(macOS)
import AppKit
#elseif os(iOS)
import UIKit
import QuickLook
#endif
import CoreImage.CIFilterBuiltins
import CryptoKit
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

/// True when this attempt completed entirely from bytes already present at
/// the destination. `bytesTransferred` is the verified total, while
/// `bytesResumed` identifies how much of that total crossed no wire this time.
func isFullyResumedCompletion(_ record: FfiTransferActivityRecord) -> Bool {
    record.state == .completed
        && record.totalBytes > 0
        && record.bytesTransferred >= record.totalBytes
        && record.bytesResumed >= record.totalBytes
}

let expectedCoreFFIAPIVersion: UInt32 = 3
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
                    copyWithToast(token, AppText.value("Token copied", "口令已复制", language: language), language: language)
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
                    copyWithToast(token, AppText.value("Token copied", "口令已复制", language: language), language: language)
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
                    copyWithToast(code, AppText.value("Room code copied", "接收码已复制", language: language), language: language)
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
                            copyWithToast(code, AppText.value("Room code copied", "接收码已复制", language: language), language: language)
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

@discardableResult
func copyToPasteboard(_ text: String) -> Bool {
    #if os(macOS)
    NSPasteboard.general.clearContents()
    guard NSPasteboard.general.setString(text, forType: .string) else { return false }
    return NSPasteboard.general.string(forType: .string) == text
    #elseif os(iOS)
    UIPasteboard.general.string = text
    return UIPasteboard.general.string == text
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
#if os(macOS)
func revealInFinder(_ url: URL) {
    revealInFinder([url])
}

func revealInFinder(_ urls: [URL]) {
    guard !urls.isEmpty else { return }
    NSWorkspace.shared.activateFileViewerSelecting(urls)
}
#endif

func isRegularFileURL(_ url: URL) -> Bool {
    (try? url.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true
}

func platformRevealTitle(language: String) -> String {
    #if os(macOS)
    return AppText.value("Reveal in Finder", "在 Finder 中显示", language: language)
    #else
    return AppText.value("Open File", "打开文件", language: language)
    #endif
}

#if os(iOS)
struct ReceivedItemsPresentation: Identifiable {
    let id = UUID()
    let urls: [URL]
}

private struct ReceivedItemsList: View {
    @Environment(\.appLanguage) private var language
    let urls: [URL]
    @Binding var previewFileURL: URL?
    let openDirectory: (URL) -> Void

    var body: some View {
        List(urls, id: \.self) { url in
            let isDirectory = availableCompletedDirectoryURL(path: url.path) != nil
            HStack(spacing: 12) {
                if isDirectory {
                    Button {
                        openDirectory(url)
                    } label: {
                        receivedItemLabel(url, systemImage: "folder")
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("received_folder_open_\(url.lastPathComponent)")
                } else {
                    Button {
                        previewFileURL = url
                    } label: {
                        receivedItemLabel(url, systemImage: "doc")
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("received_item_open_\(url.lastPathComponent)")
                }

                ShareLink(item: url) {
                    Image(systemName: "square.and.arrow.up")
                        .frame(width: 36, height: 36)
                }
                .accessibilityLabel(AppText.value("Share", "分享", language: language))
                .accessibilityIdentifier("received_item_share_\(url.lastPathComponent)")
            }
        }
    }

    private func receivedItemLabel(_ url: URL, systemImage: String) -> some View {
        Label(url.lastPathComponent, systemImage: systemImage)
            .lineLimit(2)
            .truncationMode(.middle)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
    }
}

struct ReceivedItemsSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.appLanguage) private var language
    let urls: [URL]
    @State private var previewFileURL: URL?
    @State private var directoryPath: [URL] = []

    var body: some View {
        NavigationStack(path: $directoryPath) {
            ReceivedItemsList(
                urls: urls,
                previewFileURL: $previewFileURL,
                openDirectory: { directoryPath.append($0) }
            )
            .navigationTitle(AppText.value(
                "Received Items",
                "已接收项目",
                language: language
            ))
            .navigationBarTitleDisplayMode(.inline)
            .navigationDestination(for: URL.self) { directory in
                let children = availableReceivedDirectoryItemURLs(directory: directory)
                ReceivedItemsList(
                    urls: children,
                    previewFileURL: $previewFileURL,
                    openDirectory: { directoryPath.append($0) }
                )
                    .navigationTitle(directory.lastPathComponent)
                    .navigationBarTitleDisplayMode(.inline)
                    .overlay {
                        if children.isEmpty {
                            Label(
                                AppText.value(
                                    "This folder is empty or unavailable.",
                                    "此文件夹为空或当前无法访问。",
                                    language: language
                                ),
                                systemImage: "folder"
                            )
                            .font(.callout)
                            .foregroundStyle(Theme.muted)
                            .padding()
                        }
                    }
            }
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(AppText.value("Done", "完成", language: language)) { dismiss() }
                }
            }
        }
        .quickLookPreview($previewFileURL)
        .presentationDetents([.medium, .large])
    }
}
#endif

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
    #if os(macOS)
    static let appIdentifier = "envoix-macos"
    #else
    static let appIdentifier = "envoix-ios"
    #endif
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

        if hasFailureMetadata(record) {
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
            section: section("header", ([
                "app=\(appIdentifier)",
                "version=\(appVersion)",
                "build=\(appBuild)",
                "generated=\(isoDate())",
                "activity_count=\(activities.count)",
            ] + runtimeIdentityLines).joined(separator: "\n")),
            cap: headerMaxBytes,
            into: &lines,
            remaining: &remaining
        )

        let activitySnapshots = activities.map { record in
            var sections = ["[activity \(record.activityId)]", activityText(for: record, includeSensitiveFields: false)]
            if hasFailureMetadata(record) {
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
        if event.bytesResumed > 0 {
            parts.append("resumed=\(event.bytesResumed)")
        }
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
        ([
            "app=\(appIdentifier)",
            "version=\(appVersion)",
            "build=\(appBuild)",
            "record_id=\(record.activityId)",
            "attempt_id=\(record.attemptId)",
            "generated=\(isoDate())",
        ] + runtimeIdentityLines).joined(separator: "\n")
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
            "diagnostic_message=\(record.diagnosticMessage)",
        ].joined(separator: "\n")
    }

    private static func hasFailureMetadata(_ record: FfiTransferActivityRecord) -> Bool {
        record.state == .failed
            || record.state == .unconfirmed
            || record.failureCode != .unknown
            || record.failureCategory != .unknown
            || !record.userMessageKey.isEmpty
            || record.recoveryAction != .none
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

    private static var runtimeIdentityLines: [String] {
        let core = envoixCoreInfo()
        return [
            "core_version=\(core.coreVersion)",
            "core_ffi_api=\(core.ffiApiVersion)",
            "core_capabilities=\(core.capabilities.sorted().joined(separator: ","))",
            "executable_sha256=\(executableFingerprint)",
            "runtime_code_file=\(runtimeCodeURL?.lastPathComponent ?? "unavailable")",
            "runtime_code_sha256=\(fileFingerprint(runtimeCodeURL))",
        ]
    }

    private static let executableFingerprint = fileFingerprint(Bundle.main.executableURL)

    private static let runtimeCodeURL: URL? = {
        guard let executableURL = Bundle.main.executableURL else { return nil }
        let debugDylibURL = executableURL
            .deletingLastPathComponent()
            .appendingPathComponent("\(executableURL.lastPathComponent).debug.dylib")
        if FileManager.default.isReadableFile(atPath: debugDylibURL.path) {
            return debugDylibURL
        }
        return executableURL
    }()

    private static func fileFingerprint(_ url: URL?) -> String {
        guard let url, let handle = try? FileHandle(forReadingFrom: url) else {
            return "unavailable"
        }
        defer { try? handle.close() }
        var hasher = SHA256()
        while true {
            let data = handle.readData(ofLength: 1024 * 1024)
            if data.isEmpty { break }
            hasher.update(data: data)
        }
        return hasher.finalize().prefix(12).map { String(format: "%02x", $0) }.joined()
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
