import EnvoixCore
import CryptoKit
import XCTest
@testable import Envoix_iOS

final class EnvoixIOSLoopbackTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    func testActivityActionPolicyMatchesCanonicalLifecycle() {
        for state in [
            FfiTransferActivityState.queued, .binding, .waitingForPeer, .pairing,
            .connecting, .transferring, .verifying,
        ] {
            XCTAssertEqual(
                activityActionAvailability(for: Self.activity(state: state)),
                ActivityActionAvailability(canPause: true, canResume: false, canCancel: true, canDelete: false, isFinalizing: false)
            )
        }

        XCTAssertEqual(
            activityActionAvailability(for: Self.activity(state: .verifying, diagnostic: "confirming")),
            ActivityActionAvailability(canPause: false, canResume: false, canCancel: false, canDelete: false, isFinalizing: true)
        )
        XCTAssertEqual(
            activityActionAvailability(for: Self.activity(state: .paused)),
            ActivityActionAvailability(canPause: false, canResume: true, canCancel: true, canDelete: false, isFinalizing: false)
        )
        XCTAssertEqual(
            activityActionAvailability(for: Self.activity(state: .failed)),
            ActivityActionAvailability(canPause: false, canResume: false, canCancel: false, canDelete: true, isFinalizing: false)
        )
        XCTAssertEqual(
            activityActionAvailability(for: Self.activity(state: .failed, retryable: true)),
            ActivityActionAvailability(canPause: false, canResume: true, canCancel: false, canDelete: true, isFinalizing: false)
        )
        XCTAssertEqual(
            activityActionAvailability(for: Self.activity(state: .publishing, diagnostic: "display text is not policy", retryable: true)),
            ActivityActionAvailability(canPause: false, canResume: true, canCancel: true, canDelete: false, isFinalizing: false)
        )
        XCTAssertEqual(
            activityActionAvailability(for: Self.activity(state: .completed)),
            ActivityActionAvailability(canPause: false, canResume: false, canCancel: false, canDelete: true, isFinalizing: false)
        )

