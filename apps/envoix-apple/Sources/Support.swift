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
let inviteV2URLPrefix = "envoix://invite/v2/"
let roomControlURLPrefix = "envoix://room/"
let deprecatedLogServers: Set<String> = [
    "http://67.230.187.238:8460",
    "https://envoix.chkxwlyh.us:8460",
    "http://envoix.chkxwlyh.us:8460",
]

let expectedCoreFFIAPIVersion: UInt32 = 20
let expectedRoomControlCoreCapability = "foreground_room_control_v5"
let expectedNearbyInviteCoreCapability = "nearby_invite_inbox_v1"
let expectedFailureProjectionCoreCapability = "canonical_failure_projection_v1"
let expectedRoomControlErrorCoreCapability = "typed_room_control_errors_v1"
let appDebugBuildLabel = "Debug build 2026.07.08.19"

func coreMatchesExpectedRoomControlContract(_ info: FfiCoreInfo) -> Bool {
    info.ffiApiVersion == expectedCoreFFIAPIVersion
        && info.capabilities.contains(expectedRoomControlCoreCapability)
        && info.capabilities.contains(expectedNearbyInviteCoreCapability)
        && info.capabilities.contains(expectedFailureProjectionCoreCapability)
        && info.capabilities.contains(expectedRoomControlErrorCoreCapability)
}

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
    case invite  // direct endpoint invite
    case remembered
    case token   // shared-token mDNS route; not exposed in the primary Apple UI
}

extension String {
    var trimmed: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}

enum RuntimeSettingsProvider {
    static func make(
        transferInvitation: FfiPairingInvite,
        concurrentTransfers: Bool,
        language: String,
        candidatesAllow: String = "",
        candidatesDeny: String = "",
        speedLimit: Int
    ) throws -> EnvoixRuntimeSettings {
        let endpoint = RoomControlEndpoint(transferInvitation: transferInvitation)
        return try make(
            concurrentTransfers: concurrentTransfers,
            language: language,
            serverURL: endpoint.broker,
            relayURL: endpoint.relay,
            candidatesAllow: candidatesAllow,
            candidatesDeny: candidatesDeny,
            speedLimit: speedLimit
        )
    }

