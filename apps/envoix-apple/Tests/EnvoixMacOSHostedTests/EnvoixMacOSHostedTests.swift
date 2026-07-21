import CryptoKit
import EnvoixCore
import XCTest
@testable import Envoix

@MainActor
final class EnvoixMacOSHostedTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    func testDiagnosticsIdentifyMacOSApp() {
        let record = Self.manifestRecord(
            completedRoot: URL(fileURLWithPath: "/tmp/envoix-diagnostics"),
            rootCount: 1,
            entries: [Self.manifestEntry(id: 0, path: "diagnostics.txt")]
        ).activity
        let report = TransferDiagnostics.report(for: record)
        let appReport = TransferDiagnostics.appReport(activities: [])

        XCTAssertTrue(report.hasPrefix("[header]\napp=envoix-macos\n"))
        XCTAssertTrue(appReport.hasPrefix("[header]\napp=envoix-macos\n"))
        XCTAssertTrue(report.contains("executable_sha256="))
        XCTAssertTrue(report.contains("runtime_code_sha256="))
        if let executableURL = Bundle.main.executableURL {
            let debugDylibName = "\(executableURL.lastPathComponent).debug.dylib"
            let debugDylibURL = executableURL
                .deletingLastPathComponent()
                .appendingPathComponent(debugDylibName)
            if FileManager.default.isReadableFile(atPath: debugDylibURL.path) {
                XCTAssertTrue(report.contains("runtime_code_file=\(debugDylibName)"))
            }
        }
    }

    func testSendSelectionUsesManifestOnlyForFoldersOrMultipleItems() throws {
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-send-selection-\(UUID().uuidString)", isDirectory: true)
        defer { try? fileManager.removeItem(at: root) }
        try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        let first = root.appendingPathComponent("first.bin")
        let second = root.appendingPathComponent("second.bin")
        try Data("first".utf8).write(to: first)
        try Data("second".utf8).write(to: second)

        XCTAssertFalse(sendSelectionRequiresManifest([]))
        XCTAssertFalse(sendSelectionRequiresManifest([first]))
        XCTAssertTrue(sendSelectionRequiresManifest([root]))
        XCTAssertTrue(sendSelectionRequiresManifest([first, second]))
    }

    func testWritableDirectoryProbeLeavesNoArtifacts() throws {
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-write-probe-\(UUID().uuidString)", isDirectory: true)
        defer { try? fileManager.removeItem(at: root) }

        try validateWritableDirectoryAccess(root, fileManager: fileManager)

        XCTAssertTrue(fileManager.fileExists(atPath: root.path))
        XCTAssertEqual(try fileManager.contentsOfDirectory(atPath: root.path), [])
    }

    func testWritableDirectoryProbeRejectsFilePath() throws {
        let fileManager = FileManager.default
        let file = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-write-probe-file-\(UUID().uuidString)")
        defer { try? fileManager.removeItem(at: file) }
        try Data("not a directory".utf8).write(to: file)

        XCTAssertThrowsError(try validateWritableDirectoryAccess(file, fileManager: fileManager))
    }

    func testInvalidDestinationBookmarkDoesNotFallBackToLegacyPath() {
        let fallback = URL(fileURLWithPath: "/tmp/envoix-legacy-downloads", isDirectory: true)

        XCTAssertNil(resolveRememberedOutputDirectory(
            bookmarkData: Data([0x00, 0x01]),
            legacyPath: fallback.path,
            defaultURL: fallback
        ))
    }

    func testDestinationBookmarkRoundTripsSelectedFolder() throws {
        let fileManager = FileManager.default
        let directory = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-bookmark-\(UUID().uuidString)", isDirectory: true)
        defer { try? fileManager.removeItem(at: directory) }
        try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)

        let bookmark = try makeSecurityScopedFolderBookmark(for: directory)
        let resolved = try resolveSecurityScopedFolderBookmark(bookmark)

        XCTAssertEqual(resolved.standardizedFileURL, directory.standardizedFileURL)
    }

    func testLegacyDestinationPathIsUsedOnlyWhenNoBookmarkExists() {
        let legacy = URL(fileURLWithPath: "/tmp/envoix-legacy-downloads", isDirectory: true)
        let defaultURL = URL(fileURLWithPath: "/tmp/envoix-default-downloads", isDirectory: true)

        XCTAssertEqual(
            resolveRememberedOutputDirectory(
                bookmarkData: nil,
                legacyPath: legacy.path,
                defaultURL: defaultURL
            )?.standardizedFileURL,
            legacy.standardizedFileURL
        )
        XCTAssertEqual(
            resolveRememberedOutputDirectory(
                bookmarkData: nil,
                legacyPath: "",
                defaultURL: defaultURL
            )?.standardizedFileURL,
            defaultURL.standardizedFileURL
        )
    }

    func testRateTrackerWaitsForRealByteDeltas() {
        let mebibyte = UInt64(1024 * 1024)
        var tracker = RateTracker()

        XCTAssertEqual(tracker.record(142 * mebibyte, at: 10), 0)
        XCTAssertEqual(tracker.record(152 * mebibyte, at: 10.4), 0)
        XCTAssertEqual(tracker.record(162 * mebibyte, at: 11), Double(20 * mebibyte), accuracy: 1)
        XCTAssertTrue(tracker.isStable)

        XCTAssertEqual(tracker.record(5 * mebibyte, at: 12), 0)
        XCTAssertFalse(tracker.isStable)
    }

    func testEstimatedRemainingTimeRequiresStableFiniteRate() {
        XCTAssertNil(estimatedRemainingSeconds(
            total: 1_000,
            transferred: 400,
            bytesPerSecond: 100,
            isStable: false
        ))
        XCTAssertNil(estimatedRemainingSeconds(
            total: 1_000,
            transferred: 400,
            bytesPerSecond: .infinity,
            isStable: true
        ))
        XCTAssertEqual(estimatedRemainingSeconds(
            total: 1_000,
            transferred: 400,
            bytesPerSecond: 100,
            isStable: true
        ), 6)
    }

    func testDirectReceiveRemovesReceiptWhenDestinationFileWasDeleted() throws {
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-direct-receipt-\(UUID().uuidString)", isDirectory: true)
        defer { try? fileManager.removeItem(at: root) }
        try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        let writeReceipt = { (fileName: String) throws -> URL in
            let url = root.appendingPathComponent(".envoix-receipt.\(fileName).json")
            let receipt = """
            {
              "transfer_id": "transfer-test",
              "file_name": "\(fileName)",
              "file_size": 7,
              "file_hash": "test-hash"
            }
            """
            try Data(receipt.utf8).write(to: url)
            return url
        }

        let presentFile = root.appendingPathComponent("present.jpeg")
        try Data("present".utf8).write(to: presentFile)
        let presentReceipt = try writeReceipt("present.jpeg")
        let missingReceipt = try writeReceipt("missing.jpeg")

        try removeOrphanedDirectReceiveReceipts(in: root, fileManager: fileManager)

        XCTAssertTrue(fileManager.fileExists(atPath: presentReceipt.path))
        XCTAssertTrue(fileManager.fileExists(atPath: presentFile.path))
        XCTAssertFalse(fileManager.fileExists(atPath: missingReceipt.path))
    }

    func testFullyResumedCompletionRequiresAllBytesAlreadyPresent() {
        var record = Self.manifestRecord(
            completedRoot: URL(fileURLWithPath: "/tmp/envoix-existing-file"),
            rootCount: 1,
            entries: [Self.manifestEntry(id: 0, path: "existing.bin")]
        ).activity
        record.totalBytes = 353_224
        record.bytesTransferred = 353_224
        record.bytesResumed = 353_224
        record.state = .completed

        XCTAssertTrue(isFullyResumedCompletion(record))

        record.bytesResumed = 128_000
        XCTAssertFalse(isFullyResumedCompletion(record))

        record.bytesResumed = 353_224
        record.state = .transferring
        XCTAssertFalse(isFullyResumedCompletion(record))
    }

    func testDurableCoreReportsExistingFileAsFullyResumed() async throws {
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-existing-core-\(UUID().uuidString)", isDirectory: true)
        defer { try? fileManager.removeItem(at: root) }
        let outputDirectory = root.appendingPathComponent("received", isDirectory: true)
        let receiveRecords = root.appendingPathComponent("receive-records", isDirectory: true)
        let sendRecords = root.appendingPathComponent("send-records", isDirectory: true)
        try fileManager.createDirectory(at: outputDirectory, withIntermediateDirectories: true)
        let source = root.appendingPathComponent("existing.txt")
        let payload = Data("durable Apple existing-file accounting".utf8)
        try payload.write(to: source)

        _ = try await Self.runDurableInviteTransfer(
            activitySuffix: "seed",
            source: source,
            outputDirectory: outputDirectory,
            receiveRecords: receiveRecords,
            sendRecords: sendRecords
        )
        let repeated = try await Self.runDurableInviteTransfer(
            activitySuffix: "repeat",
            source: source,
            outputDirectory: outputDirectory,
            receiveRecords: receiveRecords,
            sendRecords: sendRecords
        )

        XCTAssertEqual(repeated.state, .completed)
        XCTAssertEqual(repeated.bytesTransferred, UInt64(payload.count))
        XCTAssertEqual(repeated.bytesResumed, UInt64(payload.count))
    }

    private static func runDurableInviteTransfer(
        activitySuffix: String,
        source: URL,
        outputDirectory: URL,
        receiveRecords: URL,
        sendRecords: URL
    ) async throws -> FfiTransferActivityRecord {
        let settings = EnvoixRuntimeSettings(
            concurrentTransfers: true,
            language: "en",
            serverUrl: "",
            relayUrl: "",
            configPath: "",
            speedLimitMbps: 0
        )
        let receiverObserver = HostedCoreLoopbackObserver()
        let receiver = try startDurableManifestReceiveV2(
            settings: settings,
            request: loopbackRequest(
                activityID: "existing-receive-\(activitySuffix)",
                direction: .receive,
                mode: .showInvite,
                filePath: "",
                outputDirectory: outputDirectory.path,
                invite: ""
            ),
            recordsDir: receiveRecords.path,
            observer: receiverObserver
        )
        let invite = try await receiverObserver.waitForInvite(timeout: 10)
        try await Task.sleep(nanoseconds: 300_000_000)

        let senderObserver = HostedCoreLoopbackObserver()
        let sender = try startDurableTransferV2(
            settings: settings,
            request: loopbackRequest(
                activityID: "existing-send-\(activitySuffix)",
                direction: .send,
                mode: .invite,
                filePath: source.path,
                outputDirectory: "",
                invite: invite
            ),
            recordsDir: sendRecords.path,
            receiptServer: "https://receipt.example.test",
            observer: senderObserver,
            mailbox: HostedCoreNoopMailbox()
        )
        _ = try await senderObserver.waitForCompletion(timeout: 20)
        _ = try await receiverObserver.waitForCompletion(timeout: 20)
        _ = sender
        return receiver.activity().activity
    }

    private static func loopbackRequest(
        activityID: String,
        direction: FfiTransferDirection,
        mode: FfiTransferMode,
        filePath: String,
        outputDirectory: String,
        invite: String
    ) -> FfiTransferRequest {
        FfiTransferRequest(
            activityId: activityID,
            direction: direction,
            mode: mode,
            filePath: filePath,
            outputDir: outputDirectory,
            peerDescriptor: "",
            invite: invite,
            code: "",
            token: "",
            broker: "",
            relay: "",
            configPath: "",
            pathPolicy: .directOnly,
            resume: true,
            publicationRequired: false,
            limits: FfiTransferLimits(
                maxParallelTransfers: 1,
                maxParallelFiles: 1,
                maxParallelChunksPerFile: 1,
                speedLimitBps: 0
            ),
            rendezvous: FfiRendezvousPlan(
                useRoom: false,
                useMdns: false,
                internetAvailable: true
            )
        )
    }

    func testManifestDisplayListsOnlyTopLevelSelectionRoots() {
        let record = Self.manifestRecord(
            completedRoot: URL(fileURLWithPath: "/tmp/envoix-display"),
            rootCount: 2,
            entries: [
                Self.manifestEntry(id: 0, path: "Album", kind: .directory),
                Self.manifestEntry(id: 1, path: "Album/photo.jpg"),
                Self.manifestEntry(id: 2, path: "notes.txt"),
            ]
        )

        XCTAssertEqual(
            manifestRootEntriesForDisplay(record).map(\.relativePath),
            ["Album", "notes.txt"]
        )
    }

    func testMergedActivityDiagnosticLogKeepsManifestTimelineWithoutObserverEvents() {
        XCTAssertEqual(
            mergedActivityDiagnosticLog(
                activityTimeline: ["[10:44:15] pairing", "[10:44:45] completed"],
                observerLog: []
            ),
            ["[10:44:15] pairing", "[10:44:45] completed"]
        )
        XCTAssertEqual(
            mergedActivityDiagnosticLog(
                activityTimeline: ["[10:44:15] pairing"],
                observerLog: ["[10:44:16] status · connecting"]
            ),
            ["[10:44:15] pairing", "[10:44:16] status · connecting"]
        )
    }

    func testManifestPublicationCopiesFilesDirectoriesAndSupportsRetry() throws {
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-manifest-publish-\(UUID().uuidString)", isDirectory: true)
        let staging = root.appendingPathComponent("staging", isDirectory: true)
        let album = staging.appendingPathComponent("Album", isDirectory: true)
        let emptyDirectory = album.appendingPathComponent("Empty", isDirectory: true)
        let destination = root.appendingPathComponent("destination", isDirectory: true)
        defer { try? fileManager.removeItem(at: root) }

        try fileManager.createDirectory(at: emptyDirectory, withIntermediateDirectories: true)
        let photo = Data("photo".utf8)
        let caption = Data("hidden caption".utf8)
        let loose = Data("loose file".utf8)
        try photo.write(to: album.appendingPathComponent("photo.jpg"))
        try caption.write(to: album.appendingPathComponent(".caption"))
        try loose.write(to: staging.appendingPathComponent("loose.bin"))

        let record = Self.manifestRecord(
            completedRoot: staging,
            rootCount: 2,
            entries: [
                Self.manifestEntry(id: 0, path: "Album", kind: .directory),
                Self.manifestEntry(id: 1, path: "Album/Empty", kind: .directory),
                Self.manifestEntry(id: 2, path: "Album/.caption", size: UInt64(caption.count)),
                Self.manifestEntry(id: 3, path: "Album/photo.jpg", size: UInt64(photo.count)),
                Self.manifestEntry(id: 4, path: "loose.bin", size: UInt64(loose.count)),
            ]
        )

        let published = try publishReceivedManifest(from: staging, to: destination, record: record)
        XCTAssertEqual(published, destination)
        XCTAssertEqual(
            try Data(contentsOf: destination.appendingPathComponent("Album/photo.jpg")),
            photo
        )
        XCTAssertEqual(
            try Data(contentsOf: destination.appendingPathComponent("Album/.caption")),
            caption
        )
        XCTAssertEqual(try Data(contentsOf: destination.appendingPathComponent("loose.bin")), loose)
        var isDirectory: ObjCBool = false
        XCTAssertTrue(
            fileManager.fileExists(
                atPath: destination.appendingPathComponent("Album/Empty").path,
                isDirectory: &isDirectory
            )
        )
        XCTAssertTrue(isDirectory.boolValue)

        XCTAssertEqual(
            try publishReceivedManifest(from: staging, to: destination, record: record),
            destination
        )
        XCTAssertTrue(
            try fileManager.contentsOfDirectory(atPath: destination.path)
                .allSatisfy { !$0.hasPrefix(".envoix-publish-") }
        )
    }

    func testManifestPublicationPreflightsAllRootConflicts() throws {
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-manifest-conflict-\(UUID().uuidString)", isDirectory: true)
        let staging = root.appendingPathComponent("staging", isDirectory: true)
        let destination = root.appendingPathComponent("destination", isDirectory: true)
        defer { try? fileManager.removeItem(at: root) }
        try fileManager.createDirectory(at: staging, withIntermediateDirectories: true)
        try fileManager.createDirectory(at: destination, withIntermediateDirectories: true)

        let first = Data("first".utf8)
        let second = Data("second".utf8)
        try first.write(to: staging.appendingPathComponent("first.bin"))
        try second.write(to: staging.appendingPathComponent("second.bin"))
        try Data("other!".utf8).write(to: destination.appendingPathComponent("second.bin"))
        let record = Self.manifestRecord(
            completedRoot: staging,
            rootCount: 2,
            entries: [
                Self.manifestEntry(id: 0, path: "first.bin", size: UInt64(first.count)),
                Self.manifestEntry(id: 1, path: "second.bin", size: UInt64(second.count)),
            ]
        )

        XCTAssertThrowsError(
            try publishReceivedManifest(from: staging, to: destination, record: record)
        )
        XCTAssertFalse(fileManager.fileExists(atPath: destination.appendingPathComponent("first.bin").path))
        XCTAssertEqual(
            try Data(contentsOf: destination.appendingPathComponent("second.bin")),
            Data("other!".utf8)
        )
    }

    func testManifestPublicationRejectsUnsafeTopLevelResult() throws {
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-manifest-unsafe-\(UUID().uuidString)", isDirectory: true)
        let staging = root.appendingPathComponent("staging", isDirectory: true)
        let destination = root.appendingPathComponent("destination", isDirectory: true)
        defer { try? fileManager.removeItem(at: root) }
        try fileManager.createDirectory(at: staging, withIntermediateDirectories: true)
        let payload = Data("payload".utf8)
        try payload.write(to: staging.appendingPathComponent("safe.bin"))
        var record = Self.manifestRecord(
            completedRoot: staging,
            rootCount: 1,
            entries: [Self.manifestEntry(id: 0, path: "safe.bin", size: UInt64(payload.count))]
        )
        record.entryResults[0].finalRelativePath = ".."

        XCTAssertThrowsError(
            try publishReceivedManifest(from: staging, to: destination, record: record)
        )
        XCTAssertFalse(fileManager.fileExists(atPath: destination.path))
    }

    func testCompletedSingleRootManifestResolvesThePublishedItem() throws {
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-manifest-completed-\(UUID().uuidString)", isDirectory: true)
        defer { try? fileManager.removeItem(at: root) }
        try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        let payload = Data("published".utf8)
        let file = root.appendingPathComponent("published.bin")
        try payload.write(to: file)
        var record = Self.manifestRecord(
            completedRoot: root,
            rootCount: 1,
            entries: [Self.manifestEntry(id: 0, path: "published.bin", size: UInt64(payload.count))]
        )
        record.activity.state = .completed

        XCTAssertEqual(availableCompletedManifestURL(record: record), file)
    }

    func testCompletedMultiRootManifestResolvesTheDestinationDirectory() throws {
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-manifest-multi-completed-\(UUID().uuidString)", isDirectory: true)
        defer { try? fileManager.removeItem(at: root) }
        try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        let first = Data("first".utf8)
        let second = Data("second".utf8)
        try first.write(to: root.appendingPathComponent("first.bin"))
        try second.write(to: root.appendingPathComponent("second.bin"))
        var record = Self.manifestRecord(
            completedRoot: root,
            rootCount: 2,
            entries: [
                Self.manifestEntry(id: 0, path: "first.bin", size: UInt64(first.count)),
                Self.manifestEntry(id: 1, path: "second.bin", size: UInt64(second.count)),
            ]
        )
        record.activity.state = .completed

        XCTAssertEqual(availableCompletedManifestURL(record: record), root)
        XCTAssertEqual(
            availableCompletedManifestItemURLs(record: record),
            [
                root.appendingPathComponent("first.bin"),
                root.appendingPathComponent("second.bin"),
            ]
        )
    }

    func testCompletedManifestItemURLsUsePublishedRenameAndIgnoreMissingItems() throws {
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-manifest-renamed-completed-\(UUID().uuidString)", isDirectory: true)
        defer { try? fileManager.removeItem(at: root) }
        try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        let payload = Data("renamed".utf8)
        let renamed = root.appendingPathComponent("photo (1).jpeg")
        try payload.write(to: renamed)
        var record = Self.manifestRecord(
            completedRoot: root,
            rootCount: 2,
            entries: [
                Self.manifestEntry(id: 0, path: "photo.jpeg", size: UInt64(payload.count)),
                Self.manifestEntry(id: 1, path: "missing.jpeg", size: 5),
            ]
        )
        record.activity.state = .completed
        record.entryResults[0].status = .renamed
        record.entryResults[0].finalRelativePath = renamed.lastPathComponent

        XCTAssertEqual(availableCompletedManifestItemURLs(record: record), [renamed])
    }

    func testReceivedDirectoryItemsSupportSafeFolderNavigation() throws {
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-received-folder-\(UUID().uuidString)", isDirectory: true)
        defer { try? fileManager.removeItem(at: root) }
        try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        let folder = root.appendingPathComponent("Folder", isDirectory: true)
        let file = root.appendingPathComponent("photo.jpeg")
        try fileManager.createDirectory(at: folder, withIntermediateDirectories: true)
        try Data("photo".utf8).write(to: file)
        try Data("metadata".utf8).write(to: root.appendingPathComponent(".envoix-receipt.json"))
        try fileManager.createSymbolicLink(
            at: root.appendingPathComponent("folder-link"),
            withDestinationURL: folder
        )

        XCTAssertEqual(
            availableReceivedDirectoryItemURLs(directory: root).map(\.lastPathComponent),
            [folder, file].map(\.lastPathComponent)
        )
        XCTAssertEqual(availableReceivedDirectoryItemURLs(directory: file), [])
    }

    func testReceiveIosToMacOSAppRoom() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        try await receiveSingleFile(
            fileName: Self.expectedFileName,
            payload: Self.payload,
            expectedBytes: Self.expectedBytes,
            evidenceLabel: "receiver"
        )
