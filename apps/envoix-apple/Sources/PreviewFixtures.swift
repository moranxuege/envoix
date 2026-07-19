#if DEBUG
import EnvoixCore
import SwiftUI

enum PreviewFixtures {
    static let demoInvite = "envoix:demo-invite-token-for-preview-only"
    private static let completedReceiveFixture: (url: URL, bytes: UInt64) = {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("envoix-ui-completed-transfer.pdf")
        let data = Data("completed receive fixture\n".utf8)
        try? data.write(to: url, options: .atomic)
        return (url, UInt64(data.count))
    }()
    static let completedFolderReceiveFixture: (
        activity: FfiTransferActivityRecord,
        manifest: FfiManifestActivityRecord
    ) = {
        let fileManager = FileManager.default
        let destination = fileManager.temporaryDirectory
            .appendingPathComponent("envoix-ui-completed-folder", isDirectory: true)
        let album = destination.appendingPathComponent("Album", isDirectory: true)
        let nested = album.appendingPathComponent("Nested", isDirectory: true)
        let note = nested.appendingPathComponent("note.txt")
        let noteData = Data("nested received file fixture\n".utf8)

        try? fileManager.removeItem(at: destination)
        do {
            try fileManager.createDirectory(at: nested, withIntermediateDirectories: true)
            try noteData.write(to: note, options: .atomic)
        } catch {
            assertionFailure("Unable to create received folder UI fixture: \(error)")
        }

        let activityRecord = activity(
            id: "ui-completed-folder",
            state: .completed,
            direction: .receive,
            fileName: "Album",
            totalBytes: UInt64(noteData.count),
            bytesTransferred: UInt64(noteData.count),
            completedFilePath: destination.path
        )
        let entries = [
            manifestEntry(id: 0, path: "Album", kind: .directory),
            manifestEntry(id: 1, path: "Album/Nested", kind: .directory),
            manifestEntry(
                id: 2,
                path: "Album/Nested/note.txt",
                kind: .file,
                size: UInt64(noteData.count)
            ),
        ]
        let results = entries.map { entry in
            FfiManifestEntryResult(
                entryId: entry.entryId,
                status: .completed,
                offeredRelativePath: entry.relativePath,
                finalRelativePath: entry.relativePath,
                failureCode: ""
            )
        }
        return (
            activityRecord,
            FfiManifestActivityRecord(
                activity: activityRecord,
                manifestId: activityRecord.transferId,
                rootCount: 1,
                fileCount: 1,
                directoryCount: 2,
                completedFiles: 1,
                entries: entries,
                currentEntry: nil,
                entryResults: results
            )
        )
    }()

    static func idle() -> TransferViewModel {
        TransferViewModel()
    }

    static func waitingForSender() -> TransferViewModel {
        let viewModel = TransferViewModel()
        apply(activity(id: "preview-waiting", state: .waitingForPeer), to: viewModel)
        viewModel.invite = demoInvite
        viewModel.statusText = "Invite ready. Waiting for sender."
        return viewModel
    }

    static func transferring(name: String = "design-review.pdf") -> TransferViewModel {
        let viewModel = TransferViewModel()
        apply(activity(
            id: "preview-transferring",
            state: .transferring,
            direction: .send,
            fileName: name,
            totalBytes: 240_000_000,
            bytesTransferred: 92_000_000
        ), to: viewModel)
        viewModel.bytesPerSec = 12_400_000
        return viewModel
    }

    static func completedReceive() -> TransferViewModel {
        let viewModel = TransferViewModel()
        apply(activity(
            id: "preview-completed",
            state: .completed,
            direction: .receive,
            fileName: "field-notes.zip",
            totalBytes: 48_000_000,
            bytesTransferred: 48_000_000
        ), to: viewModel)
        viewModel.completedFileURL = URL(fileURLWithPath: "/Users/demo/Downloads/field-notes.zip")
        return viewModel
    }

    static func failed() -> TransferViewModel {
        let viewModel = TransferViewModel()
        apply(activity(
            id: "preview-failed",
            state: .failed,
            diagnosticMessage: "No device found. Check that the other side is running and the token or invite is correct."
        ), to: viewModel)
        return viewModel
    }

