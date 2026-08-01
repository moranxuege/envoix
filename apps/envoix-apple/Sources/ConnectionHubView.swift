#if os(iOS) || os(macOS)
import SwiftUI
#if os(iOS)
import UIKit
#elseif os(macOS)
import AppKit
#endif

struct ConnectionHubView: View {
    private enum RoomInvitationLayout {
        static let collapsedQRCodeSide: CGFloat = 148
        static let connectionMethodsMinimumWidth: CGFloat = 136

        #if os(macOS)
        static let revealedQRCodeImageSide: CGFloat = 208
        #else
        static let revealedQRCodeImageSide: CGFloat = 184
        #endif
    }

    @Environment(\.appLanguage) private var language
    @ObservedObject var coordinator: NearbyDiscoveryCoordinator
    @ObservedObject var presence: NearbyPresencePreferences

    let openInFixtureURL: URL?
    let roomInvitation: RoomControlInvitation?
    let roomInvitationIsRevealed: Bool
    let roomInvitationIsStarting: Bool
    let rememberedRooms: [RememberedPeerSummary]
    let rememberedRoomStatus: (String) -> RememberedRoomConnectionStatus
    let incomingRememberedRelationshipID: String?
    let onScanQRCode: () -> Void
    let onEnterCode: () -> Void
    let nfcIsAvailable: Bool
    let nfcIsActive: Bool
    let onScanNFC: () -> Void
    let onRevealRoomInvitation: () -> Void
    let onHideRoomInvitation: () -> Void
    let onRefreshRoomInvitation: () -> Void
    let onCancelRoomInvitation: () -> Void
    let onSetVisibility: (NearbyVisibilityMode) -> Void
    let onRename: (String) -> Bool
    let onSelectRememberedRoom: (String) -> Void
    let onPrepareNearbyPairing: () async -> Bool
    let onFinishNearbyPairing: () -> Void
    let onModalPresentationChanged: (Bool) -> Void
    let onSelectPeer: (NearbyPairingSelection) -> Void