#endif
    }

    func testReceiveIosPhotoDraftToMacOSAppRoom() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        try await receiveSingleFile(
            fileName: Self.photoFileName,
            payload: Self.photoPayload,
            expectedBytes: UInt64(Self.photoPayload.count),
            evidenceLabel: "photo-receiver"
        )
#endif
    }

    func testReceiveIosOpenInToMacOSAppRoom() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        try await receiveSingleFile(
            fileName: Self.openInFileName,
            payload: Self.openInPayload,
            expectedBytes: UInt64(Self.openInPayload.count),
            evidenceLabel: "open-in-receiver"
        )
#endif
    }

    func testReceiveIosMultiPhotoDraftToMacOSAppManifestRoom() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        let outputDirectory = outputDirectory()
        let model = AppModel.shared
        let existingActivityIDs = Set(model.activities.map(\.activityId))
        try? FileManager.default.removeItem(at: outputDirectory)
        try FileManager.default.createDirectory(at: outputDirectory, withIntermediateDirectories: true)

        model.receive.startReceivingWithRoom(
            outputDir: outputDirectory.path,
            code: Self.roomCode,
            settings: Self.runtimeSettings
        )
        let activityID = try await waitForNewReceiveActivity(
            in: model,
            excluding: existingActivityIDs
        )
        defer { model.removeActivity(activityID) }
        emitEvidence("multi-photo-manifest-receiver-ready activity=\(activityID) room=\(Self.roomCode)")

        let manifest = try await waitForManifestCompletion(activityID: activityID, in: model)
        let activity = manifest.activity
        let expectedBytes = UInt64(Self.photoPayload.count * 2)
        XCTAssertEqual(activity.direction, .receive)
        XCTAssertEqual(activity.state, .completed)
        XCTAssertEqual(activity.bytesTransferred, expectedBytes)
        XCTAssertEqual(activity.totalBytes, expectedBytes)
        XCTAssertNotEqual(activity.dataPathKind, .none)
        XCTAssertEqual(URL(fileURLWithPath: activity.completedFilePath), outputDirectory)
        XCTAssertEqual(manifest.rootCount, 2)
        XCTAssertEqual(manifest.fileCount, 2)
        XCTAssertEqual(manifest.directoryCount, 0)
        XCTAssertEqual(manifest.completedFiles, 2)
        XCTAssertTrue(manifest.entryResults.allSatisfy {
            $0.status == .completed || $0.status == .skippedIdentical || $0.status == .renamed
        })

        let first = outputDirectory.appendingPathComponent(Self.multiPhotoFirstName)
        let second = outputDirectory.appendingPathComponent(Self.multiPhotoSecondName)
        XCTAssertEqual(try Data(contentsOf: first), Self.photoPayload)
        XCTAssertEqual(try Data(contentsOf: second), Self.photoPayload)
        let firstHash = try Self.fileSHA256(first)
        let secondHash = try Self.fileSHA256(second)
        let expectedHash = Data(SHA256.hash(data: Self.photoPayload))
        XCTAssertEqual(firstHash, expectedHash)
        XCTAssertEqual(secondHash, expectedHash)
        emitEvidence(
            "multi-photo-manifest-completed activity=\(activityID) " +
            "pathKind=\(activity.dataPathKind) pathDetail=\(activity.dataPathDetail) " +
            "root=\(outputDirectory.path) roots=\(manifest.rootCount) " +
            "files=\(manifest.completedFiles)/\(manifest.fileCount) bytes=\(activity.bytesTransferred) " +
            "eachSha256=\(firstHash.hexString)"
        )