        let coreInfo = envoixCoreInfo()
        XCTAssertEqual(coreInfo.ffiApiVersion, expectedCoreFFIAPIVersion)
        XCTAssertTrue(coreInfo.capabilities.contains("activity_actions_v1"))
        XCTAssertTrue(coreInfo.capabilities.contains("durable_publication_recovery_v1"))
        XCTAssertTrue(coreInfo.capabilities.contains("per_session_receipt_endpoint_v1"))
    }

    func testActivityProjectionRejectsReorderedSnapshots() {
        let current = Self.activity(
            state: .transferring,
            sequence: 12,
            updatedAtMs: 200
        )

        XCTAssertFalse(ActivityProjectionPolicy.shouldAccept(
            Self.activity(state: .waitingForPeer, sequence: 11, updatedAtMs: 300),
            replacing: current
        ))
        XCTAssertFalse(ActivityProjectionPolicy.shouldAccept(
            Self.activity(state: .transferring, sequence: 12, updatedAtMs: 199),
            replacing: current
        ))
        XCTAssertTrue(ActivityProjectionPolicy.shouldAccept(
            Self.activity(state: .transferring, sequence: 12, updatedAtMs: 201),
            replacing: current
        ))
        XCTAssertTrue(ActivityProjectionPolicy.shouldAccept(
            Self.activity(state: .paused, sequence: 13, updatedAtMs: 150),
            replacing: current
        ))
        XCTAssertFalse(ActivityProjectionPolicy.shouldAccept(
            Self.activity(id: "other", state: .transferring, sequence: 13, updatedAtMs: 300),
            replacing: current
        ))
    }

    func testActivityProjectionPrunesOnlyTerminalHistory() {
        let records = [
            Self.activity(id: "active-new", state: .transferring, updatedAtMs: 80),
            Self.activity(id: "active-old", state: .paused, updatedAtMs: 10),
            Self.activity(id: "done-new", state: .completed, updatedAtMs: 100),
            Self.activity(id: "done-middle", state: .failed, updatedAtMs: 90),
            Self.activity(id: "done-old", state: .canceled, updatedAtMs: 1),
        ]

        let retained = ActivityProjectionPolicy.pruneTerminalHistory(records, limit: 3)
        XCTAssertEqual(retained.map(\.activityId), ["done-new", "active-new", "active-old"])

        let overLimitActive = records + [
            Self.activity(id: "active-third", state: .publishing, updatedAtMs: 70),
            Self.activity(id: "active-fourth", state: .unconfirmed, updatedAtMs: 60),
        ]
        let allActiveRetained = ActivityProjectionPolicy.pruneTerminalHistory(overLimitActive, limit: 3)
        XCTAssertEqual(
            Set(allActiveRetained.map(\.activityId)),
            Set(["active-new", "active-old", "active-third", "active-fourth"])
        )
    }

    func testTransferPhaseIsPureCanonicalPresentation() {
        for state in [
            FfiTransferActivityState.queued, .binding, .waitingForPeer, .pairing,
            .connecting, .verifying, .publishing, .unconfirmed,
        ] {
            XCTAssertEqual(
                TransferViewModel.presentationPhase(for: Self.activity(state: state)),
                .waiting
            )
        }
        XCTAssertEqual(
            TransferViewModel.presentationPhase(for: Self.activity(state: .transferring)),
            .transferring
        )
        XCTAssertEqual(
            TransferViewModel.presentationPhase(for: Self.activity(state: .paused)),
            .paused
        )
        XCTAssertEqual(
            TransferViewModel.presentationPhase(for: Self.activity(state: .completed)),
            .completed(bytes: 512)
        )
        XCTAssertEqual(
            TransferViewModel.presentationPhase(for: Self.activity(state: .canceled)),
            .canceled
        )
        guard case .failed = TransferViewModel.presentationPhase(
            for: Self.activity(state: .failed, diagnostic: "canonical failure")
        ) else {
            return XCTFail("failed Activity must project a failed presentation")
        }
        guard case .failed = TransferViewModel.presentationPhase(for: Self.activity(state: .unknown)) else {
            return XCTFail("unknown Activity must be surfaced instead of freezing an older phase")
        }
    }

    private static func activity(
        id: String = "activity-test",
        state: FfiTransferActivityState,
        diagnostic: String = "",
        retryable: Bool = false,
        sequence: UInt64 = 1,
        updatedAtMs: UInt64 = 1
    ) -> FfiTransferActivityRecord {
        FfiTransferActivityRecord(
            activityId: id,
            sequence: sequence,
            attemptId: "attempt-1",
            state: state,
            direction: .receive,
            mode: .room,
            transferId: "transfer-test",
            fileName: "test.bin",
            totalBytes: 1024,
            bytesTransferred: 512,
            bytesResumed: 0,
            speedBps: 0,
            averageSpeedBps: 0,
            createdAtMs: 1,
            updatedAtMs: updatedAtMs,
            startedAtMs: 1,
            completedAtMs: 0,
            completedFilePath: "",
            dataPathKind: .none,
            dataPathDetail: "",
            invite: "",
            token: "",
            peerDescriptor: "",
            diagnosticMessage: diagnostic,
            failureCode: .unknown,
            failureCategory: .unknown,
            failurePhase: .setup,
            failureOrigin: .unknown,
            userMessageKey: "",
            retryable: retryable,
            recoveryAction: .none,
            limits: FfiTransferLimits(
                maxParallelTransfers: 1,
                maxParallelFiles: 1,
                maxParallelChunksPerFile: 1,
                speedLimitBps: 0
            )
        )
    }

    func testCompletedFileAvailabilityRequiresMatchingRegularFile() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("envoix-completion-check-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let file = root.appendingPathComponent("received.bin")
        try Data([1, 2, 3, 4]).write(to: file)

        XCTAssertEqual(
            availableCompletedFileURL(path: file.path, expectedBytes: 4),
            file
        )
        XCTAssertNil(availableCompletedFileURL(path: file.path, expectedBytes: 5))
        XCTAssertNil(availableCompletedFileURL(path: root.path, expectedBytes: 0))
        XCTAssertNil(availableCompletedFileURL(path: root.appendingPathComponent("missing").path, expectedBytes: 0))
    }

    func testSendsSmallFileThroughUniffiInviteLoopback() async throws {
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

        let invite = try await receiverObserver.waitForInvite(timeout: 10)
        try await Task.sleep(nanoseconds: 300_000_000)

        let senderSession = EnvoixSession()
        let senderObserver = RecordingObserver()
        try senderSession.sendInvite(invite: invite, filePath: sendFile.path, observer: senderObserver)

        let senderBytes = try await senderObserver.waitForCompletion(timeout: 90)
        let receiverBytes = try await receiverObserver.waitForCompletion(timeout: 90)
        XCTAssertGreaterThanOrEqual(senderBytes, UInt64(payload.count))
        XCTAssertGreaterThanOrEqual(receiverBytes, UInt64(payload.count))

        let receivedPayload = try Data(contentsOf: receiveDirectory.appendingPathComponent(sendFile.lastPathComponent))
        XCTAssertEqual(receivedPayload, payload)
    }

    func testPublishingVerifiedFileIsAtomicAndPreservesFailures() throws {
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-publish-\(UUID().uuidString)", isDirectory: true)
        let staging = root.appendingPathComponent("staging", isDirectory: true)
        let destination = root.appendingPathComponent("destination", isDirectory: true)
        try fileManager.createDirectory(at: staging, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: root) }

        let payload = Data("verified envoix payload".utf8)
        let source = staging.appendingPathComponent("received.bin")
        try payload.write(to: source)

        let published = try publishReceivedFile(
            from: source,
            to: destination,
            expectedBytes: UInt64(payload.count)
        )
        XCTAssertEqual(try Data(contentsOf: published), payload)
        XCTAssertTrue(fileManager.fileExists(atPath: source.path))
        XCTAssertTrue(
            try fileManager.contentsOfDirectory(atPath: destination.path)
                .allSatisfy { !$0.hasPrefix(".envoix-publish-") }
        )

        let repeated = try publishReceivedFile(
            from: source,
            to: destination,
            expectedBytes: UInt64(payload.count)
        )
        XCTAssertEqual(repeated, published)

        let conflictSource = staging.appendingPathComponent("conflict.bin")
        let conflictingPayload = Data(repeating: 0x7f, count: payload.count)
        try conflictingPayload.write(to: conflictSource)
        let conflictingDestination = destination.appendingPathComponent("conflict.bin")
        try Data(repeating: 0x21, count: payload.count).write(to: conflictingDestination)
        XCTAssertThrowsError(
            try publishReceivedFile(
                from: conflictSource,
                to: destination,
                expectedBytes: UInt64(payload.count)
            )
        )
        XCTAssertEqual(try Data(contentsOf: conflictSource), conflictingPayload)
        XCTAssertEqual(try Data(contentsOf: published), payload)

        let sizeMismatchSource = staging.appendingPathComponent("size-mismatch.bin")
        try payload.write(to: sizeMismatchSource)
        XCTAssertThrowsError(
            try publishReceivedFile(
                from: sizeMismatchSource,
                to: destination,
                expectedBytes: UInt64(payload.count + 1)
            )
        )
        XCTAssertEqual(try Data(contentsOf: sizeMismatchSource), payload)
    }

    func testCrossDeviceReceiveAndroidToIosRoom() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        Self.emitCrossDeviceMarker("iOS receive start code=\(Self.androidToIosCode)")
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-ios-cross-device-receive-\(UUID().uuidString)", isDirectory: true)
        let receiveDirectory = root.appendingPathComponent("received", isDirectory: true)
        try fileManager.createDirectory(at: receiveDirectory, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: root) }

        let observer = RecordingObserver {
            Self.emitCrossDeviceMarker("iOS room receiver ready")
        }
        let session = try Self.startDurableCrossDeviceTransfer(
            request: Self.crossDeviceRequest(
                direction: .receive,
                mode: .room,
                code: Self.androidToIosCode,
                filePath: "",
                outputDir: receiveDirectory.path,
                invite: "",
                publicationRequired: true
            ),
            recordsDirectory: root.appendingPathComponent("records", isDirectory: true),
            observer: observer
        )
        defer { _ = session.remove() }
        print("[cross-device] iOS receive completed call returned")

        let expectedBytes = Self.androidToIosExpectedBytes
        let published = try await Self.publishAndCompleteReceive(
            session: session,
            observer: observer,
            expectedFileName: Self.androidToIosFileName,
            payload: Self.androidToIosPayload,
            expectedBytes: expectedBytes
        )
        defer { try? fileManager.removeItem(at: published.url.deletingLastPathComponent()) }
        let bytes = published.bytes
        print("[cross-device] iOS receive completion bytes=\(bytes)")
        XCTAssertEqual(bytes, expectedBytes)
        observer.assertPathPolicy(Self.crossDevicePathPolicy)
