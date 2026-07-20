#if os(iOS) && canImport(WiFiAware)
import Foundation
import OSLog
import WiFiAware

@available(iOS 26.0, *)
final class AppleWifiAwarePairingProvider: NearbyDiscoveryProvider {
    let source = NearbyDiscoverySource.wifiAware

    private static let retryDelay: Duration = .seconds(1)

    private let lock = NSLock()
    private let logger = Logger(
        subsystem: Bundle.main.bundleIdentifier ?? "com.envoix.app",
        category: "wifi-aware-pairing"
    )

    private var generation = 0
    private var sink: ((NearbyDiscoveryEvent) -> Void)?
    private var observationTask: Task<Void, Never>?
    private var lastPublishedDevices: [NearbyPairedDevice]?

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {
        stop()

        lock.lock()
        generation += 1
        let activeGeneration = generation
        self.sink = sink
        lastPublishedDevices = nil
        lock.unlock()

        emit(.status(NearbyProviderStatus(
            source: source,
            availability: .starting,
            detail: .startingWifiAware
        )), generation: activeGeneration)

        guard WACapabilities.supportedFeatures.contains(.wifiAware) else {
            emit(.status(NearbyProviderStatus(
                source: source,
                availability: .unsupported,
                detail: .wifiAwareUnsupported
            )), generation: activeGeneration)
            return
        }
        guard WAPublishableService.allServices[envoixWifiAwareService] != nil,
              WASubscribableService.allServices[envoixWifiAwareService] != nil else {
            emit(.status(NearbyProviderStatus(
                source: source,
                availability: .error,
                detail: .wifiAwareServiceMissing
            )), generation: activeGeneration)
            return
        }

        let task = Task { [weak self] in
            guard let self else { return }
            await self.observePairedDevices(generation: activeGeneration)
        }
        lock.lock()
        if generation == activeGeneration, self.sink != nil {
            observationTask = task
            lock.unlock()
        } else {
            lock.unlock()
            task.cancel()
        }
    }

    func stop() {
        lock.lock()
        generation += 1
        let task = observationTask
        observationTask = nil
        sink = nil
        lastPublishedDevices = nil
        lock.unlock()
        task?.cancel()
    }

    private func observePairedDevices(generation: Int) async {
        while !Task.isCancelled {
            do {
                let sequence = WAPairedDevice.allDevices
                do {
                    if let devices = try await sequence.current() {
                        publish(devices, generation: generation)
                    } else {
                        emitTemporarilyUnavailable(generation: generation)
                    }
                } catch let error as WAError where error.isNoPairedDevices {
                    publish([:], generation: generation)
                }

                for try await devices in sequence {
                    try Task.checkCancellation()
                    publish(devices, generation: generation)
                }
            } catch is CancellationError {
                return
            } catch let error as WAError {
                if !handle(error, generation: generation) {
                    return
                }
            } catch {
                emitTemporarilyUnavailable(generation: generation)
            }

            do {
                try await Task<Never, Never>.sleep(for: Self.retryDelay)
            } catch {
                return
            }
        }
    }

    private func publish(_ devices: WAPairedDevice.Devices, generation: Int) {
        let projected = devices.values.compactMap(Self.project).sorted { $0.id < $1.id }
        guard projected.count <= NearbyPairedDevice.maximumSnapshotCount else {
            emitStatus(.error, .wifiAwarePairedDeviceLimitExceeded, generation: generation)
            return
        }

        lock.lock()
        guard self.generation == generation, sink != nil else {
            lock.unlock()
            return
        }
        let changed = projected != lastPublishedDevices
        if changed {
            lastPublishedDevices = projected
        }
        lock.unlock()
        guard changed else { return }

        emit(.pairedDevices(source: source, devices: projected), generation: generation)
        emit(.status(NearbyProviderStatus(
            source: source,
            availability: projected.isEmpty ? .pairingRequired : .paired,
            detail: projected.isEmpty
                ? .wifiAwarePairingRequired
                : .wifiAwarePairedDevices(projected.count)
        )), generation: generation)
        logger.info("PAIRING provider=wifi_aware paired_device_count=\(projected.count, privacy: .public)")
    }