#endif
    }

    func testReceiveIosFolderPickerToMacOSAppManifestRoom() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        let outputDirectory = outputDirectory()
        let model = AppModel.shared
        let existingActivityIDs = Set(model.activities.map(\.activityId))
        try? FileManager.default.removeItem(at: outputDirectory)
        try FileManager.default.createDirectory(at: outputDirectory, withIntermediateDirectories: true)

        model.receive.startReceivingWithRoom(
            outputDir: outputDirectory.path,
            code: Self.roomCode,
            settings: Self.runtimeSettings
        )
        let activityID = try await waitForNewReceiveActivity(
            in: model,
            excluding: existingActivityIDs
        )
        defer { model.removeActivity(activityID) }
        emitEvidence("folder-picker-manifest-receiver-ready activity=\(activityID) room=\(Self.roomCode)")

        let manifest = try await waitForManifestCompletion(activityID: activityID, in: model)
        let activity = manifest.activity
        let folder = outputDirectory.appendingPathComponent(Self.folderPickerFolderName, isDirectory: true)
        let file = folder.appendingPathComponent(Self.folderPickerFileName)
        XCTAssertEqual(activity.direction, .receive)
        XCTAssertEqual(activity.state, .completed)
        XCTAssertEqual(activity.bytesTransferred, UInt64(Self.folderPickerPayload.count))
        XCTAssertEqual(activity.totalBytes, UInt64(Self.folderPickerPayload.count))
        XCTAssertNotEqual(activity.dataPathKind, .none)
        XCTAssertEqual(URL(fileURLWithPath: activity.completedFilePath), outputDirectory)
        XCTAssertEqual(manifest.rootCount, 1)
        XCTAssertEqual(manifest.fileCount, 1)
        XCTAssertEqual(manifest.directoryCount, 1)
        XCTAssertEqual(manifest.completedFiles, 1)
        XCTAssertTrue(manifest.entryResults.allSatisfy {
            $0.status == .completed || $0.status == .skippedIdentical || $0.status == .renamed
        })
        XCTAssertEqual(try Data(contentsOf: file), Self.folderPickerPayload)
        let hash = try Self.fileSHA256(file)
        XCTAssertEqual(hash, Data(SHA256.hash(data: Self.folderPickerPayload)))
        emitEvidence(
            "folder-picker-manifest-completed activity=\(activityID) " +
            "pathKind=\(activity.dataPathKind) pathDetail=\(activity.dataPathDetail) " +
            "root=\(outputDirectory.path) roots=\(manifest.rootCount) " +
            "files=\(manifest.completedFiles)/\(manifest.fileCount) " +
            "directories=\(manifest.directoryCount) bytes=\(activity.bytesTransferred) " +
            "sha256=\(hash.hexString)"
        )
