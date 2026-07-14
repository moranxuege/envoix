import CryptoKit
import EnvoixCore
import XCTest
@testable import Envoix

@MainActor
final class EnvoixMacOSHostedTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    func testReceiveIosToMacOSAppRoom() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        let outputDirectory = outputDirectory()
        let model = AppModel.shared
        let existingActivityIDs = Set(model.activities.map(\.activityId))
        let defaults = UserDefaults.standard
        let previousUseRoom = defaults.object(forKey: Self.useRoomDefaultsKey)
        let previousUseMdns = defaults.object(forKey: Self.useMdnsDefaultsKey)
        defaults.set(true, forKey: Self.useRoomDefaultsKey)
        defaults.set(true, forKey: Self.useMdnsDefaultsKey)
        defer {
            Self.restoreDefault(previousUseRoom, key: Self.useRoomDefaultsKey, defaults: defaults)
            Self.restoreDefault(previousUseMdns, key: Self.useMdnsDefaultsKey, defaults: defaults)
        }

        try FileManager.default.createDirectory(
            at: outputDirectory,
            withIntermediateDirectories: true
        )
        let finalURL = outputDirectory.appendingPathComponent(Self.expectedFileName)
        try? FileManager.default.removeItem(at: finalURL)

        model.receive.startReceivingWithRoom(
            outputDir: outputDirectory.path,
            code: Self.roomCode,
            settings: Self.runtimeSettings
        )

        let activityID = try await waitForNewReceiveActivity(
            in: model,
            excluding: existingActivityIDs
        )
        emitEvidence("receiver-ready activity=\(activityID) room=\(Self.roomCode)")
        let record = try await waitForCompletion(activityID: activityID, in: model)

        XCTAssertEqual(record.fileName, Self.expectedFileName)
        XCTAssertEqual(record.bytesTransferred, Self.expectedBytes)
        XCTAssertEqual(record.totalBytes, Self.expectedBytes)
        XCTAssertNotEqual(record.dataPathKind, .none)
        XCTAssertEqual(URL(fileURLWithPath: record.completedFilePath), finalURL)
        XCTAssertTrue(FileManager.default.fileExists(atPath: finalURL.path))
        XCTAssertEqual(try Self.fileSize(finalURL), Self.expectedBytes)

        let actualHash = try Self.fileSHA256(finalURL)
        let expectedHash = Self.repeatedPayloadSHA256(
            Self.payload,
            expectedBytes: Self.expectedBytes
        )
        XCTAssertEqual(actualHash, expectedHash)
        emitEvidence(
            "completed activity=\(activityID) pathKind=\(record.dataPathKind) " +
            "pathDetail=\(record.dataPathDetail) file=\(finalURL.path) " +
            "size=\(Self.expectedBytes) sha256=\(actualHash.hexString)"
        )
#endif
    }

    private func requireCrossDeviceTesting() throws {
#if !ENVOIX_CROSS_DEVICE_TESTING
        throw XCTSkip("Requires the explicit ENVOIX_CROSS_DEVICE_TESTING build and a paired iPhone")
#endif
    }

