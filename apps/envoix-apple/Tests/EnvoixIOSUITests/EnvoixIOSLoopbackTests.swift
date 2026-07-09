import EnvoixCore
import XCTest

final class EnvoixIOSLoopbackTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    func testTransferScreenShowsStableControls() throws {
        let app = XCUIApplication()
        app.launchArguments.append("--ui-testing")

        addUIInterruptionMonitor(withDescription: "System permissions") { alert in
            if alert.buttons["Allow"].exists {
                alert.buttons["Allow"].tap()
                return true
            }
            if alert.buttons["OK"].exists {
                alert.buttons["OK"].tap()
                return true
            }
            return false
        }

        app.launch()
        app.tap()

        XCTAssertTrue(app.tabBars.buttons["Transfer"].waitForExistence(timeout: 8))
        XCTAssertTrue(app.tabBars.buttons["Activity"].exists)
        XCTAssertTrue(app.tabBars.buttons["Settings"].exists)

        XCTAssertTrue(app.buttons["transfer_role_send"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["transfer_role_receive"].exists)

        app.buttons["transfer_role_send"].tap()

        XCTAssertTrue(app.buttons["send_file_picker"].exists)
        XCTAssertTrue(app.buttons["send_start_button"].exists)
        XCTAssertFalse(app.buttons["send_start_button"].isEnabled)

        app.buttons["transfer_role_receive"].tap()

        XCTAssertTrue(app.descendants(matching: .any)["receive_room_code"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["receive_start_button"].exists)
        XCTAssertTrue(app.buttons["receive_start_button"].isEnabled)
    }

    func testSendsSmallFileThroughUniffiInviteLoopback() throws {
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-ios-loopback-\(UUID().uuidString)", isDirectory: true)
        let receiveDirectory = root.appendingPathComponent("received", isDirectory: true)
        try fileManager.createDirectory(at: receiveDirectory, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: root) }

        let sendFile = root.appendingPathComponent("ios-loopback.txt")
        let payload = Data("envoix ios loopback \(UUID().uuidString)\n".utf8)
        try payload.write(to: sendFile)

        let receiverSession = EnvoixSession()
        let receiverObserver = RecordingObserver()
        try receiverSession.receive(outputDir: receiveDirectory.path, observer: receiverObserver)

        let invite = try receiverObserver.waitForInvite(timeout: 10)
        Thread.sleep(forTimeInterval: 0.3)

        let senderSession = EnvoixSession()
        let senderObserver = RecordingObserver()
        try senderSession.sendInvite(invite: invite, filePath: sendFile.path, observer: senderObserver)

        let senderBytes = try senderObserver.waitForCompletion(timeout: 90)
        let receiverBytes = try receiverObserver.waitForCompletion(timeout: 90)
        XCTAssertGreaterThanOrEqual(senderBytes, UInt64(payload.count))
        XCTAssertGreaterThanOrEqual(receiverBytes, UInt64(payload.count))

        let receivedPayload = try Data(contentsOf: receiveDirectory.appendingPathComponent(sendFile.lastPathComponent))
        XCTAssertEqual(receivedPayload, payload)
    }

    func testCrossDeviceReceiveAndroidToIosRoom() throws {
#if ENVOIX_CROSS_DEVICE_TESTING
        print("[cross-device] iOS receive start code=\(Self.androidToIosCode)")
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-ios-cross-device-receive-\(UUID().uuidString)", isDirectory: true)
        let receiveDirectory = root.appendingPathComponent("received", isDirectory: true)
        try fileManager.createDirectory(at: receiveDirectory, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: root) }

        let session = Self.crossDeviceSession()
        let observer = RecordingObserver()
        try session.startTransfer(
            request: Self.crossDeviceRequest(
                direction: .receive,
                mode: .room,
                code: Self.androidToIosCode,
                filePath: "",
                outputDir: receiveDirectory.path,
                invite: ""
            ),
            observer: observer
        )
        print("[cross-device] iOS receive completed call returned")

        let expectedBytes = Self.androidToIosExpectedBytes
        let bytes = try observer.waitForCompletion(timeout: Self.crossDeviceTimeout(for: expectedBytes))
        print("[cross-device] iOS receive completion bytes=\(bytes)")
        XCTAssertGreaterThanOrEqual(bytes, expectedBytes)