#endif
    }

    func testReceiveIosFilePickerToMacOSAppManifestRoom() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        try await receiveIosFileSelectionManifest(
            runID: Self.runID,
            evidenceLabel: "file-picker-manifest"
        )
#endif
    }

    func testReceiveIosShareExtensionFilesToMacOSAppManifestRoom() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        try await receiveIosFileSelectionManifest(
            runID: Self.runID,
            evidenceLabel: "share-extension-files-manifest"
        )
#endif
    }

    func testReceiveIosManualPhotosShareToMacOSAppManifestRoom() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        let outputDirectory = outputDirectory()
        let model = AppModel.shared
        let existingActivityIDs = Set(model.activities.map(\.activityId))
        try? FileManager.default.removeItem(at: outputDirectory)
        try FileManager.default.createDirectory(at: outputDirectory, withIntermediateDirectories: true)

        model.receive.startReceivingWithRoom(
            outputDir: outputDirectory.path,
            code: Self.roomCode,
            settings: Self.runtimeSettings
        )
        let activityID = try await waitForNewReceiveActivity(
            in: model,
            excluding: existingActivityIDs,
            timeout: Self.manualPhotosTimeout
        )
        defer { model.removeActivity(activityID) }
        emitEvidence(
            "manual-photos-share-manifest-receiver-ready activity=\(activityID) room=\(Self.roomCode)"
        )

        let manifest = try await waitForManifestCompletion(
            activityID: activityID,
            in: model,
            timeout: Self.manualPhotosTimeout
        )
        let activity = manifest.activity
        let files = manifest.entries.filter { $0.kind == .file }
        XCTAssertEqual(activity.direction, .receive)
        XCTAssertEqual(activity.state, .completed)
        XCTAssertNotEqual(activity.dataPathKind, .none)
        XCTAssertEqual(URL(fileURLWithPath: activity.completedFilePath), outputDirectory)
        XCTAssertEqual(manifest.rootCount, 2)
        XCTAssertEqual(manifest.fileCount, 2)
        XCTAssertEqual(manifest.directoryCount, 0)
        XCTAssertEqual(manifest.completedFiles, 2)
        XCTAssertEqual(files.count, 2)
        XCTAssertEqual(activity.bytesTransferred, files.reduce(UInt64(0)) { $0 + $1.size })
        XCTAssertEqual(activity.totalBytes, activity.bytesTransferred)

        let rootPath = outputDirectory.standardizedFileURL.path + "/"
        let receivedFiles = try files.map { entry -> (FfiPreparedManifestEntry, URL) in
            guard let result = manifest.entryResults.first(where: { $0.entryId == entry.entryId }),
                  result.status == .completed
                    || result.status == .skippedIdentical
                    || result.status == .renamed else {
                throw HostedTestError.transferFailed("manual Photos entry did not complete")
            }
            let finalURL = outputDirectory
                .appendingPathComponent(result.finalRelativePath)
                .standardizedFileURL
            guard finalURL.path.hasPrefix(rootPath) else {
                throw HostedTestError.transferFailed("manual Photos entry resolved outside its output directory")
            }
            XCTAssertTrue(FileManager.default.fileExists(atPath: finalURL.path))
            XCTAssertEqual(try Self.fileSize(finalURL), entry.size)
            return (entry, finalURL)
        }

        let prepared = try await prepareManifestSend(
            activityId: "manual-photos-receive-\(UUID().uuidString)",
            selectedPaths: receivedFiles.map { $0.1.path }
        )
        XCTAssertEqual(prepared.rootCount, 2)
        XCTAssertEqual(prepared.fileCount, 2)
        XCTAssertEqual(prepared.directoryCount, 0)
        let receivedHashesByPath = Dictionary(
            uniqueKeysWithValues: receivedFiles.map { ($0.1.path, $0.0.hash) }
        )
        let preparedHashesByPath = Dictionary(
            uniqueKeysWithValues: prepared.entries.map { ($0.sourcePath, $0.hash) }
        )
        XCTAssertEqual(preparedHashesByPath, receivedHashesByPath)
        emitEvidence(
            "manual-photos-share-manifest-completed activity=\(activityID) " +
            "pathKind=\(activity.dataPathKind) pathDetail=\(activity.dataPathDetail) " +
            "root=\(outputDirectory.path) roots=\(manifest.rootCount) " +
            "files=\(manifest.completedFiles)/\(manifest.fileCount) bytes=\(activity.bytesTransferred) " +
            "contentHashesVerified=\(receivedFiles.count)"
        )
