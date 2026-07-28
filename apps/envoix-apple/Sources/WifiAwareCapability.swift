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

enum AppleWifiAwareCapabilityProbe {
    static func read() async -> WifiAwareCapabilitySnapshot {
        #if os(iOS) && canImport(WiFiAware)
        guard #available(iOS 26.0, *) else {
            return snapshot(osSupported: false)
        }

        let hardwareSupported = WACapabilities.supportedFeatures.contains(.wifiAware)
        let serviceDeclared =
            WAPublishableService.allServices[envoixWifiAwareService] != nil &&
            WASubscribableService.allServices[envoixWifiAwareService] != nil

        guard hardwareSupported, serviceDeclared else {
            return snapshot(
                hardwareSupported: hardwareSupported,
                serviceDeclared: serviceDeclared
            )
        }

        do {
            guard let pairedDevices = try await WAPairedDevice.allDevices.current() else {
                return snapshot(
                    hardwareSupported: true,
                    entitlementPresent: true,
                    serviceDeclared: true
                )
            }
            return snapshot(
                hardwareSupported: true,
                entitlementPresent: true,
                serviceDeclared: true,
                temporarilyAvailable: true,
                pairingSupported: true,
                pairedDeviceCount: pairedDevices.count
            )
        } catch let error as WAError {
            return snapshot(for: error, serviceDeclared: serviceDeclared)
        } catch {
            return snapshot(
                hardwareSupported: true,
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
        serviceDeclared: Bool
    ) -> WifiAwareCapabilitySnapshot {
        switch error {
        case .wifiAwareUnsupported(_):
            return snapshot(hardwareSupported: false, serviceDeclared: serviceDeclared)
        case .entitlementMissing(_):
            return snapshot(entitlementPresent: false, serviceDeclared: serviceDeclared)
        case .noPairedDevices(_):
            return snapshot(
                serviceDeclared: serviceDeclared,
                temporarilyAvailable: true,
                pairingSupported: true,
                pairedDeviceCount: 0
            )
        default:
            return snapshot(
                serviceDeclared: serviceDeclared,
                temporarilyAvailable: false,
                pairingSupported: true
            )
        }
    }
    #endif
}
