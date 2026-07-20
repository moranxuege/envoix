import Foundation

let envoixWifiAwareProbeService = "_envoix-probe._tcp"

enum WifiAwareProbeProtocolError: Error, Equatable {
    case invalidNonceLength
    case invalidFrameLength
    case invalidRequestMagic
    case invalidResponseMagic
    case nonceMismatch
}

enum WifiAwareProbeProtocol {
    static let nonceLength = 32
    static let frameLength = 40

    private static let requestMagic = Data("ENVXWA01".utf8)
    private static let responseMagic = Data("ENVXWA02".utf8)

    static func makeRequest(nonce: Data) throws -> Data {
        guard nonce.count == nonceLength else {
            throw WifiAwareProbeProtocolError.invalidNonceLength
        }
        return requestMagic + nonce
    }

    static func makeResponse(for request: Data) throws -> Data {
        guard request.count == frameLength else {
            throw WifiAwareProbeProtocolError.invalidFrameLength
        }
        guard request.prefix(requestMagic.count) == requestMagic else {
            throw WifiAwareProbeProtocolError.invalidRequestMagic
        }
        return responseMagic + request.dropFirst(requestMagic.count)
    }

    static func validateResponse(_ response: Data, nonce: Data) throws {
        guard nonce.count == nonceLength else {
            throw WifiAwareProbeProtocolError.invalidNonceLength
        }
        guard response.count == frameLength else {
            throw WifiAwareProbeProtocolError.invalidFrameLength
        }
        guard response.prefix(responseMagic.count) == responseMagic else {
            throw WifiAwareProbeProtocolError.invalidResponseMagic
        }
        guard response.dropFirst(responseMagic.count) == nonce else {
            throw WifiAwareProbeProtocolError.nonceMismatch
        }
    }
}

#if os(iOS) && canImport(DeviceDiscoveryUI) && canImport(WiFiAware)
import DeviceDiscoveryUI
import Network
import OSLog
import SwiftUI
import WiFiAware

@available(iOS 26.0, *)
enum WifiAwareProbePhase: String, Sendable {
    case idle
    case pairing
    case publishing
    case browsing
    case connecting
    case exchanging
    case succeeded
    case failed
}

@available(iOS 26.0, *)
struct WifiAwareProbeSnapshot: Equatable, Sendable {
    let phase: WifiAwareProbePhase
    let detail: String
    let pairedDeviceCount: Int?

    static let idle = WifiAwareProbeSnapshot(
        phase: .idle,
        detail: "not_started",
        pairedDeviceCount: nil
    )

    var diagnosticSummary: String {
        let paired = pairedDeviceCount.map(String.init) ?? "unknown"
        return "phase=\(phase.rawValue) · detail=\(detail) · paired_devices=\(paired)"
    }
}

@available(iOS 26.0, *)
private enum AppleWifiAwareProbeError: Error {
    case serviceNotDeclared
    case timedOut
    case noEndpoint
    case noWifiAwarePath
}

@available(iOS 26.0, *)
@MainActor
final class AppleWifiAwareDiagnosticController: ObservableObject {
    @Published private(set) var snapshot = WifiAwareProbeSnapshot.idle

    private static let logger = Logger(subsystem: "com.envoix.app.ios", category: "wifi-aware-probe")
    private static let operationTimeout: Duration = .seconds(30)
    private static let evidenceRetryDelay: Duration = .milliseconds(100)
    private static let evidenceRetryCount = 10

    private var operation: Task<Void, Never>?

    func refreshPairedDevices() {
        Task { [weak self] in
            do {
                let devices = try await WAPairedDevice.allDevices.current()
                self?.updatePairedDeviceCount(devices?.count)
            } catch {
                self?.recordFailure(error)
            }
        }
    }

    func pairingEndpointSelected() {
        update(phase: .pairing, detail: "endpoint_selected")
        refreshPairedDevices()
    }

    func startPublisherProbe() {
        stop()
        update(phase: .publishing, detail: "starting")
        operation = Task { [weak self] in
            guard let self else { return }
            do {
                try await self.runPublisherProbe()
            } catch is CancellationError {
                return
            } catch {
                self.recordFailure(error)
            }
        }
    }

