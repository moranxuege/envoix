#if os(iOS)
import SwiftUI

enum NearbyTransferContextPresentationText {
    static func fallbackDeviceName(language: String) -> String {
        AppText.localized("connection.nearby.peer.fallback", language: language)
    }

    static func trustLabel(language: String) -> String {
        AppText.localized("connection.nearby.context.unverified", language: language)
    }

    static func detail(deliversInvitationOnStart: Bool, language: String) -> String {
        AppText.localized(
            deliversInvitationOnStart
                ? "connection.nearby.context.sender_help"
                : "connection.nearby.context.receiver_help",
            language: language
        )
    }

    static func deliveryStatus(language: String) -> String {
        AppText.localized("connection.nearby.context.delivering", language: language)
    }
}

struct NearbyTransferContextView: View {
    @Environment(\.appLanguage) private var language

    let selection: NearbyPairingSelection
    let deliversInvitationOnStart: Bool
    let isDelivering: Bool
    let error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            Label(
                selection.displayName
                    ?? NearbyTransferContextPresentationText.fallbackDeviceName(language: language),
                systemImage: "dot.radiowaves.left.and.right"
            )
            .font(.headline)
            .foregroundStyle(Theme.text)

            HStack(spacing: 7) {
                ForEach(selection.sources.sorted(by: { $0.rawValue < $1.rawValue }), id: \.self) { source in
                    ModePill(text: sourceTitle(source))
                }
                ModePill(text: NearbyTransferContextPresentationText.trustLabel(language: language))
            }

            Text(NearbyTransferContextPresentationText.detail(
                deliversInvitationOnStart: deliversInvitationOnStart,
                language: language
            ))
                .font(.footnote)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)

            if isDelivering {
                HStack(spacing: 8) {
                    ProgressView()
                        .controlSize(.small)
                    Text(NearbyTransferContextPresentationText.deliveryStatus(language: language))
                    .font(.footnote)
                    .foregroundStyle(Theme.muted)
                }
                .accessibilityIdentifier("nearby_invite_delivery_progress")
            }

            if let error {
                Text(error)
                    .font(.footnote)
                    .foregroundStyle(Theme.danger)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("nearby_invite_delivery_error")
            }
        }
        .card(padding: 16)
        .accessibilityIdentifier("nearby_transfer_context")
    }

    private func sourceTitle(_ source: NearbyDiscoverySource) -> String {
        switch source {
        case .bluetooth: return "BLE"
        case .mdns: return "mDNS"
        case .wifiAware: return "Aware"
        }
    }
}
#endif
