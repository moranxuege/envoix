import Foundation

let envoixWifiAwareProbeService = "_envoix-probe._tcp"

enum WifiAwareProbeProtocolError: Error, Equatable {
    case invalidNonceLength
    case invalidFrameLength
    case invalidRequestMagic
    case invalidResponseMagic
    case nonceMismatch
}

struct WifiAwareProbeFrameAccumulator {
    private var buffer = Data()

    var bufferedByteCount: Int {
        buffer.count
    }

    mutating func append(_ bytes: Data) throws -> Data? {
        guard buffer.count + bytes.count <= WifiAwareProbeProtocol.frameLength else {
            throw WifiAwareProbeProtocolError.invalidFrameLength
        }
        buffer.append(bytes)
        return buffer.count == WifiAwareProbeProtocol.frameLength ? buffer : nil
    }

    func finish() throws -> Data {
        guard buffer.count == WifiAwareProbeProtocol.frameLength else {
            throw WifiAwareProbeProtocolError.invalidFrameLength
        }
        return buffer
    }
}

struct WifiAwareProbeAttemptGate {
    typealias Token = UInt64

    private(set) var currentToken: Token = 0
    private(set) var isActive = false

    mutating func begin() -> Token {
        currentToken &+= 1
        isActive = true
        return currentToken
    }

    mutating func cancel() {
        guard isActive else { return }
        isActive = false
        currentToken &+= 1
    }

    func accepts(_ token: Token) -> Bool {
        isActive && token == currentToken
    }

    mutating func complete(_ token: Token) -> Bool {
        guard accepts(token) else { return false }
        isActive = false
        return true
    }
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
enum AppleWifiAwareProbeError: Error, Equatable {
    case serviceNotDeclared
    case noSelectedDevice
    case timedOut
    case noEndpoint
    case noWifiAwarePath
}

@available(iOS 26.0, *)
struct WifiAwareProbeDeviceChoice: Identifiable, Equatable, Sendable {
    let id: WAPairedDevice.ID
    let displayName: String
}

@available(iOS 26.0, *)
@MainActor
final class AppleWifiAwareDiagnosticController: ObservableObject {
    @Published private(set) var snapshot = WifiAwareProbeSnapshot.idle
    @Published private(set) var pairedDevices: [WifiAwareProbeDeviceChoice] = []
    @Published private(set) var selectedDeviceID: WAPairedDevice.ID?

    private static let logger = Logger(subsystem: "com.envoix.app.ios", category: "wifi-aware-probe")
    private static let operationTimeout: Duration = .seconds(30)
    private static let evidenceRetryDelay: Duration = .milliseconds(100)
    private static let evidenceRetryCount = 10
    private static let maxDisplayNameLength = 128

    private enum Role {
        case publisher
        case subscriber
    }

    private enum PublisherRunSignal: Error {
        case probeCompleted
    }

    private var operation: Task<Void, Never>?
    private var attemptGate = WifiAwareProbeAttemptGate()
    private var pairedDeviceSnapshot: WAPairedDevice.Devices = [:]
    private var refreshGeneration: UInt64 = 0

    func refreshPairedDevices() {
        refreshGeneration &+= 1
        let activeGeneration = refreshGeneration
        Task { [weak self] in
            do {
                let devices = try await WAPairedDevice.allDevices.current()
                guard let self, self.refreshGeneration == activeGeneration else { return }
                self.applyPairedDevices(devices ?? [:])
            } catch {
                guard let self, self.refreshGeneration == activeGeneration else { return }
                self.recordFailure(error)
            }
        }
    }

    func selectDevice(id: WAPairedDevice.ID?) {
        selectedDeviceID = id.flatMap { pairedDeviceSnapshot[$0] == nil ? nil : $0 }
    }

    func pairingEndpointSelected() {
        update(phase: .pairing, detail: "endpoint_selected")
        refreshPairedDevices()
    }

    func startPublisherProbe() {
        startProbe(role: .publisher)
    }

    func startSubscriberProbe() {
        startProbe(role: .subscriber)
    }

    func stop() {
        attemptGate.cancel()
        let activeOperation = operation
        operation = nil
        activeOperation?.cancel()
        update(phase: .idle, detail: "stopped")
    }