    static func make(
        concurrentTransfers: Bool,
        language: String,
        serverURL: String,
        relayURL: String,
        candidatesAllow: String = "",
        candidatesDeny: String = "",
        speedLimit: Int
    ) throws -> EnvoixRuntimeSettings {
        guard speedLimit >= 0 else {
            throw RuntimeSettingsError("Speed limit cannot be negative.")
        }

        let configPath = try resolveConfigPath(
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

func formatRoomCodeInput(_ input: String) -> String {
    let lowercaseInput = input.lowercased()
    if lowercaseInput.hasPrefix("envoix:")
        || "envoix:".hasPrefix(lowercaseInput) {
        return input
    }

    var compact = ""
    var separatorOffsets: Set<Int> = []
    for character in input {
        if character == "-" {
            let offset = compact.count
            guard (offset == 6 || offset == 10),
                  separatorOffsets.insert(offset).inserted else {
                return input
            }
            continue
        }
        guard compact.count < 14,
              character.isASCII,
              character.isLetter || character.isNumber else {
            return input
        }
        compact.append(contentsOf: character.lowercased())
    }
    if compact.count == 14 && separatorOffsets.count == 1 {
        return input
    }

    var formatted = String(compact.enumerated().reduce(into: "") { result, item in
        if item.offset == 6 || item.offset == 10 {
            result.append("-")
        }
        result.append(item.element)
    })
    if separatorOffsets.contains(compact.count) {
        formatted.append("-")
    }
    return formatted
}

enum ConnectionInputKind: Equatable {
    case inviteV2
    case roomControl
}

struct ClassifiedConnectionInput {
    let kind: ConnectionInputKind
    let normalizedInput: String
    let pairingInvite: FfiPairingInvite?
}

func canonicalBareRoomControlCode(_ input: String) -> String? {
    let characters = Array(input)
    let compact: [Character]
    switch characters.count {
    case 14:
        compact = characters
    case 16:
        guard characters[6] == "-", characters[11] == "-" else { return nil }
        compact = Array(characters[0..<6])
            + Array(characters[7..<11])
            + Array(characters[12..<16])
    default:
        return nil
    }

    guard compact.prefix(6).allSatisfy({
        $0.isASCII && $0 >= "0" && $0 <= "9"
    }), compact.dropFirst(6).allSatisfy({
        $0.isASCII && ($0.isLetter || $0.isNumber)
    }) else {
        return nil
    }

    let normalized = compact.map { String($0).lowercased() }
    return normalized[0..<6].joined()
        + "-"
        + normalized[6..<10].joined()
        + "-"
        + normalized[10..<14].joined()
}

func classifyConnectionInput(
    _ input: String,
    fallbackBroker: String,
    fallbackRelay: String,
    allowBareRoomControl: Bool
) throws -> ClassifiedConnectionInput {
    let normalized = input.trimmed
    guard !normalized.isEmpty else {
        throw RuntimeSettingsError("The connection input is empty.")
    }

    if normalized.hasPrefix(inviteV2URLPrefix) {
        let invitation = try parsePairingInvite(input: normalized)
        return ClassifiedConnectionInput(
            kind: .inviteV2,
            normalizedInput: normalized,
            pairingInvite: invitation
        )
    }

    if normalized.hasPrefix(roomControlURLPrefix) {
        _ = try parseRoomControlInvite(
            input: normalized,
            fallbackBroker: fallbackBroker,
            fallbackRelay: fallbackRelay
        )
        return ClassifiedConnectionInput(
            kind: .roomControl,
            normalizedInput: normalized,
            pairingInvite: nil
        )
    }

    if allowBareRoomControl,
       let roomCode = canonicalBareRoomControlCode(normalized) {
        _ = try parseRoomControlInvite(
            input: roomCode,
            fallbackBroker: fallbackBroker,
            fallbackRelay: fallbackRelay
        )
        return ClassifiedConnectionInput(
            kind: .roomControl,
            normalizedInput: roomCode,
            pairingInvite: nil
        )
    }

    throw RuntimeSettingsError(
        "Enter a complete InviteV2 link, Room link, or current Room code."
    )
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

    static func localized(_ key: String, language: String) -> String {
        let resourceLanguage = language == "zh-Hans" ? "zh-Hans" : "en"
        guard
            let path = Bundle.main.path(forResource: resourceLanguage, ofType: "lproj"),
            let languageBundle = Bundle(path: path)
        else {
            return key
        }
        return languageBundle.localizedString(forKey: key, value: nil, table: "Localizable")
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
    var title: String
    var placeholder: String
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
                TextField(
                    placeholder,
                    text: Binding(
                        get: { code },
                        set: { code = formatRoomCodeInput($0) }
                    )
                )
                    .textFieldStyle(.plain)
                    .font(.body.monospaced())
                    .foregroundStyle(Theme.text)
                    .disabled(disabled)
                    .accessibilityIdentifier(accessibilityIdentifier)
                if let pasteAction {
                    Button(action: pasteAction) {
                        Label(
                            AppText.value("Paste", "粘贴", language: language),
                            systemImage: "doc.on.clipboard"
                        )
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                    }
                    .disabled(disabled)
                }
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
                TextField(
                    placeholder,
                    text: Binding(
                        get: { code },
                        set: { code = formatRoomCodeInput($0) }
                    )
                )
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

func availableReceivedDirectoryItemURLs(
    directory: URL,
    fileManager: FileManager = .default
) -> [URL] {
    guard availableCompletedDirectoryURL(path: directory.path) != nil,
          let contents = try? fileManager.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: [.isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey],
            options: [.skipsHiddenFiles]
          ) else {
        return []
    }
    return contents.compactMap { url -> (url: URL, isDirectory: Bool)? in
        guard let values = try? url.resourceValues(
            forKeys: [.isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey]
        ), values.isSymbolicLink != true else {
            return nil
        }
        if values.isDirectory == true { return (url, true) }
        if values.isRegularFile == true { return (url, false) }
        return nil
    }.sorted { lhs, rhs in
        if lhs.isDirectory != rhs.isDirectory { return lhs.isDirectory }
        return lhs.url.lastPathComponent.localizedStandardCompare(
            rhs.url.lastPathComponent
        ) == .orderedAscending
    }.map(\.url)
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

#if os(macOS)
struct ReceivedItemsPresentation: Identifiable {
    let id = UUID()
    let urls: [URL]
}

struct ReceivedItemsSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.appLanguage) private var language
    let urls: [URL]

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text(AppText.value("Received Items", "已接收项目", language: language))
                    .font(.title2.weight(.semibold))
                Spacer()
                Button(AppText.value("Done", "完成", language: language)) {
                    dismiss()
                }
            }

            List(urls, id: \.self) { url in
                Button {
                    revealInFinder(url)
                } label: {
                    Label(
                        url.lastPathComponent,
                        systemImage: availableCompletedDirectoryURL(path: url.path) == nil
                            ? "doc"
                            : "folder"
                    )
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("received_item_reveal_\(url.lastPathComponent)")
            }
        }
        .padding(20)
        .frame(minWidth: 440, minHeight: 320)
    }
}
#endif

/// Formats a byte count as a short human-readable string (auto KB/MB/GB).
func byteString(_ bytes: UInt64) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(clamping: bytes), countStyle: .file)
}

/// Writes a minimal `config.toml` fragment to app storage and returns its path,
/// or returns an empty string when no config overrides are configured.
private let runtimeConfigFileName = "envoix-runtime-config.toml"

func resolveConfigPath(
    candidatesAllow: String = "",
    candidatesDeny: String = ""
) throws -> String {
    let allow = configListLines(candidatesAllow)
    let deny = configListLines(candidatesDeny)
    if allow.isEmpty && deny.isEmpty {
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
    guard bytesPerSec.isFinite, bytesPerSec > 0 else { return "0 B/s" }
    let rate = min(bytesPerSec, Double(Int64.max))
    let format: String
    let value: Double
    let fractionDigits: Int
    switch rate {
    case 1_000_000_000...:
        format = "%.2f GB/s"
        value = rate / 1_000_000_000
        fractionDigits = 2
    case 1_000_000...:
        format = "%.1f MB/s"
        value = rate / 1_000_000
        fractionDigits = 1
    case 1_000...:
        format = "%.0f KB/s"
        value = rate / 1_000
        fractionDigits = 0
    default:
        format = "%.0f B/s"
        value = rate
        fractionDigits = 0
    }
    let scale = pow(10, Double(fractionDigits))
    let rounded = (value * scale).rounded(.toNearestOrAwayFromZero) / scale
    return String(format: format, locale: Locale(identifier: "en_US_POSIX"), rounded)
}

/// Formats a remaining-time estimate as "ETA 1:20" / "ETA 1:02:03".
func etaString(_ seconds: Double) -> String {
    let s = Int(seconds.rounded())
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60)
    if h > 0 { return String(format: "ETA %d:%02d:%02d", h, m, sec) }
    return String(format: "ETA %d:%02d", m, sec)
}
