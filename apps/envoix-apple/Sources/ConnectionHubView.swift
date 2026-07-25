#if os(iOS)
import SwiftUI
import UIKit

struct ConnectionHubView: View {
    @Environment(\.appLanguage) private var language
    @ObservedObject var coordinator: NearbyDiscoveryCoordinator

    let openInFixtureURL: URL?
    let onScanQRCode: () -> Void
    let onShowQRCode: () -> Void
    let onEnterCode: () -> Void
    let onSelectPeer: (NearbyPairingSelection) -> Void

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) {
                intro
                connectionMethods
                nearbyHeader
                providerSummary

                if coordinator.state.statuses[.bluetooth]?.availability == .permissionRequired {
                    Button(action: openSettings) {
                        Label(
                            AppText.value("Open Bluetooth settings", "打开蓝牙设置", language: language),
                            systemImage: "gearshape"
                        )
                        .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.bordered)
                    .tint(Theme.accentStrong)
                    .accessibilityIdentifier("nearby_open_settings")
                }

                if coordinator.state.peers.isEmpty {
                    nearbyEmptyState
                } else {
                    ForEach(coordinator.state.peers) { peer in
                        Button {
                            onSelectPeer(NearbyPairingSelection(peer: peer))
                        } label: {
                            peerCard(peer)
                        }
                        .buttonStyle(.plain)
                        .accessibilityHint(AppText.value(
                            "Open an unverified one-time room",
                            "打开未经验证的一次性房间",
                            language: language
                        ))
                        .accessibilityIdentifier("nearby_peer_card")
                    }
                }

                Label(
                    AppText.value(
                        "Nearby names are unverified. Confirm the invitation on the other device before transferring.",
                        "附近设备名称未经验证。传输前，请在另一台设备上确认邀请。",
                        language: language
                    ),
                    systemImage: "exclamationmark.shield"
                )
                .font(.footnote)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)

                #if DEBUG
                if let openInFixtureURL {
                    Text(openInFixtureURL.absoluteString)
                        .font(.caption2)
                        .accessibilityIdentifier("open_in_fixture_url")
                }
                #endif
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 16)
        }
        .background(Theme.bg)
        .accessibilityIdentifier("connection_hub")
    }

    private var intro: some View {
        HStack(alignment: .top, spacing: 13) {
            Image(systemName: "point.3.connected.trianglepath.dotted")
                .font(.title2.weight(.semibold))
                .foregroundStyle(Theme.accentStrong)
                .frame(width: 50, height: 50)
                .background(Theme.accentSoft, in: RoundedRectangle(cornerRadius: 15, style: .continuous))

            VStack(alignment: .leading, spacing: 4) {
                Text(AppText.value("Connect to a device", "连接设备", language: language))
                    .font(.title.bold())
                    .foregroundStyle(Theme.text)
                Text(AppText.value(
                    "Choose one simple way to open a one-time room.",
                    "选择一种简单方式，打开一次性房间。",
                    language: language
                ))
                .font(.subheadline)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var connectionMethods: some View {
        HStack(spacing: 10) {
            methodButton(
                AppText.value("Scan QR", "扫描二维码", language: language),
                systemImage: "qrcode.viewfinder",
                identifier: "connect_scan_qr",
                action: onScanQRCode
            )
            methodButton(
                AppText.value("Show QR", "显示二维码", language: language),
                systemImage: "qrcode",
                identifier: "connect_show_qr",
                action: onShowQRCode
            )
            methodButton(
                AppText.value("Enter code", "输入配对码", language: language),
                systemImage: "keyboard",
                identifier: "connect_enter_code",
                action: onEnterCode
            )
        }
    }

    private func methodButton(
        _ title: String,
        systemImage: String,
        identifier: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            VStack(spacing: 9) {
                Image(systemName: systemImage)
                    .font(.title3.weight(.semibold))
                Text(title)
                    .font(.subheadline.weight(.semibold))
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .foregroundStyle(Theme.accentStrong)
            .frame(maxWidth: .infinity, minHeight: 90)
            .background(Theme.surfaceRaised, in: RoundedRectangle(cornerRadius: Theme.cardRadius))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(Theme.line.opacity(0.72), lineWidth: 0.8)
            )
            .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(identifier)
    }

    private var nearbyHeader: some View {
        HStack {
            VStack(alignment: .leading, spacing: 3) {
                Text(AppText.value("Nearby", "附近设备", language: language))
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Text(AppText.value(
                    "Visible as \(coordinator.state.localName)",
                    "显示为 \(coordinator.state.localName)",
                    language: language
                ))
                .font(.caption)
                .foregroundStyle(Theme.muted)
            }
            Spacer()
            Button(action: coordinator.restart) {
                Image(systemName: "arrow.clockwise")
                    .font(.body.weight(.semibold))
                    .frame(width: 36, height: 36)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(Theme.accentStrong)
            .accessibilityLabel(AppText.value("Restart discovery", "重新开始发现", language: language))
            .accessibilityIdentifier("nearby_restart")
        }
        .padding(.top, 4)
    }

    private var providerSummary: some View {
        HStack(spacing: 8) {
            ForEach(NearbyDiscoverySource.allCases, id: \.self) { source in
                let status = coordinator.state.statuses[source] ?? NearbyProviderStatus(
                    source: source,
                    availability: .stopped,
                    detail: .discoveryStopped
                )
                HStack(spacing: 5) {
                    Circle()
                        .fill(statusColor(status.availability))
                        .frame(width: 7, height: 7)
                    Text(sourceTitle(source))
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(Theme.muted)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 9)
                .frame(maxWidth: .infinity, minHeight: 32)
                .background(Theme.surface, in: Capsule())
                .accessibilityElement(children: .combine)
                .accessibilityLabel("\(sourceTitle(source)), \(availabilityText(status.availability))")
                .accessibilityIdentifier("nearby_provider_\(source.logName)")
            }
        }
    }

    private var nearbyEmptyState: some View {
        HStack(spacing: 12) {
            if coordinator.state.isActive && hasReadyProvider {
                ProgressView()
                    .tint(Theme.accentStrong)
            } else {
                Image(systemName: "antenna.radiowaves.left.and.right.slash")
                    .foregroundStyle(Theme.muted)
            }
            Text(nearbyEmptyText)
                .font(.body)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
        }
        .card(padding: 18)
        .accessibilityIdentifier("nearby_empty_state")
    }

    private var nearbyEmptyText: String {
        guard coordinator.state.isActive else {
            return AppText.value("Nearby discovery is paused.", "附近设备发现已暂停。", language: language)
        }
        guard hasReadyProvider else {
            return AppText.value(
                "Nearby discovery is unavailable. Use QR or a pairing code.",
                "附近设备发现不可用。请使用二维码或配对码。",
                language: language
            )
        }
        return AppText.value("Looking for Envoix devices…", "正在搜索 Envoix 设备…", language: language)
    }

    private var hasReadyProvider: Bool {
        coordinator.state.statuses.values.contains {
            $0.availability == .ready || $0.availability == .degraded || $0.availability == .starting
        }
    }

    private func peerCard(_ peer: NearbyDiscoveredPeer) -> some View {
        HStack(spacing: 13) {
            Image(systemName: "iphone.gen2.radiowaves.left.and.right")
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.accentStrong)
                .frame(width: 44, height: 44)
                .background(Theme.accentSoft, in: RoundedRectangle(cornerRadius: 13))

            VStack(alignment: .leading, spacing: 5) {
                Text(peer.displayName ?? AppText.value(
                    "Nearby Envoix device",
                    "附近的 Envoix 设备",
                    language: language
                ))
                .font(.headline)
                .foregroundStyle(Theme.text)
                .lineLimit(1)

                HStack(spacing: 6) {
                    ForEach(peer.sources.sorted(by: { $0.rawValue < $1.rawValue }), id: \.self) { source in
                        ModePill(text: sourceTitle(source))
                    }
                    Text(AppText.value("Unverified", "未经验证", language: language))
                        .font(.caption)
                        .foregroundStyle(Theme.warning)
                }
            }

            Spacer(minLength: 8)
            Image(systemName: "chevron.right")
                .font(.caption.weight(.bold))
                .foregroundStyle(Theme.muted)
        }
        .card(raised: true, padding: 14)
        .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
    }

    private func sourceTitle(_ source: NearbyDiscoverySource) -> String {
        switch source {
        case .bluetooth: return "BLE"
        case .mdns: return "mDNS"
        case .wifiAware: return "Aware"
        }
    }

    private func availabilityText(_ availability: NearbyProviderAvailability) -> String {
        switch availability {
        case .stopped: return AppText.value("Stopped", "已停止", language: language)
        case .starting: return AppText.value("Starting", "启动中", language: language)
        case .ready: return AppText.value("Ready", "就绪", language: language)
        case .degraded: return AppText.value("Degraded", "部分可用", language: language)
        case .permissionRequired: return AppText.value("Permission required", "需要权限", language: language)
        case .disabled: return AppText.value("Off", "已关闭", language: language)
        case .unsupported: return AppText.value("Unsupported", "不支持", language: language)
        case .temporarilyUnavailable: return AppText.value("Unavailable", "不可用", language: language)
        case .reserved: return AppText.value("Planned", "计划中", language: language)
        case .error: return AppText.value("Error", "错误", language: language)
        }
    }

    private func statusColor(_ availability: NearbyProviderAvailability) -> Color {
        switch availability {
        case .ready: return Theme.success
        case .starting, .stopped, .reserved: return Theme.muted
        case .degraded: return Theme.warning
        case .permissionRequired, .disabled, .unsupported, .temporarilyUnavailable, .error:
            return Theme.danger
        }
    }

    private func openSettings() {
        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
        UIApplication.shared.open(url)
    }
}
#endif