        try Self.assertReceivedFile(
            receiveDirectory.appendingPathComponent(Self.androidToIosFileName),
            payload: Self.androidToIosPayload,
            expectedBytes: expectedBytes
        )
#endif
    }

    func testCrossDeviceSendIosToAndroidRoom() throws {
#if ENVOIX_CROSS_DEVICE_TESTING
        print("[cross-device] iOS send start code=\(Self.iosToAndroidCode)")
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-ios-cross-device-send-\(UUID().uuidString)", isDirectory: true)
        try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: root) }

        let sendFile = root.appendingPathComponent(Self.iosToAndroidFileName)
        let expectedBytes = Self.iosToAndroidExpectedBytes
        try Self.writeCrossDevicePayload(Self.iosToAndroidPayload, expectedBytes: expectedBytes, to: sendFile)

        let session = Self.crossDeviceSession()
        let observer = RecordingObserver()
        try session.startTransfer(
            request: Self.crossDeviceRequest(
                direction: .send,
                mode: .room,
                code: Self.iosToAndroidCode,
                filePath: sendFile.path,
                outputDir: "",
                invite: ""
            ),
            observer: observer
        )
        print("[cross-device] iOS send completed call returned")

        let bytes = try observer.waitForCompletion(timeout: Self.crossDeviceTimeout(for: expectedBytes))
        print("[cross-device] iOS send completion bytes=\(bytes)")
        XCTAssertGreaterThanOrEqual(bytes, expectedBytes)
#endif
    }

    func testCrossDeviceReceiveAndroidToIosInvite() throws {
#if ENVOIX_CROSS_DEVICE_TESTING
        print("[cross-device] iOS invite receive start")
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-ios-cross-device-invite-receive-\(UUID().uuidString)", isDirectory: true)
        let receiveDirectory = root.appendingPathComponent("received", isDirectory: true)
        try fileManager.createDirectory(at: receiveDirectory, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: root) }

        let session = Self.crossDeviceSession()
        let observer = RecordingObserver()
        try session.startTransfer(
            request: Self.crossDeviceRequest(
                direction: .receive,
                mode: .showInvite,
                code: "",
                filePath: "",
                outputDir: receiveDirectory.path,
                invite: ""
            ),
            observer: observer
        )

        let invite = try observer.waitForInvite(timeout: 10)
        print("[cross-device] iOS invite \(invite)")

        let expectedBytes = Self.androidToIosExpectedBytes
        let bytes = try observer.waitForCompletion(timeout: Self.crossDeviceTimeout(for: expectedBytes))
        print("[cross-device] iOS invite receive completion bytes=\(bytes)")
        XCTAssertGreaterThanOrEqual(bytes, expectedBytes)

        try Self.assertReceivedFile(
            receiveDirectory.appendingPathComponent(Self.androidToIosFileName),
            payload: Self.androidToIosPayload,
            expectedBytes: expectedBytes
        )
#endif
    }

    func testCrossDeviceSendIosToAndroidInvite() throws {
#if ENVOIX_CROSS_DEVICE_TESTING
        print("[cross-device] iOS invite send start")
        guard let invite = ProcessInfo.processInfo.environment["ENVOIX_TRANSFER_INVITE"], !invite.isEmpty else {
            throw LoopbackTestError.missingValue("ENVOIX_TRANSFER_INVITE")
        }
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-ios-cross-device-invite-send-\(UUID().uuidString)", isDirectory: true)
        try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: root) }

        let sendFile = root.appendingPathComponent(Self.iosToAndroidFileName)
        let expectedBytes = Self.iosToAndroidExpectedBytes
        try Self.writeCrossDevicePayload(Self.iosToAndroidPayload, expectedBytes: expectedBytes, to: sendFile)

        let session = Self.crossDeviceSession()
        let observer = RecordingObserver()
        try session.startTransfer(
            request: Self.crossDeviceRequest(
                direction: .send,
                mode: .invite,
                code: "",
                filePath: sendFile.path,
                outputDir: "",
                invite: invite
            ),
            observer: observer
        )

        let bytes = try observer.waitForCompletion(timeout: Self.crossDeviceTimeout(for: expectedBytes))
        print("[cross-device] iOS invite send completion bytes=\(bytes)")
        XCTAssertGreaterThanOrEqual(bytes, expectedBytes)
#endif
    }

