import SwiftUI
#if os(macOS)
import AppKit
#elseif os(iOS)
import UIKit
#endif

// MARK: - Card

private struct CardModifier: ViewModifier {
    var raised: Bool
    var padding: CGFloat

    func body(content: Content) -> some View {
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(raised ? Theme.surfaceRaised : Theme.surface)
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
            )
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
            .shadow(color: Color.black.opacity(raised ? 0.025 : 0), radius: raised ? 3 : 0, y: raised ? 1 : 0)
    }
}

extension View {
    /// Wraps content in a light rounded panel.
    func card(raised: Bool = false, padding: CGFloat = 16) -> some View {
        modifier(CardModifier(raised: raised, padding: padding))
    }
}

// MARK: - Pills

/// Rounded status chip (e.g. "Completed", "Waiting…", an error).
struct StatusPill: View {
    enum Kind { case success, warning, error, neutral }
    var text: String
    var systemImage: String?
    var kind: Kind = .success

    private var tint: Color {
        switch kind {
        case .success: return Theme.success
        case .warning: return Theme.warning
        case .error: return Theme.danger
        case .neutral: return Theme.muted
        }
    }

    var body: some View {
        HStack(spacing: 5) {
            if let systemImage { Image(systemName: systemImage) }
            Text(text)
        }
        .font(.body.weight(.semibold))
        .foregroundStyle(tint)
        .padding(.horizontal, 14)
        .padding(.vertical, 7)
        .background(tint.opacity(0.10))
        .clipShape(Capsule())
    }
}

/// Small accent chip marking the active pairing mode.
struct ModePill: View {
    var text: String

    var body: some View {
        Text(text)
            .font(.body.weight(.semibold))
            .foregroundStyle(Theme.accentStrong)
            .padding(.horizontal, 14)
            .padding(.vertical, 7)
            .background(Theme.accentSoft.opacity(0.75))
            .clipShape(Capsule())
    }
}

// MARK: - Pairing selector

/// Selector for choosing the pairing transport without hiding each mode's
/// behavioral difference.
struct PairingModeSelector: View {
    enum Role { case send, receive }

    @Environment(\.appLanguage) private var language
    @AppStorage("envoix.developerMode") private var developerMode = false
    @Binding var selection: PairingMode
    var role: Role = .send
    var disabled: Bool

    var body: some View {
        #if os(iOS)
        mobileBody
        #else
        desktopBody
        #endif
    }

