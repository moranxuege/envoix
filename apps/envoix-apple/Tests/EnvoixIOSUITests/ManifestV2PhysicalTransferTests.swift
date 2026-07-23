import CryptoKit
import EnvoixCore
import Foundation
import XCTest
#if os(iOS)
import UniformTypeIdentifiers
@testable import Envoix_iOS
#elseif os(macOS)
@testable import Envoix
#endif

/// Scenario-driven physical coverage for the canonical Manifest-v2 job and
/// session path. The suite is hosted by each production Apple app target so
/// networking, source preparation, and destination writes match the client.
final class ManifestV2PhysicalTransferTests: XCTestCase {
    func testShareSourceFailureDoesNotPoisonNextSelection() throws {
        try requirePhysicalRun()
        #if os(iOS)
        let root = try makeTestRoot("share-recovery")
        defer { try? FileManager.default.removeItem(at: root) }
        let store = try ShareDraftStore.live()
        let missingURL = root.appendingPathComponent("not-readable.txt")
        XCTAssertThrowsError(
            try store.stage(
                sourceURL: missingURL,
                contentTypeIdentifier: UTType.plainText.identifier,
                mediaKind: .file
            )
        ) { error in
            XCTAssertEqual(error as? ShareDraftStoreError, .sourceIsUnreadable)
        }

        let validURL = root.appendingPathComponent("share-recovery.txt")
        try Self.shareRecoveryBytes.write(to: validURL)
        let draft = try store.stage(
            sourceURL: validURL,
            contentTypeIdentifier: UTType.plainText.identifier,
            mediaKind: .file
        )
        defer { try? store.discard(id: draft.descriptor.id) }
        XCTAssertEqual(draft.fileURLs.count, 1)
        XCTAssertEqual(try Data(contentsOf: try XCTUnwrap(draft.fileURLs.first)), Self.shareRecoveryBytes)
        Self.marker("iOS unreadable Share source recovered with a valid selection")
        #else
        throw XCTSkip("The macOS client has no Share extension source provider")
        #endif
    }

    func testSendScenarioManifestV2Room() async throws {
        try requirePhysicalRun()
        let fixture = try Self.fixture()
        let fileManager = FileManager.default
        let root = try makeTestRoot("send")
        let sourceDirectory = root.appendingPathComponent("sources", isDirectory: true)
        let jobStore = root.appendingPathComponent("jobs", isDirectory: true)
        let stateDirectory = root.appendingPathComponent("state", isDirectory: true)
        try fileManager.createDirectory(at: sourceDirectory, withIntermediateDirectories: true)
        try fileManager.createDirectory(at: jobStore, withIntermediateDirectories: true)
        try fileManager.createDirectory(at: stateDirectory, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: root) }

        let materialized = try fixture.materialize(in: sourceDirectory)
        let selectedPaths: [String]
        #if os(iOS)
        var sharedDraft: (store: ShareDraftStore, id: UUID)?
        if fixture.scenario == .share {
            let store = try ShareDraftStore.live()
            let items = materialized.rootURLs.map { url in
                ShareDraftStagingItem(
                    sourceURL: url,
                    contentTypeIdentifier: UTType(filenameExtension: url.pathExtension)?.identifier
                        ?? UTType.data.identifier,
                    mediaKind: url.pathExtension.lowercased() == "png" ? .image : .file,
                    preferredFileName: url.lastPathComponent
                )
            }
            let draft = try store.stage(items: items)
            sharedDraft = (store, draft.descriptor.id)
            selectedPaths = draft.fileURLs.map(\.path)
        } else {
            selectedPaths = materialized.selectedURLs.map(\.path)
        }
        defer {
            if let sharedDraft {
                try? sharedDraft.store.discard(id: sharedDraft.id)
            }
        }
        #else
        guard fixture.scenario != .share else {
            throw XCTSkip("The macOS client has no Share extension source provider")
        }
        selectedPaths = materialized.selectedURLs.map(\.path)
        #endif