#endif
    }

#if ENVOIX_CROSS_DEVICE_TESTING
    private func receiveIosFileSelectionManifest(
        runID: String,
        evidenceLabel: String
    ) async throws {
        let outputDirectory = outputDirectory()
        let model = AppModel.shared
        let existingActivityIDs = Set(model.activities.map(\.activityId))
        try? FileManager.default.removeItem(at: outputDirectory)
        try FileManager.default.createDirectory(at: outputDirectory, withIntermediateDirectories: true)

        model.receive.startReceivingWithRoom(
            outputDir: outputDirectory.path,
            code: Self.roomCode,
            settings: Self.runtimeSettings
        )
        let activityID = try await waitForNewReceiveActivity(
            in: model,
            excluding: existingActivityIDs
        )
        defer { model.removeActivity(activityID) }
        emitEvidence(
            "\(evidenceLabel)-receiver-ready activity=\(activityID) room=\(Self.roomCode)"
        )

        let manifest = try await waitForManifestCompletion(activityID: activityID, in: model)
        let activity = manifest.activity
        let firstPayload = Data("envoix file picker payload first \(runID)\n".utf8)
        let secondPayload = Data("envoix file picker payload second \(runID)\n".utf8)
        let expectedBytes = UInt64(firstPayload.count + secondPayload.count)
        let first = outputDirectory.appendingPathComponent("envoix-\(runID)-file-first.txt")
        let second = outputDirectory.appendingPathComponent("envoix-\(runID)-file-second.txt")
        XCTAssertEqual(activity.direction, .receive)
        XCTAssertEqual(activity.state, .completed)
        XCTAssertEqual(activity.bytesTransferred, expectedBytes)
        XCTAssertEqual(activity.totalBytes, expectedBytes)
        XCTAssertNotEqual(activity.dataPathKind, .none)
        XCTAssertEqual(URL(fileURLWithPath: activity.completedFilePath), outputDirectory)
        XCTAssertEqual(manifest.rootCount, 2)
        XCTAssertEqual(manifest.fileCount, 2)
        XCTAssertEqual(manifest.directoryCount, 0)
        XCTAssertEqual(manifest.completedFiles, 2)
        XCTAssertTrue(manifest.entryResults.allSatisfy {
            $0.status == .completed || $0.status == .skippedIdentical || $0.status == .renamed
        })
        XCTAssertEqual(try Data(contentsOf: first), firstPayload)
        XCTAssertEqual(try Data(contentsOf: second), secondPayload)
        let firstHash = try Self.fileSHA256(first)
        let secondHash = try Self.fileSHA256(second)
        XCTAssertEqual(firstHash, Data(SHA256.hash(data: firstPayload)))
        XCTAssertEqual(secondHash, Data(SHA256.hash(data: secondPayload)))
        emitEvidence(
            "\(evidenceLabel)-completed activity=\(activityID) " +
            "pathKind=\(activity.dataPathKind) pathDetail=\(activity.dataPathDetail) " +
            "root=\(outputDirectory.path) roots=\(manifest.rootCount) " +
            "files=\(manifest.completedFiles)/\(manifest.fileCount) bytes=\(activity.bytesTransferred) " +
            "firstSha256=\(firstHash.hexString) secondSha256=\(secondHash.hexString)"
        )
    }
