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

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                appearanceSection

                VStack(alignment: .leading, spacing: 8) {
                    Text(AppText.value("Language", "语言", language: language))
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.muted)
                    Picker("Language", selection: $language) {
                        Text("English").tag("en")
                        Text("简体中文").tag("zh-Hans")
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                }
                .card(padding: 14)

                VStack(alignment: .leading, spacing: 8) {
                    Text(AppText.value("Compression", "压缩", language: language))
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.muted)
                    Picker("Compression", selection: $compressionPolicy) {
                        Text(AppText.value("Never", "从不", language: language)).tag("never")
                        Text(AppText.value("Always", "始终", language: language)).tag("always")
                        Text(AppText.value("Smart", "智能", language: language)).tag("smart")
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    Text(AppText.value(
                        "Smart uses a conservative, case-insensitive final file-extension list. It does not read a sample or probe the network. “Never” keeps the original bytes; “Always” applies Zstandard. The selected policy is fixed when a new transfer job is created.",
                        "智能模式仅按大小写不敏感的最终文件后缀白名单判断，不读取样本，也不探测网络。从不模式发送原始字节；始终模式应用 Zstandard。新建传输任务时会固定当前策略。",
                        language: language
                    ))
                        .font(.footnote)
                        .foregroundStyle(Theme.muted)
                }
                .card(padding: 14)

                transferCacheSection

                advancedHeader

                if showAdvanced {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(AppText.value("Pairing and network", "配对与网络", language: language))
                            .font(.title3.weight(.semibold))
                            .foregroundStyle(Theme.muted)
                        settingToggle(
                            AppText.value("Avoid Tailscale addresses", "避开 Tailscale 地址", language: language),
                            subtitle: AppText.value("Prefer the real WAN or relay path instead of 100.x candidates.", "不广播 100.x 候选地址，优先使用真实网络或中继。", language: language),
                            isOn: avoidTailscaleBinding
                        )
                    }
                    .card(padding: 14)

                    settingField(
                        AppText.value("Rendezvous broker", "配对服务器", language: language),
                        text: $serverURL,
                        placeholder: defaultRendezvousBroker,
                        helper: AppText.value("Leave empty to use the built-in Envoix broker.", "留空则使用内置 Envoix 配对服务器。", language: language),
                        isURL: true
                    )
                    settingField(
                        AppText.value("Relay URL", "中继 URL", language: language),
                        text: $relayURL,
                        placeholder: defaultRelayURL,
                        helper: AppText.value("Leave empty to use the built-in relay for Room pairing.", "留空则使用内置中继服务。", language: language),
                        isURL: true
                    )

                    settingMultilineField(
                        AppText.value("Candidate allow", "候选地址 allow", language: language),
                        text: $candidatesAllow,
                        helper: AppText.value("One CIDR per line. Empty means allow all.", "每行一个 CIDR；留空表示全部允许。", language: language)
                    )
                    settingMultilineField(
                        AppText.value("Candidate deny", "候选地址 deny", language: language),
                        text: $candidatesDeny,
                        helper: AppText.value("One CIDR per line. Avoid Tailscale edits this list.", "每行一个 CIDR；避开 Tailscale 会修改此列表。", language: language)
                    )
                    developerToolsSection
                    coreBuildInfo
                }
            }
            .padding(.vertical, 12)
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
            Text(AppText.value("Developer tools", "开发者工具", language: language))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
            settingToggle(
                AppText.value("Enable developer mode", "开启开发者模式", language: language),
                subtitle: AppText.value(
                    "Reveal path selection, IDs, failure details, live logs and diagnostic reports.",
                    "显示链路选择、ID、失败详情、实时日志和诊断报告。",
                    language: language
                ),
                isOn: $developerMode
            )
            .accessibilityIdentifier("settings_developer_mode")
            if developerMode {
                Divider().overlay(Theme.line.opacity(0.5))
                settingToggle(
                    AppText.value("Verbose logging", "详细日志", language: language),
                    subtitle: AppText.value(
                        "Capture path selection and hole-punching internals. High volume.",
                        "记录链路选择和打洞内部信息；日志量较大。",
                        language: language
                    ),
                    isOn: $verboseLog
                )
                #if DEBUG
                Divider().overlay(Theme.line.opacity(0.5))
                VStack(alignment: .leading, spacing: 8) {
                    let title = AppText.value("Remote log server", "远程日志服务器", language: language)
                    settingInput(
                        title: title,
                        text: $logServer,
                        placeholder: defaultLogServer,
                        isURL: true
                    )
                    Text(AppText.value(
                        "Redacted reports only. HTTPS and a developer upload token are required.",
                        "只上传脱敏报告；必须使用 HTTPS 和开发者上传令牌。",
                        language: language
                    ))
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

    private var transferCacheSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(AppText.value("Transfer cache", "传输缓存", language: language))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
            Text(ByteCountFormatter.string(
                fromByteCount: Int64(clamping: model.transferCacheSummary.totalBytes),
                countStyle: .file
            ))
                .font(.title2.weight(.semibold))
                .foregroundStyle(Theme.text)
            Text(AppText.value(
                "Temporary Share and receive data. Active, paused, and resumable transfers are always protected.",
                "用于分享和接收的临时数据；活动中、已暂停和可续传的任务始终会被保护。",
                language: language
            ))
                .font(.body)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
            if model.transferCacheSummary.protectedBytes > 0 {
                Text(AppText.value(
                    "Protected: \(cacheByteString(model.transferCacheSummary.protectedBytes))",
                    "受保护：\(cacheByteString(model.transferCacheSummary.protectedBytes))",
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
                    Text(AppText.value("Clean Up", "清理缓存", language: language))
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
            Text(AppText.value("Appearance", "外观", language: language))
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
                    Text(AppText.value("System / Light / Dark", "跟随系统 / 浅色 / 深色", language: language))
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
            return AppText.value("System", "跟随系统", language: language)
        case .light:
            return AppText.value("Light", "浅色", language: language)
        case .dark:
            return AppText.value("Dark", "深色", language: language)
        }
    }

    private var advancedHeader: some View {
        Button {
            withAnimation(.easeInOut(duration: 0.16)) {
                showAdvanced.toggle()
            }
        } label: {
            HStack {
                Text(AppText.value("Advanced", "高级", language: language))
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
        .accessibilityLabel(AppText.value("Advanced", "高级", language: language))
        .accessibilityValue(
            showAdvanced
                ? AppText.value("Expanded", "已展开", language: language)
                : AppText.value("Collapsed", "已收起", language: language)
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