    func startSubscriberProbe() {
        stop()
        update(phase: .browsing, detail: "starting")
        operation = Task { [weak self] in
            guard let self else { return }
            do {
                try await self.runSubscriberProbe()
            } catch is CancellationError {
                return
            } catch {
                self.recordFailure(error)
            }
        }
    }

    func stop() {
        operation?.cancel()
        operation = nil
        update(phase: .idle, detail: "stopped")
    }

    private func runPublisherProbe() async throws {
        guard let service = WAPublishableService.allServices[envoixWifiAwareProbeService] else {
            throw AppleWifiAwareProbeError.serviceNotDeclared
        }

        let listener: NetworkListener<TCP> = try NetworkListener(
            for: .wifiAware(.connecting(to: service, from: .allPairedDevices)),
            using: .parameters {
                TCP().noDelay(true)
            }
            .wifiAware { $0.performanceMode = .bulk }
            .serviceClass(.background)
        )
        .newConnectionLimit(1)
        .onStateUpdate { _, state in
            Self.logListenerState(state)
        }

        update(phase: .publishing, detail: "waiting_for_connection")
        try await listener.run { [weak self] connection in
            guard let self else { return }
            try await self.handleIncoming(connection)
        }
    }

    private func runSubscriberProbe() async throws {
        guard let service = WASubscribableService.allServices[envoixWifiAwareProbeService] else {
            throw AppleWifiAwareProbeError.serviceNotDeclared
        }

        let browser = NetworkBrowser(
            for: WASubscriberBrowser.wifiAware(
                .connecting(to: .allPairedDevices, from: service)
            )
        )
        .onStateUpdate { _, state in
            Self.logBrowserState(state)
        }

        update(phase: .browsing, detail: "waiting_for_endpoint")
        let endpoint: WAEndpoint = try await withProbeTimeout(Self.operationTimeout) {
            try await browser.run { endpoints -> NetworkBrowser<WASubscriberBrowser>.RunResult<WAEndpoint> in
                guard let endpoint = endpoints.first else {
                    return .continue
                }
                return .finish(endpoint)
            }
        }

        let connection: NetworkConnection<TCP> = NetworkConnection(
            to: endpoint,
            using: .parameters {
                TCP().noDelay(true)
            }
            .wifiAware { $0.performanceMode = .bulk }
            .serviceClass(.background)
        )
        .onStateUpdate { _, state in
            Self.logConnectionState(state)
        }

        update(phase: .connecting, detail: "endpoint_found")
        try await withProbeTimeout(Self.operationTimeout) { [weak self] in
            guard let self else { throw CancellationError() }
            try await self.exchangeProbe(on: connection)
        }
    }

    private func handleIncoming(_ connection: NetworkConnection<TCP>) async throws {
        connection.onStateUpdate { _, state in
            Self.logConnectionState(state)
        }
        update(phase: .exchanging, detail: "receiving_probe")

        let message = try await withProbeTimeout(Self.operationTimeout) {
            try await connection.receive(exactly: WifiAwareProbeProtocol.frameLength)
        }
        let response = try WifiAwareProbeProtocol.makeResponse(for: message.content)
        try await connection.send(response, endOfStream: true)
        let evidence = try await pathEvidence(for: connection)
        update(phase: .succeeded, detail: evidence)
    }

    private func exchangeProbe(on connection: NetworkConnection<TCP>) async throws {
        update(phase: .exchanging, detail: "sending_probe")
        let nonce = Data((0 ..< WifiAwareProbeProtocol.nonceLength).map { _ in UInt8.random(in: .min ... .max) })
        let request = try WifiAwareProbeProtocol.makeRequest(nonce: nonce)
        try await connection.send(request)
        let response = try await connection.receive(exactly: WifiAwareProbeProtocol.frameLength)
        try WifiAwareProbeProtocol.validateResponse(response.content, nonce: nonce)
        let evidence = try await pathEvidence(for: connection)
        update(phase: .succeeded, detail: evidence)
    }