#if ENVOIX_CROSS_DEVICE_TESTING
    private static let defaultAndroidToIosCode = "741203-amber-comet"
    private static let defaultIosToAndroidCode = "741204-azure-river"
    private static let androidToIosCode = envString("ENVOIX_ANDROID_TO_IOS_CODE") ?? defaultAndroidToIosCode
    private static let iosToAndroidCode = envString("ENVOIX_IOS_TO_ANDROID_CODE") ?? defaultIosToAndroidCode
    private static let androidToIosFileName = "envoix-cross-android-to-ios.txt"
    private static let iosToAndroidFileName = "envoix-cross-ios-to-android.txt"
    private static let androidToIosPayload = Data("envoix cross-device android to ios\n".utf8)
    private static let iosToAndroidPayload = Data("envoix cross-device ios to android\n".utf8)
    private static let androidToIosExpectedBytes =
        envUInt64("ENVOIX_ANDROID_TO_IOS_BYTES") ?? UInt64(androidToIosPayload.count)
    private static let iosToAndroidExpectedBytes =
        envUInt64("ENVOIX_IOS_TO_ANDROID_BYTES") ?? UInt64(iosToAndroidPayload.count)
    private static let crossDeviceTimeout: TimeInterval = 180
    private static let timeoutBytesPerSecond: UInt64 = 2 * 1024 * 1024
    private static let rendezvousBroker = "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445"
    private static let relayURL = "https://envoix.chkxwlyh.us:8444"

    private static func crossDeviceTimeout(for expectedBytes: UInt64) -> TimeInterval {
        if let override = envDouble("ENVOIX_CROSS_DEVICE_TIMEOUT_SECONDS") {
            return override
        }
        let scaled = crossDeviceTimeout + TimeInterval(expectedBytes / timeoutBytesPerSecond)
        return max(crossDeviceTimeout, scaled)
    }

    private static func writeCrossDevicePayload(_ payload: Data, expectedBytes: UInt64, to url: URL) throws {
        if expectedBytes == UInt64(payload.count) {
            try payload.write(to: url)
            return
        }
        _ = FileManager.default.createFile(atPath: url.path, contents: nil)
        let handle = try FileHandle(forWritingTo: url)
        defer { try? handle.close() }
        try handle.truncate(atOffset: expectedBytes)
        guard expectedBytes > 0 else { return }

        let prefixCount = expectedBytes < UInt64(payload.count) ? Int(expectedBytes) : payload.count
        try handle.seek(toOffset: 0)
        try handle.write(contentsOf: payload.prefix(prefixCount))
        if expectedBytes > UInt64(payload.count) {
            try handle.seek(toOffset: expectedBytes - 1)
            let lastByte = payload[Int((expectedBytes - 1) % UInt64(payload.count))]
            try handle.write(contentsOf: Data([lastByte]))
        }
    }

    private static func assertReceivedFile(_ url: URL, payload: Data, expectedBytes: UInt64) throws {
        let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
        let actualBytes = try XCTUnwrap(attributes[.size] as? NSNumber).uint64Value
        XCTAssertEqual(actualBytes, expectedBytes)
        if expectedBytes == UInt64(payload.count) {
            let receivedPayload = try Data(contentsOf: url)
            XCTAssertEqual(receivedPayload, payload)
        }
    }

    private static func envUInt64(_ name: String) -> UInt64? {
        guard let raw = ProcessInfo.processInfo.environment[name], !raw.isEmpty else {
            return nil
        }
        return UInt64(raw)
    }

    private static func envDouble(_ name: String) -> Double? {
        guard let raw = ProcessInfo.processInfo.environment[name], !raw.isEmpty else {
            return nil
        }
        return Double(raw)
    }

    private static func envString(_ name: String) -> String? {
        guard let raw = ProcessInfo.processInfo.environment[name], !raw.isEmpty else {
            return nil
        }
        return raw
    }

    private static func crossDeviceSession() -> EnvoixSession {
        EnvoixSession.newWithSettings(
            settings: EnvoixRuntimeSettings(
                concurrentTransfers: true,
                language: "en",
                serverUrl: rendezvousBroker,
                relayUrl: relayURL,
                configPath: "",
                speedLimitMbps: 40
            )
        )
    }

    private static func crossDeviceRequest(
        direction: FfiTransferDirection,
        mode: FfiTransferMode,
        code: String,
        filePath: String,
        outputDir: String,
        invite: String
    ) -> FfiTransferRequest {
        FfiTransferRequest(
            activityId: "ios-\(UUID().uuidString)",
            direction: direction,
            mode: mode,
            filePath: filePath,
            outputDir: outputDir,
            peerDescriptor: "",
            invite: invite,
            code: code,
            token: code,
            broker: rendezvousBroker,
            relay: relayURL,
            configPath: "",
            pathPolicy: .auto,
            resume: true,
            limits: FfiTransferLimits(
                maxParallelTransfers: 1,
                maxParallelFiles: 1,
                maxParallelChunksPerFile: 1,
                speedLimitBps: 0
            ),
            rendezvous: rendezvousPlan(for: mode)
        )
    }

    private static func rendezvousPlan(for mode: FfiTransferMode) -> FfiRendezvousPlan {
        switch mode {
        case .room:
            return FfiRendezvousPlan(useRoom: true, useMdns: true, internetAvailable: true)
        case .mdns:
            return FfiRendezvousPlan(useRoom: false, useMdns: true, internetAvailable: true)
        default:
            return FfiRendezvousPlan(useRoom: false, useMdns: false, internetAvailable: true)
        }
    }