    private func handle(_ error: WAError, generation: Int) -> Bool {
        switch error {
        case .wifiAwareUnsupported:
            emitStatus(.unsupported, .wifiAwareUnsupported, generation: generation)
            return false
        case .entitlementMissing:
            emitStatus(.error, .wifiAwareEntitlementMissing, generation: generation)
            return false
        case .serviceNotDeclared:
            emitStatus(.error, .wifiAwareServiceMissing, generation: generation)
            return false
        case .noPairedDevices:
            publish([:], generation: generation)
            return true
        default:
            emitTemporarilyUnavailable(generation: generation)
            return true
        }
    }

    private func emitTemporarilyUnavailable(generation: Int) {
        emitStatus(
            .temporarilyUnavailable,
            .wifiAwareTemporarilyUnavailable,
            generation: generation
        )
    }

    private func emitStatus(
        _ availability: NearbyProviderAvailability,
        _ detail: NearbyProviderDetail,
        generation: Int
    ) {
        emit(.status(NearbyProviderStatus(
            source: source,
            availability: availability,
            detail: detail
        )), generation: generation)
    }

    private func emit(_ event: NearbyDiscoveryEvent, generation: Int) {
        lock.lock()
        let activeSink = self.generation == generation ? sink : nil
        lock.unlock()
        activeSink?(event)
    }

    private static func project(_ device: WAPairedDevice) -> NearbyPairedDevice? {
        let pairingInfo = device.pairingInfo
        let displayName = device.name ?? pairingInfo?.pairingName
        let model = [pairingInfo?.vendorName, pairingInfo?.modelName]
            .compactMap { NearbyDiscoveryPeerRegistry.sanitizeDeviceDetail($0) }
            .filter { !$0.isEmpty }
            .joined(separator: " · ")
        return NearbyPairedDevice(
            sourceScopedID: String(format: "%016llx", device.id),
            source: .wifiAware,
            displayName: displayName,
            model: model.isEmpty ? nil : model
        )
    }
}

@available(iOS 26.0, *)
private extension WAError {
    var isNoPairedDevices: Bool {
        if case .noPairedDevices = self {
            return true
        }
        return false
    }
}

#if canImport(DeviceDiscoveryUI)
import DeviceDiscoveryUI
import SwiftUI

@available(iOS 26.0, *)
struct AppleWifiAwarePairingControls: View {
    let language: String

    var body: some View {
        if let publishable = WAPublishableService.allServices[envoixWifiAwareService],
           let subscribable = WASubscribableService.allServices[envoixWifiAwareService] {
            VStack(spacing: 10) {
                DevicePairingView(
                    .wifiAware(.connecting(to: publishable, from: .userSpecifiedDevices))
                ) {
                    Label(
                        AppText.value("Allow nearby device", "允许附近设备", language: language),
                        systemImage: "dot.radiowaves.left.and.right"
                    )
                    .frame(maxWidth: .infinity)
                } fallback: {
                    pairingUnavailable
                }
                .buttonStyle(.bordered)
                .accessibilityIdentifier("nearby_wifi_aware_allow")

                DevicePicker(
                    .wifiAware(.connecting(to: .userSpecifiedDevices, from: subscribable))
                ) { _ in
                    // WAPairedDevice.allDevices is the source of truth. The
                    // provider observes the resulting snapshot continuously.
                } label: {
                    Label(
                        AppText.value("Add nearby device", "添加附近设备", language: language),
                        systemImage: "plus"
                    )
                    .frame(maxWidth: .infinity)
                } fallback: {
                    pairingUnavailable
                }
                .buttonStyle(PrimaryActionButtonStyle())
                .accessibilityIdentifier("nearby_wifi_aware_add")
            }
        } else {
            Text(AppText.value(
                "Wi-Fi Aware pairing is unavailable in this build.",
                "此版本无法使用 Wi-Fi Aware 配对。",
                language: language
            ))
            .font(.footnote)
            .foregroundStyle(Theme.danger)
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var pairingUnavailable: some View {
        Text(AppText.value("Pairing unavailable", "配对不可用", language: language))
            .frame(maxWidth: .infinity)
    }
}
#endif
#endif
