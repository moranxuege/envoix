#if os(macOS)
import EnvoixCore
import Foundation

enum MacOSAgentBoundaryError: Error, Equatable {
    case applicationSupportUnavailable
    case invalidControlEndpoint
    case incompatibleCore
    case incompatibleReadiness(agentProtocol: UInt16, applicationContract: UInt16)
}

enum MacOSAgentBoundary {
#if ENVOIX_SIGNED_DEBUG
    static let helperBundleIdentifier = "com.envoix.app.engine-helper.debug"
#else
    static let helperBundleIdentifier = "com.envoix.app.engine-helper"
#endif
    static let helperKeychainAccessGroup = "6638TTB2SF.com.envoix.engine.credentials"

    private static let applicationDirectoryName = "com.envoix.app"
    private static let agentDirectoryName = "agent-v1"
    private static let inboxDirectoryName = "inbox"
    private static let controlSocketName = "agent.sock"
    private static let fallbackDeviceName = "Mac"
    private static let maximumDeviceNameCharacters = 64

    static func stateDirectory(
        fileManager: FileManager = .default
    ) throws -> URL {
        guard let supportDirectory = fileManager.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else {
            throw MacOSAgentBoundaryError.applicationSupportUnavailable
        }
        return supportDirectory
            .appendingPathComponent(applicationDirectoryName, isDirectory: true)
            .appendingPathComponent(agentDirectoryName, isDirectory: true)
            .standardizedFileURL
    }

    static func controlEndpoint(
        fileManager: FileManager = .default
    ) throws -> URL {
        try stateDirectory(fileManager: fileManager)
            .appendingPathComponent(controlSocketName, isDirectory: false)
            .standardizedFileURL
    }

    static func hostConfiguration(
        fileManager: FileManager = .default,
        localizedDeviceName: String? = Host.current().localizedName
    ) throws -> FfiAgentHostConfiguration {
        let stateDirectory = try stateDirectory(fileManager: fileManager)
        let controlEndpoint = try controlEndpoint(fileManager: fileManager)
        guard stateDirectory.isFileURL,
              controlEndpoint.isFileURL,
              stateDirectory.path.hasPrefix("/"),
              controlEndpoint.path.hasPrefix("/") else {
            throw MacOSAgentBoundaryError.invalidControlEndpoint
        }
        return FfiAgentHostConfiguration(
            stateDirectory: stateDirectory.path,
            inboxDirectory: stateDirectory
                .appendingPathComponent(inboxDirectoryName, isDirectory: true)
                .path,
            controlEndpoint: controlEndpoint.path,
            deviceName: deviceName(from: localizedDeviceName),
            broker: defaultRendezvousBroker,
            relay: defaultRelayURL,
            credentialProtection: .appleKeychain
        )
    }

    static func validateReadiness(
        _ readiness: FfiAgentHostReady,
        configuration: FfiAgentHostConfiguration
    ) throws {
        guard readiness.controlEndpoint == configuration.controlEndpoint else {
            throw MacOSAgentBoundaryError.invalidControlEndpoint
        }
        guard readiness.agentProtocolVersion == expectedAgentProtocolVersion,
              readiness.applicationContractVersion == expectedApplicationContractVersion else {
            throw MacOSAgentBoundaryError.incompatibleReadiness(
                agentProtocol: readiness.agentProtocolVersion,
                applicationContract: readiness.applicationContractVersion
            )
        }
    }

    static func deviceName(from localizedName: String?) -> String {
        let filtered = (localizedName ?? "").unicodeScalars.filter {
            !CharacterSet.controlCharacters.contains($0)
        }
        let trimmed = String(String.UnicodeScalarView(filtered))
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let bounded = String(trimmed.prefix(maximumDeviceNameCharacters))
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return bounded.isEmpty ? fallbackDeviceName : bounded
    }
}
#endif
