#if DEBUG
import EnvoixCore
import SwiftUI

enum PreviewFixtures {
    static let demoInvite = "envoix:demo-invite-token-for-preview-only"

    static func idle() -> TransferViewModel {
        TransferViewModel()
    }

    static func waitingForSender() -> TransferViewModel {
        let viewModel = TransferViewModel()
        viewModel.phase = .waiting
        viewModel.invite = demoInvite
        viewModel.statusText = "Invite ready. Waiting for sender."
        return viewModel
    }

    static func transferring(name: String = "design-review.pdf") -> TransferViewModel {
        let viewModel = TransferViewModel()
        viewModel.handleStarted(name, 240_000_000)
        viewModel.transferred = 92_000_000
        viewModel.bytesPerSec = 12_400_000
        return viewModel
    }

    static func completedReceive() -> TransferViewModel {
        let viewModel = TransferViewModel()
        viewModel.fileName = "field-notes.zip"
        viewModel.total = 48_000_000
        viewModel.transferred = 48_000_000
        viewModel.completedFileURL = URL(fileURLWithPath: "/Users/demo/Downloads/field-notes.zip")
        viewModel.phase = .completed(bytes: 48_000_000)
        return viewModel
    }

    static func failed() -> TransferViewModel {
        let viewModel = TransferViewModel()
        viewModel.phase = .failed("No device found. Check that the other side is running and the token or invite is correct.")
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
            totalBytes: 8_400_000,
            bytesTransferred: 8_400_000
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

    private static func activity(
        id: String,
        state: FfiTransferActivityState,
        direction: FfiTransferDirection,
        fileName: String,
        totalBytes: UInt64,
        bytesTransferred: UInt64,
        diagnosticMessage: String = "",
        retryable: Bool = false
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
            completedFilePath: "",
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
            recoveryAction: retryable ? .retry : .none,
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