        let job = try await createTransferJobV2(storeDirectory: jobStore.path, compressionPolicy: .never)
        let prepared = try await job.addLocalPaths(paths: selectedPaths)
        XCTAssertEqual(prepared.state, .readyToSend)
        XCTAssertEqual(prepared.inventory.rootCount, UInt32(fixture.roots.count))
        XCTAssertEqual(prepared.inventory.fileCount, UInt32(fixture.fileCount))
        XCTAssertEqual(prepared.inventory.directoryCount, UInt32(fixture.directoryCount))
        XCTAssertEqual(prepared.inventory.totalPlaintextBytes, fixture.totalBytes)
        XCTAssertEqual(prepared.inventory.warningCount, 0)
        _ = try await job.sealForSend()

        let observer = ManifestV2PhysicalObserver()
        let completion = try await sendTransferJobV2(
            job: job,
            settings: Self.settings,
            request: Self.request(direction: .send),
            stateDirectory: stateDirectory.path,
            cancellation: FfiManifestV2Cancellation(),
            observer: observer
        )
        XCTAssertEqual(completion.entryCount, UInt32(fixture.entryCount))
        XCTAssertEqual(completion.totalPlaintextBytes, fixture.totalBytes)
        XCTAssertEqual(completion.deliveryProofDigest.count, Self.deliveryProofDigestBytes)
        XCTAssertTrue(completion.savedPaths.isEmpty)
        XCTAssertTrue(observer.phases.contains(.waitingForReceiverSave))
        XCTAssertTrue(observer.phases.contains(.finalizingDelivery))
        XCTAssertTrue(observer.phases.contains(.delivered))
        XCTAssertNil(observer.failureMessage)
        Self.marker("\(Self.platformName) send completed scenario=\(fixture.scenario.rawValue) bytes=\(fixture.totalBytes)")
    }

    func testReceiveScenarioManifestV2Room() async throws {
        try requirePhysicalRun()
        let fixture = try Self.fixture()
        let fileManager = FileManager.default
        let root = try makeTestRoot("receive")
        let destination = root.appendingPathComponent("received", isDirectory: true)
        let stateDirectory = root.appendingPathComponent("state", isDirectory: true)
        try fileManager.createDirectory(at: destination, withIntermediateDirectories: true)
        try fileManager.createDirectory(at: stateDirectory, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: root) }

        var collisionURL: URL?
        if fixture.scenario == .collision {
            let url = destination.appendingPathComponent(fixture.roots[0].name)
            try Self.collisionSentinel.write(to: url, options: .atomic)
            collisionURL = url
        }

        let observer = ManifestV2PhysicalObserver()
        Self.marker("\(Self.platformName) receiver ready scenario=\(fixture.scenario.rawValue)")
        let pending = try await receiveTransferOfferV2(
            settings: Self.settings,
            request: Self.request(direction: .receive),
            stateDirectory: stateDirectory.path,
            cancellation: FfiManifestV2Cancellation(),
            observer: observer
        )

        let summary = pending.summary()
        XCTAssertEqual(summary.rootCount, UInt32(fixture.roots.count))
        XCTAssertEqual(summary.fileCount, UInt32(fixture.fileCount))
        XCTAssertEqual(summary.directoryCount, UInt32(fixture.directoryCount))
        XCTAssertEqual(summary.totalPlaintextBytes, fixture.totalBytes)
        XCTAssertFalse(summary.exceptionalOffer)
        var offeredEntries: [FfiManifestOfferEntryV2] = []
        var offset: UInt32 = 0
        repeat {
            let page = pending.listEntries(offset: offset, limit: 64)
            offeredEntries.append(contentsOf: page.entries)
            guard let next = page.nextOffset else { break }
            offset = next
        } while true
        XCTAssertEqual(offeredEntries.count, fixture.entryCount)
        XCTAssertEqual(offeredEntries.filter { $0.kind == .file }.count, fixture.fileCount)

        let completion = try await pending.receive(
            destination: FfiDestinationRequestV2(
                targetDirectory: destination.path,
                copyStagingDirectory: nil,
                decision: .saveDirectly,
                targetAllocatableBytes: try availableCapacity(at: destination),
                stagingAllocatableBytes: nil,
                stableObjectIdentity: true,
                exceptionalTransferApproved: false
            ),
            observer: observer
        )
        XCTAssertEqual(completion.entryCount, UInt32(fixture.entryCount))
        XCTAssertEqual(completion.totalPlaintextBytes, fixture.totalBytes)
        XCTAssertEqual(completion.deliveryProofDigest.count, Self.deliveryProofDigestBytes)
        XCTAssertEqual(completion.savedPaths.count, fixture.roots.count)
        for (rootSpec, savedPath) in zip(fixture.roots, completion.savedPaths) {
            try rootSpec.verify(at: URL(fileURLWithPath: savedPath))
        }
        if let collisionURL {
            XCTAssertEqual(try Data(contentsOf: collisionURL), Self.collisionSentinel)
            XCTAssertNotEqual(completion.savedPaths.first, collisionURL.path)
        }
        XCTAssertTrue(observer.phases.contains(.saving))
        XCTAssertTrue(observer.phases.contains(.delivered))
        XCTAssertNil(observer.failureMessage)
        Self.marker("\(Self.platformName) receive saved scenario=\(fixture.scenario.rawValue) bytes=\(fixture.totalBytes)")
    }

    private func requirePhysicalRun() throws {
        guard ProcessInfo.processInfo.environment[Self.enabledEnvironment] == "1" else {
            throw XCTSkip("Manifest v2 physical tests require \(Self.enabledEnvironment)=1")
        }
    }

    private func makeTestRoot(_ suffix: String) throws -> URL {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("envoix-manifest-v2-\(Self.runID)-\(suffix)", isDirectory: true)
        try? FileManager.default.removeItem(at: root)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    private func availableCapacity(at directory: URL) throws -> UInt64 {
        let values = try directory.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey])
        guard let capacity = values.volumeAvailableCapacityForImportantUsage, capacity > 0 else {
            throw PhysicalTestError.missingCapacity(directory.path)
        }
        return UInt64(capacity)
    }

    private static func request(direction: FfiTransferDirection) -> FfiTransferRequest {
        FfiTransferRequest(
            direction: direction,
            mode: .room,
            peerDescriptor: "",
            invite: "",
            code: scenarioCode,
            token: scenarioCode,
            broker: defaultRendezvousBroker,
            relay: defaultRelayURL,
            configPath: "",
            pathPolicy: .auto,
            rendezvous: FfiRendezvousPlan(useRoom: true, useMdns: false, internetAvailable: true)
        )
    }

    private static func fixture() throws -> ManifestV2Fixture {
        guard let scenario = ManifestV2Scenario(rawValue: scenarioName) else {
            throw PhysicalTestError.invalidScenario(scenarioName)
        }
        return ManifestV2Fixture.make(scenario: scenario, runID: runID, largeBytes: largeBytes)
    }

    private static func environmentUInt64(_ name: String, default fallback: UInt64) -> UInt64 {
        guard let raw = ProcessInfo.processInfo.environment[name], !raw.isEmpty else { return fallback }
        return UInt64(raw) ?? fallback
    }

    private static func environmentString(_ name: String, default fallback: String) -> String {
        let value = ProcessInfo.processInfo.environment[name]?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return value.isEmpty ? fallback : value
    }

    private static func marker(_ message: String) {
        FileHandle.standardError.write(Data("[cross-device] \(message)\n".utf8))
    }

    private static let settings = EnvoixRuntimeSettings(
        concurrentTransfers: false,
        language: "en",
        serverUrl: defaultRendezvousBroker,
        relayUrl: defaultRelayURL,
        configPath: "",
        speedLimitMbps: 0
    )
    private static let enabledEnvironment = "ENVOIX_CROSS_DEVICE"
    private static let runID = environmentString("ENVOIX_CROSS_DEVICE_RUN_ID", default: "manual")
    private static let scenarioName = environmentString("ENVOIX_CROSS_DEVICE_SCENARIO", default: "single_file")
    private static let scenarioCode = environmentString("ENVOIX_CROSS_DEVICE_CODE", default: "741203-amber-comet")
    private static let largeBytes = environmentUInt64("ENVOIX_CROSS_DEVICE_LARGE_BYTES", default: 128 * 1_024 * 1_024)
    private static let collisionSentinel = Data("pre-existing destination must remain unchanged\n".utf8)
    private static let shareRecoveryBytes = Data("valid source after an unreadable Share item\n".utf8)
    private static let deliveryProofDigestBytes = 32
    private static let platformName: String = {
        #if os(macOS)
        "macOS"
        #else
        "iOS"
        #endif
    }()
}