    private func pathEvidence(for connection: NetworkConnection<TCP>) async throws -> String {
        for _ in 0 ..< Self.evidenceRetryCount {
            if let path = connection.currentPath,
               let awarePath = try await path.wifiAware {
                let signal = awarePath.performance.signalStrength
                    .map { String(format: "%.2f", $0) } ?? "unknown"
                return "path=wifi_aware · bytes=\(WifiAwareProbeProtocol.frameLength) · signal=\(signal)"
            }
            try await Task<Never, Never>.sleep(for: Self.evidenceRetryDelay)
        }
        throw AppleWifiAwareProbeError.noWifiAwarePath
    }

    private func update(phase: WifiAwareProbePhase, detail: String) {
        snapshot = WifiAwareProbeSnapshot(
            phase: phase,
            detail: detail,
            pairedDeviceCount: snapshot.pairedDeviceCount
        )
        Self.logger.info(
            "phase=\(phase.rawValue, privacy: .public) detail=\(detail, privacy: .public)"
        )
    }

    private func updatePairedDeviceCount(_ count: Int?) {
        snapshot = WifiAwareProbeSnapshot(
            phase: snapshot.phase,
            detail: snapshot.detail,
            pairedDeviceCount: count
        )
        Self.logger.info("paired_device_count=\(count ?? -1, privacy: .public)")
    }

    private func recordFailure(_ error: Error) {
        let detail = Self.redactedFailureDetail(error)
        update(phase: .failed, detail: detail)
    }

    private static func redactedFailureDetail(_ error: Error) -> String {
        if let error = error as? AppleWifiAwareProbeError {
            switch error {
            case .serviceNotDeclared: return "service_not_declared"
            case .timedOut: return "timeout"
            case .noEndpoint: return "no_endpoint"
            case .noWifiAwarePath: return "wrong_or_missing_path"
            }
        }
        if let error = error as? NWError, let awareError = error.wifiAware {
            return awareError.wireName
        }
        if error is WifiAwareProbeProtocolError {
            return "probe_protocol_error"
        }
        return "unexpected_\(String(describing: type(of: error)))"
    }

    private static func logListenerState(_ state: NetworkListener<TCP>.State) {
        logger.debug("listener_state=\(listenerStateName(state), privacy: .public)")
    }

    private static func logBrowserState(_ state: NetworkBrowser<WASubscriberBrowser>.State) {
        logger.debug("browser_state=\(browserStateName(state), privacy: .public)")
    }

    private static func logConnectionState(_ state: NetworkChannel<TCP>.State) {
        logger.debug("connection_state=\(connectionStateName(state), privacy: .public)")
    }

    private static func listenerStateName(_ state: NetworkListener<TCP>.State) -> String {
        switch state {
        case .setup: return "setup"
        case .waiting: return "waiting"
        case .ready: return "ready"
        case .failed(let error): return "failed:\(error.wifiAware?.wireName ?? "network")"
        case .cancelled: return "cancelled"
        @unknown default: return "unknown"
        }
    }

    private static func browserStateName(_ state: NetworkBrowser<WASubscriberBrowser>.State) -> String {
        switch state {
        case .setup: return "setup"
        case .ready: return "ready"
        case .failed(let error): return "failed:\(error.wifiAware?.wireName ?? "network")"
        case .cancelled: return "cancelled"
        case .waiting(let error): return "waiting:\(error.wifiAware?.wireName ?? "network")"
        @unknown default: return "unknown"
        }
    }

    private static func connectionStateName(_ state: NetworkChannel<TCP>.State) -> String {
        switch state {
        case .setup: return "setup"
        case .waiting(let error): return "waiting:\(error.wifiAware?.wireName ?? "network")"
        case .preparing: return "preparing"
        case .ready: return "ready"
        case .failed(let error): return "failed:\(error.wifiAware?.wireName ?? "network")"
        case .cancelled: return "cancelled"
        @unknown default: return "unknown"
        }
    }
}