#endif

    func testSendMacOSToIosAppInvite() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        let model = AppModel.shared
        guard !model.send.isBusy else {
            throw HostedTestError.transferFailed("the production sender is already busy")
        }

        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-macos-app-send-\(UUID().uuidString)", isDirectory: true)
        try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: root) }
        let sourceURL = root.appendingPathComponent(Self.macOSToIosFileName)
        try Self.macOSToIosPayload.write(to: sourceURL)

        let defaults = UserDefaults.standard
        let invite = Self.environment("ENVOIX_MACOS_TO_IOS_INVITE")
            ?? defaults.string(forKey: Self.macOSToIosInviteDefaultsKey)
        defaults.removeObject(forKey: Self.macOSToIosInviteDefaultsKey)
        guard let invite, !invite.isEmpty else {
            throw HostedTestError.transferFailed("ENVOIX_MACOS_TO_IOS_INVITE is required")
        }
        model.send.startSendingWithInvite(
            filePath: sourceURL.path,
            invite: invite,
            settings: Self.runtimeSettings,
            pathPolicy: .relayOnly
        )
        let activityID = model.send.activeActivityID
        guard !activityID.isEmpty else {
            throw HostedTestError.transferFailed("production macOS sender did not create an Activity")
        }
        defer { model.removeActivity(activityID) }
        emitEvidence("sender-started activity=\(activityID) mode=invite pathPolicy=relay-only")

        let completed = try await waitForCompletion(activityID: activityID, in: model)
        XCTAssertEqual(completed.direction, .send)
        XCTAssertEqual(completed.fileName, Self.macOSToIosFileName)
        XCTAssertEqual(completed.bytesTransferred, UInt64(Self.macOSToIosPayload.count))
        XCTAssertEqual(completed.totalBytes, UInt64(Self.macOSToIosPayload.count))
        XCTAssertNotEqual(completed.dataPathKind, .none)
        let hash = try Self.fileSHA256(sourceURL)
        XCTAssertEqual(hash, Data(SHA256.hash(data: Self.macOSToIosPayload)))
        emitEvidence(
            "sender-completed activity=\(activityID) pathKind=\(completed.dataPathKind) " +
            "pathDetail=\(completed.dataPathDetail) file=\(completed.fileName) " +
            "size=\(completed.bytesTransferred) sha256=\(hash.hexString)"
        )
#endif
    }

    func testSendMacOSToIosAppManifestInvite() async throws {
        try requireCrossDeviceTesting()
#if ENVOIX_CROSS_DEVICE_TESTING
        let model = AppModel.shared
        guard !model.send.isBusy else {
            throw HostedTestError.transferFailed("the production sender is already busy")
        }

        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-macos-app-manifest-send-\(UUID().uuidString)", isDirectory: true)
        let album = root.appendingPathComponent(Self.macOSToIosManifestAlbumName, isDirectory: true)
        let emptyDirectory = album.appendingPathComponent("Empty", isDirectory: true)
        let photo = album.appendingPathComponent("photo.bin")
        let loose = root.appendingPathComponent(Self.macOSToIosManifestLooseName)
        try fileManager.createDirectory(at: emptyDirectory, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: root) }
        try Self.macOSToIosManifestPhotoPayload.write(to: photo)
        try Self.macOSToIosManifestLoosePayload.write(to: loose)

        let defaults = UserDefaults.standard
        let invite = Self.environment("ENVOIX_MACOS_TO_IOS_MANIFEST_INVITE")
            ?? defaults.string(forKey: Self.macOSToIosManifestInviteDefaultsKey)
        defaults.removeObject(forKey: Self.macOSToIosManifestInviteDefaultsKey)
        guard let invite, !invite.isEmpty else {
            throw HostedTestError.transferFailed("ENVOIX_MACOS_TO_IOS_MANIFEST_INVITE is required")
        }
        model.send.startSendingManifestWithInvite(
            selectedPaths: [album.path, loose.path],
            invite: invite,
            settings: Self.runtimeSettings,
            pathPolicy: .relayOnly
        )
        let activityID = model.send.activeActivityID
        guard !activityID.isEmpty else {
            throw HostedTestError.transferFailed("production macOS sender did not create a Manifest Activity")
        }
        defer { model.removeActivity(activityID) }
        emitEvidence("manifest-sender-started activity=\(activityID) mode=invite pathPolicy=relay-only")

        let manifest = try await waitForManifestCompletion(activityID: activityID, in: model)
        let activity = manifest.activity
        let expectedBytes = UInt64(
            Self.macOSToIosManifestPhotoPayload.count + Self.macOSToIosManifestLoosePayload.count
        )
        XCTAssertEqual(activity.direction, .send)
        XCTAssertEqual(activity.state, .completed)
        XCTAssertEqual(activity.bytesTransferred, expectedBytes)
        XCTAssertEqual(activity.totalBytes, expectedBytes)
        XCTAssertEqual(activity.dataPathKind, .relay)
        XCTAssertEqual(manifest.rootCount, 2)
        XCTAssertEqual(manifest.fileCount, 2)
        XCTAssertEqual(manifest.directoryCount, 2)
        XCTAssertEqual(manifest.completedFiles, 2)
        XCTAssertTrue(manifest.entryResults.allSatisfy {
            $0.status == .completed || $0.status == .skippedIdentical || $0.status == .renamed
        })

        let photoHash = try Self.fileSHA256(photo)
        let looseHash = try Self.fileSHA256(loose)
        XCTAssertEqual(photoHash, Data(SHA256.hash(data: Self.macOSToIosManifestPhotoPayload)))
        XCTAssertEqual(looseHash, Data(SHA256.hash(data: Self.macOSToIosManifestLoosePayload)))
        emitEvidence(
            "manifest-sender-completed activity=\(activityID) pathKind=\(activity.dataPathKind) " +
            "pathDetail=\(activity.dataPathDetail) roots=\(manifest.rootCount) " +
            "files=\(manifest.completedFiles)/\(manifest.fileCount) " +
            "directories=\(manifest.directoryCount) bytes=\(activity.bytesTransferred) " +
            "photoSha256=\(photoHash.hexString) looseSha256=\(looseHash.hexString)"
        )
#endif
    }