private enum ManifestV2Scenario: String {
    case singleFile = "single_file"
    case multipleFiles = "multiple_files"
    case folder
    case multipleFolders = "multiple_folders"
    case image
    case share
    case largeFile = "large_file"
    case collision
    case overlap
    case unicodeAndEmpty = "unicode_empty"
    case sameNameRoots = "same_name_roots"
}

private struct ManifestV2Fixture {
    let scenario: ManifestV2Scenario
    let roots: [FixtureRoot]
    let overlappingSelection: Bool

    var fileCount: Int { roots.reduce(0) { $0 + $1.files.count } }
    var directoryCount: Int { roots.reduce(0) { $0 + ($1.directory ? 1 + $1.directories.count : 0) } }
    var entryCount: Int { fileCount + directoryCount }
    var totalBytes: UInt64 { roots.flatMap(\.files).reduce(0) { $0 + $1.payload.size } }

    static func make(scenario: ManifestV2Scenario, runID: String, largeBytes: UInt64) -> ManifestV2Fixture {
        let text: (String, String) -> FixtureFile = { path, value in
            FixtureFile(path: path.split(separator: "/").map(String.init), payload: .data(Data(value.utf8)))
        }
        let file: (String, Data) -> FixtureRoot = { name, data in
            FixtureRoot(name: name, directory: false, directories: [], files: [FixtureFile(path: [], payload: .data(data))])
        }
        let roots: [FixtureRoot]
        let overlappingSelection: Bool
        switch scenario {
        case .singleFile:
            roots = [file("single-\(runID).txt", Data("single file fixture \(runID)\n".utf8))]
            overlappingSelection = false
        case .multipleFiles:
            roots = [
                file("alpha-\(runID).txt", Data("alpha\n".utf8)),
                file("beta-\(runID).bin", Data((0..<257).map { UInt8($0 % 251) })),
                file("空 白-\(runID).txt", Data("多文件内容\n".utf8)),
            ]
            overlappingSelection = false
        case .folder:
            roots = [FixtureRoot(
                name: "Folder-\(runID)",
                directory: true,
                directories: [["Empty"], ["Nested"], ["Nested", "深层"]],
                files: [
                    text("alpha.txt", "folder alpha\n"),
                    text("Nested/beta.bin", "folder beta\n"),
                    FixtureFile(path: ["Nested", "深层", "zero.dat"], payload: .data(Data())),
                ]
            )]
            overlappingSelection = false
        case .multipleFolders:
            roots = [
                FixtureRoot(
                    name: "First-\(runID)",
                    directory: true,
                    directories: [["Nested"]],
                    files: [text("one.txt", "first root\n"), text("Nested/two.txt", "nested root\n")]
                ),
                FixtureRoot(
                    name: "Second-\(runID)",
                    directory: true,
                    directories: [["Empty"]],
                    files: [FixtureFile(path: ["photo.png"], payload: .data(Self.pngData))]
                ),
            ]
            overlappingSelection = false
        case .image:
            roots = [file("photo-\(runID).png", Self.pngData)]
            overlappingSelection = false
        case .share:
            roots = [
                file("shared-note-\(runID).txt", Data("shared through platform provider\n".utf8)),
                file("shared-photo-\(runID).png", Self.pngData),
            ]
            overlappingSelection = false
        case .largeFile:
            roots = [FixtureRoot(
                name: "large-\(runID).bin",
                directory: false,
                directories: [],
                files: [FixtureFile(
                    path: [],
                    payload: .repeated(Data("large Manifest v2 fixture \(runID)\n".utf8), largeBytes)
                )]
            )]
            overlappingSelection = false
        case .collision:
            roots = [file("Photo.png", Self.pngData)]
            overlappingSelection = false
        case .overlap:
            roots = [FixtureRoot(
                name: "Overlap-\(runID)",
                directory: true,
                directories: [["Empty"], ["Nested"]],
                files: [text("inside.txt", "selected twice but sent once\n"), text("Nested/deep.bin", "deep\n")]
            )]
            overlappingSelection = true
        case .unicodeAndEmpty:
            roots = [FixtureRoot(
                name: "资料-\(runID)",
                directory: true,
                directories: [["空目录"], ["子 目录"]],
                files: [
                    text("résumé.txt", "naïve café\n"),
                    FixtureFile(path: ["子 目录", "照片 ①.png"], payload: .data(Self.pngData)),
                    FixtureFile(path: ["零字节.dat"], payload: .data(Data())),
                ]
            )]
            overlappingSelection = false
        case .sameNameRoots:
            roots = [
                file("duplicate.txt", Data("first duplicate root\n".utf8)),
                file("duplicate.txt", Data("second duplicate root\n".utf8)),
            ]
            overlappingSelection = false
        }
        return ManifestV2Fixture(scenario: scenario, roots: roots, overlappingSelection: overlappingSelection)
    }