#endif
    }

    func testCrossDeviceSendIosToAndroidRoom() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        try await runCrossDeviceRoomSend(
            code: Self.iosToAndroidCode,
            fileName: Self.iosToAndroidFileName,
            payload: Self.iosToAndroidPayload,
            expectedBytes: Self.iosToAndroidExpectedBytes,
            peerLabel: "Android"
        )
#endif
    }

    func testCrossDeviceSendIosToMacOSRoom() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        try await runCrossDeviceRoomSend(
            code: Self.iosToMacOSCode,
            fileName: Self.iosToMacOSFileName,
            payload: Self.iosToMacOSPayload,
            expectedBytes: Self.iosToMacOSExpectedBytes,
            peerLabel: "macOS"
        )
#endif
    }

    func testCrossDeviceSendIosToMacOSManifestRoom() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-ios-manifest-send-\(UUID().uuidString)", isDirectory: true)
        let album = root.appendingPathComponent(Self.iosToMacOSManifestAlbumName, isDirectory: true)
        let emptyDirectory = album.appendingPathComponent("Empty", isDirectory: true)
        let photo = album.appendingPathComponent("photo.bin")
        let loose = root.appendingPathComponent(Self.iosToMacOSManifestLooseName)
        try fileManager.createDirectory(at: emptyDirectory, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: root) }
        try Self.iosToMacOSManifestPhotoPayload.write(to: photo)
        try Self.iosToMacOSManifestLoosePayload.write(to: loose)

        let activityID = "ios-manifest-\(UUID().uuidString)"
        let prepared = try await prepareManifestSend(
            activityId: activityID,
            selectedPaths: [album.path, loose.path]
        )
        XCTAssertEqual(prepared.rootCount, 2)
        XCTAssertEqual(prepared.fileCount, 2)
        XCTAssertEqual(prepared.directoryCount, 2)
        XCTAssertEqual(
            prepared.totalBytes,
            UInt64(Self.iosToMacOSManifestPhotoPayload.count + Self.iosToMacOSManifestLoosePayload.count)
        )

        let recordsDirectory = root.appendingPathComponent("records", isDirectory: true)
        let request = Self.crossDeviceRequest(
            activityID: activityID,
            direction: .send,
            mode: .room,
            code: Self.iosToMacOSCode,
            filePath: "",
            outputDir: "",
            invite: ""
        )
        let observer = ManifestEvidenceObserver(peerLabel: "macOS")
        let session = try startDurableManifestSend(
            settings: Self.crossDeviceSettings(),
            request: request,
            prepared: prepared,
            recordsDir: recordsDirectory.path,
            observer: observer
        )
        defer { _ = session.remove() }

        let completed = try await Self.waitForManifestCompletion(
            session: session,
            timeout: Self.crossDeviceTimeout(for: prepared.totalBytes)
        )
        XCTAssertEqual(completed.activity.state, .completed)
        XCTAssertEqual(completed.completedFiles, prepared.fileCount)
        XCTAssertEqual(completed.rootCount, prepared.rootCount)
        XCTAssertEqual(completed.activity.bytesTransferred, prepared.totalBytes)
        XCTAssertNotEqual(completed.activity.dataPathKind, .none)
        XCTAssertTrue(completed.entryResults.allSatisfy {
            $0.status == .completed || $0.status == .skippedIdentical || $0.status == .renamed
        })
        Self.emitCrossDeviceMarker(
            "manifest evidence id=\(completed.manifestId) roots=\(completed.rootCount) " +
            "files=\(completed.completedFiles)/\(completed.fileCount) " +
            "bytes=\(completed.activity.bytesTransferred) " +
            "path=\(completed.activity.dataPathKind):\(completed.activity.dataPathDetail)"
        )
