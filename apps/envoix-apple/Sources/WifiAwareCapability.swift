import Foundation

#if os(iOS) && canImport(WiFiAware)
import WiFiAware
#endif

let envoixWifiAwareService = "_envoix._udp"

enum WifiAwareAvailability: String, CaseIterable, Equatable, Sendable {
    case unsupportedOS = "unsupported_os"
    case unsupportedHardware = "unsupported_hardware"
    case entitlementMissing = "entitlement_missing"
    case permissionRequired = "permission_required"
    case permissionDenied = "permission_denied"
    case wifiDisabled = "wifi_disabled"
    case temporarilyUnavailable = "temporarily_unavailable"
    case pairingRequired = "pairing_required"
    case ready
}

enum WifiAwarePermissionState: Equatable, Sendable {
    case granted
    case required
    case denied
}

struct WifiAwareCapabilityFacts: Equatable, Sendable {
    let osSupported: Bool
    let hardwareSupported: Bool
    let authenticatedRendezvousSupported: Bool
    let entitlementPresent: Bool
    let permissionState: WifiAwarePermissionState
    let wifiEnabled: Bool
    let serviceDeclared: Bool
    let temporarilyAvailable: Bool
    let pairingSupported: Bool?
    let pairedDeviceCount: Int?
}

struct WifiAwareCapabilitySnapshot: Equatable, Sendable {
    let availability: WifiAwareAvailability
    let pairingSupported: Bool?
    let pairedDeviceCount: Int?

    var diagnosticSummary: String {
        let pairing = pairingSupported.map { String($0) } ?? "unknown"
        let pairedDevices = pairedDeviceCount.map { String($0) } ?? "unknown"
        return "\(availability.rawValue) · pairing=\(pairing) · paired_devices=\(pairedDevices)"
    }
}

enum WifiAwareCapabilityPolicy {
    static func evaluate(_ facts: WifiAwareCapabilityFacts) -> WifiAwareCapabilitySnapshot {
        let availability: WifiAwareAvailability
        if !facts.osSupported {
            availability = .unsupportedOS
        } else if !facts.hardwareSupported || facts.pairingSupported == false {
            availability = .unsupportedHardware
        } else if !facts.entitlementPresent {
            availability = .entitlementMissing
        } else if facts.permissionState == .required {
            availability = .permissionRequired
        } else if facts.permissionState == .denied {
            availability = .permissionDenied
        } else if !facts.wifiEnabled {
            availability = .wifiDisabled
        } else if !facts.serviceDeclared || !facts.temporarilyAvailable || facts.pairingSupported == nil {
            availability = .temporarilyUnavailable
        } else if facts.pairedDeviceCount == nil {
            availability = .temporarilyUnavailable
        } else if facts.pairedDeviceCount == 0 {
            availability = .pairingRequired
        } else if !facts.authenticatedRendezvousSupported {
            availability = .temporarilyUnavailable
        } else {
            availability = .ready
        }

        return WifiAwareCapabilitySnapshot(
            availability: availability,
            pairingSupported: facts.pairingSupported,
            pairedDeviceCount: facts.pairedDeviceCount
        )
    }
}

enum WifiAwarePairingDeviceObservation: Equatable, Sendable {
    case loading
    case snapshot(
        baselineDeviceIDs: Set<UInt64>,
        currentDeviceIDs: Set<UInt64>
    )
    case failed
}

enum WifiAwarePairingCompletion: Equatable, Sendable {
    case pairedDevicesObserved(deviceIDs: Set<UInt64>, totalCount: Int)
    case pickerSelected(deviceID: UInt64, snapshotConfirmed: Bool)
}

enum WifiAwarePairingGuidance: Equatable, Sendable {
    case newPair
    case existingPairs(count: Int)
}

enum WifiAwarePairingPresentation: Equatable, Sendable {
    case loading
    case guidance(WifiAwarePairingGuidance)
    case success(WifiAwarePairingCompletion)
    case observationFailed
}

/// Projects only observable system facts into user-facing pairing state.
/// `DevicePairingView` has no completion callback, so publisher-side success
/// is inferred from a new paired-device ID. `DevicePicker` provides its device
/// ID directly and therefore remains authoritative while snapshots catch up.
enum WifiAwarePairingPresentationPolicy {
    static func evaluate(
        observation: WifiAwarePairingDeviceObservation,
        pickerSelectedDeviceID: UInt64?
    ) -> WifiAwarePairingPresentation {
        if let pickerSelectedDeviceID {
            let snapshotConfirmed: Bool
            if case .snapshot(_, let currentDeviceIDs) = observation {
                snapshotConfirmed = currentDeviceIDs.contains(pickerSelectedDeviceID)
            } else {
                snapshotConfirmed = false
            }
            return .success(.pickerSelected(
                deviceID: pickerSelectedDeviceID,
                snapshotConfirmed: snapshotConfirmed
            ))
        }

        switch observation {
        case .loading:
            return .loading

        case .snapshot(let baselineDeviceIDs, let currentDeviceIDs):
            let newDeviceIDs = currentDeviceIDs.subtracting(baselineDeviceIDs)
            if !newDeviceIDs.isEmpty {
                return .success(.pairedDevicesObserved(
                    deviceIDs: newDeviceIDs,
                    totalCount: currentDeviceIDs.count
                ))
            }
            if currentDeviceIDs.isEmpty {
                return .guidance(.newPair)
            }
            return .guidance(.existingPairs(count: currentDeviceIDs.count))

        case .failed:
            return .observationFailed
        }
    }
}

