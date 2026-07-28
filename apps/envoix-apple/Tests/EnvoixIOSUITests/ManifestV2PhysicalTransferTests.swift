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
        let evidence = Self.endpointEvidence(fixture: fixture, role: "sender")
        try await runWithEndpointEvidence(evidence) {
            try await runSendScenario(fixture: fixture, evidence: evidence)
        }
    }

    func testReceiveScenarioManifestV2Room() async throws {
        try requirePhysicalRun()
        let fixture = try Self.fixture()
        let evidence = Self.endpointEvidence(fixture: fixture, role: "receiver")
        try await runWithEndpointEvidence(evidence) {
            try await runReceiveScenario(fixture: fixture, evidence: evidence)
        }
    }

    private func runSendScenario(
        fixture: ManifestV2Fixture,
        evidence: AppleMatrixEndpointEvidence
    ) async throws {
        let fileManager = FileManager.default
        let root = try makeTestRoot("send")
        var cleanupFailed = false
        defer {
            do {
                try fileManager.removeItem(at: root)
            } catch {
                cleanupFailed = true
            }
            evidence.recordCleanup(completed: !cleanupFailed)
        }
        let sourceDirectory = root.appendingPathComponent("sources", isDirectory: true)
        let jobStore = root.appendingPathComponent("jobs", isDirectory: true)
        let stateDirectory = root.appendingPathComponent("state", isDirectory: true)
        try fileManager.createDirectory(at: sourceDirectory, withIntermediateDirectories: true)
        try fileManager.createDirectory(at: jobStore, withIntermediateDirectories: true)
        try fileManager.createDirectory(at: stateDirectory, withIntermediateDirectories: true)

        let materialized = try fixture.materialize(in: sourceDirectory)
        try evidence.recordSource(roots: materialized.rootURLs)
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
                do {
                    try sharedDraft.store.discard(id: sharedDraft.id)
                } catch {
                    cleanupFailed = true
                }
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
        evidence.recordJobID(prepared.jobId)
        XCTAssertEqual(prepared.state, .readyToSend)
        XCTAssertEqual(prepared.inventory.rootCount, UInt32(fixture.roots.count))
        XCTAssertEqual(prepared.inventory.fileCount, UInt32(fixture.fileCount))
        XCTAssertEqual(prepared.inventory.directoryCount, UInt32(fixture.directoryCount))
        XCTAssertEqual(prepared.inventory.totalPlaintextBytes, fixture.totalBytes)
        XCTAssertEqual(prepared.inventory.warningCount, 0)
        _ = try await job.sealForSend()

        let observer = ManifestV2PhysicalObserver(evidence: evidence)
        let completion = try await sendTransferJobV2(
            job: job,
            settings: Self.settings,
            request: try Self.request(direction: .send),
            stateDirectory: stateDirectory.path,
            cancellation: FfiManifestV2Cancellation(),
            observer: observer
        )
        XCTAssertEqual(completion.entryCount, UInt32(fixture.entryCount))
        XCTAssertEqual(completion.totalPlaintextBytes, fixture.totalBytes)
        XCTAssertEqual(completion.deliveryProofDigest.count, Self.deliveryProofDigestBytes)
        evidence.recordDeliveryProof(
            completion.deliveryProofDigest.count == Self.deliveryProofDigestBytes
        )
        XCTAssertTrue(completion.savedPaths.isEmpty)
        XCTAssertTrue(observer.phases.contains(.waitingForReceiverSave))
        XCTAssertTrue(observer.phases.contains(.finalizingDelivery))
        XCTAssertTrue(observer.phases.contains(.delivered))
        XCTAssertNil(observer.failure)
        Self.marker("\(Self.platformName) send completed scenario=\(fixture.scenario.rawValue) bytes=\(fixture.totalBytes)")
    }

    private func runReceiveScenario(
        fixture: ManifestV2Fixture,
        evidence: AppleMatrixEndpointEvidence
    ) async throws {
        let fileManager = FileManager.default
        let root = try makeTestRoot("receive")
        defer {
            do {
                try fileManager.removeItem(at: root)
                evidence.recordCleanup(completed: true)
            } catch {
                evidence.recordCleanup(completed: false)
            }
        }
        let destination = root.appendingPathComponent("received", isDirectory: true)
        let stateDirectory = root.appendingPathComponent("state", isDirectory: true)
        try fileManager.createDirectory(at: destination, withIntermediateDirectories: true)
        try fileManager.createDirectory(at: stateDirectory, withIntermediateDirectories: true)

        var collisionURL: URL?
        if fixture.scenario == .collision {
            let url = destination.appendingPathComponent(fixture.roots[0].name)
            try Self.collisionSentinel.write(to: url, options: .atomic)
            collisionURL = url
        }

        let invitation = try makePairingInvite(
            role: .receive,
            broker: Self.settings.serverUrl,
            relay: Self.settings.relayUrl
        )
        defer { withExtendedLifetime(invitation) {} }
        Self.marker("invitation=\(invitation.roomCode)")
        let observer = ManifestV2PhysicalObserver(evidence: evidence)
        Self.marker("\(Self.platformName) receiver ready scenario=\(fixture.scenario.rawValue)")
        let pending = try await receiveTransferOfferV2(
            settings: Self.settings,
            request: try Self.request(
                direction: .receive,
                roomCode: invitation.roomCode
            ),
            stateDirectory: stateDirectory.path,
            cancellation: FfiManifestV2Cancellation(),
            observer: observer
        )
        evidence.recordOffer()

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
        evidence.recordDeliveryProof(
            completion.deliveryProofDigest.count == Self.deliveryProofDigestBytes
        )
        for (rootSpec, savedPath) in zip(fixture.roots, completion.savedPaths) {
            try rootSpec.verify(at: URL(fileURLWithPath: savedPath))
        }
        try evidence.recordDestination(
            roots: completion.savedPaths.map { URL(fileURLWithPath: $0) }
        )
        if let collisionURL {
            XCTAssertEqual(try Data(contentsOf: collisionURL), Self.collisionSentinel)
            XCTAssertNotEqual(completion.savedPaths.first, collisionURL.path)
        }
        XCTAssertTrue(observer.phases.contains(.saving))
        XCTAssertTrue(observer.phases.contains(.delivered))
        XCTAssertNil(observer.failure)
        Self.marker("\(Self.platformName) receive saved scenario=\(fixture.scenario.rawValue) bytes=\(fixture.totalBytes)")
    }

    private func runWithEndpointEvidence(
        _ evidence: AppleMatrixEndpointEvidence,
        operation: () async throws -> Void
    ) async throws {
        do {
            try await operation()
            try evidence.complete()
        } catch {
            evidence.fail()
            do {
                try evidence.attach(to: self)
            } catch {
                XCTFail("could not attach Apple matrix endpoint evidence: \(error)")
            }
            throw error
        }
        try evidence.attach(to: self)
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

    private static func request(
        direction: FfiTransferDirection,
        roomCode: String? = nil
    ) throws -> FfiTransferRequest {
        FfiTransferRequest(
            direction: direction,
            mode: .room,
            peerDescriptor: "",
            invite: "",
            code: try normalizeRoomCode(input: roomCode ?? scenarioCode),
            token: "",
            rememberConsent: false,
            rememberedCredentialRef: "",
            rememberedGeneration: 0,
            rememberedPreviousGeneration: nil,
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

    private static func endpointEvidence(
        fixture: ManifestV2Fixture,
        role: String
    ) -> AppleMatrixEndpointEvidence {
        AppleMatrixEndpointEvidence(
            fixture: fixture,
            runID: runID,
            caseID: environmentString("ENVOIX_CROSS_DEVICE_CASE_ID", default: "manual"),
            repetition: Int(
                environmentUInt64("ENVOIX_CROSS_DEVICE_REPETITION", default: 1)
            ),
            role: role,
            platform: platformIdentifier,
            buildVariant: environmentString(
                "ENVOIX_CROSS_DEVICE_BUILD_VARIANT",
                default: "debug"
            )
        )
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
    private static let scenarioCode = environmentString(
        "ENVOIX_CROSS_DEVICE_CODE",
        default: "741203-ambe-come"
    )
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
    private static let platformIdentifier: String = {
        #if os(macOS)
        "macos"
        #else
        "ios"
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

private final class AppleMatrixEndpointEvidence: @unchecked Sendable {
    private let lock = NSLock()
    private let fixture: ManifestV2Fixture
    private let runID: String
    private let caseID: String
    private let repetition: Int
    private let role: String
    private let platform: String
    private let buildVariant: String
    private let startedAt: UInt64
    private var phases: [String] = []
    private var selectedPath: String?
    private var nativeFailure: FfiTransferFailure?
    private var sourceSummary: [String: Any]?
    private var destinationSummary: [String: Any]?
    private var jobID: String?
    private var deliveryProof = false
    private var cleanupCompleted = false
    private var terminalState: String?

    init(
        fixture: ManifestV2Fixture,
        runID: String,
        caseID: String,
        repetition: Int,
        role: String,
        platform: String,
        buildVariant: String
    ) {
        self.fixture = fixture
        self.runID = runID
        self.caseID = caseID
        self.repetition = repetition
        self.role = role
        self.platform = platform
        self.buildVariant = buildVariant
        startedAt = Self.timestamp()
    }

    func recordPhase(_ phase: FfiManifestV2Phase) {
        appendPhase(Self.wirePhase(phase))
    }

    func recordOffer() {
        appendPhase("offer")
    }

    func recordFailure(_ failure: FfiTransferFailure) {
        locked { nativeFailure = failure }
    }

    func recordPath(_ path: FfiDataPathKind) {
        locked { selectedPath = Self.wirePath(path) }
    }

    func recordSource(roots: [URL]) throws {
        let summary = try Self.endpointSummary(roots: roots, publication: nil)
        locked { sourceSummary = summary }
    }

    func recordDestination(roots: [URL]) throws {
        let summary = try Self.endpointSummary(
            roots: roots,
            publication: [
                "mechanism": "test_local_directory",
                "committed": true,
            ]
        )
        locked { destinationSummary = summary }
    }

    func recordJobID(_ value: String) {
        locked { jobID = value }
    }

    func recordDeliveryProof(_ value: Bool) {
        locked { deliveryProof = value }
    }

    func recordCleanup(completed: Bool) {
        locked { cleanupCompleted = completed }
    }

    func complete() throws {
        try locked {
            guard deliveryProof else {
                throw AppleMatrixEvidenceError.missingDeliveryProof
            }
            guard cleanupCompleted else {
                throw AppleMatrixEvidenceError.cleanupIncomplete
            }
            if role == "sender", sourceSummary == nil {
                throw AppleMatrixEvidenceError.missingSummary("source")
            }
            if role == "receiver", destinationSummary == nil {
                throw AppleMatrixEvidenceError.missingSummary("destination")
            }
            terminalState = "completed"
            if phases.last != "completed" {
                phases.append("completed")
            }
        }
    }

    func fail() {
        locked {
            terminalState = "failed"
            if phases.last != "failed" {
                phases.append("failed")
            }
        }
    }

    func attach(to testCase: XCTestCase) throws {
        let data = try resultData()
        let attachment = XCTAttachment(
            data: data,
            uniformTypeIdentifier: "public.json"
        )
        attachment.name = "envoix-matrix-\(role).json"
        attachment.lifetime = .keepAlways
        testCase.add(attachment)
    }

    private func appendPhase(_ phase: String) {
        locked {
            if phases.last != phase {
                phases.append(phase)
            }
        }
    }

    private func resultData() throws -> Data {
        try locked {
            let finishedAt = Self.timestamp()
            guard let terminalState else {
                throw AppleMatrixEvidenceError.missingTerminalState
            }
            let coreInfo = envoixCoreInfo()
            let failure: Any
            if terminalState == "failed" {
                let fallbackPhase: String
                if !cleanupCompleted {
                    fallbackPhase = "cleanup"
                } else if phases == ["failed"] {
                    fallbackPhase = "setup"
                } else {
                    fallbackPhase = "driver_validation"
                }
                failure = [
                    "code": nativeFailure.map { Self.wireFailureCode($0.code) }
                        ?? "endpoint_assertion_failed",
                    "phase": nativeFailure.map { Self.wireFailurePhase($0.phase) }
                        ?? fallbackPhase,
                    "recovery_action": nativeFailure.map {
                        Self.wireRecoveryAction($0.recoveryAction)
                    } ?? "none",
                ]
            } else {
                failure = NSNull()
            }
            let capability = role == "receiver"
                ? "test_local_directory_publication"
                : "source_fixture"
            let result: [String: Any] = [
                "schema_version": 1,
                "run_id": runID,
                "case_id": caseID,
                "repetition": repetition,
                "role": role,
                "platform": platform,
                "test_layer": "l1_native",
                "driver": "direct_ffi",
                "build_variant": buildVariant,
                "app_version": Self.appVersion,
                "core_version": coreInfo.coreVersion,
                "protocol_version": 2,
                "device_model": Self.deviceModel,
                "os_version": ProcessInfo.processInfo.operatingSystemVersionString,
                "capabilities": ["manifest_v2", capability],
                "activity_id": NSNull(),
                "job_id": jobID as Any? ?? NSNull(),
                "started_at": startedAt,
                "finished_at": finishedAt,
                "terminal_state": terminalState,
                "ordered_phases": phases,
                "attempt_count": 1,
                "selected_path": selectedPath as Any? ?? NSNull(),
                "path_reason": NSNull(),
                "source_summary": sourceSummary as Any? ?? NSNull(),
                "destination_summary": destinationSummary as Any? ?? NSNull(),
                "delivery_proof": deliveryProof && terminalState == "completed",
                "failure": failure,
                "cleanup": [
                    "test_owned": true,
                    "completed": cleanupCompleted,
                ],
                "metrics": [
                    "plaintext_bytes": fixture.totalBytes,
                    "elapsed_ms": finishedAt - startedAt,
                ],
            ]
            return try JSONSerialization.data(
                withJSONObject: result,
                options: [.sortedKeys]
            )
        }
    }

    private static func endpointSummary(
        roots: [URL],
        publication: [String: Any]?
    ) throws -> [String: Any] {
        var entries: [AppleMatrixEntry] = []
        for root in roots {
            entries.append(contentsOf: try entriesFromRoot(root))
        }
        entries.sort {
            $0.relativePath.utf8.lexicographicallyPrecedes($1.relativePath.utf8)
        }
        let canonical = entries.map { entry in
            "\(entry.kind)\u{0}\(entry.relativePath)\u{0}\(entry.plaintextBytes)"
                + "\u{0}\(entry.sha256 ?? "-")\n"
        }.joined()
        return [
            "root_count": roots.count,
            "file_count": entries.filter { $0.kind == "file" }.count,
            "directory_count": entries.filter { $0.kind == "directory" }.count,
            "plaintext_bytes": entries
                .filter { $0.kind == "file" }
                .reduce(UInt64(0)) { $0 + $1.plaintextBytes },
            "manifest_digest": NSNull(),
            "tree_digest": Data(SHA256.hash(data: Data(canonical.utf8))).hex,
            "entries": entries.map(\.json),
            "publication": publication as Any? ?? NSNull(),
        ]
    }

    private static func entriesFromRoot(_ root: URL) throws -> [AppleMatrixEntry] {
        let values = try root.resourceValues(
            forKeys: [.isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey]
        )
        guard values.isSymbolicLink != true else {
            throw AppleMatrixEvidenceError.unsupportedFile(root.lastPathComponent)
        }
        if values.isRegularFile == true {
            return [try fileEntry(url: root, relativePath: root.lastPathComponent)]
        }
        guard values.isDirectory == true else {
            throw AppleMatrixEvidenceError.unsupportedFile(root.lastPathComponent)
        }

        var result = [
            AppleMatrixEntry(
                relativePath: root.lastPathComponent,
                kind: "directory",
                plaintextBytes: 0,
                sha256: nil
            ),
        ]
        var enumerationError: Error?
        guard let enumerator = FileManager.default.enumerator(
            at: root,
            includingPropertiesForKeys: [
                .isDirectoryKey,
                .isRegularFileKey,
                .isSymbolicLinkKey,
            ],
            options: [],
            errorHandler: { _, error in
                enumerationError = error
                return false
            }
        ) else {
            throw AppleMatrixEvidenceError.couldNotEnumerate(root.lastPathComponent)
        }
        for case let child as URL in enumerator {
            let childValues = try child.resourceValues(
                forKeys: [.isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey]
            )
            let suffix = child.path.dropFirst(root.path.count)
                .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            let relativePath = "\(root.lastPathComponent)/\(suffix)"
            if childValues.isDirectory == true {
                result.append(
                    AppleMatrixEntry(
                        relativePath: relativePath,
                        kind: "directory",
                        plaintextBytes: 0,
                        sha256: nil
                    )
                )
            } else if childValues.isRegularFile == true, childValues.isSymbolicLink != true {
                result.append(try fileEntry(url: child, relativePath: relativePath))
            } else {
                throw AppleMatrixEvidenceError.unsupportedFile(relativePath)
            }
        }
        if let enumerationError {
            throw enumerationError
        }
        return result
    }

    private static func fileEntry(
        url: URL,
        relativePath: String
    ) throws -> AppleMatrixEntry {
        let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
        guard let number = attributes[.size] as? NSNumber else {
            throw AppleMatrixEvidenceError.missingFileSize(relativePath)
        }
        var hasher = SHA256()
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        while let chunk = try handle.read(upToCount: 1_024 * 1_024), !chunk.isEmpty {
            hasher.update(data: chunk)
        }
        return AppleMatrixEntry(
            relativePath: relativePath,
            kind: "file",
            plaintextBytes: number.uint64Value,
            sha256: Data(hasher.finalize()).hex
        )
    }

    private static func wirePhase(_ value: FfiManifestV2Phase) -> String {
        switch value {
        case .pairing: return "pairing"
        case .connecting: return "connecting"
        case .transferring: return "transferring"
        case .verifying: return "verifying"
        case .saving: return "saving"
        case .waitingForReceiverSave: return "waiting_for_receiver_save"
        case .finalizingDelivery: return "finalizing_delivery"
        case .delivered: return "completed"
        case .waitingForPeer: return "waiting_for_peer"
        }
    }

    private static func wirePath(_ value: FfiDataPathKind) -> String {
        switch value {
        case .direct: return "direct"
        case .relay: return "relay"
        case .wifiAware: return "wifi_aware"
        case .other: return "other"
        }
    }

    private static func wireFailurePhase(_ value: FfiFailurePhase) -> String {
        switch value {
        case .setup: return "setup"
        case .pairing: return "pairing"
        case .connecting: return "connecting"
        case .authenticating: return "authenticating"
        case .negotiating: return "negotiating"
        case .transferring: return "transferring"
        case .verifying: return "verifying"
        case .committing: return "committing"
        }
    }

    private static func wireRecoveryAction(_ value: FfiRecoveryAction) -> String {
        switch value {
        case .retry: return "retry"
        case .resume: return "resume"
        case .chooseFolder: return "choose_folder"
        case .openSettings: return "open_settings"
        case .rePair: return "re_pair"
        case .none: return "none"
        }
    }

    private static func wireFailureCode(_ value: FfiFailureCode) -> String {
        switch value {
        case .userCanceled: return "user_canceled"
        case .networkLost: return "network_lost"
        case .authenticationFailed: return "authentication_failed"
        case .roomNotFound: return "room_not_found"
        case .roomExpired: return "room_expired"
        case .roomFull: return "room_full"
        case .roomRateLimited: return "room_rate_limited"
        case .roomUnderAttack: return "room_under_attack"
        case .endpointRateLimited: return "endpoint_rate_limited"
        case .ipRateLimited: return "ip_rate_limited"
        case .serverBusy: return "server_busy"
        case .malformedJoin: return "malformed_join"
        case .unsupportedRendezvousVersion: return "unsupported_rendezvous_version"
        case .unsupportedFeature: return "unsupported_feature"
        case .internalError: return "internal_error"
        case .senderSourceUnavailable: return "sender_source_unavailable"
        case .senderPermissionLost: return "sender_permission_lost"
        case .senderSourceChanged: return "sender_source_changed"
        case .senderItemRemoved: return "sender_item_removed"
        case .senderCanceled: return "sender_canceled"
        case .protocolOrIntegrityFailure: return "protocol_or_integrity_failure"
        case .receiverSpaceInsufficient: return "receiver_space_insufficient"
        case .receiverDestinationDecisionRequired:
            return "receiver_destination_decision_required"
        case .receiverDestinationUnavailable: return "receiver_destination_unavailable"
        case .receiverSaveFailed: return "receiver_save_failed"
        case .receiverReusedObjectLost: return "receiver_reused_object_lost"
        case .receiverFinalizationOutcomeUnknown:
            return "receiver_finalization_outcome_unknown"
        }
    }

    private static func timestamp() -> UInt64 {
        UInt64((Date().timeIntervalSince1970 * 1_000).rounded(.down))
    }

    private static let appVersion =
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
            ?? "unknown"
    private static let deviceModel: String = {
        #if os(macOS)
        "Mac"
        #else
        "iPhone"
        #endif
    }()

    @discardableResult
    private func locked<T>(_ operation: () throws -> T) rethrows -> T {
        lock.lock()
        defer { lock.unlock() }
        return try operation()
    }
}

private struct AppleMatrixEntry {
    let relativePath: String
    let kind: String
    let plaintextBytes: UInt64
    let sha256: String?

    var json: [String: Any] {
        [
            "relative_path": relativePath,
            "kind": kind,
            "plaintext_bytes": plaintextBytes,
            "sha256": sha256 as Any? ?? NSNull(),
            "disposition": "completed",
        ]
    }
}

private final class ManifestV2PhysicalObserver: TransferObserver, @unchecked Sendable {
    private let lock = NSLock()
    private let evidence: AppleMatrixEndpointEvidence
    private var recordedPhases: [FfiManifestV2Phase] = []
    private var recordedFailure: FfiTransferFailure?

    var phases: [FfiManifestV2Phase] { locked { recordedPhases } }
    var failure: FfiTransferFailure? { locked { recordedFailure } }

    init(evidence: AppleMatrixEndpointEvidence) {
        self.evidence = evidence
    }

    func onInviteReady(invite _: String) {}
    func onStarted(itemCount: UInt32, totalBytes: UInt64) {
        marker("started items=\(itemCount) bytes=\(totalBytes)")
    }
    func onPhase(phase: FfiManifestV2Phase) {
        locked { recordedPhases.append(phase) }
        evidence.recordPhase(phase)
        marker("phase=\(phase)")
    }
    func onProgress(transferred: UInt64, total: UInt64) {
        marker("progress=\(transferred)/\(total)")
    }
    func onCompleted(bytes: UInt64) { marker("completed bytes=\(bytes)") }
    func onTransferFailed(failure: FfiTransferFailure) {
        locked { recordedFailure = failure }
        evidence.recordFailure(failure)
        marker("failed code=\(failure.code)")
    }
    func onConnectionPath(event: FfiConnectionPathEvent) {
        evidence.recordPath(event.pathKind)
        marker("path=\(event.pathKind) event=\(event.eventKind)")
    }
    func onDiagnostic(message: String) { marker("diagnostic=\(message)") }
    func onRememberedCredential(opaqueCredential _: Data, generation _: UInt64) -> Bool { false }

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

private enum AppleMatrixEvidenceError: Error {
    case cleanupIncomplete
    case couldNotEnumerate(String)
    case missingDeliveryProof
    case missingFileSize(String)
    case missingSummary(String)
    case missingTerminalState
    case unsupportedFile(String)
}

private enum PhysicalTestError: Error {
    case invalidScenario(String)
    case missingCapacity(String)
}

private extension Data {
    var hex: String {
        map { String(format: "%02x", $0) }.joined()
    }
}