#endif
    }

#if ENVOIX_CROSS_DEVICE_TESTING
    private func runCrossDeviceRoomSend(
        code: String,
        fileName: String,
        payload: Data,
        expectedBytes: UInt64,
        peerLabel: String
    ) async throws {
        print("[cross-device] iOS to \(peerLabel) send start code=\(code)")
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-ios-cross-device-send-\(UUID().uuidString)", isDirectory: true)
        try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: root) }

        let sendFile = root.appendingPathComponent(fileName)
        try Self.writeCrossDevicePayload(payload, expectedBytes: expectedBytes, to: sendFile)

        let observer = RecordingObserver()
        let session = try Self.startDurableCrossDeviceTransfer(
            request: Self.crossDeviceRequest(
                direction: .send,
                mode: .room,
                code: code,
                filePath: sendFile.path,
                outputDir: "",
                invite: ""
            ),
            recordsDirectory: root.appendingPathComponent("records", isDirectory: true),
            observer: observer
        )
        defer { _ = session.remove() }
        print("[cross-device] iOS send completed call returned")

        let pauseTask = Task {
            try await Self.pauseAndResumeIfRequested(
                session: session,
                observer: observer,
                expectedBytes: expectedBytes
            )
        }
        let bytes = try await observer.waitForCompletion(timeout: Self.crossDeviceTimeout(for: expectedBytes))
        try await pauseTask.value
        print("[cross-device] iOS to \(peerLabel) send completion bytes=\(bytes)")
        XCTAssertEqual(bytes, expectedBytes)
        observer.assertPathPolicy(Self.crossDevicePathPolicy)
    }
#endif

    func testCrossDeviceReceiveAndroidToIosInvite() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        Self.emitCrossDeviceMarker("iOS invite receive start")
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-ios-cross-device-invite-receive-\(UUID().uuidString)", isDirectory: true)
        let receiveDirectory = root.appendingPathComponent("received", isDirectory: true)
        try fileManager.createDirectory(at: receiveDirectory, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: root) }

        let observer = RecordingObserver()
        let session = try Self.startDurableCrossDeviceTransfer(
            request: Self.crossDeviceRequest(
                direction: .receive,
                mode: .showInvite,
                code: "",
                filePath: "",
                outputDir: receiveDirectory.path,
                invite: "",
                publicationRequired: true
            ),
            recordsDirectory: root.appendingPathComponent("records", isDirectory: true),
            observer: observer
        )
        defer { _ = session.remove() }

        let expectedBytes = Self.androidToIosExpectedBytes
        let invite = try await observer.waitForInvite(timeout: Self.crossDeviceTimeout(for: expectedBytes))
        Self.emitCrossDeviceMarker("iOS invite \(invite)")

        let published = try await Self.publishAndCompleteReceive(
            session: session,
            observer: observer,
            expectedFileName: Self.androidToIosFileName,
            payload: Self.androidToIosPayload,
            expectedBytes: expectedBytes
        )
        defer { try? fileManager.removeItem(at: published.url.deletingLastPathComponent()) }
        let bytes = published.bytes
        print("[cross-device] iOS invite receive completion bytes=\(bytes)")
        XCTAssertEqual(bytes, expectedBytes)
