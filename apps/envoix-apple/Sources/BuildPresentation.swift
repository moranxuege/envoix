import Foundation

enum AppleBuildConfiguration {
    case debug
    case release

    static var current: Self {
        #if DEBUG
        .debug
        #else
        .release
        #endif
    }

    var label: String {
        switch self {
        case .debug:
            "Debug"
        case .release:
            "Release"
        }
    }
}

enum AppleBuildPresentation {
    static let timestampInfoKey = "EnvoixBuildTimestamp"
    private static let unavailable = "unavailable"

    static func label(
        infoDictionary: [String: Any]?,
        coreVersion: String,
        apiVersion: UInt32,
        configuration: AppleBuildConfiguration = .current
    ) -> String {
        let appVersion = metadataValue(
            for: "CFBundleShortVersionString",
            in: infoDictionary
        )
        let buildNumber = metadataValue(
            for: "CFBundleVersion",
            in: infoDictionary
        )
        let timestamp = metadataValue(
            for: timestampInfoKey,
            in: infoDictionary
        )

        return "\(configuration.label) · App \(appVersion) (\(buildNumber))"
            + " · Core \(coreVersion) · API \(apiVersion) · Built \(timestamp)"
    }

    private static func metadataValue(
        for key: String,
        in infoDictionary: [String: Any]?
    ) -> String {
        guard let value = infoDictionary?[key] as? String else {
            return unavailable
        }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, !trimmed.contains("$(") else {
            return unavailable
        }
        return trimmed
    }
}
