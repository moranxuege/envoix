import Foundation

enum WifiAwarePairingCopy: String, CaseIterable {
    case showDevice = "show_device"
    case findDevice = "find_device"
    case buildUnavailable = "build_unavailable"
    case checking
    case firstPairGuidance = "first_pair_guidance"
    case observationFailed = "observation_failed"
    case retry
    case device
    case unavailable
}

enum WifiAwarePairingPresentationText {
    static func value(_ copy: WifiAwarePairingCopy, language: String) -> String {
        AppText.localized("wifi_aware.pairing.\(copy.rawValue)", language: language)
    }

    static func existingPairs(_ count: Int, language: String) -> String {
        let displayCount = Int64(max(count, 0))
        return AppText.localized(
            "wifi_aware.pairing.existing_pairs",
            defaultValue: "\(displayCount) Apple-paired devices already available. Existing pairs do not show another six-digit code; tap Done to resume automatic discovery. Use the controls below only to add a new device.",
            language: language
        )
    }

    static func newPairing(totalCount: Int, language: String) -> String {
        let displayCount = Int64(max(totalCount, 0))
        return AppText.localized(
            "wifi_aware.pairing.new_pairing",
            defaultValue: "New pairing detected · \(displayCount) total",
            language: language
        )
    }

    static func pickerResult(
        displayName: String?,
        snapshotConfirmed: Bool,
        language: String
    ) -> String {
        let trimmedName = displayName?.trimmingCharacters(in: .whitespacesAndNewlines)
        let resolvedName = trimmedName.flatMap { $0.isEmpty ? nil : $0 }
            ?? value(.device, language: language)
        if snapshotConfirmed {
            return AppText.localized(
                "wifi_aware.pairing.picker_ready",
                defaultValue: "\(resolvedName) is paired and ready",
                language: language
            )
        }
        return AppText.localized(
            "wifi_aware.pairing.picker_waiting",
            defaultValue: "\(resolvedName) selected; waiting for Apple's pairing list",
            language: language
        )
    }
}

enum WifiAwareProbeCopy: String, CaseIterable {
    case title
    case receive
    case send
    case stop
    case selectFirst = "select_first"
    case target
    case waitingAccess = "waiting_access"
    case allowDevice = "allow_device"
    case addDevice = "add_device"
    case pickerUnavailable = "picker_unavailable"
    case serviceMissing = "service_missing"
}

enum WifiAwareDeveloperPresentationText {
    static func value(_ copy: WifiAwareProbeCopy, language: String) -> String {
        AppText.localized("wifi_aware.probe.\(copy.rawValue)", language: language)
    }
}