#endif
    }

    func testCrossDeviceSendIosToAndroidInvite() async throws {
        try requireCrossDeviceTesting()
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

        let observer = RecordingObserver()
        let session = try Self.startDurableCrossDeviceTransfer(
            request: Self.crossDeviceRequest(
                direction: .send,
                mode: .invite,
                code: "",
                filePath: sendFile.path,
                outputDir: "",
                invite: invite
            ),
            recordsDirectory: root.appendingPathComponent("records", isDirectory: true),
            observer: observer
        )
        defer { _ = session.remove() }

        let pauseTask = Task {
            try await Self.pauseAndResumeIfRequested(
                session: session,
                observer: observer,
                expectedBytes: expectedBytes
            )
        }
        let bytes = try await observer.waitForCompletion(timeout: Self.crossDeviceTimeout(for: expectedBytes))
        try await pauseTask.value
        print("[cross-device] iOS invite send completion bytes=\(bytes)")
        XCTAssertEqual(bytes, expectedBytes)
#endif
    }

    private func requireCrossDeviceTesting() throws {
#if !ENVOIX_CROSS_DEVICE_TESTING
        throw XCTSkip("Requires the explicit ENVOIX_CROSS_DEVICE_TESTING build and a paired peer")
#endif
    }

#if ENVOIX_CROSS_DEVICE_TESTING
    private static let defaultAndroidToIosCode = "741203-amber-comet"
    private static let defaultIosToAndroidCode = "741204-azure-river"
    private static let defaultIosToMacOSCode = "741205-silver-forest"
    private static let androidToIosCode = envString("ENVOIX_ANDROID_TO_IOS_CODE") ?? defaultAndroidToIosCode
    private static let iosToAndroidCode = envString("ENVOIX_IOS_TO_ANDROID_CODE") ?? defaultIosToAndroidCode
    private static let iosToMacOSCode = envString("ENVOIX_IOS_TO_MACOS_CODE") ?? defaultIosToMacOSCode
    private static let crossDeviceRunID: String = {
        let value = envString("ENVOIX_CROSS_DEVICE_RUN_ID") ?? "manual"
        guard value.count <= 80,
              value.range(of: "^[A-Za-z0-9_-]+$", options: .regularExpression) != nil else {
            fatalError("ENVOIX_CROSS_DEVICE_RUN_ID must contain only letters, digits, '-' or '_'")
        }
        return value
    }()
    private static let androidToIosFileName = "envoix-\(crossDeviceRunID)-android-to-ios.bin"
    private static let iosToAndroidFileName = "envoix-\(crossDeviceRunID)-ios-to-android.bin"
    private static let iosToMacOSFileName = "envoix-\(crossDeviceRunID)-ios-to-macos.bin"
    private static let iosToMacOSManifestAlbumName = "envoix-\(crossDeviceRunID)-album"
    private static let iosToMacOSManifestLooseName = "envoix-\(crossDeviceRunID)-loose.txt"
    private static let androidToIosPayload = Data("envoix cross-device android to ios\n".utf8)
    private static let iosToAndroidPayload = Data("envoix cross-device ios to android\n".utf8)
    private static let iosToMacOSPayload = Data("envoix cross-device ios to macos\n".utf8)
    private static let iosToMacOSManifestPhotoPayload = Data(
        "envoix manifest photo \(crossDeviceRunID)\n".utf8
    )
    private static let iosToMacOSManifestLoosePayload = Data(
        "envoix manifest loose file \(crossDeviceRunID)\n".utf8
    )
    private static let androidToIosExpectedBytes =
        envUInt64("ENVOIX_ANDROID_TO_IOS_BYTES") ?? UInt64(androidToIosPayload.count)
    private static let iosToAndroidExpectedBytes =
        envUInt64("ENVOIX_IOS_TO_ANDROID_BYTES") ?? UInt64(iosToAndroidPayload.count)
    private static let iosToMacOSExpectedBytes =
        envUInt64("ENVOIX_IOS_TO_MACOS_BYTES") ?? UInt64(iosToMacOSPayload.count)
    private static let pauseAfterBytes = envUInt64("ENVOIX_CROSS_DEVICE_PAUSE_AFTER_BYTES")
    private static let pauseDurationMilliseconds =
        envUInt64("ENVOIX_CROSS_DEVICE_PAUSE_DURATION_MS") ?? 2_000
    private static let crossDeviceTimeout: TimeInterval = 180
    private static let timeoutBytesPerSecond: UInt64 = 2 * 1024 * 1024
    private static let rendezvousBroker = "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445"
    private static let relayURL = "https://envoix.chkxwlyh.us:8444"
    private static let crossDevicePathPolicy: FfiPathPolicy = {
        switch envString("ENVOIX_CROSS_DEVICE_PATH_POLICY")?.lowercased() {
        case nil, "", "auto": return .auto
        case "direct", "direct-only": return .directOnly
        default: fatalError("ENVOIX_CROSS_DEVICE_PATH_POLICY must be auto or direct-only")
        }
    }()

    private static func crossDeviceTimeout(for expectedBytes: UInt64) -> TimeInterval {
        if let override = envDouble("ENVOIX_CROSS_DEVICE_TIMEOUT_SECONDS") {
            return override
        }
        let scaled = crossDeviceTimeout + TimeInterval(expectedBytes / timeoutBytesPerSecond)
        return max(crossDeviceTimeout, scaled)
    }

    private static func writeCrossDevicePayload(_ payload: Data, expectedBytes: UInt64, to url: URL) throws {
        guard !payload.isEmpty || expectedBytes == 0 else {
            throw LoopbackTestError.missingValue("non-empty deterministic payload")
        }
        _ = FileManager.default.createFile(atPath: url.path, contents: nil)
        let handle = try FileHandle(forWritingTo: url)
        defer { try? handle.close() }
        let block = repeatedPayloadBlock(payload)
        var remaining = expectedBytes
        while remaining > 0 {
            let count = Int(min(remaining, UInt64(block.count)))
            try handle.write(contentsOf: block.prefix(count))
            remaining -= UInt64(count)
        }
    }

    private static func assertReceivedFile(_ url: URL, payload: Data, expectedBytes: UInt64) throws {
        let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
        let actualBytes = try XCTUnwrap(attributes[.size] as? NSNumber).uint64Value
        XCTAssertEqual(actualBytes, expectedBytes)
        XCTAssertEqual(
            try fileSHA256(url),
            repeatedPayloadSHA256(payload, expectedBytes: expectedBytes),
            "received file SHA-256 does not match the deterministic payload"
        )
        if expectedBytes == UInt64(payload.count) {
            let receivedPayload = try Data(contentsOf: url)
            XCTAssertEqual(receivedPayload, payload)
        }
    }

    private static func publishAndCompleteReceive(
        session: DurableEnvoixSession,
        observer: RecordingObserver,
        expectedFileName: String,
        payload: Data,
        expectedBytes: UInt64
    ) async throws -> (url: URL, bytes: UInt64) {
        let timeout = crossDeviceTimeout(for: expectedBytes)
        let publishing = try await observer.waitForPublishing(timeout: timeout)
        XCTAssertEqual(publishing.fileName, expectedFileName)
        XCTAssertEqual(publishing.bytesTransferred, expectedBytes)
        let staged = URL(fileURLWithPath: publishing.completedFilePath)
        try assertReceivedFile(staged, payload: payload, expectedBytes: expectedBytes)

        guard let documents = FileManager.default.urls(
            for: .documentDirectory,
            in: .userDomainMask
        ).first else {
            throw LoopbackTestError.missingValue("iOS Documents directory")
        }
        let destination = documents.appendingPathComponent(
            "EnvoixCrossDeviceTests-\(crossDeviceRunID)",
            isDirectory: true
        )
        try? FileManager.default.removeItem(at: destination)
        let finalURL = try publishReceivedFile(
            from: staged,
            to: destination,
            expectedBytes: expectedBytes
        )
        XCTAssertTrue(FileManager.default.fileExists(atPath: staged.path))
        XCTAssertTrue(
            session.publicationSucceeded(path: finalURL.path),
            "canonical core rejected publication success"
        )

        let bytes = try await observer.waitForCompletion(timeout: timeout)
        let completed = session.activity()
        XCTAssertEqual(completed.state, .completed)
        XCTAssertEqual(completed.completedFilePath, finalURL.path)
        try assertReceivedFile(finalURL, payload: payload, expectedBytes: expectedBytes)
        try FileManager.default.removeItem(at: staged)

        let hash = try fileSHA256(finalURL).map { String(format: "%02x", $0) }.joined()
        emitCrossDeviceMarker(
            "evidence path=\(finalURL.path) size=\(expectedBytes) sha256=\(hash)"
        )
        return (finalURL, bytes)
    }

    private static func pauseAndResumeIfRequested(
        session: DurableEnvoixSession,
        observer: RecordingObserver,
        expectedBytes: UInt64
    ) async throws {
        guard let pauseAfterBytes else { return }
        guard pauseAfterBytes > 0, pauseAfterBytes < expectedBytes else {
            throw LoopbackTestError.missingValue(
                "ENVOIX_CROSS_DEVICE_PAUSE_AFTER_BYTES must be between 1 and expectedBytes - 1"
            )
        }
        let timeout = crossDeviceTimeout(for: expectedBytes)
        try await observer.waitForProgress(atLeast: pauseAfterBytes, timeout: timeout)
        XCTAssertTrue(session.pause(), "canonical pause request was rejected")
        try await observer.waitForPaused(timeout: timeout)
        try await Task.sleep(
            nanoseconds: pauseDurationMilliseconds * 1_000_000
        )
        XCTAssertTrue(session.resume(), "canonical resume request was rejected")
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
        let block = repeatedPayloadBlock(payload)
        var remaining = expectedBytes
        while remaining > 0 {
            let count = Int(min(remaining, UInt64(block.count)))
            hasher.update(data: block.prefix(count))
            remaining -= UInt64(count)
        }
        return Data(hasher.finalize())
    }

    private static func repeatedPayloadBlock(_ payload: Data) -> Data {
        guard !payload.isEmpty else { return Data() }
        let repeats = max(1, hashBlockBytes / payload.count)
        var block = Data(capacity: repeats * payload.count)
        for _ in 0..<repeats {
            block.append(payload)
        }
        return block
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

    private static func emitCrossDeviceMarker(_ message: String) {
        FileHandle.standardError.write(Data("[cross-device] \(message)\n".utf8))
    }

    private static let hashBlockBytes = 1024 * 1024

    private static func crossDeviceSettings() -> EnvoixRuntimeSettings {
        EnvoixRuntimeSettings(
            concurrentTransfers: true,
            language: "en",
            serverUrl: rendezvousBroker,
            relayUrl: relayURL,
            configPath: "",
            speedLimitMbps: 40
        )
    }

    private static func startDurableCrossDeviceTransfer(
        request: FfiTransferRequest,
        recordsDirectory: URL,
        observer: RecordingObserver
    ) throws -> DurableEnvoixSession {
        try startDurableTransfer(
            settings: crossDeviceSettings(),
            request: request,
            recordsDir: recordsDirectory.path,
            observer: observer,
            mailbox: NoopTestMailboxObserver()
        )
    }

    private static func crossDeviceRequest(
        activityID: String = "ios-\(UUID().uuidString)",
        direction: FfiTransferDirection,
        mode: FfiTransferMode,
        code: String,
        filePath: String,
        outputDir: String,
        invite: String,
        publicationRequired: Bool = false
    ) -> FfiTransferRequest {
        FfiTransferRequest(
            activityId: activityID,
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
            pathPolicy: crossDevicePathPolicy,
            resume: true,
            publicationRequired: publicationRequired,
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

    private static func waitForManifestCompletion(
        session: DurableEnvoixManifestSession,
        timeout: TimeInterval
    ) async throws -> FfiManifestActivityRecord {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let record = session.activity()
            switch record.activity.state {
            case .completed:
                return record
            case .failed, .canceled:
                throw LoopbackTestError.transferFailed(record.activity.diagnosticMessage)
            case .queued, .binding, .waitingForPeer, .pairing, .connecting,
                    .transferring, .verifying, .publishing, .unconfirmed,
                    .paused, .unknown:
                break
            }
            try await Task.sleep(nanoseconds: 200_000_000)
        }
        throw LoopbackTestError.timeout("iOS Manifest completion")
    }
#endif
}

private final class ManifestEvidenceObserver: ManifestTransferObserver, @unchecked Sendable {
    private let peerLabel: String

    init(peerLabel: String) {
        self.peerLabel = peerLabel
    }

    func onManifestActivity(record: FfiManifestActivityRecord) {
        print(
            "[cross-device] iOS to \(peerLabel) Manifest state=\(record.activity.state) " +
            "files=\(record.completedFiles)/\(record.fileCount) " +
            "bytes=\(record.activity.bytesTransferred)/\(record.activity.totalBytes)"
        )
    }
}

private final class NoopTestMailboxObserver: MailboxObserver, @unchecked Sendable {
    func onFetchReceipt(activityId: String, key: String) {}

    func onPostReceipt(activityId: String, key: String, blob: Data) {}
}

private final class RecordingObserver: TransferObserver, @unchecked Sendable {
    private let lock = NSLock()
    private let inviteSemaphore = DispatchSemaphore(value: 0)
    private let publishingSemaphore = DispatchSemaphore(value: 0)
    private let pausedSemaphore = DispatchSemaphore(value: 0)
    private let terminalSemaphore = DispatchSemaphore(value: 0)
    private let onRoomReady: () -> Void

    private var invite: String?
    private var publishingRecord: FfiTransferActivityRecord?
    private var latestProgress: UInt64 = 0
    private var pausedObserved = false
    private var completedBytes: UInt64?
    private var failure: String?
    private var roomReady = false
    private var pathKinds: [FfiDataPathKind] = []

    init(onRoomReady: @escaping () -> Void = {}) {
        self.onRoomReady = onRoomReady
    }

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
        locked {
            latestProgress = max(latestProgress, transferred)
        }
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
        if event.dataPathKind != .none {
            locked { pathKinds.append(event.dataPathKind) }
        }
        let shouldReportRoomReady = locked {
            guard !roomReady, event.kind == .pairing, event.pairingStep == .joining else {
                return false
            }
            roomReady = true
            return true
        }
        if shouldReportRoomReady {
            onRoomReady()
        }
        print(
            "[cross-device] onTransferEvent kind=\(event.kind) mode=\(event.mode) direction=\(event.direction) " +
            "pairing=\(event.pairingStep) path=\(event.dataPathKind):\(event.dataPathDetail) " +
            "bytes=\(event.bytesTransferred)/\(event.totalBytes) token=\(Self.tokenLabel(event.token)) " +
            "peerLen=\(event.peerDescriptor.count)"
        )
    }

    func onTransferActivity(record: FfiTransferActivityRecord) {
        let signals = locked {
            var publishing = false
            var paused = false
            guard record.state == .publishing, publishingRecord == nil else {
                if record.state == .paused, !pausedObserved {
                    pausedObserved = true
                    paused = true
                }
                return (publishing, paused)
            }
            publishingRecord = record
            publishing = true
            return (publishing, paused)
        }
        if signals.0 {
            publishingSemaphore.signal()
        }
        if signals.1 {
            pausedSemaphore.signal()
        }
        print("[cross-device] onTransferActivity \(record)")
    }

    func onStatus(message: String) {
        if !message.isEmpty {
            print("[cross-device] status \(message)")
        }
    }

    func waitForInvite(timeout: TimeInterval) async throws -> String {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .utility).async { [self] in
                guard inviteSemaphore.wait(timeout: .now() + timeout) == .success else {
                    continuation.resume(throwing: LoopbackTestError.timeout("invite"))
                    return
                }
                do {
                    let value = try locked {
                        guard let invite else {
                            throw LoopbackTestError.missingValue("invite")
                        }
                        return invite
                    }
                    continuation.resume(returning: value)
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    func waitForCompletion(timeout: TimeInterval) async throws -> UInt64 {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .utility).async { [self] in
                guard terminalSemaphore.wait(timeout: .now() + timeout) == .success else {
                    continuation.resume(throwing: LoopbackTestError.timeout("completion"))
                    return
                }
                do {
                    let value = try locked {
                        if let failure {
                            throw LoopbackTestError.transferFailed(failure)
                        }
                        guard let completedBytes else {
                            throw LoopbackTestError.missingValue("completed bytes")
                        }
                        return completedBytes
                    }
                    continuation.resume(returning: value)
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    func waitForPublishing(timeout: TimeInterval) async throws -> FfiTransferActivityRecord {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .utility).async { [self] in
                guard publishingSemaphore.wait(timeout: .now() + timeout) == .success else {
                    continuation.resume(throwing: LoopbackTestError.timeout("publication"))
                    return
                }
                do {
                    let value = try locked {
                        guard let publishingRecord else {
                            throw LoopbackTestError.missingValue("publishing activity")
                        }
                        return publishingRecord
                    }
                    continuation.resume(returning: value)
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    func waitForProgress(atLeast bytes: UInt64, timeout: TimeInterval) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let snapshot = locked { (latestProgress, failure, completedBytes) }
            if let failure = snapshot.1 {
                throw LoopbackTestError.transferFailed(failure)
            }
            if snapshot.0 >= bytes {
                return
            }
            if snapshot.2 != nil {
                throw LoopbackTestError.missingValue(
                    "transfer completed before pause threshold; progress=\(snapshot.0)"
                )
            }
            try await Task.sleep(nanoseconds: 25_000_000)
        }
        throw LoopbackTestError.timeout("pause threshold")
    }

    func waitForPaused(timeout: TimeInterval) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            DispatchQueue.global(qos: .utility).async { [self] in
                guard pausedSemaphore.wait(timeout: .now() + timeout) == .success else {
                    continuation.resume(throwing: LoopbackTestError.timeout("Paused snapshot"))
                    return
                }
                continuation.resume()
            }
        }
    }

    func assertPathPolicy(_ policy: FfiPathPolicy) {
        let paths = locked { pathKinds }
        XCTAssertFalse(paths.isEmpty, "transfer did not report a selected data path")
        guard policy == .directOnly else { return }
        XCTAssertTrue(paths.contains(.direct), "direct-only transfer did not report a direct path: \(paths)")
        XCTAssertFalse(paths.contains(.relay), "direct-only transfer reported a relay path: \(paths)")
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
