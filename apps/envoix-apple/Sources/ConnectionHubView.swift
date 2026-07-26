#if os(iOS)
import SwiftUI
import UIKit

struct ConnectionHubView: View {
    @Environment(\.appLanguage) private var language
    @ObservedObject var coordinator: NearbyDiscoveryCoordinator
    @ObservedObject var presence: NearbyPresencePreferences

    let openInFixtureURL: URL?
    let roomInvitation: RoomControlInvitation?
    let roomInvitationIsRevealed: Bool
    let roomInvitationIsStarting: Bool
    let onScanQRCode: () -> Void
    let onEnterCode: () -> Void
    let onRevealRoomInvitation: () -> Void
    let onHideRoomInvitation: () -> Void
    let onRefreshRoomInvitation: () -> Void
    let onCancelRoomInvitation: () -> Void
    let onSetVisibility: (NearbyVisibilityMode) -> Void
    let onRename: (String) -> Bool
    let onSelectPeer: (NearbyPairingSelection) -> Void

    @State private var isNameEditorPresented = false
    @State private var editedDisplayName = ""

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 16) {
                roomInvitationCard
                connectionMethods
                identityAndVisibility
                nearbyHeader

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

                if roomInvitation != nil {
                    HStack(spacing: 12) {
                        Label(
                            roomInvitationIsStarting
                                ? AppText.value(
                                    "Joining room…",
                                    "正在加入房间…",
                                    language: language
                                )
                                : AppText.value(
                                    "Room ready · Waiting for another device",
                                    "房间已就绪 · 正在等待另一台设备",
                                    language: language
                                ),
                            systemImage: roomInvitationIsStarting ? "link" : "timer"
                        )
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(Theme.accentStrong)

                        Spacer(minLength: 8)

                        Button(
                            AppText.value("Cancel", "取消", language: language),
                            role: .destructive,
                            action: onCancelRoomInvitation
                        )
                        .buttonStyle(.bordered)
                        .accessibilityIdentifier("room_hosting_cancel")
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .card(padding: 14)
                    .accessibilityIdentifier("room_hosting_status")
                }

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
        .sheet(isPresented: $isNameEditorPresented) {
            NavigationStack {
                VStack(alignment: .leading, spacing: 16) {
                    TextField(
                        AppText.value("Visible name", "显示名称", language: language),
                        text: $editedDisplayName
                    )
                    .textInputAutocapitalization(.words)
                    .textFieldStyle(.roundedBorder)
                    .accessibilityIdentifier("nearby_display_name_input")

                    Text(AppText.value(
                        "This name is visible to nearby Envoix users.",
                        "附近的 Envoix 用户会看到这个名称。",
                        language: language
                    ))
                    .font(.footnote)
                    .foregroundStyle(Theme.muted)

                    Spacer()
                }
                .padding(20)
                .background(Theme.bg)
                .navigationTitle(AppText.value("Device name", "设备名称", language: language))
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button(AppText.value("Cancel", "取消", language: language)) {
                            isNameEditorPresented = false
                        }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button(AppText.value("Save", "保存", language: language)) {
                            if onRename(editedDisplayName) {
                                isNameEditorPresented = false
                            }
                        }
                        .disabled(
                            NearbyDiscoveryPeerRegistry.sanitizeDisplayName(editedDisplayName) == nil
                        )
                        .accessibilityIdentifier("nearby_display_name_save")
                    }
                }
            }
            .presentationDetents([.medium])
        }
    }

    private var roomInvitationCard: some View {
        VStack(spacing: 14) {
            ZStack {
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .fill(Theme.surface)
                    .frame(width: 196, height: 196)

                if let image = roomInvitation.flatMap({ QRCode.image(from: $0.payload) }) {
                    Image(uiImage: image)
                        .interpolation(.none)
                        .resizable()
                        .scaledToFit()
                        .padding(14)
                        .frame(width: 196, height: 196)
                        .blur(radius: roomInvitationIsRevealed ? 0 : 13)
                } else {
                    Image(systemName: "qrcode")
                        .font(.system(size: 112, weight: .regular))
                        .foregroundStyle(Theme.text)
                        .frame(width: 196, height: 196)
                        .blur(radius: roomInvitationIsRevealed ? 0 : 10)
                }

                if !roomInvitationIsRevealed {
                    Button(action: onRevealRoomInvitation) {
                        VStack(spacing: 7) {
                            if roomInvitationIsStarting {
                                ProgressView()
                                    .tint(Theme.accentStrong)
                            } else {
                                Image(systemName: "eye")
                                    .font(.title2.weight(.semibold))
                            }
                            Text(AppText.value("Reveal room QR", "显示房间二维码", language: language))
                                .font(.callout.weight(.semibold))
                        }
                        .foregroundStyle(Theme.accentStrong)
                        .padding(.horizontal, 18)
                        .padding(.vertical, 13)
                        .background(.ultraThinMaterial, in: Capsule())
                    }
                    .buttonStyle(.plain)
                    .disabled(roomInvitationIsStarting)
                    .accessibilityIdentifier("room_qr_reveal")
                }
            }
            .frame(width: 196, height: 196)
            .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))

            HStack(spacing: 10) {
                Text(roomCodeText)
                    .font(.body.monospaced().weight(.semibold))
                    .foregroundStyle(Theme.text)
                    .lineLimit(1)
                    .blur(radius: roomInvitationIsRevealed ? 0 : 7)
                    .privacySensitive()
                    .accessibilityLabel(
                        roomInvitationIsRevealed
                            ? roomCodeText
                            : AppText.value("Hidden room code", "已隐藏房间码", language: language)
                    )
                    .accessibilityIdentifier("room_code")

                Spacer(minLength: 8)

                Button {
                    guard let roomInvitation else { return }
                    copyWithToast(
                        roomInvitation.code,
                        AppText.value("Room code copied", "房间码已复制", language: language),
                        language: language
                    )
                } label: {
                    Image(systemName: "doc.on.doc")
                }
                .disabled(!roomInvitationIsRevealed || roomInvitation == nil)
                .accessibilityLabel(AppText.value("Copy room code", "复制房间码", language: language))
                .accessibilityIdentifier("room_code_copy")

                Button(action: onRefreshRoomInvitation) {
                    Image(systemName: "arrow.clockwise")
                }
                .disabled(roomInvitationIsStarting)
                .accessibilityLabel(AppText.value("New room code", "刷新房间码", language: language))
                .accessibilityIdentifier("room_code_refresh")

                if roomInvitationIsRevealed {
                    Button(action: onHideRoomInvitation) {
                        Image(systemName: "eye.slash")
                    }
                    .accessibilityLabel(AppText.value("Hide room QR", "隐藏房间二维码", language: language))
                    .accessibilityIdentifier("room_qr_hide")
                }
            }
        }
        .frame(maxWidth: .infinity)
        .card(raised: true, padding: 16)
    }

    private var roomCodeText: String {
        roomInvitation?.code ?? "R000000-ROOM-CODE"
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
            .frame(maxWidth: .infinity, minHeight: 72)
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

    private var identityAndVisibility: some View {
        HStack(spacing: 12) {
            Button {
                editedDisplayName = presence.displayName
                isNameEditorPresented = true
            } label: {
                HStack(spacing: 9) {
                    Image(systemName: "person.crop.circle")
                        .font(.title3)
                        .foregroundStyle(Theme.accentStrong)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(AppText.value("Visible as", "显示为", language: language))
                            .font(.caption)
                            .foregroundStyle(Theme.muted)
                        Text(presence.displayName)
                            .font(.subheadline.weight(.semibold))
                            .foregroundStyle(Theme.text)
                            .lineLimit(1)
                    }
                }
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("nearby_display_name")

            Spacer(minLength: 8)

            Menu {
                visibilityButton(.hidden)
                visibilityButton(.everyoneTenMinutes)
                visibilityButton(.whileAppOpen)
            } label: {
                Label(visibilityTitle(presence.visibility), systemImage: visibilityIcon)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Theme.accentStrong)
                    .padding(.horizontal, 12)
                    .frame(minHeight: 38)
                    .background(Theme.accentSoft, in: Capsule())
            }
            .accessibilityIdentifier("nearby_visibility_menu")
        }
        .card(padding: 14)
    }

    private func visibilityButton(_ value: NearbyVisibilityMode) -> some View {
        Button {
            onSetVisibility(value)
        } label: {
            if presence.visibility == value {
                Label(visibilityTitle(value), systemImage: "checkmark")
            } else {
                Text(visibilityTitle(value))
            }
        }
    }

    private var visibilityIcon: String {
        switch presence.visibility {
        case .hidden: return "eye.slash"
        case .everyoneTenMinutes: return "timer"
        case .whileAppOpen: return "eye"
        }
    }

    private func visibilityTitle(_ value: NearbyVisibilityMode) -> String {
        switch value {
        case .hidden:
            return AppText.value("Hidden", "隐藏", language: language)
        case .everyoneTenMinutes:
            return AppText.value("Everyone · 10 min", "所有人 · 10 分钟", language: language)
        case .whileAppOpen:
            return AppText.value("While open", "打开时可见", language: language)
        }
    }

    private var nearbyHeader: some View {
        HStack {
            Text(AppText.value("Nearby", "附近设备", language: language))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.text)
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
            return AppText.value("Nearby is paused.", "附近发现已暂停。", language: language)
        }
        guard hasReadyProvider else {
            return AppText.value("Nearby is unavailable.", "附近发现不可用。", language: language)
        }
        return AppText.value("Looking for devices…", "正在搜索设备…", language: language)
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
                    Text(AppText.value("Nearby", "附近", language: language))
                        .font(.caption)
                        .foregroundStyle(Theme.muted)
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

    private func openSettings() {
        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
        UIApplication.shared.open(url)
    }
}
#endif