    private func startProbe(role: Role) {
        stop()
        guard let selectedDeviceID,
              let device = pairedDeviceSnapshot[selectedDeviceID] else {
            recordFailure(AppleWifiAwareProbeError.noSelectedDevice)
            return
        }

        let token = attemptGate.begin()
        switch role {
        case .publisher:
            update(phase: .publishing, detail: "starting", generation: token)
        case .subscriber:
            update(phase: .browsing, detail: "starting", generation: token)
        }

        operation = Task { [weak self] in
            guard let self else { return }
            do {
                switch role {
                case .publisher:
                    try await self.runPublisherProbe(device: device, generation: token)
                case .subscriber:
                    try await self.runSubscriberProbe(device: device, generation: token)
                }
            } catch is CancellationError {
                self.finishAttempt(token)
            } catch {
                self.recordFailure(error, generation: token)
                self.finishAttempt(token)
            }
        }
    }

    private func runPublisherProbe(
        device: WAPairedDevice,
        generation: WifiAwareProbeAttemptGate.Token
    ) async throws {
        guard let service = WAPublishableService.allServices[envoixWifiAwareProbeService] else {
            throw AppleWifiAwareProbeError.serviceNotDeclared
        }

        let listener: NetworkListener<TCP> = try NetworkListener(
            for: .wifiAware(.connecting(to: service, from: .selected([device]))),
            using: envoixWifiAwareTCPParameters()
        )
        .newConnectionLimit(1)
        .onStateUpdate { _, state in
            Self.logListenerState(state)
        }

        update(phase: .publishing, detail: "waiting_for_connection", generation: generation)
        do {
            try await withProbeTimeout(Self.operationTimeout) { [weak self] in
                guard let controller = self else {
                    throw CancellationError()
                }
                try await listener.run { connection in
                    guard await controller.attemptGate.accepts(generation) else {
                        throw CancellationError()
                    }
                    try await controller.handleIncoming(connection, generation: generation)
                    throw PublisherRunSignal.probeCompleted
                }
            }
        } catch PublisherRunSignal.probeCompleted {
            finishAttempt(generation)
        }
    }

    private func runSubscriberProbe(
        device: WAPairedDevice,
        generation: WifiAwareProbeAttemptGate.Token
    ) async throws {
        guard let service = WASubscribableService.allServices[envoixWifiAwareProbeService] else {
            throw AppleWifiAwareProbeError.serviceNotDeclared
        }

        let browser = NetworkBrowser(
            for: WASubscriberBrowser.wifiAware(
                .connecting(to: .selected([device]), from: service)
            )
        )
        .onStateUpdate { _, state in
            Self.logBrowserState(state)
        }

        update(phase: .browsing, detail: "waiting_for_endpoint", generation: generation)
        let endpoint: WAEndpoint = try await withProbeTimeout(Self.operationTimeout) {
            try await browser.run { endpoints -> NetworkBrowser<WASubscriberBrowser>.RunResult<WAEndpoint> in
                guard let endpoint = endpoints.first(where: { $0.device.id == device.id }) else {
                    return .continue
                }
                return .finish(endpoint)
            }
        }

        let connection: NetworkConnection<TCP> = NetworkConnection(
            to: endpoint,
            using: envoixWifiAwareTCPParameters()
        )
        .onStateUpdate { _, state in
            Self.logConnectionState(state)
        }

        update(phase: .connecting, detail: "endpoint_found", generation: generation)
        try await withProbeTimeout(Self.operationTimeout) { [weak self] in
            guard let self else { throw CancellationError() }
            guard await self.attemptGate.accepts(generation) else {
                throw CancellationError()
            }
            try await self.exchangeProbe(on: connection, generation: generation)
        }
        finishAttempt(generation)
    }

    private func handleIncoming(
        _ connection: NetworkConnection<TCP>,
        generation: WifiAwareProbeAttemptGate.Token
    ) async throws {
        connection.onStateUpdate { _, state in
            Self.logConnectionState(state)
        }
        update(phase: .exchanging, detail: "receiving_probe", generation: generation)

        let request = try await receiveProbeFrame(on: connection)
        let response = try WifiAwareProbeProtocol.makeResponse(for: request)
        try await connection.send(response, endOfStream: true)
        let evidence = try await pathEvidence(for: connection)
        update(phase: .succeeded, detail: evidence, generation: generation)
    }

    private func exchangeProbe(
        on connection: NetworkConnection<TCP>,
        generation: WifiAwareProbeAttemptGate.Token
    ) async throws {
        update(phase: .exchanging, detail: "sending_probe", generation: generation)
        let nonce = Data((0 ..< WifiAwareProbeProtocol.nonceLength).map { _ in UInt8.random(in: .min ... .max) })
        let request = try WifiAwareProbeProtocol.makeRequest(nonce: nonce)
        try await connection.send(request)
        let response = try await receiveProbeFrame(on: connection)
        try WifiAwareProbeProtocol.validateResponse(response, nonce: nonce)
        let evidence = try await pathEvidence(for: connection)
        update(phase: .succeeded, detail: evidence, generation: generation)
    }

