import SwiftUI
import EnvoixCore

// Extracted from ContentView.swift (2026-07-20 split, no behavior change)

struct SettingsStageView: View {
    @EnvironmentObject private var model: AppModel
    @AppStorage("envoix.appearance") private var appearance: Appearance = .system
    @AppStorage("envoix.language") private var language = "en"
    @AppStorage("envoix.serverURL") private var serverURL = ""
    @AppStorage("envoix.relayURL") private var relayURL = ""
    @AppStorage("envoix.candidatesAllow") private var candidatesAllow = ""
    @AppStorage("envoix.candidatesDeny") private var candidatesDeny = ""
    @AppStorage("envoix.compressionPolicy") private var compressionPolicy = "smart"
    @AppStorage("envoix.developerMode") private var developerMode = false
    @AppStorage("envoix.verboseLog") private var verboseLog = false
    @AppStorage("envoix.logServer") private var logServer = defaultLogServer
    @State private var showAdvanced = false
    private let coreInfo = envoixCoreInfo()
    #if os(macOS)
    @ObservedObject private var agentService = AppleApplicationRuntime.shared.helperService
    #endif

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                appearanceSection

                #if os(macOS)
                agentServiceSection
                #endif

                VStack(alignment: .leading, spacing: 8) {
                    Text(AppText.localized("settings.language.title", language: language))
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.muted)
                    Picker(AppText.localized("settings.language.title", language: language), selection: $language) {
                        Text("English").tag("en")
                        Text("简体中文").tag("zh-Hans")
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                }
                .card(padding: 14)

                VStack(alignment: .leading, spacing: 8) {
                    Text(AppText.localized("settings.compression.title", language: language))
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.muted)
                    Picker(AppText.localized("settings.compression.title", language: language), selection: $compressionPolicy) {
                        Text(AppText.localized("settings.compression.never", language: language)).tag("never")
                        Text(AppText.localized("settings.compression.always", language: language)).tag("always")
                        Text(AppText.localized("settings.compression.smart", language: language)).tag("smart")
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    Text(AppText.localized("settings.compression.detail", language: language))
                        .font(.footnote)
                        .foregroundStyle(Theme.muted)
                }
                .card(padding: 14)

                transferCacheSection

                advancedHeader

                if showAdvanced {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(AppText.localized("settings.network.title", language: language))
                            .font(.title3.weight(.semibold))
                            .foregroundStyle(Theme.muted)
                        settingToggle(
                            AppText.localized("settings.network.avoid_tailscale", language: language),
                            subtitle: AppText.localized("settings.network.avoid_tailscale_detail", language: language),
                            isOn: avoidTailscaleBinding
                        )
                    }
                    .card(padding: 14)

                    settingField(
                        AppText.localized("settings.network.broker", language: language),
                        text: $serverURL,
                        placeholder: defaultRendezvousBroker,
                        helper: AppText.localized("settings.network.broker_detail", language: language),
                        isURL: true
                    )
                    settingField(
                        AppText.localized("settings.network.relay", language: language),
                        text: $relayURL,
                        placeholder: defaultRelayURL,
                        helper: AppText.localized("settings.network.relay_detail", language: language),
                        isURL: true
                    )