@available(iOS 26.0, *)
private extension WAError {
    var wireName: String {
        switch self {
        case .error: return "error"
        case .wifiAwareUnsupported: return "unsupported_hardware"
        case .entitlementMissing: return "entitlement_missing"
        case .noRadioResources: return "no_radio_resources"
        case .serviceNotDeclared: return "service_not_declared"
        case .serviceAlreadySubscribing: return "already_subscribing"
        case .serviceAlreadyPublishing: return "already_publishing"
        case .noPairedDevices: return "pairing_required"
        case .deviceInvalid: return "device_invalid"
        case .deviceNoLongerAvailable: return "device_unavailable"
        case .publisherTimeout: return "publisher_timeout"
        case .subscriberTimeout: return "subscriber_timeout"
        case .connectionFailed: return "connection_failed"
        case .connectionIdleTimeout: return "connection_idle_timeout"
        case .connectionTerminated: return "connection_terminated"
        @unknown default: return "unknown"
        }
    }
}

@available(iOS 26.0, *)
private func withProbeTimeout<Value: Sendable>(
    _ timeout: Duration,
    operation: @escaping @Sendable () async throws -> Value
) async throws -> Value {
    try await withThrowingTaskGroup(of: Value.self) { group in
        group.addTask(operation: operation)
        group.addTask {
            try await Task<Never, Never>.sleep(for: timeout)
            throw AppleWifiAwareProbeError.timedOut
        }
        guard let value = try await group.next() else {
            throw AppleWifiAwareProbeError.noEndpoint
        }
        group.cancelAll()
        return value
    }
}

@available(iOS 26.0, *)
struct AppleWifiAwareDeveloperPanel: View {
    let language: String

    @StateObject private var controller = AppleWifiAwareDiagnosticController()

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(AppText.value("Wi-Fi Aware connection probe", "Wi-Fi Aware 连接探针", language: language))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)

            Text(controller.snapshot.diagnosticSummary)
                .font(.caption.monospaced())
                .foregroundStyle(controller.snapshot.phase == .failed ? Theme.danger : Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityIdentifier("settings_wifi_aware_probe")

            pairingControls

            HStack(spacing: 8) {
                Button(AppText.value("Receive probe", "接收探针", language: language)) {
                    controller.startPublisherProbe()
                }
                .buttonStyle(.borderedProminent)

                Button(AppText.value("Send probe", "发送探针", language: language)) {
                    controller.startSubscriberProbe()
                }
                .buttonStyle(.bordered)

                Button(AppText.value("Stop", "停止", language: language)) {
                    controller.stop()
                }
                .buttonStyle(.bordered)
            }
            .controlSize(.small)
        }
        .task {
            controller.refreshPairedDevices()
        }
        .onDisappear {
            controller.stop()
        }
    }

    @ViewBuilder
    private var pairingControls: some View {
        if let publishable = WAPublishableService.allServices[envoixWifiAwareProbeService],
           let subscribable = WASubscribableService.allServices[envoixWifiAwareProbeService] {
            HStack(spacing: 8) {
                DevicePairingView(
                    .wifiAware(.connecting(to: publishable, from: .userSpecifiedDevices))
                ) {
                    Label(
                        AppText.value("Allow device", "允许设备", language: language),
                        systemImage: "plus"
                    )
                } fallback: {
                    Text(AppText.value("Pairing unavailable", "配对不可用", language: language))
                }
                .buttonStyle(.bordered)

                DevicePicker(
                    .wifiAware(.connecting(to: .userSpecifiedDevices, from: subscribable))
                ) { _ in
                    controller.pairingEndpointSelected()
                } label: {
                    Label(
                        AppText.value("Add device", "添加设备", language: language),
                        systemImage: "plus"
                    )
                } fallback: {
                    Text(AppText.value("Picker unavailable", "选择器不可用", language: language))
                }
                .buttonStyle(.bordered)
            }
            .controlSize(.small)
        } else {
            Text(AppText.value(
                "The TCP probe service is missing from Info.plist.",
                "Info.plist 中缺少 TCP 探针服务。",
                language: language
            ))
            .font(.footnote)
            .foregroundStyle(Theme.danger)
        }
    }
}
#endif