    func materialize(in directory: URL) throws -> MaterializedFixture {
        let rootURLs = try roots.enumerated().map { index, root in
            let selectionDirectory = directory.appendingPathComponent("selection-\(index)", isDirectory: true)
            try FileManager.default.createDirectory(at: selectionDirectory, withIntermediateDirectories: true)
            return try root.materialize(in: selectionDirectory)
        }
        var selected = rootURLs
        if overlappingSelection, let root = rootURLs.first, let child = roots.first?.files.first {
            selected.append(child.path.reduce(root) { $0.appendingPathComponent($1) })
        }
        return MaterializedFixture(rootURLs: rootURLs, selectedURLs: selected)
    }

    private static let pngData = Data(base64Encoded:
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
    )!
}

private struct MaterializedFixture {
    let rootURLs: [URL]
    let selectedURLs: [URL]
}

private struct FixtureRoot {
    let name: String
    let directory: Bool
    let directories: [[String]]
    let files: [FixtureFile]

    func materialize(in parent: URL) throws -> URL {
        let root = parent.appendingPathComponent(name, isDirectory: directory)
        if directory {
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            for components in directories.sorted(by: { $0.count < $1.count }) {
                let url = components.reduce(root) { $0.appendingPathComponent($1, isDirectory: true) }
                try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
            }
        }
        for file in files {
            let url = file.path.reduce(root) { $0.appendingPathComponent($1) }
            if directory {
                try FileManager.default.createDirectory(
                    at: url.deletingLastPathComponent(),
                    withIntermediateDirectories: true
                )
            }
            try file.payload.write(to: url)
        }
        return root
    }