    private var desktopBody: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(AppText.value("Pairing method", "配对方式", language: language))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)

            HStack(spacing: 10) {
                pairingOptions
            }
        }
        .card(padding: 14)
        .disabled(disabled)
        .onAppear(perform: ensureVisibleSelection)
        .onChange(of: developerMode) { _ in ensureVisibleSelection() }
    }

    #if os(iOS)
    private var mobileBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(AppText.value("Pairing", "配对", language: language))
                .font(.headline.weight(.semibold))
                .foregroundStyle(Theme.muted)

            HStack(spacing: 8) {
                mobileOption(.room, title: AppText.value("QR / Code", "扫码 / 短码", language: language), systemImage: "qrcode")
                if developerMode {
                    mobileOption(.token, title: AppText.value("Token", "口令", language: language), systemImage: "key")
                }
            }

            Text(selectedHint)
                .font(.footnote)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
        }
        .card(padding: 12)
        .disabled(disabled)
        .onAppear(perform: ensureVisibleSelection)
        .onChange(of: developerMode) { _ in ensureVisibleSelection() }
    }

    private func mobileOption(_ mode: PairingMode, title: String, systemImage: String) -> some View {
        let selected = selection == mode
        return Button {
            selection = mode
        } label: {
            VStack(spacing: 4) {
                Image(systemName: systemImage)
                    .font(.headline.weight(.semibold))
                Text(title)
                    .font(.caption.weight(.semibold))
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
            }
            .frame(maxWidth: .infinity, minHeight: 50)
            .foregroundStyle(selected ? Theme.accentStrong : Theme.muted)
            .background(selected ? Theme.accentSoft : Theme.surface)
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(selected ? Theme.accent.opacity(0.55) : Theme.line.opacity(0.75), lineWidth: selected ? 1.2 : 0.8)
            )
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
            .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        }
        .buttonStyle(.plain)
    }

    private var selectedHint: String {
        switch selection {
        case .room:
            return role == .receive
                ? AppText.value("Show a QR code and short code on this device.", "在本机显示二维码和短码。", language: language)
                : AppText.value("Scan the receiver QR or enter its short code.", "扫描接收端二维码，或输入短码。", language: language)
        case .invite:
            return AppText.value("Legacy invite link.", "旧版邀请链接。", language: language)
        case .token:
            return AppText.value("Same network only. Hotspots may block discovery.", "仅适合同一局域网；热点可能阻止发现。", language: language)
        }
    }
    #endif

    @ViewBuilder private var pairingOptions: some View {
        option(
            mode: .room,
            title: role == .receive
                ? AppText.value("Share QR / Code", "分享二维码 / 短码", language: language)
                : AppText.value("Scan or Enter Code", "扫码或输入短码", language: language),
            subtitle: role == .receive
                ? AppText.value("Recommended. Android-compatible QR and short code.", "推荐。兼容 Android 的二维码和短码。", language: language)
                : AppText.value("Recommended. Scan the receiver QR or type its code.", "推荐。扫描接收端二维码，或输入短码。", language: language),
            systemImage: "qrcode"
        )
        if developerMode {
            option(
                mode: .token,
                title: AppText.value("Use Shared Token", "使用共享口令", language: language),
                subtitle: AppText.value("Advanced same-LAN discovery without the broker.", "高级：不通过配对服务器，仅同局域网发现。", language: language),
                systemImage: "key"
            )
        }
    }

    private func ensureVisibleSelection() {
        if selection == .invite || (selection == .token && !developerMode) {
            selection = .room
        }
    }

    private func option(
        mode: PairingMode,
        title: String,
        subtitle: String,
        systemImage: String
    ) -> some View {
        let selected = selection == mode

        return Button {
            selection = mode
        } label: {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: systemImage)
                    .font(.title3.weight(.semibold))
                    .frame(width: 28, height: 28)
                    .foregroundStyle(selected ? Theme.accentStrong : Theme.muted)

                VStack(alignment: .leading, spacing: 4) {
                    Text(title)
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(selected ? Theme.accentStrong : Theme.text)
                    Text(subtitle)
                        .font(.callout)
                        .foregroundStyle(Theme.muted)
                        .lineLimit(2)
                }

                Spacer(minLength: 6)

                Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(selected ? Theme.accent : Theme.line)
            }
            .padding(12)
            .frame(maxWidth: .infinity, minHeight: 86, alignment: .leading)
            .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        }
        .buttonStyle(.plain)
        .background(selected ? Theme.accentSoft : Theme.surface)
        .overlay(
            RoundedRectangle(cornerRadius: Theme.cardRadius)
                .strokeBorder(selected ? Theme.accent.opacity(0.55) : Theme.line.opacity(0.75), lineWidth: selected ? 1.2 : 0.8)
        )
        .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
    }
}

// MARK: - Progress

/// Slim, rounded progress track with an accent fill.
struct ProgressBar: View {
    var value: Double  // 0...1

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.line.opacity(0.65))
                Capsule().fill(Theme.accent)
                    .frame(width: max(0, min(1, value)) * geo.size.width)
            }
        }
        .frame(height: 7)
    }
}

// MARK: - File drop

/// Dashed accent drop area on a soft accent background.
struct FileDropStyle: ViewModifier {
    var targeted: Bool

    func body(content: Content) -> some View {
        content
            .padding(16)
            .frame(maxWidth: .infinity)
            .background(Theme.surface)
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(
                        targeted ? Theme.accent : Theme.accent.opacity(0.38),
                        style: StrokeStyle(lineWidth: targeted ? 2 : 1, dash: [6])
                    )
            )
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
    }
}

extension View {
    func fileDropStyle(targeted: Bool) -> some View { modifier(FileDropStyle(targeted: targeted)) }
}

// MARK: - Link row