#if ENVOIX_CROSS_DEVICE_TESTING
    private func receiveSingleFile(
        fileName: String,
        payload: Data,
        expectedBytes: UInt64,
        evidenceLabel: String
    ) async throws {
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
        let finalURL = outputDirectory.appendingPathComponent(fileName)
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
        emitEvidence("\(evidenceLabel)-ready activity=\(activityID) room=\(Self.roomCode)")
        let record = try await waitForCompletion(activityID: activityID, in: model)

        XCTAssertEqual(record.fileName, fileName)
        XCTAssertEqual(record.bytesTransferred, expectedBytes)
        XCTAssertEqual(record.totalBytes, expectedBytes)
        XCTAssertNotEqual(record.dataPathKind, .none)
        let resolvedURL = model.manifestActivities[activityID]
            .flatMap(availableCompletedManifestURL)
            ?? availableCompletedFileURL(
                path: record.completedFilePath,
                expectedBytes: expectedBytes
            )
        XCTAssertEqual(resolvedURL, finalURL)
        XCTAssertTrue(FileManager.default.fileExists(atPath: finalURL.path))
        XCTAssertEqual(try Self.fileSize(finalURL), expectedBytes)

        let actualHash = try Self.fileSHA256(finalURL)
        let expectedHash = Self.repeatedPayloadSHA256(
            payload,
            expectedBytes: expectedBytes
        )
        XCTAssertEqual(actualHash, expectedHash)
        emitEvidence(
            "\(evidenceLabel)-completed activity=\(activityID) pathKind=\(record.dataPathKind) " +
            "pathDetail=\(record.dataPathDetail) corePath=\(record.completedFilePath) " +
            "resolvedFile=\(finalURL.path) " +
            "size=\(expectedBytes) sha256=\(actualHash.hexString)"
        )
    }
