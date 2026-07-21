#if os(iOS)
import SwiftUI
import UIKit

struct NearbyDiscoveryView: View {
    @Environment(\.appLanguage) private var language
    @Environment(\.scenePhase) private var scenePhase
    @ObservedObject private var coordinator: NearbyDiscoveryCoordinator
    @State private var pageIsVisible = false
    private let onSelectPeer: (NearbyPairingSelection) -> Void

    init(
        coordinator: NearbyDiscoveryCoordinator = NearbyDiscoveryCoordinator(),
        onSelectPeer: @escaping (NearbyPairingSelection) -> Void = { _ in }
    ) {
        self.coordinator = coordinator
        self.onSelectPeer = onSelectPeer
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                intro
                providerPanel

                if coordinator.state.statuses[.bluetooth]?.availability == .permissionRequired {
                    Button(action: openSettings) {
                        Label(
                            AppText.value("Open Bluetooth settings", "打开蓝牙设置", language: language),
                            systemImage: "gearshape"
                        )
                    }
                    .buttonStyle(PrimaryActionButtonStyle())
                    .accessibilityIdentifier("nearby_open_settings")
                }

                Text(AppText.value("NEARBY DEVICES", "附近设备", language: language))
                    .font(.caption.weight(.bold))
                    .tracking(0.8)
                    .foregroundStyle(Theme.muted)
                    .padding(.top, 4)

                if coordinator.state.peers.isEmpty {
                    emptyState
                } else {
                    ForEach(coordinator.state.peers) { peer in
                        Button {
                            onSelectPeer(NearbyPairingSelection(peer: peer))
                        } label: {
                            peerCard(peer)
                        }
                        .buttonStyle(.plain)
                        .accessibilityHint(AppText.value(
                            "Open experimental Bluetooth pairing",
                            "打开实验性蓝牙配对",
                            language: language
                        ))
                        .accessibilityIdentifier("nearby_peer_card")
                    }
                }

                Text(AppText.value(
                    "Experimental BLE pairing is unauthenticated. A nearby attacker may impersonate or relay a selected device.",
                    "实验性蓝牙配对未经身份认证。附近的攻击者可能冒充或中继所选设备。",
                    language: language
                ))
                .font(.footnote)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 16)
        }
        .background(Theme.bg)
        .navigationTitle(AppText.value("Nearby devices", "附近设备", language: language))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(action: coordinator.restart) {
                    Image(systemName: "arrow.clockwise")
                        .font(.body.weight(.semibold))
                }
                .accessibilityLabel(AppText.value("Restart discovery", "重新开始发现", language: language))
                .accessibilityIdentifier("nearby_restart")
            }
        }
        .onAppear {
            pageIsVisible = true
            if scenePhase == .active {
                coordinator.start()
            }
        }
        .onDisappear {
            pageIsVisible = false
            coordinator.stop()
        }
        .onChange(of: scenePhase) { phase in
            if pageIsVisible, phase == .active {
                coordinator.start()
            } else {
                coordinator.stop()
            }
        }
        .accessibilityIdentifier("nearby_screen")
    }

    private var intro: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(AppText.value(
                "Visible as \(coordinator.state.localName)",
                "显示为 \(coordinator.state.localName)",
                language: language
            ))
            .font(.headline)
            .foregroundStyle(Theme.text)

            Text(AppText.value(
                "Bluetooth and the local network are checked only while this page is open.",
                "仅在此页面打开时检查蓝牙和局域网。",
                language: language
            ))
            .font(.subheadline)
            .foregroundStyle(Theme.muted)
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var providerPanel: some View {
        VStack(spacing: 12) {
            ForEach(NearbyDiscoverySource.allCases, id: \.self) { source in
                providerRow(status(for: source))
            }
        }
        .card(padding: 14)
    }

    private func providerRow(_ status: NearbyProviderStatus) -> some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(sourceTitle(status.source))
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Text(detailText(status.detail))
                    .font(.caption)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            StatusPill(
                text: availabilityText(status.availability),
                kind: statusKind(status.availability)
            )
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("nearby_provider_\(status.source.logName)")
    }

    private var emptyState: some View {
        HStack(spacing: 12) {
            ProgressView()
                .tint(Theme.accentStrong)
            Text(coordinator.state.isActive
                 ? AppText.value("Searching for Envoix devices…", "正在搜索 Envoix 设备…", language: language)
                 : AppText.value("Discovery is paused.", "发现已暂停。", language: language))
                .font(.body)
                .foregroundStyle(Theme.muted)
        }
        .card(padding: 18)
        .accessibilityIdentifier("nearby_empty_state")
    }

    private func peerCard(_ peer: NearbyDiscoveredPeer) -> some View {
        VStack(alignment: .leading, spacing: 9) {
            Text(peer.displayName ?? AppText.value("Nearby Envoix device", "附近的 Envoix 设备", language: language))
                .font(.headline)
                .foregroundStyle(Theme.text)
                .lineLimit(1)

            HStack(spacing: 7) {
                ForEach(peer.sources.sorted(by: { $0.rawValue < $1.rawValue }), id: \.self) { source in
                    ModePill(text: sourceShortTitle(source))
                }
                if let rssi = peer.rssi {
                    Text("\(rssi) dBm")
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(Theme.muted)
                }
            }

            Text(lastSeenText(peer.lastSeenAtMilliseconds))
                .font(.caption)
                .foregroundStyle(Theme.muted)
        }
        .card(raised: true, padding: 15)
        .accessibilityElement(children: .contain)
    }

    private func status(for source: NearbyDiscoverySource) -> NearbyProviderStatus {
        coordinator.state.statuses[source] ?? NearbyProviderStatus(
            source: source,
            availability: .stopped,
            detail: .discoveryStopped
        )
    }

    private func lastSeenText(_ lastSeenMilliseconds: Int64) -> String {
        let ageSeconds = max(0, coordinator.state.nowMilliseconds - lastSeenMilliseconds) / 1_000
        if ageSeconds == 0 {
            return AppText.value("Seen just now", "刚刚发现", language: language)
        }
        return AppText.value("Seen \(ageSeconds)s ago", "\(ageSeconds) 秒前发现", language: language)
    }

    private func sourceTitle(_ source: NearbyDiscoverySource) -> String {
        switch source {
        case .bluetooth: return "Bluetooth LE"
        case .mdns: return AppText.value("mDNS / local network", "mDNS / 局域网", language: language)
        case .wifiAware: return "Wi-Fi Aware"
        }
    }

    private func sourceShortTitle(_ source: NearbyDiscoverySource) -> String {
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
        case .permissionRequired: return AppText.value("Permission", "需授权", language: language)
        case .disabled: return AppText.value("Off", "已关闭", language: language)
        case .unsupported: return AppText.value("Unsupported", "不支持", language: language)
        case .temporarilyUnavailable: return AppText.value("Unavailable", "不可用", language: language)
        case .reserved: return AppText.value("Reserved", "已预留", language: language)
        case .error: return AppText.value("Error", "错误", language: language)
        }
    }

    private func statusKind(_ availability: NearbyProviderAvailability) -> StatusPill.Kind {
        switch availability {
        case .ready: return .success
        case .starting, .stopped, .reserved: return .neutral
        case .degraded: return .warning
        case .permissionRequired, .disabled, .unsupported, .temporarilyUnavailable, .error: return .error
        }
    }

    private func detailText(_ detail: NearbyProviderDetail) -> String {
        switch detail {
        case .discoveryStopped:
            return AppText.value("Discovery is stopped", "发现已停止", language: language)
        case .startingBluetooth:
            return AppText.value("Starting Bluetooth discovery", "正在启动蓝牙发现", language: language)
        case .bluetoothAccessRequired:
            return AppText.value("Bluetooth access is required", "需要蓝牙权限", language: language)
        case .bluetoothUnavailable:
            return AppText.value("Bluetooth discovery is unavailable", "蓝牙发现不可用", language: language)
        case .bluetoothOff:
            return AppText.value("Bluetooth is turned off", "蓝牙已关闭", language: language)
        case .bluetoothReady:
            return AppText.value("Scanning and visible over Bluetooth", "正在通过蓝牙扫描并保持可见", language: language)
        case .bluetoothVisibilityStarting:
            return AppText.value("Scanning; visibility is starting", "正在扫描；可见性启动中", language: language)
        case .bluetoothScanningOnly:
            return AppText.value("Scanning only; visibility is unavailable", "仅可扫描；无法保持可见", language: language)
        case .bluetoothVisibleOnly:
            return AppText.value("Visible only; scanning is unavailable", "仅保持可见；无法扫描", language: language)
        case .startingLocalNetwork:
            return AppText.value("Starting local-network discovery", "正在启动局域网发现", language: language)
        case .localNetworkReady:
            return AppText.value("Scanning and visible on the local network", "正在局域网中扫描并保持可见", language: language)
        case .localNetworkScanningOnly:
            return AppText.value("Scanning only; local visibility is unavailable", "仅可扫描；无法在局域网中保持可见", language: language)
        case .localNetworkVisibleOnly:
            return AppText.value("Visible only; local scanning is unavailable", "仅保持可见；无法进行局域网扫描", language: language)
        case .localNetworkPermissionOrUnavailable:
            return AppText.value(
                "Local-network permission was denied or discovery is unavailable",
                "局域网权限被拒绝，或发现功能暂不可用",
                language: language
            )
        case .wifiAwareReserved:
            return AppText.value("Provider boundary reserved for a later phase", "已为后续阶段预留 Provider 接口", language: language)
        }
    }

    private func openSettings() {
        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
        UIApplication.shared.open(url)
    }
}