    @State private var isNameEditorPresented = false
    @State private var isNearbyPairingPresented = false
    @State private var isPreparingNearbyPairing = false
    @State private var editedDisplayName = ""

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 16) {
                roomInvitationCard
                identityAndVisibility
                rememberedRoomsSection
                nearbyHeader

                if presence.visibility != .hidden {
                    if coordinator.state.statuses[.bluetooth]?.availability == .permissionRequired {
                        Button(action: openSettings) {
                            Label(
                                AppText.value(
                                    "Open Bluetooth settings",
                                    "打开蓝牙设置",
                                    language: language
                                ),
                                systemImage: "gearshape"
                            )
                            .frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.bordered)
                        .tint(Theme.accentStrong)
                        .accessibilityIdentifier("nearby_open_settings")
                    }

                    VStack(alignment: .leading, spacing: 12) {
                        if coordinator.state.peers.isEmpty {
                            nearbyEmptyState
                        } else {
                            ForEach(coordinator.state.peers) { peer in
                                let selection = NearbyPairingSelection(peer: peer)
                                let invitationAvailable = coordinator.canOfferRoomInvite(
                                    to: selection
                                )
                                Button {
                                    onSelectPeer(selection)
                                } label: {
                                    peerCard(
                                        peer,
                                        invitationAvailable: invitationAvailable
                                    )
                                }
                                .buttonStyle(.plain)
                                .disabled(!invitationAvailable)
                                .accessibilityHint(AppText.value(
                                    invitationAvailable
                                        ? "Open an unverified one-time room"
                                        : "Waiting for a secure invitation path",
                                    invitationAvailable
                                        ? "打开未经验证的一次性房间"
                                        : "正在等待安全邀请路径",
                                    language: language
                                ))
                                .accessibilityIdentifier("nearby_peer_card")
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .accessibilityIdentifier("nearby_peer_list")
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
                    #if os(iOS)
                    .textInputAutocapitalization(.words)
                    #endif
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
                #if os(iOS)
                .navigationBarTitleDisplayMode(.inline)
                #endif
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
            #if os(iOS)
            .presentationDetents([.medium])
            #endif
        }
        .sheet(
            isPresented: $isNearbyPairingPresented,
            onDismiss: finishNearbyPairing
        ) {
            nearbyPairingSheet
        }
        .onChange(of: isNameEditorPresented) { _ in
            reportModalPresentation()
        }
        .onChange(of: isNearbyPairingPresented) { _ in
            reportModalPresentation()
        }
        .onDisappear {
            onModalPresentationChanged(false)
        }
    }

    @ViewBuilder
    private var rememberedRoomsSection: some View {
        if !rememberedRooms.isEmpty {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Text(AppText.value("Rooms", "房间", language: language))
                        .font(.headline.weight(.semibold))
                    Spacer()
                    Text(AppText.value(
                        "\(rememberedRooms.count) saved",
                        "已保存 \(rememberedRooms.count) 个",
                        language: language
                    ))
                    .font(.caption)
                    .foregroundStyle(Theme.muted)
                }

                ForEach(rememberedRooms) { room in
                    let hasIncomingOffer =
                        incomingRememberedRelationshipID == room.relationshipID
                    Button {
                        onSelectRememberedRoom(room.relationshipID)
                    } label: {
                        HStack(spacing: 12) {
                            Image(systemName: rememberedRoomIcon(
                                rememberedRoomStatus(room.relationshipID)
                            ))
                            .font(.title3)
                            .foregroundStyle(rememberedRoomTint(
                                rememberedRoomStatus(room.relationshipID)
                            ))

                            VStack(alignment: .leading, spacing: 3) {
                                Text(room.label)
                                    .font(.subheadline.weight(.semibold))
                                    .foregroundStyle(Theme.text)
                                    .lineLimit(1)
                                Text(hasIncomingOffer
                                     ? AppText.value(
                                         "Incoming files",
                                         "收到文件邀请",
                                         language: language
                                     )
                                     : rememberedRoomStatusText(
                                         rememberedRoomStatus(room.relationshipID)
                                     ))
                                .font(.caption)
                                .foregroundStyle(
                                    hasIncomingOffer ? Theme.accentStrong : Theme.muted
                                )
                                .lineLimit(1)
                            }
                            Spacer()
                            if hasIncomingOffer {
                                Text(AppText.value(
                                    "Open",
                                    "查看",
                                    language: language
                                ))
                                .font(.caption2.weight(.bold))
                                .foregroundStyle(Theme.accentStrong)
                                .padding(.horizontal, 8)
                                .padding(.vertical, 4)
                                .background(
                                    Theme.accentSoft,
                                    in: Capsule()
                                )
                                .accessibilityHidden(true)
                            }
                            Image(systemName: "chevron.right")
                                .font(.caption.weight(.semibold))
                                .foregroundStyle(Theme.muted)
                        }
                        .frame(maxWidth: .infinity, minHeight: 48)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityValue(hasIncomingOffer
                        ? AppText.value(
                            "Incoming files waiting for your decision",
                            "有文件邀请等待处理",
                            language: language
                        )
                        : rememberedRoomStatusText(
                            rememberedRoomStatus(room.relationshipID)
                        ))
                    .accessibilityIdentifier("remembered_room_\(room.relationshipID)")
                }
            }
            .card(padding: 14)
            .accessibilityIdentifier("remembered_rooms")
        }
    }

    private func rememberedRoomStatusText(
        _ status: RememberedRoomConnectionStatus
    ) -> String {
        switch status {
        case .offline:
            return AppText.value("Available when both apps are open", "双方打开应用时可连接", language: language)
        case .connecting:
            return AppText.value("Connecting…", "正在连接…", language: language)
        case .waiting:
            return AppText.value("Available to the other device…", "正在等待另一台设备…", language: language)
        case .connected:
            return AppText.value("Connected", "已连接", language: language)
        case .needsRepair:
            return AppText.value("Pair again to reconnect", "请重新配对后连接", language: language)
        }
    }

    private func rememberedRoomIcon(
        _ status: RememberedRoomConnectionStatus
    ) -> String {
        switch status {
        case .offline: return "bubble.left.and.bubble.right"
        case .connecting: return "arrow.triangle.2.circlepath"
        case .waiting: return "antenna.radiowaves.left.and.right"
        case .connected: return "checkmark.circle.fill"
        case .needsRepair: return "exclamationmark.triangle.fill"
        }
    }

    private func rememberedRoomTint(
        _ status: RememberedRoomConnectionStatus
    ) -> Color {
        switch status {
        case .connected: return Theme.success
        case .connecting, .waiting: return Theme.accentStrong
        case .needsRepair: return Theme.danger
        case .offline: return Theme.muted
        }
    }

    private var roomInvitationCard: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(AppText.value("Room", "房间", language: language))
                        .font(.headline.weight(.semibold))
                        .foregroundStyle(Theme.text)
                    Text(roomStatusText)
                        .font(.caption)
                        .foregroundStyle(roomInvitation == nil ? Theme.muted : Theme.accentStrong)
                }

                Spacer(minLength: 8)

                if roomInvitation != nil || roomInvitationIsStarting {
                    Button(role: .destructive, action: onCancelRoomInvitation) {
                        Image(systemName: "xmark.circle.fill")
                            .font(.title3)
                            .frame(width: 36, height: 36)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(AppText.value("Close room", "关闭房间", language: language))
                    .accessibilityIdentifier("room_end")
                }
            }

            ViewThatFits(in: .horizontal) {
                HStack(alignment: .center, spacing: 12) {
                    roomQRCodeToggle
                    roomConnectionMethods
                        .frame(
                            minWidth: RoomInvitationLayout.connectionMethodsMinimumWidth,
                            maxWidth: .infinity
                        )
                }

                VStack(spacing: 12) {
                    roomQRCodeToggle
                    roomConnectionMethods
                        .frame(maxWidth: .infinity)
                }
            }

            if let roomInvitation {
                HStack(spacing: 10) {
                    Group {
                        if roomInvitationIsRevealed {
                            Text(roomInvitation.code)
                                .privacySensitive()
                                .accessibilityLabel(roomInvitation.code)
                        } else {
                            Label(
                                AppText.value(
                                    "Room code hidden",
                                    "房间码已隐藏",
                                    language: language
                                ),
                                systemImage: "eye.slash"
                            )
                            .accessibilityLabel(AppText.value(
                                "Hidden room code",
                                "已隐藏房间码",
                                language: language
                            ))
                        }
                    }
                    .font(.subheadline.monospaced().weight(.semibold))
                    .foregroundStyle(Theme.text)
                    .lineLimit(1)
                    .minimumScaleFactor(0.72)
                    .accessibilityIdentifier("room_code")

                    Spacer(minLength: 6)

                    Button {
                        copyWithToast(
                            roomInvitation.code,
                            AppText.value(
                                "Room code copied",
                                "房间码已复制",
                                language: language
                            ),
                            language: language
                        )
                    } label: {
                        Image(systemName: "doc.on.doc")
                            .frame(width: 32, height: 32)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(!roomInvitationIsRevealed)
                    .accessibilityLabel(AppText.value(
                        "Copy room code",
                        "复制房间码",
                        language: language
                    ))
                    .accessibilityIdentifier("room_code_copy")

                    Button(action: onRefreshRoomInvitation) {
                        Image(systemName: "arrow.triangle.2.circlepath")
                            .frame(width: 32, height: 32)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(roomInvitationIsStarting)
                    .accessibilityLabel(AppText.value(
                        "Renew room invitation",
                        "更新房间邀请",
                        language: language
                    ))
                    .accessibilityIdentifier("room_invitation_renew")
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .card(raised: true, padding: 16)
        .accessibilityIdentifier("room_connection_card")
    }

    private var roomQRCodeToggle: some View {
        Button {
            if roomInvitationIsRevealed {
                onHideRoomInvitation()
            } else {
                onRevealRoomInvitation()
            }
        } label: {
            if roomInvitationIsRevealed,
               let image = roomInvitation.flatMap({ QRCode.image(from: $0.payload) }) {
                QRCard(
                    image: image,
                    size: RoomInvitationLayout.revealedQRCodeImageSide
                )
            } else {
                ZStack {
                    RoundedRectangle(cornerRadius: 16, style: .continuous)
                        .fill(Theme.surface)

                    VStack(spacing: 8) {
                        if roomInvitationIsStarting {
                            ProgressView()
                                .tint(Theme.accentStrong)
                        } else {
                            Image(systemName: roomInvitation == nil ? "qrcode" : "eye")
                                .font(.title2.weight(.semibold))
                        }
                        Text(roomInvitationIsStarting
                            ? AppText.value(
                                "Creating invitation…",
                                "正在创建邀请…",
                                language: language
                            )
                            : roomInvitation == nil
                                ? AppText.value("Create room", "创建房间", language: language)
                                : AppText.value("Reveal QR", "显示二维码", language: language))
                            .font(.callout.weight(.semibold))
                            .multilineTextAlignment(.center)
                    }
                    .foregroundStyle(Theme.accentStrong)
                    .padding(12)
                }
                .frame(
                    width: RoomInvitationLayout.collapsedQRCodeSide,
                    height: RoomInvitationLayout.collapsedQRCodeSide
                )
            }
        }
        .buttonStyle(.plain)
        .disabled(roomInvitationIsStarting)
        .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .accessibilityLabel(
            roomInvitationIsRevealed
                ? AppText.value("Hide room QR", "隐藏房间二维码", language: language)
                : roomInvitationIsStarting
                    ? AppText.value(
                        "Creating invitation",
                        "正在创建邀请",
                        language: language
                    )
                    : roomInvitation == nil
                        ? AppText.value("Create room", "创建房间", language: language)
                        : AppText.value("Reveal room QR", "显示房间二维码", language: language)
        )
        .accessibilityHint(
            roomInvitationIsRevealed
                ? AppText.value(
                    "Hides the invitation without ending the room.",
                    "隐藏邀请，但不会结束房间。",
                    language: language
                )
                : roomInvitationIsStarting
                    ? AppText.value(
                        "Please wait while the invitation is created.",
                        "请等待邀请创建完成。",
                        language: language
                    )
                    : AppText.value(
                        "Shows an invitation another device can scan.",
                        "显示可供另一台设备扫描的邀请。",
                        language: language
                    )
        )
        .accessibilityValue(
            roomInvitationIsRevealed
                ? AppText.value("Revealed", "已显示", language: language)
                : roomInvitationIsStarting
                    ? AppText.value("Creating", "正在创建", language: language)
                    : roomInvitation == nil
                        ? AppText.value("No active room", "没有活动房间", language: language)
                        : AppText.value("Hidden", "已隐藏", language: language)
        )
        .accessibilityIdentifier("room_qr_toggle")
    }

    private var roomConnectionMethods: some View {
        VStack(spacing: 10) {
            #if os(iOS)
            methodButton(
                AppText.value("Scan QR", "扫描二维码", language: language),
                systemImage: "qrcode.viewfinder",
                identifier: "connect_scan_qr",
                action: onScanQRCode
            )
            #endif
            methodButton(
                AppText.value("Enter code", "输入房间码", language: language),
                systemImage: "keyboard",
                identifier: "connect_enter_code",
                action: onEnterCode
            )
        }
    }

    private var roomStatusText: String {
        if roomInvitationIsStarting {
            return AppText.value("Creating invitation…", "正在创建邀请…", language: language)
        }
        if roomInvitation != nil {
            return AppText.value(
                "Ready · Waiting for another device",
                "已就绪 · 正在等待另一台设备",
                language: language
            )
        }
        return AppText.value("No active room", "没有活动房间", language: language)
    }

    private func methodButton(
        _ title: String,
        systemImage: String,
        identifier: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: 9) {
                Image(systemName: systemImage)
                    .font(.body.weight(.semibold))
                Text(title)
                    .font(.subheadline.weight(.semibold))
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
            }
            .foregroundStyle(Theme.accentStrong)
            .padding(.horizontal, 12)
            .frame(maxWidth: .infinity, minHeight: 56, alignment: .leading)
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
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Text(AppText.value("Nearby", "附近设备", language: language))
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Spacer()

                #if os(iOS)
                if wifiAwarePairingIsAvailable {
                    nearbyHeaderButton(
                        AppText.value("Aware", "感知", language: language),
                        systemImage: "wifi",
                        isBusy: isPreparingNearbyPairing,
                        identifier: "nearby_wifi_aware",
                        action: prepareNearbyPairing
                    )
                }

                if nfcIsAvailable {
                    nearbyHeaderButton(
                        "NFC",
                        systemImage: "wave.3.right",
                        isBusy: nfcIsActive,
                        identifier: "connect_scan_nfc",
                        action: onScanNFC
                    )
                }
                #endif

                nearbyHeaderButton(
                    presence.visibility == .hidden
                        ? AppText.value("Open", "开启", language: language)
                        : AppText.value("Close", "关闭", language: language),
                    systemImage: presence.visibility == .hidden ? "eye" : "eye.slash",
                    isBusy: false,
                    identifier: "nearby_discovery_toggle"
                ) {
                    onSetVisibility(
                        presence.visibility == .hidden ? .whileAppOpen : .hidden
                    )
                }
            }

            #if os(macOS)
            Text(AppText.value(
                "Discovery uses Bluetooth and the local network. Wi‑Fi Aware and NFC phone scanning are not available on macOS.",
                "通过蓝牙和局域网发现设备；macOS 暂不支持 Wi‑Fi Aware 和手机 NFC 扫描。",
                language: language
            ))
                .font(.caption)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityIdentifier("nearby_transport_note")
            #endif
        }
        .padding(.top, 4)
    }

    private func nearbyHeaderButton(
        _ title: String,
        systemImage: String,
        isBusy: Bool,
        identifier: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: 5) {
                if isBusy {
                    ProgressView()
                        .controlSize(.small)
                } else {
                    Image(systemName: systemImage)
                }
                Text(title)
                    .lineLimit(1)
            }
            .font(.caption.weight(.semibold))
            .foregroundStyle(Theme.accentStrong)
            .padding(.horizontal, 9)
            .frame(minHeight: 34)
            .background(Theme.accentSoft, in: Capsule())
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .disabled(isBusy)
        .accessibilityIdentifier(identifier)
    }

    private var wifiAwarePairingIsAvailable: Bool {
        #if os(iOS) && canImport(DeviceDiscoveryUI) && canImport(WiFiAware)
        if #available(iOS 26.0, *) {
            return coordinator.state.statuses[.wifiAware]?.availability != .unsupported
        }
        #endif
        return false
    }

    @ViewBuilder
    private var nearbyPairingSheet: some View {
        #if os(iOS) && canImport(DeviceDiscoveryUI) && canImport(WiFiAware)
        if #available(iOS 26.0, *) {
            VStack(alignment: .leading, spacing: 18) {
                Text(AppText.value("Wi‑Fi Aware", "Wi‑Fi Aware", language: language))
                    .font(.title2.weight(.bold))
                    .foregroundStyle(Theme.text)
                Text(AppText.value(
                    "Pair once using Apple's system controls. Paired Envoix devices are then discovered automatically when both apps are open.",
                    "使用 Apple 系统控件完成一次配对。之后双方打开 Envoix 时，已配对设备会被自动发现。",
                    language: language
                ))
                .font(.body)
                .foregroundStyle(Theme.muted)
                AppleWifiAwarePairingControls(language: language)
                Button {
                    isNearbyPairingPresented = false
                } label: {
                    Text(AppText.value("Done", "完成", language: language))
                        .frame(maxWidth: .infinity, minHeight: 44)
                }
                .buttonStyle(.bordered)
                .accessibilityIdentifier("nearby_pairing_done")
                Spacer(minLength: 0)
            }
            .padding(20)
            .background(Theme.bg)
            .presentationDetents([.medium])
            .accessibilityIdentifier("nearby_pairing_sheet")
        }
        #endif
    }

    private func prepareNearbyPairing() {
        guard !isPreparingNearbyPairing else { return }
        isPreparingNearbyPairing = true
        Task { @MainActor in
            guard await onPrepareNearbyPairing() else {
                isPreparingNearbyPairing = false
                return
            }
            isPreparingNearbyPairing = false
            isNearbyPairingPresented = true
        }
    }

    private func finishNearbyPairing() {
        onFinishNearbyPairing()
    }

    private func reportModalPresentation() {
        onModalPresentationChanged(
            isNameEditorPresented || isNearbyPairingPresented
        )
    }

    private var nearbyEmptyState: some View {
        VStack(alignment: .leading, spacing: 12) {
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

            if !coordinator.state.isActive || !hasReadyProvider {
                Button(action: coordinator.restart) {
                    Label(
                        AppText.value("Try again", "重试", language: language),
                        systemImage: "arrow.clockwise"
                    )
                }
                .buttonStyle(.bordered)
                .tint(Theme.accentStrong)
                .accessibilityIdentifier("nearby_try_again")
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
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

    private func peerCard(
        _ peer: NearbyDiscoveredPeer,
        invitationAvailable: Bool
    ) -> some View {
        HStack(spacing: 13) {
            Image(systemName: "antenna.radiowaves.left.and.right")
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.accentStrong)
                .frame(width: 44, height: 44)
                .background(Theme.accentSoft, in: RoundedRectangle(cornerRadius: 13))

            VStack(alignment: .leading, spacing: 5) {
                Text(nearbyPeerDisplayName(
                    peer,
                    among: coordinator.state.peers,
                    fallback: AppText.value(
                        "Nearby Envoix device",
                        "附近的 Envoix 设备",
                        language: language
                    )
                ))
                .font(.headline)
                .foregroundStyle(Theme.text)
                .lineLimit(1)

                Text(discoverySourcesText(peer.sources))
                    .font(.caption)
                    .foregroundStyle(Theme.muted)
                    .lineLimit(2)

                Text(invitationAvailable
                    ? AppText.value("Unverified", "未经验证", language: language)
                    : AppText.value(
                        "Invitation path not ready",
                        "邀请路径尚未就绪",
                        language: language
                    ))
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(invitationAvailable ? Theme.warning : Theme.muted)
            }

            Spacer(minLength: 8)
            if invitationAvailable {
                Image(systemName: "chevron.right")
                    .font(.caption.weight(.bold))
                    .foregroundStyle(Theme.muted)
            } else {
                Image(systemName: "hourglass")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Theme.muted)
            }
        }
        .card(raised: true, padding: 14)
        .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
    }

    private func discoverySourcesText(_ sources: Set<NearbyDiscoverySource>) -> String {
        let labels = NearbyDiscoverySource.allCases.compactMap { source -> String? in
            guard sources.contains(source) else { return nil }
            switch source {
            case .bluetooth:
                return AppText.value("Bluetooth", "蓝牙", language: language)
            case .mdns:
                return AppText.value("Local network", "局域网", language: language)
            case .wifiAware:
                return "Wi‑Fi Aware"
            }
        }
        guard !labels.isEmpty else {
            return AppText.value(
                "Discovery path unavailable",
                "发现路径不可用",
                language: language
            )
        }
        let paths = labels.joined(separator: " · ")
        return AppText.value(
            "Discovered via \(paths)",
            "发现路径：\(paths)",
            language: language
        )
    }

    private func openSettings() {
        #if os(iOS)
        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
        UIApplication.shared.open(url)
        #elseif os(macOS)
        guard let url = URL(string: "x-apple.systempreferences:com.apple.BluetoothSettings") else {
            return
        }
        NSWorkspace.shared.open(url)
        #endif
    }
}
#endif
