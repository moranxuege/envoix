import SwiftUI
#if os(macOS)
import AppKit
#elseif os(iOS)
import UIKit
#endif

// MARK: - Dynamic colors

#if os(macOS)
private extension NSColor {
    /// Builds an opaque sRGB color from a `0xRRGGBB` literal.
    convenience init(rgb: UInt32) {
        self.init(
            srgbRed: CGFloat((rgb >> 16) & 0xff) / 255,
            green: CGFloat((rgb >> 8) & 0xff) / 255,
            blue: CGFloat(rgb & 0xff) / 255,
            alpha: 1
        )
    }
}
#elseif os(iOS)
private extension UIColor {
    /// Builds an opaque sRGB color from a `0xRRGGBB` literal.
    convenience init(rgb: UInt32) {
        self.init(
            red: CGFloat((rgb >> 16) & 0xff) / 255,
            green: CGFloat((rgb >> 8) & 0xff) / 255,
            blue: CGFloat(rgb & 0xff) / 255,
            alpha: 1
        )
    }
}
#endif

extension Color {
    /// A color that resolves to `light` or `dark` (each `0xRRGGBB`) based on the
    /// effective appearance, so the system theme and `.preferredColorScheme`
    /// (driven by the in-app toggle) both switch it.
    init(light: UInt32, dark: UInt32) {
        #if os(macOS)
        self.init(nsColor: NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            return NSColor(rgb: isDark ? dark : light)
        })
        #elseif os(iOS)
        self.init(uiColor: UIColor { traits in
            UIColor(rgb: traits.userInterfaceStyle == .dark ? dark : light)
        })
        #endif
    }
}

// MARK: - Design tokens (from the reference demo)

enum Theme {
    static let bg = Color(light: 0xf8fafd, dark: 0x061126)
    static let surface = Color(light: 0xffffff, dark: 0x0a1830)
    static let surfaceRaised = Color(light: 0xfdfeff, dark: 0x10213d)
    static let text = Color(light: 0x0a1330, dark: 0xffffff)
    static let muted = Color(light: 0x53627a, dark: 0xb8c5d9)
    static let line = Color(light: 0xe6ecf5, dark: 0x263b5d)
    static let accent = Color(light: 0x1677ff, dark: 0x66a9ff)
    static let accentStrong = Color(light: 0x0d47a1, dark: 0xa8ceff)
    static let accentSoft = Color(light: 0xeaf2ff, dark: 0x142f55)
    static let success = Color(light: 0x147a4b, dark: 0x61d69a)
    static let warning = Color(light: 0xa05a00, dark: 0xffc166)
    static let danger = Color(light: 0xe74c3c, dark: 0xf07167)
    static let dangerStrong = Color(light: 0xb42318, dark: 0xffb4aa)
    static let dangerSoft = Color(light: 0xfff4f2, dark: 0x3a2020)

    static let cardRadius: CGFloat = 16
    static let pillRadius: CGFloat = 999

    /// Subtle shadow reserved for transient overlays.
    static let shadowColor = Color.black.opacity(0.08)
    static let shadowRadius: CGFloat = 8
    static let shadowY: CGFloat = 3
}

// MARK: - Appearance preference

/// User's appearance choice, persisted and applied at the app root.
enum Appearance: String, CaseIterable {
    case system, light, dark

    var colorScheme: ColorScheme? {
        switch self {
        case .system: return nil
        case .light: return .light
        case .dark: return .dark
        }
    }

    var icon: String {
        switch self {
        case .system: return "circle.lefthalf.filled"
        case .light: return "sun.max"
        case .dark: return "moon"
        }
    }

    var next: Appearance {
        let all = Appearance.allCases
        return all[(all.firstIndex(of: self)! + 1) % all.count]
    }
}

struct PrimaryActionButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        PrimaryActionButton(configuration: configuration)
    }

    private struct PrimaryActionButton: View {
        @Environment(\.isEnabled) private var isEnabled
        let configuration: Configuration

        var body: some View {
            configuration.label
                .font(.headline.weight(.semibold))
                .foregroundStyle(isEnabled ? Color.white : Theme.text)
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                .background(
                    isEnabled ? Theme.accentStrong : Theme.line,
                    in: RoundedRectangle(cornerRadius: Theme.cardRadius)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.cardRadius)
                        .strokeBorder(isEnabled ? Color.clear : Theme.line, lineWidth: 1)
                )
                .opacity(configuration.isPressed ? 0.82 : 1)
        }
    }
}
