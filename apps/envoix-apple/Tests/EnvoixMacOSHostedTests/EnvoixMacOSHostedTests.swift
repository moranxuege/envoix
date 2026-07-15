import CryptoKit
import EnvoixCore
import XCTest
@testable import Envoix

@MainActor
final class EnvoixMacOSHostedTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
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

    private func waitForManifestCompletion(
        activityID: String,
        in model: AppModel
    ) async throws -> FfiManifestActivityRecord {
        let deadline = Date().addingTimeInterval(Self.timeout)
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