#if ENVOIX_CROSS_DEVICE_TESTING
    private static let roomCode = environment("ENVOIX_IOS_TO_MACOS_CODE") ?? "741205-silver-forest"
    private static let runID = environment("ENVOIX_CROSS_DEVICE_RUN_ID") ?? "manual"
    private static let expectedFileName = "envoix-\(runID)-ios-to-macos.bin"
    private static let payload = Data("envoix cross-device ios to macos\n".utf8)
    private static let expectedBytes = environment("ENVOIX_IOS_TO_MACOS_BYTES")
        .flatMap(UInt64.init) ?? UInt64(payload.count)
    private static let timeout = environment("ENVOIX_CROSS_DEVICE_TIMEOUT_SECONDS")
        .flatMap(TimeInterval.init) ?? 180
    private static let useRoomDefaultsKey = "envoix.useRoom"
    private static let useMdnsDefaultsKey = "envoix.useMdns"
    private static let hashBlockBytes = 1024 * 1024
    private static let runtimeSettings = EnvoixRuntimeSettings(
        concurrentTransfers: true,
        language: "en",
        serverUrl: defaultRendezvousBroker,
        relayUrl: defaultRelayURL,
        configPath: "",
        speedLimitMbps: 40
    )

    private func outputDirectory() -> URL {
        if let path = Self.environment("ENVOIX_MACOS_APP_RECEIVE_DIR") {
            return URL(fileURLWithPath: path, isDirectory: true)
        }
        return FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "envoix-macos-hosted-\(ProcessInfo.processInfo.processIdentifier)/received",
                isDirectory: true
            )
    }

    private func waitForNewReceiveActivity(
        in model: AppModel,
        excluding existingActivityIDs: Set<String>
    ) async throws -> String {
        let deadline = Date().addingTimeInterval(Self.timeout)
        while Date() < deadline {
            if let record = model.activities.first(where: {
                $0.direction == .receive && !existingActivityIDs.contains($0.activityId)
            }) {
                if record.state == .failed {
                    throw HostedTestError.transferFailed(record.diagnosticMessage)
                }
                return record.activityId
            }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        throw HostedTestError.timedOut("waiting for the macOS App receive activity")
    }

    private func waitForCompletion(
        activityID: String,
        in model: AppModel
    ) async throws -> FfiTransferActivityRecord {
        let deadline = Date().addingTimeInterval(Self.timeout)
        while Date() < deadline {
            if let record = model.activities.first(where: { $0.activityId == activityID }) {
                switch record.state {
                case .completed:
                    return record
                case .failed, .canceled:
                    throw HostedTestError.transferFailed(record.diagnosticMessage)
                case .queued, .binding, .waitingForPeer, .pairing, .connecting,
                        .transferring, .verifying, .publishing, .unconfirmed,
                        .paused, .unknown:
                    break
                }
            }
            try await Task.sleep(nanoseconds: 200_000_000)
        }
        throw HostedTestError.timedOut("waiting for macOS App completion")
    }

    private static func restoreDefault(_ value: Any?, key: String, defaults: UserDefaults) {
        if let value {
            defaults.set(value, forKey: key)
        } else {
            defaults.removeObject(forKey: key)
        }
    }

    private static func fileSize(_ url: URL) throws -> UInt64 {
        let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
        guard let size = attributes[.size] as? NSNumber else {
            throw HostedTestError.missingFileSize(url.path)
        }
        return size.uint64Value
    }

    private static func fileSHA256(_ url: URL) throws -> Data {
        var hasher = SHA256()
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        while let chunk = try handle.read(upToCount: hashBlockBytes), !chunk.isEmpty {
            hasher.update(data: chunk)
        }
        return Data(hasher.finalize())
    }

    private static func repeatedPayloadSHA256(_ payload: Data, expectedBytes: UInt64) -> Data {
        var hasher = SHA256()
        var remaining = expectedBytes
        while remaining > 0 {
            let count = Int(min(remaining, UInt64(payload.count)))
            hasher.update(data: payload.prefix(count))
            remaining -= UInt64(count)
        }
        return Data(hasher.finalize())
    }

    private static func environment(_ name: String) -> String? {
        guard let value = ProcessInfo.processInfo.environment[name], !value.isEmpty else {
            return nil
        }
        return value
    }

    private func emitEvidence(_ message: String) {
        FileHandle.standardError.write(Data("[macos-app-cross-device] \(message)\n".utf8))
    }
#endif
}

private enum HostedTestError: LocalizedError {
    case missingFileSize(String)
    case timedOut(String)
    case transferFailed(String)

    var errorDescription: String? {
        switch self {
        case .missingFileSize(let path): return "Could not read file size: \(path)"
        case .timedOut(let operation): return "Timed out \(operation)"
        case .transferFailed(let reason): return "Transfer failed: \(reason)"
        }
    }
}

private extension Data {
    var hexString: String {
        map { String(format: "%02x", $0) }.joined()
    }
}