    static let activityRecords: [FfiTransferActivityRecord] = [
        activity(
            id: "ui-transferring",
            state: .transferring,
            direction: .send,
            fileName: "Kazam_screencast_00012.mp4",
            totalBytes: 459_624_246,
            bytesTransferred: 173_800_000
        ),
        activity(
            id: "ui-paused",
            state: .paused,
            direction: .receive,
            fileName: "field-observations-and-design-notes.zip",
            totalBytes: 92_000_000,
            bytesTransferred: 41_000_000
        ),
        activity(
            id: "ui-completed",
            state: .completed,
            direction: .receive,
            fileName: "completed-transfer.pdf",
            totalBytes: completedReceiveFixture.bytes,
            bytesTransferred: completedReceiveFixture.bytes,
            completedFilePath: completedReceiveFixture.url.path
        ),
        activity(
            id: "ui-failed",
            state: .failed,
            direction: .send,
            fileName: "retryable-archive.tar",
            totalBytes: 120_000_000,
            bytesTransferred: 26_000_000,
            diagnosticMessage: "connection lost; partial retained",
            retryable: true
        ),
        activity(
            id: "ui-publish-failed",
            state: .publishing,
            direction: .receive,
            fileName: "already-received.mov",
            totalBytes: 64_000_000,
            bytesTransferred: 64_000_000,
            diagnosticMessage: "selected Files folder is unavailable",
            retryable: true,
            recoveryAction: .chooseFolder
        ),
    ]

    static var activityMetrics: [String: ActivityMetrics] {
        var metrics = ActivityMetrics()
        metrics.speedBps = 12_400_000
        metrics.avgBps = 10_800_000
        metrics.peakBps = 16_200_000
        metrics.speedHistory = [8_000_000, 10_500_000, 12_400_000, 11_900_000]
        metrics.log = ["[12:00:01] connected via relay", "[12:00:04] transferring"]
        return ["ui-transferring": metrics]
    }

    private static func apply(_ record: FfiTransferActivityRecord, to viewModel: TransferViewModel) {
        viewModel.transferActivity = record
        viewModel.fileName = record.fileName
        viewModel.total = record.totalBytes
        viewModel.transferred = record.bytesTransferred
    }

    private static func manifestEntry(
        id: UInt32,
        path: String,
        kind: FfiManifestEntryKind,
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

    private static func activity(
        id: String,
        state: FfiTransferActivityState,
        direction: FfiTransferDirection = .receive,
        fileName: String = "",
        totalBytes: UInt64 = 0,
        bytesTransferred: UInt64 = 0,
        diagnosticMessage: String = "",
        retryable: Bool = false,
        recoveryAction: FfiRecoveryAction? = nil,
        completedFilePath: String = ""
    ) -> FfiTransferActivityRecord {
        FfiTransferActivityRecord(
            activityId: id,
            sequence: 1,
            attemptId: "attempt-1",
            state: state,
            direction: direction,
            mode: .room,
            transferId: "transfer-\(id)",
            fileName: fileName,
            totalBytes: totalBytes,
            bytesTransferred: bytesTransferred,
            bytesResumed: 0,
            speedBps: 0,
            averageSpeedBps: 0,
            createdAtMs: 1,
            updatedAtMs: 1,
            startedAtMs: 1,
            completedAtMs: state == .completed ? 1 : 0,
            completedFilePath: completedFilePath,
            dataPathKind: .relay,
            dataPathDetail: "https://envoix.chkxwlyh.us:8444/",
            invite: "",
            token: "",
            peerDescriptor: "ui-test-peer",
            diagnosticMessage: diagnosticMessage,
            failureCode: .unknown,
            failureCategory: .unknown,
            failurePhase: .transferring,
            failureOrigin: .unknown,
            userMessageKey: "",
            retryable: retryable,
            recoveryAction: recoveryAction ?? (retryable ? .retry : .none),
            limits: FfiTransferLimits(
                maxParallelTransfers: 2,
                maxParallelFiles: 1,
                maxParallelChunksPerFile: 1,
                speedLimitBps: 0
            )
        )
    }
}

private struct PreviewScreen<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        ZStack {
            Theme.bg.ignoresSafeArea()
            content
                .environmentObject(AppModel.shared)
                .padding(16)
                .frame(width: 520, height: 720)
        }
    }
}

#Preview("App Shell") {
    ContentView()
        .environmentObject(AppModel.shared)
        .frame(width: 960, height: 720)
}

#Preview("Send - Progress") {
    PreviewScreen {
        SendView(viewModel: PreviewFixtures.transferring())
    }
}

#Preview("Receive - Invite") {
    PreviewScreen {
        ReceiveView(viewModel: PreviewFixtures.waitingForSender())
    }
}

#Preview("Receive - Completed") {
    PreviewScreen {
        ReceiveView(viewModel: PreviewFixtures.completedReceive())
    }
}

#Preview("Status - Failed") {
    PreviewScreen {
        TransferStatusView(viewModel: PreviewFixtures.failed())
    }
}
#endif