struct NearbyPairingView: View {
    @Environment(\.appLanguage) private var language

    let selection: NearbyPairingSelection
    let sendEnabled: Bool
    let receiveEnabled: Bool
    let isBusy: Bool
    let error: String?
    let onSend: () -> Void
    let onReceive: () -> Void

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                VStack(alignment: .leading, spacing: 7) {
                    Text(selection.displayName ?? AppText.value(
                        "Nearby Envoix device",
                        "附近的 Envoix 设备",
                        language: language
                    ))
                    .font(.title2.bold())
                    .foregroundStyle(Theme.text)

                    HStack(spacing: 7) {
                        ForEach(selection.sources.sorted(by: { $0.rawValue < $1.rawValue }), id: \.self) { source in
                            ModePill(text: sourceTitle(source))
                        }
                    }
                }

                VStack(alignment: .leading, spacing: 8) {
                    Label(
                        AppText.value("Unauthenticated experimental channel", "未经认证的实验通道", language: language),
                        systemImage: "exclamationmark.triangle"
                    )
                    .font(.headline)
                    .foregroundStyle(Theme.text)
                    Text(AppText.value(
                        "The invitation will be sent over BLE without authenticating the selected device. SPAKE2 still protects the transfer channel, but cannot restore peer identity because its invitation crosses the same unauthenticated path.",
                        "邀请码将通过 BLE 发送，但不会认证所选设备。SPAKE2 仍保护传输通道；由于邀请码经过同一条未认证路径，它无法恢复对端身份保证。",
                        language: language
                    ))
                    .font(.subheadline)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
                }
                .card(padding: 16)
                .accessibilityIdentifier("nearby_pairing_security")

                if let error {
                    Text(error)
                        .font(.footnote)
                        .foregroundStyle(Theme.danger)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityIdentifier("nearby_pairing_error")
                }

                Button(action: onSend) {
                    Label(
                        AppText.value("Send to this device", "发送到此设备", language: language),
                        systemImage: "paperplane"
                    )
                    .frame(maxWidth: .infinity)
                }
                .buttonStyle(PrimaryActionButtonStyle())
                .disabled(!sendEnabled || isBusy)
                .accessibilityIdentifier("nearby_pairing_send")

                Button(action: onReceive) {
                    Label(
                        AppText.value("Receive from this device", "从此设备接收", language: language),
                        systemImage: "tray.and.arrow.down"
                    )
                    .frame(maxWidth: .infinity)
                }
                .buttonStyle(.bordered)
                .tint(Theme.accentStrong)
                .disabled(!receiveEnabled || isBusy)
                .accessibilityIdentifier("nearby_pairing_receive")
            }
            .padding(.vertical, 16)
        }
        .accessibilityIdentifier("nearby_pairing_context")
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