#endif

    func testReceiveIosToMacOSAppManifestRoom() async throws {
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

        try? FileManager.default.removeItem(at: outputDirectory)
        try FileManager.default.createDirectory(
            at: outputDirectory,
            withIntermediateDirectories: true
        )
        model.receive.startReceivingWithRoom(
            outputDir: outputDirectory.path,
            code: Self.roomCode,
            settings: Self.runtimeSettings
        )

        let activityID = try await waitForNewReceiveActivity(
            in: model,
            excluding: existingActivityIDs
        )
        emitEvidence("manifest-receiver-ready activity=\(activityID) room=\(Self.roomCode)")
        let manifest = try await waitForManifestCompletion(activityID: activityID, in: model)
        let activity = manifest.activity
        XCTAssertEqual(activity.state, .completed)
        XCTAssertEqual(manifest.rootCount, 2)
        XCTAssertEqual(manifest.fileCount, 2)
        XCTAssertEqual(manifest.directoryCount, 2)
        XCTAssertEqual(manifest.completedFiles, 2)
        XCTAssertEqual(URL(fileURLWithPath: activity.completedFilePath), outputDirectory)
        XCTAssertNotEqual(activity.dataPathKind, .none)
        XCTAssertTrue(manifest.entryResults.allSatisfy {
            $0.status == .completed || $0.status == .skippedIdentical || $0.status == .renamed
        })

        let album = outputDirectory.appendingPathComponent(Self.manifestAlbumName, isDirectory: true)
        let emptyDirectory = album.appendingPathComponent("Empty", isDirectory: true)
        let photo = album.appendingPathComponent("photo.bin")
        let loose = outputDirectory.appendingPathComponent(Self.manifestLooseName)
        let emptyValues = try emptyDirectory.resourceValues(forKeys: [.isDirectoryKey])
        XCTAssertEqual(emptyValues.isDirectory, true)
        XCTAssertEqual(try Data(contentsOf: photo), Self.manifestPhotoPayload)
        XCTAssertEqual(try Data(contentsOf: loose), Self.manifestLoosePayload)

        let photoHash = try Self.fileSHA256(photo)
        let looseHash = try Self.fileSHA256(loose)
        XCTAssertEqual(photoHash, Data(SHA256.hash(data: Self.manifestPhotoPayload)))
        XCTAssertEqual(looseHash, Data(SHA256.hash(data: Self.manifestLoosePayload)))
        XCTAssertEqual(
            activity.bytesTransferred,
            UInt64(Self.manifestPhotoPayload.count + Self.manifestLoosePayload.count)
        )
        emitEvidence(
            "manifest-completed activity=\(activityID) pathKind=\(activity.dataPathKind) " +
            "pathDetail=\(activity.dataPathDetail) root=\(outputDirectory.path) " +
            "roots=\(manifest.rootCount) files=\(manifest.completedFiles)/\(manifest.fileCount) " +
            "directories=\(manifest.directoryCount) bytes=\(activity.bytesTransferred) " +
            "photoSha256=\(photoHash.hexString) looseSha256=\(looseHash.hexString)"
        )
#endif
    }

    private static func manifestEntry(
        id: UInt32,
        path: String,
        kind: FfiManifestEntryKind = .file,
        size: UInt64 = 0
    ) -> FfiPreparedManifestEntry {
        FfiPreparedManifestEntry(
            entryId: id,
            relativePath: path,
            kind: kind,
            size: size,
            hash: Data(),
            modifiedAtUnixMs: nil,
            sourcePath: ""
        )
    }

    private static func manifestRecord(
        completedRoot: URL,
        rootCount: UInt32,
        entries: [FfiPreparedManifestEntry]
    ) -> FfiManifestActivityRecord {
        let totalBytes = entries.reduce(UInt64(0)) { $0 + $1.size }
        let results = entries.map {
            FfiManifestEntryResult(
                entryId: $0.entryId,
                status: .completed,
                offeredRelativePath: $0.relativePath,
                finalRelativePath: $0.relativePath,
                failureCode: ""
            )
        }
        return FfiManifestActivityRecord(
            activity: FfiTransferActivityRecord(
                activityId: "manifest-publication-test",
                sequence: 1,
                attemptId: "attempt-1",
                state: .publishing,
                direction: .receive,
                mode: .room,
                transferId: "manifest-publication-test",
                fileName: rootCount == 1 ? entries[0].relativePath : "\(rootCount) items",
                totalBytes: totalBytes,
                bytesTransferred: totalBytes,
                bytesResumed: 0,
                speedBps: 0,
                averageSpeedBps: 0,
                createdAtMs: 1,
                updatedAtMs: 1,
                startedAtMs: 1,
                completedAtMs: 0,
                completedFilePath: completedRoot.path,
                dataPathKind: .direct,
                dataPathDetail: "test",
                invite: "",
                token: "",
                peerDescriptor: "",
                diagnosticMessage: "",
                failureCode: .unknown,
                failureCategory: .unknown,
                failurePhase: .committing,
                failureOrigin: .unknown,
                userMessageKey: "",
                retryable: false,
                recoveryAction: .none,
                limits: FfiTransferLimits(
                    maxParallelTransfers: 1,
                    maxParallelFiles: 1,
                    maxParallelChunksPerFile: 1,
                    speedLimitBps: 0
                )
            ),
            manifestId: "manifest-publication-test",
            rootCount: rootCount,
            fileCount: UInt32(entries.filter { $0.kind == .file }.count),
            directoryCount: UInt32(entries.filter { $0.kind == .directory }.count),
            completedFiles: UInt32(entries.filter { $0.kind == .file }.count),
            entries: entries,
            currentEntry: nil,
            entryResults: results
        )
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
    private static let photoFileName = "envoix-\(runID)-photo.png"
    private static let openInFileName = "envoix-\(runID)-open-in.txt"
    private static let openInPayload = Data("envoix Open In payload \(runID)\n".utf8)
    private static let multiPhotoFirstName = "envoix-\(runID)-photo-first.png"
    private static let multiPhotoSecondName = "envoix-\(runID)-photo-second.png"
    private static let folderPickerFolderName = "envoix-\(runID)-folder"
    private static let folderPickerFileName = "payload.txt"
    private static let folderPickerPayload = Data("envoix folder picker payload \(runID)\n".utf8)
    private static let photoPayload = Data(
        base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
    )!
    private static let macOSToIosFileName = "envoix-\(runID)-macos-to-ios.bin"
    private static let macOSToIosPayload = Data("envoix cross-device macos to ios app\n".utf8)
    private static let macOSToIosManifestAlbumName = "envoix-\(runID)-macos-album"
    private static let macOSToIosManifestLooseName = "envoix-\(runID)-macos-loose.txt"
    private static let macOSToIosManifestPhotoPayload = Data(
        "envoix manifest macos photo \(runID)\n".utf8
    )
    private static let macOSToIosManifestLoosePayload = Data(
        "envoix manifest macos loose file \(runID)\n".utf8
    )
    private static let manifestAlbumName = "envoix-\(runID)-album"
    private static let manifestLooseName = "envoix-\(runID)-loose.txt"
    private static let manifestPhotoPayload = Data("envoix manifest photo \(runID)\n".utf8)
    private static let manifestLoosePayload = Data("envoix manifest loose file \(runID)\n".utf8)
    private static let expectedBytes = environment("ENVOIX_IOS_TO_MACOS_BYTES")
        .flatMap(UInt64.init) ?? UInt64(payload.count)
    private static let timeout = environment("ENVOIX_CROSS_DEVICE_TIMEOUT_SECONDS")
        .flatMap(TimeInterval.init) ?? 180
    private static let defaultManualPhotosTimeout: TimeInterval = 300
    private static let manualPhotosTimeout = max(
        environment("ENVOIX_CROSS_DEVICE_TIMEOUT_SECONDS")
            .flatMap(TimeInterval.init) ?? defaultManualPhotosTimeout,
        defaultManualPhotosTimeout
    )
    private static let useRoomDefaultsKey = "envoix.useRoom"
    private static let useMdnsDefaultsKey = "envoix.useMdns"
    private static let macOSToIosInviteDefaultsKey = "envoix.test.macOSToIosInvite"
    private static let macOSToIosManifestInviteDefaultsKey = "envoix.test.macOSToIosManifestInvite"
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
        excluding existingActivityIDs: Set<String>,
        timeout: TimeInterval? = nil
    ) async throws -> String {
        let deadline = Date().addingTimeInterval(timeout ?? Self.timeout)
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

    private func waitForManifestCompletion(
        activityID: String,
        in model: AppModel,
        timeout: TimeInterval? = nil
    ) async throws -> FfiManifestActivityRecord {
        let deadline = Date().addingTimeInterval(timeout ?? Self.timeout)
        while Date() < deadline {
            if let record = model.manifestActivities[activityID] {
                switch record.activity.state {
                case .completed:
                    return record
                case .failed, .canceled:
                    throw HostedTestError.transferFailed(record.activity.diagnosticMessage)
                case .queued, .binding, .waitingForPeer, .pairing, .connecting,
                        .transferring, .verifying, .publishing, .unconfirmed,
                        .paused, .unknown:
                    break
                }
            }
            try await Task.sleep(nanoseconds: 200_000_000)
        }
        throw HostedTestError.timedOut("waiting for macOS App Manifest completion")
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

private enum HostedCoreLoopbackError: Error {
    case timeout(String)
    case failed(String)
    case missing(String)
}

private final class HostedCoreNoopMailbox: MailboxObserverV2, @unchecked Sendable {
    func onFetchReceipt(activityId: String, key: String, server: String?) {}

    func onPostReceipt(activityId: String, key: String, blob: Data, server: String?) {}
}

private final class HostedCoreLoopbackObserver: TransferObserver, ManifestTransferObserverV2, @unchecked Sendable {
    private let lock = NSLock()
    private let inviteSemaphore = DispatchSemaphore(value: 0)
    private let terminalSemaphore = DispatchSemaphore(value: 0)
    private var invite: String?
    private var completedBytes: UInt64?
    private var failure: String?
    private var terminalReported = false

    func onInviteReady(invite: String) {
        lock.lock()
        let shouldSignal = self.invite == nil
        self.invite = invite
        lock.unlock()
        if shouldSignal { inviteSemaphore.signal() }
    }

    func onStarted(fileName: String, totalBytes: UInt64) {}
    func onProgress(transferred: UInt64, total: UInt64) {}

    func onCompleted(bytes: UInt64) {
        finish(bytes: bytes, failure: nil)
    }

    func onTransferFailed(failure: FfiTransferFailure) {
        finish(
            bytes: nil,
            failure: failure.diagnosticMessage.isEmpty
                ? failure.userMessageKey
                : failure.diagnosticMessage
        )
    }

    func onFailed(reason: String) {
        finish(bytes: nil, failure: reason)
    }

    func onTransferEvent(event: FfiTransferEvent) {}
    func onTransferActivity(record: FfiTransferActivityRecord) {
        observe(record)
    }
    func onStatus(message: String) {}

    func onManifestEvent(event: FfiTransferEvent) {}
    func onManifestActivity(record: FfiManifestActivityRecord) {
        observe(record.activity)
    }

    func waitForInvite(timeout: TimeInterval) async throws -> String {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .utility).async { [self] in
                guard inviteSemaphore.wait(timeout: .now() + timeout) == .success else {
                    continuation.resume(throwing: HostedCoreLoopbackError.timeout("invite"))
                    return
                }
                lock.lock()
                let invite = self.invite
                lock.unlock()
                guard let invite else {
                    continuation.resume(throwing: HostedCoreLoopbackError.missing("invite"))
                    return
                }
                continuation.resume(returning: invite)
            }
        }
    }

    func waitForCompletion(timeout: TimeInterval) async throws -> UInt64 {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .utility).async { [self] in
                guard terminalSemaphore.wait(timeout: .now() + timeout) == .success else {
                    continuation.resume(throwing: HostedCoreLoopbackError.timeout("completion"))
                    return
                }
                lock.lock()
                let completedBytes = self.completedBytes
                let failure = self.failure
                lock.unlock()
                if let failure {
                    continuation.resume(throwing: HostedCoreLoopbackError.failed(failure))
                } else if let completedBytes {
                    continuation.resume(returning: completedBytes)
                } else {
                    continuation.resume(throwing: HostedCoreLoopbackError.missing("completion"))
                }
            }
        }
    }

    private func finish(bytes: UInt64?, failure: String?) {
        lock.lock()
        guard !terminalReported else {
            lock.unlock()
            return
        }
        terminalReported = true
        completedBytes = bytes
        self.failure = failure
        lock.unlock()
        terminalSemaphore.signal()
    }

    private func observe(_ record: FfiTransferActivityRecord) {
        if !record.invite.isEmpty {
            onInviteReady(invite: record.invite)
        }
        switch record.state {
        case .completed:
            finish(bytes: record.bytesTransferred, failure: nil)
        case .failed, .canceled:
            finish(
                bytes: nil,
                failure: record.diagnosticMessage.isEmpty
                    ? "\(record.state)"
                    : record.diagnosticMessage
            )
        case .queued, .binding, .waitingForPeer, .pairing, .connecting,
                .transferring, .verifying, .publishing, .unconfirmed, .paused, .unknown:
            break
        }
    }
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
