import EnvoixCore
import XCTest

final class EnvoixIOSLoopbackTests: XCTestCase {
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

        let session = EnvoixSession()
        let observer = RecordingObserver()
        try session.receiveRoom(outputDir: receiveDirectory.path, code: Self.androidToIosCode, observer: observer)
        print("[cross-device] iOS receive completed call returned")

        let bytes = try observer.waitForCompletion(timeout: Self.crossDeviceTimeout)
        print("[cross-device] iOS receive completion bytes=\(bytes)")
        XCTAssertGreaterThanOrEqual(bytes, UInt64(Self.androidToIosPayload.count))

        let receivedPayload = try Data(contentsOf: receiveDirectory.appendingPathComponent(Self.androidToIosFileName))
        XCTAssertEqual(receivedPayload, Self.androidToIosPayload)
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
        try Self.iosToAndroidPayload.write(to: sendFile)

        let session = EnvoixSession()
        let observer = RecordingObserver()
        try session.sendRoom(filePath: sendFile.path, code: Self.iosToAndroidCode, observer: observer)
        print("[cross-device] iOS send completed call returned")

        let bytes = try observer.waitForCompletion(timeout: Self.crossDeviceTimeout)
        print("[cross-device] iOS send completion bytes=\(bytes)")
        XCTAssertGreaterThanOrEqual(bytes, UInt64(Self.iosToAndroidPayload.count))
#endif
    }

#if ENVOIX_CROSS_DEVICE_TESTING
    private static let androidToIosCode = "741203-amber-comet"
    private static let iosToAndroidCode = "741204-azure-river"
    private static let androidToIosFileName = "envoix-cross-android-to-ios.txt"
    private static let iosToAndroidFileName = "envoix-cross-ios-to-android.txt"
    private static let androidToIosPayload = Data("envoix cross-device android to ios\n".utf8)
    private static let iosToAndroidPayload = Data("envoix cross-device ios to android\n".utf8)
    private static let crossDeviceTimeout: TimeInterval = 180
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