                    settingMultilineField(
                        AppText.localized("settings.network.candidate_allow", language: language),
                        text: $candidatesAllow,
                        helper: AppText.localized("settings.network.candidate_allow_detail", language: language)
                    )
                    settingMultilineField(
                        AppText.localized("settings.network.candidate_deny", language: language),
                        text: $candidatesDeny,
                        helper: AppText.localized("settings.network.candidate_deny_detail", language: language)
                    )
                    developerToolsSection
                    coreBuildInfo
                }
            }
            .padding(.vertical, 12)
            #if os(macOS)
            .frame(maxWidth: 960)
            .frame(maxWidth: .infinity)
            #endif
        }
        .onAppear {
            migrateLogServerIfNeeded()
            model.refreshTransferCache()
        }
    }

    private func migrateLogServerIfNeeded() {
        if deprecatedLogServers.contains(logServer.trimmed) {
            logServer = defaultLogServer
        }
    }

    private var coreBuildLabel: String {
        AppleBuildPresentation.label(
            infoDictionary: Bundle.main.infoDictionary,
            coreVersion: coreInfo.coreVersion,
            apiVersion: coreInfo.ffiApiVersion
        )
    }

    private var developerToolsSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(AppText.localized("settings.developer.title", language: language))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
            settingToggle(
                AppText.localized("settings.developer.enable", language: language),
                subtitle: AppText.localized("settings.developer.enable_detail", language: language),
                isOn: $developerMode
            )
            .accessibilityIdentifier("settings_developer_mode")
            if developerMode {
                Divider().overlay(Theme.line.opacity(0.5))
                settingToggle(
                    AppText.localized("settings.developer.verbose_logging", language: language),
                    subtitle: AppText.localized("settings.developer.verbose_logging_detail", language: language),
                    isOn: $verboseLog
                )
                #if DEBUG
                Divider().overlay(Theme.line.opacity(0.5))
                VStack(alignment: .leading, spacing: 8) {
                    let title = AppText.localized("settings.developer.log_server", language: language)
                    settingInput(
                        title: title,
                        text: $logServer,
                        placeholder: defaultLogServer,
                        isURL: true
                    )
                    Text(AppText.localized("settings.developer.log_server_detail", language: language))
                        .font(.footnote)
                        .foregroundStyle(Theme.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
                #endif
            }
        }
        .card(padding: 14)
    }

    private var coreBuildInfo: some View {
        Text(coreBuildLabel)
            .font(.caption.monospaced())
            .foregroundStyle(
                coreMatchesExpectedRoomControlContract(coreInfo)
                    ? Theme.muted
                    : Theme.danger
            )
            .frame(maxWidth: .infinity, alignment: .center)
            .padding(.top, 2)
            .accessibilityIdentifier("settings_core_version")
    }

    #if os(macOS)
    private var agentServiceSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(AppText.localized("settings.background.title", language: language))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
            settingToggle(
                AppText.localized("settings.background.enable", language: language),
                subtitle: AppText.localized("settings.background.enable_detail", language: language),
                isOn: Binding(
                    get: { agentService.isRequestedEnabled },
                    set: { enabled in
                        Task { await agentService.setEnabled(enabled) }
                    }
                )
            )
            Divider().overlay(Theme.line.opacity(0.5))
            HStack(spacing: 10) {
                if agentService.connectionState == .checking {
                    ProgressView().controlSize(.small)
                }
                Text(agentServiceStatusText)
                    .font(.footnote)
                    .foregroundStyle(agentServiceStatusColor)
                Spacer()
                Button(AppText.localized("settings.background.refresh", language: language)) {
                    Task { await agentService.refresh() }
                }
                .buttonStyle(.borderless)
            }
        }
        .card(padding: 14)
        .task {
            await agentService.refresh()
        }
        .accessibilityIdentifier("settings_background_service")
    }

    private var agentServiceStatusText: String {
        switch agentService.registrationState {
        case .unknown:
            return AppText.localized("settings.background.status.checking", language: language)
        case .notRegistered:
            return AppText.localized("settings.background.status.off", language: language)
        case .requiresApproval:
            return AppText.localized("settings.background.status.approval_required", language: language)
        case .helperNotFound:
            return AppText.localized("settings.background.status.helper_missing", language: language)
        case .failed:
            return AppText.localized("settings.background.status.registration_failed", language: language)
        case .enabled:
            switch agentService.connectionState {
            case .idle, .checking:
                return AppText.localized("settings.background.status.starting", language: language)
            case let .ready(pairedDevices):
                return AppText.localized(
                    "settings.background.status.ready",
                    defaultValue: "Ready · \(Int64(max(pairedDevices, 0))) paired devices",
                    language: language
                )
            case .unavailable:
                return AppText.localized("settings.background.status.unavailable", language: language)
            case .incompatible:
                return AppText.localized("settings.background.status.incompatible", language: language)
            }
        }
    }

    private var agentServiceStatusColor: Color {
        switch agentService.registrationState {
        case .failed, .helperNotFound:
            return Theme.danger
        case .requiresApproval:
            return Theme.warning
        case .enabled:
            switch agentService.connectionState {
            case .ready:
                return Theme.success
            case .unavailable, .incompatible:
                return Theme.danger
            case .idle, .checking:
                return Theme.muted
            }
        case .unknown, .notRegistered:
            return Theme.muted
        }
    }
    #endif

    private var transferCacheSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(AppText.localized("settings.cache.title", language: language))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
            Text(ByteCountFormatter.string(
                fromByteCount: Int64(clamping: model.transferCacheSummary.totalBytes),
                countStyle: .file
            ))
                .font(.title2.weight(.semibold))
                .foregroundStyle(Theme.text)
            Text(AppText.localized("settings.cache.detail", language: language))
                .font(.body)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
            if model.transferCacheSummary.protectedBytes > 0 {
                Text(AppText.localized(
                    "settings.cache.protected",
                    defaultValue: "Protected: \(cacheByteString(model.transferCacheSummary.protectedBytes))",
                    language: language
                ))
                    .font(.footnote)
                    .foregroundStyle(Theme.muted)
            }
            if let error = model.transferCacheError {
                Text(error)
                    .font(.footnote)
                    .foregroundStyle(Theme.danger)
            }
            Button {
                model.cleanTransferCache()
            } label: {
                HStack(spacing: 8) {
                    if model.isCleaningTransferCache {
                        ProgressView().controlSize(.small)
                    }
                    Text(AppText.localized("settings.cache.clean_up", language: language))
                }
                .frame(maxWidth: .infinity, minHeight: 40)
            }
            .buttonStyle(.borderedProminent)
            .tint(Theme.accentStrong)
            .disabled(model.isCleaningTransferCache)
            .accessibilityIdentifier("settings_clean_transfer_cache")
        }
        .card(padding: 14)
    }

    private func cacheByteString(_ bytes: UInt64) -> String {
        ByteCountFormatter.string(
            fromByteCount: Int64(clamping: bytes),
            countStyle: .file
        )
    }

    private var appearanceSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(AppText.localized("settings.appearance.title", language: language))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)

            Button {
                appearance = appearance.next
            } label: {
                HStack(spacing: 10) {
                    Image(systemName: appearance.icon)
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.accentStrong)
                        .frame(width: 24)
                    Text(appearanceTitle)
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.text)
                    Spacer()
                    Text(AppText.localized("settings.appearance.options", language: language))
                        .font(.body)
                        .foregroundStyle(Theme.muted)
                }
                .frame(minHeight: 42)
                .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
            }
            .buttonStyle(.plain)
        }
        .card(padding: 14)
    }

    private var appearanceTitle: String {
        switch appearance {
        case .system:
            return AppText.localized("settings.appearance.system", language: language)
        case .light:
            return AppText.localized("settings.appearance.light", language: language)
        case .dark:
            return AppText.localized("settings.appearance.dark", language: language)
        }
    }

    private var advancedHeader: some View {
        Button {
            withAnimation(.easeInOut(duration: 0.16)) {
                showAdvanced.toggle()
            }
        } label: {
            HStack {
                Text(AppText.localized("settings.advanced", language: language))
                    .font(.headline.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Spacer()
                Image(systemName: showAdvanced ? "chevron.up" : "chevron.down")
                    .foregroundStyle(Theme.muted)
            }
            .frame(maxWidth: .infinity, minHeight: 36, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(AppText.localized("settings.advanced", language: language))
        .accessibilityValue(
            showAdvanced
                ? AppText.localized("accessibility.expanded", language: language)
                : AppText.localized("accessibility.collapsed", language: language)
        )
        .accessibilityIdentifier("settings_advanced_toggle")
    }

    private var avoidTailscaleBinding: Binding<Bool> {
        Binding(
            get: {
                let deny = Set(configListLines(candidatesDeny))
                return Self.tailscaleCIDRs.allSatisfy { deny.contains($0) }
            },
            set: { enabled in
                var deny = configListLines(candidatesDeny)
                if enabled {
                    deny = Array(Set(deny).union(Self.tailscaleCIDRs)).sorted()
                } else {
                    deny.removeAll { Self.tailscaleCIDRs.contains($0) }
                }
                candidatesDeny = deny.joined(separator: "\n")
            }
        )
    }

    private func settingField(
        _ title: String,
        text: Binding<String>,
        placeholder: String = "",
        helper: String? = nil,
        isURL: Bool = false
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
            settingInput(title: title, text: text, placeholder: placeholder, isURL: isURL)
            if let helper {
                Text(helper)
                    .font(.body)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .card(padding: 14)
    }

    private func settingMultilineField(
        _ title: String,
        text: Binding<String>,
        helper: String
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
            TextEditor(text: text)
                .font(.body.monospaced())
                .foregroundStyle(Theme.text)
                .frame(minHeight: 88)
                .scrollContentBackground(.hidden)
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
                .background(Theme.surface)
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.cardRadius)
                        .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
                )
                .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
            Text(helper)
                .font(.body)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
        }
        .card(padding: 14)
    }

    @ViewBuilder
    private func settingInput(
        title: String,
        text: Binding<String>,
        placeholder: String,
        isURL: Bool
    ) -> some View {
        let prompt = placeholder.isEmpty ? title : placeholder
        #if os(iOS)
        TextField(prompt, text: text)
            .textFieldStyle(.plain)
            .font(.body.monospaced())
            .foregroundStyle(Theme.text)
            .lineLimit(1)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .keyboardType(isURL ? .URL : .default)
            .padding(.horizontal, 10)
            .frame(minHeight: 44)
            .background(Theme.surface)
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
            )
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        #else
        TextField(prompt, text: text)
            .textFieldStyle(.plain)
            .font(.body.monospaced())
            .foregroundStyle(Theme.text)
            .lineLimit(1)
            .padding(.horizontal, 10)
            .frame(minHeight: 44)
            .background(Theme.surface)
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
            )
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        #endif
    }

    private func settingToggle(_ title: String, isOn: Binding<Bool>) -> some View {
        settingToggle(title, subtitle: nil, isOn: isOn)
    }

    private func settingToggle(
        _ title: String,
        subtitle: String?,
        isOn: Binding<Bool>
    ) -> some View {
        Button {
            withAnimation(.easeInOut(duration: 0.15)) {
                isOn.wrappedValue.toggle()
            }
        } label: {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.title3)
                        .foregroundStyle(Theme.text)
                    if let subtitle {
                        Text(subtitle)
                            .font(.body)
                            .foregroundStyle(Theme.muted)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 12)
                SettingSwitchIndicator(isOn: isOn.wrappedValue)
            }
            .frame(maxWidth: .infinity, minHeight: 52, alignment: .leading)
            .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(title)
        .accessibilityValue(isOn.wrappedValue ? "On" : "Off")
    }

    private static let tailscaleCIDRs = ["100.64.0.0/10", "fd7a:115c:a1e0::/48"]
}

private struct SettingSwitchIndicator: View {
    let isOn: Bool

    var body: some View {
        RoundedRectangle(cornerRadius: 16)
            .fill(isOn ? Theme.accent : Theme.line)
            .frame(width: 48, height: 28)
            .overlay(alignment: isOn ? .trailing : .leading) {
                Circle()
                    .fill(Color.white)
                    .shadow(color: Color.black.opacity(0.12), radius: 2, y: 1)
                    .frame(width: 24, height: 24)
                    .padding(2)
            }
    }
}
