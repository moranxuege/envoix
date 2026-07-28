#if os(iOS)
import SwiftUI

struct NearbyTransferContextView: View {
    @Environment(\.appLanguage) private var language

    let selection: NearbyPairingSelection
    let deliversInvitationOnStart: Bool
    let isDelivering: Bool
    let error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            Label(
                selection.displayName ?? AppText.value(
                    "Nearby Envoix device",
                    "附近的 Envoix 设备",
                    language: language
                ),
                systemImage: "dot.radiowaves.left.and.right"
            )
            .font(.headline)
            .foregroundStyle(Theme.text)

            HStack(spacing: 7) {
                ForEach(selection.sources.sorted(by: { $0.rawValue < $1.rawValue }), id: \.self) { source in
                    ModePill(text: sourceTitle(source))
                }
                ModePill(text: AppText.value("Unverified", "未经验证", language: language))
            }

            Text(deliversInvitationOnStart
                 ? AppText.value(
                    "Choose the transfer details first. The BLE invitation is sent only after you tap Start.",
                    "请先完成传输设置；只有点击开始后才会发送 BLE 邀请。",
                    language: language
                 )
                 : AppText.value(
                    "The accepted invitation is loaded below. The nearby device name remains unverified.",
                    "已接受的邀请码载入下方；附近设备名称仍未经验证。",
                    language: language
                 ))
                .font(.footnote)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)

            if isDelivering {
                HStack(spacing: 8) {
                    ProgressView()
                        .controlSize(.small)
                    Text(AppText.value(
                        "Delivering invitation…",
                        "正在发送邀请码…",
                        language: language
                    ))
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