    private func receiveProbeFrame(on connection: NetworkConnection<TCP>) async throws -> Data {
        try await withProbeTimeout(Self.operationTimeout) {
            var accumulator = WifiAwareProbeFrameAccumulator()
            while accumulator.bufferedByteCount < WifiAwareProbeProtocol.frameLength {
                let remaining = WifiAwareProbeProtocol.frameLength - accumulator.bufferedByteCount
                let message = try await connection.receive(atMost: remaining)
                if let frame = try accumulator.append(message.content) {
                    return frame
                }
                if message.metadata.endOfStream {
                    return try accumulator.finish()
                }
            }
            return try accumulator.finish()
        }
    }

    private func pathEvidence(for connection: NetworkConnection<TCP>) async throws -> String {
        for _ in 0 ..< Self.evidenceRetryCount {
            try Task.checkCancellation()
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

    private func applyPairedDevices(_ devices: WAPairedDevice.Devices) {
        pairedDeviceSnapshot = devices
        pairedDevices = devices.values
            .map {
                WifiAwareProbeDeviceChoice(
                    id: $0.id,
                    displayName: Self.displayName(for: $0)
                )
            }
            .sorted {
                let comparison = $0.displayName.localizedCaseInsensitiveCompare($1.displayName)
                return comparison == .orderedSame ? $0.id < $1.id : comparison == .orderedAscending
            }

        if let selectedDeviceID, devices[selectedDeviceID] != nil {
            self.selectedDeviceID = selectedDeviceID
        } else {
            selectedDeviceID = pairedDevices.first?.id
        }
        updatePairedDeviceCount(devices.count)
    }

    private func finishAttempt(_ generation: WifiAwareProbeAttemptGate.Token) {
        if attemptGate.complete(generation) {
            operation = nil
        }
    }

    private func update(
        phase: WifiAwareProbePhase,
        detail: String,
        generation: WifiAwareProbeAttemptGate.Token? = nil
    ) {
        if let generation, !attemptGate.accepts(generation) {
            return
        }
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

    private func recordFailure(
        _ error: Error,
        generation: WifiAwareProbeAttemptGate.Token? = nil
    ) {
        let detail = Self.redactedFailureDetail(error)
        update(phase: .failed, detail: detail, generation: generation)
    }

    private static func displayName(for device: WAPairedDevice) -> String {
        for candidate in [device.name, device.pairingInfo?.pairingName] {
            let value = candidate?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            if !value.isEmpty,
               value.count <= maxDisplayNameLength,
               value.unicodeScalars.allSatisfy({ !CharacterSet.controlCharacters.contains($0) }) {
                return value
            }
        }
        return "Paired Apple device"
    }

    private static func redactedFailureDetail(_ error: Error) -> String {
        if let error = error as? AppleWifiAwareProbeError {
            switch error {
            case .serviceNotDeclared: return "service_not_declared"
            case .noSelectedDevice: return "no_selected_device"
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
extension WAError {
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
func withProbeTimeout<Value: Sendable>(
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

            targetPicker
            pairingControls

            HStack(spacing: 8) {
                Button(AppText.value("Receive probe", "接收探针", language: language)) {
                    controller.startPublisherProbe()
                }
                .buttonStyle(.borderedProminent)
                .disabled(controller.selectedDeviceID == nil)
                .accessibilityIdentifier("settings_wifi_aware_probe_receive")

                Button(AppText.value("Send probe", "发送探针", language: language)) {
                    controller.startSubscriberProbe()
                }
                .buttonStyle(.bordered)
                .disabled(controller.selectedDeviceID == nil)
                .accessibilityIdentifier("settings_wifi_aware_probe_send")

                Button(AppText.value("Stop", "停止", language: language)) {
                    controller.stop()
                }
                .buttonStyle(.bordered)
                .accessibilityIdentifier("settings_wifi_aware_probe_stop")
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
    private var targetPicker: some View {
        if controller.pairedDevices.isEmpty {
            Text(AppText.value(
                "Pair and select one device before starting a probe.",
                "开始探针前，请先配对并选择一台设备。",
                language: language
            ))
            .font(.footnote)
            .foregroundStyle(Theme.muted)
        } else {
            Picker(
                AppText.value("Probe target", "探针目标", language: language),
                selection: Binding(
                    get: { controller.selectedDeviceID },
                    set: { controller.selectDevice(id: $0) }
                )
            ) {
                ForEach(controller.pairedDevices) { device in
                    Text(device.displayName).tag(Optional(device.id))
                }
            }
            .pickerStyle(.menu)
            .accessibilityIdentifier("settings_wifi_aware_probe_target")
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