    func verify(at root: URL) throws {
        var isDirectory: ObjCBool = false
        XCTAssertTrue(FileManager.default.fileExists(atPath: root.path, isDirectory: &isDirectory))
        XCTAssertEqual(isDirectory.boolValue, directory)
        if directory {
            let expected = Set(
                directories.map { $0.joined(separator: "/") }
                    + files.map { $0.path.joined(separator: "/") }
            )
            let actual = Set(try FileManager.default.subpathsOfDirectory(atPath: root.path))
            XCTAssertEqual(actual, expected)
        }
        for file in files {
            let url = file.path.reduce(root) { $0.appendingPathComponent($1) }
            XCTAssertEqual(try Self.sha256(url), file.payload.digest)
        }
    }

    private static func sha256(_ url: URL) throws -> Data {
        var hasher = SHA256()
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        while let chunk = try handle.read(upToCount: 1_024 * 1_024), !chunk.isEmpty {
            hasher.update(data: chunk)
        }
        return Data(hasher.finalize())
    }
}

private struct FixtureFile {
    let path: [String]
    let payload: FixturePayload
}

private enum FixturePayload {
    case data(Data)
    case repeated(Data, UInt64)

    var size: UInt64 {
        switch self {
        case let .data(data): return UInt64(data.count)
        case let .repeated(_, size): return size
        }
    }