enum WifiAwareRendezvousRuntimePolicy {
    static var authenticatedControlPlaneSupported: Bool {
        #if os(iOS)
        if #available(iOS 26.4, *) {
            return true
        }
        #endif
        return false
    }
}

enum AppleWifiAwareCapabilityProbe {
    static func read() async -> WifiAwareCapabilitySnapshot {
        #if os(iOS) && canImport(WiFiAware)
        guard #available(iOS 26.0, *) else {
            return snapshot(osSupported: false)
        }

        let hardwareSupported = WACapabilities.supportedFeatures.contains(.wifiAware)
        let authenticatedRendezvousSupported =
            WifiAwareRendezvousRuntimePolicy.authenticatedControlPlaneSupported
        let serviceDeclared =
            WAPublishableService.allServices[envoixWifiAwareService] != nil &&
            WASubscribableService.allServices[envoixWifiAwareService] != nil

        guard hardwareSupported, serviceDeclared else {
            return snapshot(
                hardwareSupported: hardwareSupported,
                authenticatedRendezvousSupported: authenticatedRendezvousSupported,
                serviceDeclared: serviceDeclared
            )
        }

        do {
            guard let pairedDevices = try await WAPairedDevice.allDevices.current() else {
                return snapshot(
                    hardwareSupported: true,
                    authenticatedRendezvousSupported: authenticatedRendezvousSupported,
                    entitlementPresent: true,
                    serviceDeclared: true
                )
            }
            return snapshot(
                hardwareSupported: true,
                authenticatedRendezvousSupported: authenticatedRendezvousSupported,
                entitlementPresent: true,
                serviceDeclared: true,
                temporarilyAvailable: true,
                pairingSupported: true,
                pairedDeviceCount: pairedDevices.count
            )
        } catch let error as WAError {
            return snapshot(
                for: error,
                authenticatedRendezvousSupported: authenticatedRendezvousSupported,
                serviceDeclared: serviceDeclared
            )
        } catch {
            return snapshot(
                hardwareSupported: true,
                authenticatedRendezvousSupported: authenticatedRendezvousSupported,
                entitlementPresent: true,
                serviceDeclared: true
            )
        }
        #else
        return snapshot(osSupported: false)
        #endif
    }

    private static func snapshot(
        osSupported: Bool = true,
        hardwareSupported: Bool = true,
        authenticatedRendezvousSupported: Bool = true,
        entitlementPresent: Bool = true,
        serviceDeclared: Bool = true,
        temporarilyAvailable: Bool = false,
        pairingSupported: Bool? = nil,
        pairedDeviceCount: Int? = nil
    ) -> WifiAwareCapabilitySnapshot {
        WifiAwareCapabilityPolicy.evaluate(
            WifiAwareCapabilityFacts(
                osSupported: osSupported,
                hardwareSupported: hardwareSupported,
                authenticatedRendezvousSupported: authenticatedRendezvousSupported,
                entitlementPresent: entitlementPresent,
                permissionState: .granted,
                wifiEnabled: true,
                serviceDeclared: serviceDeclared,
                temporarilyAvailable: temporarilyAvailable,
                pairingSupported: pairingSupported,
                pairedDeviceCount: pairedDeviceCount
            )
        )
    }

    #if os(iOS) && canImport(WiFiAware)
    @available(iOS 26.0, *)
    private static func snapshot(
        for error: WAError,
        authenticatedRendezvousSupported: Bool,
        serviceDeclared: Bool
    ) -> WifiAwareCapabilitySnapshot {
        switch error {
        case .wifiAwareUnsupported(_):
            return snapshot(hardwareSupported: false, serviceDeclared: serviceDeclared)
        case .entitlementMissing(_):
            return snapshot(entitlementPresent: false, serviceDeclared: serviceDeclared)
        case .noPairedDevices(_):
            return snapshot(
                authenticatedRendezvousSupported: authenticatedRendezvousSupported,
                serviceDeclared: serviceDeclared,
                temporarilyAvailable: true,
                pairingSupported: true,
                pairedDeviceCount: 0
            )
        default:
            return snapshot(
                authenticatedRendezvousSupported: authenticatedRendezvousSupported,
                serviceDeclared: serviceDeclared,
                temporarilyAvailable: false,
                pairingSupported: true
            )
        }
    }
    #endif
}