/// A bordered row showing a value with trailing action buttons.
struct LinkRow<Trailing: View>: View {
    var text: String
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(spacing: 8) {
            Text(text)
                .font(.body.monospaced())
                .foregroundStyle(Theme.muted)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            trailing
        }
        .padding(8)
        .background(Theme.surface)
        .overlay(
            RoundedRectangle(cornerRadius: Theme.cardRadius)
                .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
        )
        .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
    }
}

// MARK: - QR card

/// White, bordered card framing a QR image (white in both themes, by design).
struct QRCard: View {
    var image: PlatformImage
    var size: CGFloat = 184

    var body: some View {
        platformImage
            .interpolation(.none)
            .resizable()
            .frame(width: size, height: size)
            .padding(14)
            .background(Color.white)
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
            )
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
    }

    private var platformImage: Image {
        #if os(macOS)
        Image(nsImage: image)
        #elseif os(iOS)
        Image(uiImage: image)
        #endif
    }
}

// MARK: - Sidebar rail

/// Left-aligned navigation item with a selected (accent-soft) state.
struct RailButton: View {
    var title: String
    var systemImage: String
    var isSelected: Bool
    var badge: Int = 0
    var action: () -> Void
    @State private var isHovering = false

    var body: some View {
        #if os(macOS)
        content
            .onHover { isHovering = $0 }
        #else
        content
        #endif
    }

    private var content: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                RoundedRectangle(cornerRadius: 2)
                    .fill(isSelected ? Theme.accent : Color.clear)
                    .frame(width: 4, height: 28)

                Image(systemName: systemImage)
                    .font(.title3.weight(.semibold))
                    .frame(width: 24)

                Text(title)
                    .font(.title3.weight(isSelected ? .semibold : .regular))

                Spacer(minLength: 8)

                if badge > 0 {
                    Text("\(badge)")
                        .font(.callout.weight(.bold))
                        .monospacedDigit()
                        .foregroundStyle(.white)
                        .padding(.horizontal, badge > 9 ? 7 : 8)
                        .frame(minHeight: 24)
                        .background(Theme.danger, in: Capsule())
                }
            }
            .padding(.horizontal, 10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .frame(minHeight: 54)
            .contentShape(RoundedRectangle(cornerRadius: 10))
        }
        .buttonStyle(.plain)
        .foregroundStyle(isSelected ? Theme.accentStrong : Theme.text)
        .background(
            isSelected ? Theme.accentSoft : (isHovering ? Theme.line.opacity(0.28) : Color.clear),
            in: RoundedRectangle(cornerRadius: 10)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(isSelected ? Theme.accent.opacity(0.45) : Theme.line.opacity(0.72), lineWidth: 1)
        )
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .contentShape(RoundedRectangle(cornerRadius: 10))
    }
}

// MARK: - Toast

/// Transient bottom message (e.g. "Invite copied"), shown via `ToastCenter`.
@MainActor
final class ToastCenter: ObservableObject {
    static let shared = ToastCenter()
    @Published var message: String?
    private var dismiss: Task<Void, Never>?

    func show(_ message: String) {
        self.message = message
        dismiss?.cancel()
        dismiss = Task {
            try? await Task.sleep(nanoseconds: 1_800_000_000)
            if !Task.isCancelled { self.message = nil }
        }
    }
}

private struct ToastOverlay: ViewModifier {
    @ObservedObject private var center = ToastCenter.shared

    func body(content: Content) -> some View {
        content.overlay(alignment: .bottom) {
            if let message = center.message {
                Text(message)
                    .font(.body.weight(.medium))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 12)
                    .background(Color(light: 0x17202a, dark: 0x17202a))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
                    .shadow(color: Theme.shadowColor, radius: Theme.shadowRadius, y: Theme.shadowY)
                    .padding(.bottom, 22)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        .animation(.spring(response: 0.3, dampingFraction: 0.8), value: center.message)
    }
}

extension View {
    /// Hosts transient toasts posted to `ToastCenter.shared`.
    func toastHost() -> some View { modifier(ToastOverlay()) }
}

/// Convenience: copy text and flash a toast.
@MainActor
func copyWithToast(_ text: String, _ message: String) {
    copyToPasteboard(text)
    ToastCenter.shared.show(message)
}