    var digest: Data {
        var hasher = SHA256()
        forEachChunk { hasher.update(data: $0) }
        return Data(hasher.finalize())
    }

    func write(to url: URL) throws {
        FileManager.default.createFile(atPath: url.path, contents: nil)
        let handle = try FileHandle(forWritingTo: url)
        defer { try? handle.close() }
        try forEachChunk { try handle.write(contentsOf: $0) }
    }

    private func forEachChunk(_ body: (Data) throws -> Void) rethrows {
        switch self {
        case let .data(data):
            try body(data)
        case let .repeated(pattern, expectedBytes):
            precondition(!pattern.isEmpty || expectedBytes == 0)
            var block = Data()
            if !pattern.isEmpty {
                let repeats = max(1, (1_024 * 1_024) / pattern.count)
                block.reserveCapacity(repeats * pattern.count)
                for _ in 0..<repeats { block.append(pattern) }
            }
            var remaining = expectedBytes
            while remaining > 0 {
                let count = Int(min(remaining, UInt64(block.count)))
                try body(block.prefix(count))
                remaining -= UInt64(count)
            }
        }
    }
}

private final class ManifestV2PhysicalObserver: TransferObserver, @unchecked Sendable {
    private let lock = NSLock()
    private var recordedPhases: [FfiManifestV2Phase] = []
    private var recordedFailure: String?

    var phases: [FfiManifestV2Phase] { locked { recordedPhases } }
    var failureMessage: String? { locked { recordedFailure } }

    func onInviteReady(invite _: String) {}
    func onStarted(itemCount: UInt32, totalBytes: UInt64) {
        marker("started items=\(itemCount) bytes=\(totalBytes)")
    }
    func onPhase(phase: FfiManifestV2Phase) {
        locked { recordedPhases.append(phase) }
        marker("phase=\(phase)")
    }
    func onProgress(transferred: UInt64, total: UInt64) {
        marker("progress=\(transferred)/\(total)")
    }
    func onCompleted(bytes: UInt64) { marker("completed bytes=\(bytes)") }
    func onTransferFailed(failure: FfiTransferFailure) {
        locked { recordedFailure = failure.diagnosticMessage }
        marker("failed code=\(failure.code) detail=\(failure.diagnosticMessage)")
    }
    func onDiagnostic(message: String) { marker("diagnostic=\(message)") }

    @discardableResult
    private func locked<T>(_ operation: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return operation()
    }

    private func marker(_ message: String) {
        FileHandle.standardError.write(Data("[cross-device] Apple Manifest v2 \(message)\n".utf8))
    }
}

private enum PhysicalTestError: Error {
    case invalidScenario(String)
    case missingCapacity(String)
}