#endif
}

private final class RecordingObserver: TransferObserver, @unchecked Sendable {
    private let lock = NSLock()
    private let inviteSemaphore = DispatchSemaphore(value: 0)
    private let terminalSemaphore = DispatchSemaphore(value: 0)

    private var invite: String?
    private var completedBytes: UInt64?
    private var failure: String?

    func onInviteReady(invite: String) {
        let shouldSignal = locked {
            guard self.invite == nil else { return false }
            self.invite = invite
            return true
        }
        if shouldSignal {
            inviteSemaphore.signal()
        }
    }

    func onStarted(fileName: String, totalBytes: UInt64) {
        if !fileName.isEmpty {
            print("[cross-device] onStarted fileName=\(fileName) totalBytes=\(totalBytes)")
        } else {
            print("[cross-device] onStarted fileName=<unknown> totalBytes=\(totalBytes)")
        }
    }

    func onProgress(transferred: UInt64, total: UInt64) {
        print("[cross-device] onProgress transferred=\(transferred) total=\(total)")
    }

    func onCompleted(bytes: UInt64) {
        complete(bytes: bytes, failure: nil)
    }

    func onTransferFailed(failure: FfiTransferFailure) {
        let message = failure.diagnosticMessage.isEmpty ? failure.userMessageKey : failure.diagnosticMessage
        complete(bytes: nil, failure: message)
    }

    func onFailed(reason: String) {
        complete(bytes: nil, failure: reason)
    }

    func onTransferEvent(event: FfiTransferEvent) {
        print(
            "[cross-device] onTransferEvent kind=\(event.kind) mode=\(event.mode) direction=\(event.direction) " +
            "pairing=\(event.pairingStep) path=\(event.dataPathKind):\(event.dataPathDetail) " +
            "bytes=\(event.bytesTransferred)/\(event.totalBytes) token=\(Self.tokenLabel(event.token)) " +
            "peerLen=\(event.peerDescriptor.count)"
        )
    }

    func onTransferActivity(record: FfiTransferActivityRecord) {
        print("[cross-device] onTransferActivity \(record)")
    }

    func onStatus(message: String) {
        if !message.isEmpty {
            print("[cross-device] status \(message)")
        }
    }

    func waitForInvite(timeout: TimeInterval) throws -> String {
        guard inviteSemaphore.wait(timeout: .now() + timeout) == .success else {
            throw LoopbackTestError.timeout("invite")
        }
        return try locked {
            guard let invite else {
                throw LoopbackTestError.missingValue("invite")
            }
            return invite
        }
    }

    func waitForCompletion(timeout: TimeInterval) throws -> UInt64 {
        guard terminalSemaphore.wait(timeout: .now() + timeout) == .success else {
            throw LoopbackTestError.timeout("completion")
        }
        return try locked {
            if let failure {
                throw LoopbackTestError.transferFailed(failure)
            }
            guard let completedBytes else {
                throw LoopbackTestError.missingValue("completed bytes")
            }
            return completedBytes
        }
    }

    private func complete(bytes: UInt64?, failure: String?) {
        let shouldSignal = locked {
            guard completedBytes == nil && self.failure == nil else { return false }
            completedBytes = bytes
            self.failure = failure
            return true
        }
        if shouldSignal {
            terminalSemaphore.signal()
        }
    }

    private func locked<T>(_ body: () throws -> T) rethrows -> T {
        lock.lock()
        defer { lock.unlock() }
        return try body()
    }

    private static func tokenLabel(_ token: String) -> String {
        let trimmed = token.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return "<none>" }
        let room = trimmed.split(separator: "-", maxSplits: 1, omittingEmptySubsequences: false).first.map(String.init) ?? ""
        return room != trimmed && !room.isEmpty ? room : "set(len=\(trimmed.count))"
    }
}

private enum LoopbackTestError: Error, CustomStringConvertible {
    case timeout(String)
    case missingValue(String)
    case transferFailed(String)

    var description: String {
        switch self {
        case .timeout(let value):
            return "timed out waiting for \(value)"
        case .missingValue(let value):
            return "missing \(value)"
        case .transferFailed(let reason):
            return "transfer failed: \(reason)"
        }
    }
}
