import Foundation
import XCTest
import EnvoixCore
@testable import Envoix_iOS

final class TransferPresentationPolicyTests: XCTestCase {
    func testActivityCatalogProvidesStaticScreenCopy() {
        let cases: [(String, String, String)] = [
            ("accessibility.collapsed", "Collapsed", "已收起"),
            ("accessibility.expanded", "Expanded", "已展开"),
            ("activity.accessibility.collapse_hint", "Double-tap to collapse transfer details.", "轻点两下以收起传输详情。"),
            ("activity.accessibility.expand_hint", "Double-tap to expand transfer details.", "轻点两下以展开传输详情。"),
            ("activity.diagnostics.copied", "Diagnostics copied", "诊断信息已复制"),
            ("activity.diagnostics.copy", "Copy diagnostics", "复制诊断信息"),
            ("activity.diagnostics.copy_app", "Copy app diagnostics", "复制应用诊断信息"),
            ("activity.diagnostics.upload", "Upload diagnostics", "上传诊断信息"),
            ("activity.empty.detail", "Prepared and active transfers will appear here.", "准备中和活动中的传输会显示在这里。"),
            ("activity.empty.title", "No transfers yet", "暂无传输"),
            ("activity.group.one_time_room", "One-time Room", "一次性房间"),
            ("activity.group.standalone", "Standalone transfer", "独立传输"),
            ("activity.incoming", "Incoming transfer", "待接收内容"),
            ("activity.outgoing", "Outgoing transfer", "待发送内容"),
            ("activity.saved.view_items", "View received items", "查看已接收项目"),
            ("activity.timeline", "Transfer timeline", "传输时间线"),
            ("common.cancel", "Cancel", "取消"),
            ("common.open", "Open", "打开"),
            ("common.pause", "Pause", "暂停"),
            ("common.receive", "Receive", "接收"),
            ("common.resume", "Resume", "恢复"),
            ("common.share", "Share", "分享"),
        ]
        for (key, english, chinese) in cases {
            XCTAssertEqual(AppText.localized(key, language: "en"), english)
            XCTAssertEqual(AppText.localized(key, language: "zh-Hans"), chinese)
        }
    }

    func testActivityTextProjectsEveryStateAndStageFromSemanticCatalog() {
        let stateCases: [(
            TransferActivityState,
            FfiTransferDirection,
            String,
            String
        )] = [
            (.preparing, .send, "Preparing locally", "正在本地准备"),
            (.waitingForPeer, .send, "Waiting for peer", "正在等待对端"),
            (.pairing, .send, "Pairing", "正在配对"),
            (.connecting, .send, "Connecting", "正在连接"),
            (.awaitingDecision, .receive, "Waiting for your decision", "等待你的确认"),
            (.transferring, .send, "Sending", "正在发送"),
            (.transferring, .receive, "Receiving", "正在接收"),
            (.verifying, .receive, "Verifying", "正在校验"),
            (.saving, .receive, "Saving to destination", "正在保存到目标位置"),
            (.waitingForReceiverSave, .send, "Waiting for receiver to save", "等待接收方完成保存"),
            (.finalizingDelivery, .send, "Saved; finalizing delivery", "已保存，正在完成交付确认"),
            (.paused, .send, "Paused", "已暂停"),
            (.delivered, .send, "Delivered", "已送达"),
            (.delivered, .receive, "Received", "已接收"),
            (.failed, .send, "Failed", "失败"),
            (.canceled, .send, "Canceled", "已取消"),
        ]
        for (state, direction, english, chinese) in stateCases {
            XCTAssertEqual(
                TransferActivityText.state(state, direction: direction, language: "en"),
                english
            )
            XCTAssertEqual(
                TransferActivityText.state(state, direction: direction, language: "zh-Hans"),
                chinese
            )
        }

        let stageCases: [(FfiTransferStage, String, String)] = [
            (.sessionStarted, "Started", "开始"),
            (.connectionReady, "Connected", "连接完成"),
            (.authenticationStarted, "Authenticating", "开始认证"),
            (.authenticationComplete, "Authenticated", "认证完成"),
            (.manifestOffer, "Offer", "清单送达"),
            (.manifestAccepted, "Accepted", "清单确认"),
            (.firstPayload, "First byte", "首个数据"),
            (.payloadComplete, "Payload complete", "数据完成"),
            (.deliveryComplete, "Delivered", "交付完成"),
            (.canceled, "Canceled", "已取消"),
            (.failed, "Failed", "失败"),
        ]
        for (stage, english, chinese) in stageCases {
            XCTAssertEqual(TransferActivityText.stage(stage, language: "en"), english)
            XCTAssertEqual(TransferActivityText.stage(stage, language: "zh-Hans"), chinese)
        }

        XCTAssertEqual(TransferActivityText.direction(.send, language: "en"), "Send")
        XCTAssertEqual(TransferActivityText.direction(.receive, language: "zh-Hans"), "接收")
    }

    func testActivityTextUsesLocalizedFormatsAndPluralRules() {
        XCTAssertEqual(TransferActivityText.itemCount(0, language: "en"), "0 items")
        XCTAssertEqual(TransferActivityText.itemCount(1, language: "en"), "1 item")
        XCTAssertEqual(TransferActivityText.itemCount(2, language: "en"), "2 items")
        XCTAssertEqual(TransferActivityText.itemCount(2, language: "zh-Hans"), "2 个项目")
        XCTAssertEqual(
            TransferActivityText.itemCount(UInt64.max, language: "en"),
            "9,223,372,036,854,775,807 items"
        )

        XCTAssertEqual(TransferActivityText.transferCount(1, language: "en"), "1 transfer")
        XCTAssertEqual(TransferActivityText.transferCount(3, language: "zh-Hans"), "3 次传输")
        XCTAssertEqual(TransferActivityText.updated("2 min ago", language: "en"), "Updated 2 min ago")
        XCTAssertEqual(TransferActivityText.updated("2 分钟前", language: "zh-Hans"), "2 分钟前更新")
        XCTAssertEqual(TransferActivityText.savedIn("Downloads", language: "en"), "Saved in Downloads")
        XCTAssertEqual(TransferActivityText.savedIn("下载", language: "zh-Hans"), "已保存到 下载")
        XCTAssertEqual(TransferActivityText.savedItems(1, language: "en"), "Saved 1 item")
        XCTAssertEqual(TransferActivityText.savedItems(4, language: "zh-Hans"), "已保存 4 个项目")
    }

    func testFriendlyFailureProjectsEveryTypedFailureWithoutLeakingDiagnostics() {
        let cases: [(FfiFailureCode, String, String)] = [
            (.userCanceled, "Transfer canceled.", "传输已取消。"),
            (.senderCanceled, "Transfer canceled.", "传输已取消。"),
            (.networkLost, "Connection lost. Resume to continue.", "连接已断开，可恢复继续。"),
            (.authenticationFailed, "Pairing authentication failed.", "配对认证失败。"),
            (
                .roomNotFound,
                "The Room is not available yet. Ask the creator to keep it open and retry.",
                "房间尚不可用。请让创建者保持房间开启后重试。"
            ),
            (.roomExpired, "This Room expired. Create a new Room Code.", "此房间已过期。请创建新的房间码。"),
            (.roomFull, "This Room is already in use. Retry shortly.", "此房间正在使用中。请稍后重试。"),
            (.roomRateLimited, "Too many Room attempts. Wait before retrying.", "房间尝试次数过多。请稍后再试。"),
            (.endpointRateLimited, "Too many Room attempts. Wait before retrying.", "房间尝试次数过多。请稍后再试。"),
            (.ipRateLimited, "Too many Room attempts. Wait before retrying.", "房间尝试次数过多。请稍后再试。"),
            (
                .roomUnderAttack,
                "This Room was closed for security. Create a new Room Code.",
                "此房间因安全原因已关闭。请创建新的房间码。"
            ),
            (.serverBusy, "The Room service is busy. Retry shortly.", "房间服务繁忙。请稍后重试。"),
            (.malformedJoin, "Update Envoix before joining this Room.", "请更新 Envoix 后再加入此房间。"),
            (.unsupportedRendezvousVersion, "Update Envoix before joining this Room.", "请更新 Envoix 后再加入此房间。"),
            (
                .senderPermissionLost,
                "Source permission expired. Choose the source again.",
                "来源权限已失效，请重新选择。"
            ),
            (.senderSourceUnavailable, "A selected source is unavailable.", "所选来源不可用。"),
            (.senderItemRemoved, "A selected source is unavailable.", "所选来源不可用。"),
            (.senderSourceChanged, "Content verification failed.", "内容校验失败。"),
            (.protocolOrIntegrityFailure, "Content verification failed.", "内容校验失败。"),
            (
                .receiverSpaceInsufficient,
                "The destination does not have enough space.",
                "目标位置空间不足。"
            ),
            (
                .receiverDestinationDecisionRequired,
                "Choose an available destination.",
                "请选择可用的目标位置。"
            ),
            (
                .receiverDestinationUnavailable,
                "Choose an available destination.",
                "请选择可用的目标位置。"
            ),
            (
                .receiverSaveFailed,
                "The receiver could not finish saving. Resume to reconcile it.",
                "接收端未能完成保存，请恢复以进行确认。"
            ),
            (
                .receiverReusedObjectLost,
                "An existing destination item selected for reuse changed or disappeared. Restore it and resume, or start a new transfer.",
                "接收端原定复用的已有项目已更改或消失。请恢复该项目后继续，或重新发起传输。"
            ),
            (
                .receiverFinalizationOutcomeUnknown,
                "The receiver cannot yet confirm the final save after an interruption. Resume to reconcile the destination.",
                "中断后接收端暂时无法确认最终保存结果，请恢复传输以核对目标位置。"
            ),
            (.unsupportedFeature, "This transfer request is not supported.", "不支持此传输请求。"),
            (.internalError, "The transfer failed.", "传输失败。"),
        ]

        for (code, english, chinese) in cases {
            XCTAssertEqual(
                friendlyFailure(code: code, diagnosticMessage: "not user-facing", language: "en"),
                english
            )
            XCTAssertEqual(
                friendlyFailure(
                    code: code,
                    diagnosticMessage: "not user-facing",
                    language: "zh-Hans"
                ),
                chinese
            )
        }
        XCTAssertEqual(friendlyError("disk", language: "en"), "Transfer failed: disk")
        XCTAssertEqual(friendlyError("磁盘", language: "zh-Hans"), "传输失败：磁盘")
    }

    func testTransferStatusTitlesCoverEveryLifecycleState() {
        let cases: [(TransferActivityState?, FfiTransferDirection?, String, String)] = [
            (nil, nil, "", "Selection status"),
            (.preparing, .send, "", "Preparing locally"),
            (.waitingForPeer, .send, "", "Waiting for the other device"),
            (.pairing, .send, "", "Pairing devices"),
            (.connecting, .send, "", "Connecting"),
            (.awaitingDecision, .receive, "", "Review incoming transfer"),
            (.transferring, .send, "", "Sending"),
            (.transferring, .receive, "", "Receiving"),
            (.transferring, .send, "report.pdf", "report.pdf"),
            (.verifying, .receive, "", "Verifying"),
            (.saving, .receive, "", "Saving"),
            (.waitingForReceiverSave, .send, "", "Waiting for receiver to save"),
            (.finalizingDelivery, .send, "", "Finalizing delivery"),
            (.paused, .send, "", "Transfer paused"),
            (.delivered, .send, "", "Delivered"),
            (.delivered, .receive, "", "Received"),
            (.delivered, nil, "", "Delivered"),
            (.canceled, .send, "", "Transfer canceled"),
            (.failed, .send, "", "Custom failure"),
        ]
        for (state, direction, fileName, expected) in cases {
            XCTAssertEqual(
                TransferStatusText.title(
                    state: state,
                    direction: direction,
                    fileName: fileName,
                    failureTitle: state == .failed ? "Custom failure" : nil,
                    language: "en"
                ),
                expected
            )
        }
        XCTAssertEqual(
            TransferStatusText.title(
                state: .paused,
                direction: .send,
                fileName: "",
                language: "zh-Hans"
            ),
            "传输已暂停"
        )
    }

    func testTransferStatusDetailsCoverEveryLifecycleState() {
        let cases: [(TransferActivityState?, FfiTransferDirection?, String, String?)] = [
            (nil, nil, "", nil),
            (nil, nil, "Selecting", "Selecting"),
            (.preparing, .send, "", "Reading and validating the selected items."),
            (.waitingForPeer, .send, "", "Keep this window open until the peer connects."),
            (.pairing, .send, "", "Keep both devices awake while the connection is established."),
            (.connecting, .send, "", "Keep both devices awake while the connection is established."),
            (.awaitingDecision, .receive, "", "Review the authenticated inventory before accepting."),
            (.awaitingDecision, .receive, "Custom", "Custom"),
            (.transferring, .send, "", "Keep both devices awake until payload transfer finishes."),
            (.verifying, .receive, "", "Checking received content before publication."),
            (.saving, .receive, "", "Payload is complete; delivery is still being finalized."),
            (.waitingForReceiverSave, .send, "", "Payload is complete; delivery is still being finalized."),
            (.finalizingDelivery, .send, "", "Payload is complete; delivery is still being finalized."),
            (.paused, .send, "", "Resume or remove this transfer from Activity."),
            (.delivered, .receive, "", "The received content is ready."),
            (.delivered, .send, "", "The receiver confirmed the saved content."),
            (.canceled, .send, "", "Ready to start another transfer."),
        ]
        for (state, direction, status, expected) in cases {
            XCTAssertEqual(
                TransferStatusText.detail(
                    state: state,
                    direction: direction,
                    statusText: status,
                    language: "en"
                ),
                expected
            )
        }
        XCTAssertEqual(
            TransferStatusText.detail(
                state: .failed,
                direction: .send,
                statusText: "raw",
                failureDetail: "Friendly",
                language: "en"
            ),
            "Friendly"
        )
        XCTAssertEqual(
            TransferStatusText.detail(
                state: .failed,
                direction: .send,
                statusText: "raw",
                language: "en"
            ),
            "raw"
        )
    }

    func testTransferStatusLastStepOnlySurfacesTrimmedFailureState() {
        XCTAssertNil(TransferStatusText.lastStep(
            state: .connecting,
            statusText: "dialing",
            language: "en"
        ))
        XCTAssertNil(TransferStatusText.lastStep(
            state: .failed,
            statusText: "  ",
            language: "en"
        ))
        XCTAssertEqual(
            TransferStatusText.lastStep(
                state: .failed,
                statusText: "  authenticating  ",
                language: "en"
            ),
            "Last step: authenticating"
        )
        XCTAssertEqual(
            TransferStatusText.lastStep(
                state: .failed,
                statusText: "认证",
                language: "zh-Hans"
            ),
            "上一步：认证"
        )
    }

    func testTransferFailureTitlesAndFallbacksUsePresentationCatalog() {
        let cases: [(FfiFailureCode, String)] = [
            (.userCanceled, "Transfer canceled"),
            (.networkLost, "Connection failed"),
            (.authenticationFailed, "Pairing failed"),
            (.roomNotFound, "Room unavailable"),
            (.roomExpired, "Room expired"),
            (.roomFull, "Room in use"),
            (.endpointRateLimited, "Try again later"),
            (.roomUnderAttack, "New Room required"),
            (.serverBusy, "Service busy"),
            (.malformedJoin, "Update required"),
            (.unsupportedFeature, "Update required"),
            (.internalError, "Transfer failed"),
            (.senderPermissionLost, "Source unavailable"),
            (.protocolOrIntegrityFailure, "Verification failed"),
            (.receiverSpaceInsufficient, "Not enough space"),
            (.receiverFinalizationOutcomeUnknown, "Could not save"),
        ]
        for (code, expected) in cases {
            XCTAssertEqual(TransferStatusText.failureTitle(code, language: "en"), expected)
        }

        XCTAssertEqual(
            TransferStatusText.fallbackFailure(
                reason: "mDNS: 0 peers discovered",
                language: "en"
            ),
            TransferFailurePresentationCopy(
                title: "No device found on the local network",
                detail: "Make sure the other device is receiving with the same token and both devices are on the same network."
            )
        )
        XCTAssertEqual(
            TransferStatusText.fallbackFailure(reason: "  ", language: "zh-Hans"),
            TransferFailurePresentationCopy(
                title: "传输失败",
                detail: "请重试；如果一直无法发现设备，请切换配对方式。"
            )
        )
        XCTAssertEqual(
            TransferStatusText.fallbackFailure(reason: "  raw detail  ", language: "en"),
            TransferFailurePresentationCopy(title: "Transfer failed", detail: "raw detail")
        )
    }

    func testTransferStatusCatalogProvidesStaticCopy() {
        let cases: [(String, String, String)] = [
            ("common.copy", "Copy", "复制"),
            ("transfer.status.completed.copy_path", "Copy Path", "复制路径"),
            ("transfer.status.completed.path_copied", "Path copied", "路径已复制"),
            (
                "transfer.status.inventory.approve_large",
                "Receive this large transfer",
                "接收此大文件传输"
            ),
            ("transfer.status.inventory.title", "Incoming items", "即将接收"),
            ("transfer.status.log.copied", "Log copied", "日志已复制"),
            ("transfer.status.log.title", "Activity log", "活动日志"),
            ("transfer.status.log.verbose", "Verbose", "详细"),
            ("transfer.status.metric.average", "Average", "平均"),
            ("transfer.status.metric.now", "Now", "当前"),
        ]
        for (key, english, chinese) in cases {
            XCTAssertEqual(AppText.localized(key, language: "en"), english, key)
            XCTAssertEqual(AppText.localized(key, language: "zh-Hans"), chinese, key)
        }
    }

    func testTransferContentAndCompletedItemFormatsUseNativePluralRules() {
        XCTAssertEqual(TransferContentText.rootCount(1, language: "en"), "1 root")
        XCTAssertEqual(TransferContentText.rootCount(2, language: "en"), "2 roots")
        XCTAssertEqual(TransferContentText.rootCount(2, language: "zh-Hans"), "2 个根项目")
        XCTAssertEqual(TransferContentText.fileCount(1, language: "en"), "1 file")
        XCTAssertEqual(TransferContentText.folderCount(2, language: "en"), "2 folders")

        XCTAssertEqual(
            TransferStatusText.additionalManifestItems(1, language: "en"),
            "1 more item is included in the authenticated manifest."
        )
        XCTAssertEqual(
            TransferStatusText.additionalManifestItems(2, language: "zh-Hans"),
            "已认证清单中还包含 2 个项目。"
        )
        XCTAssertEqual(
            TransferStatusText.additionalManifestItems(-1, language: "en"),
            "0 more items are included in the authenticated manifest."
        )
        XCTAssertEqual(
            TransferStatusText.inventorySummary(
                rootCount: 1,
                fileCount: 2,
                folderCount: 3,
                byteDescription: "4 MB",
                language: "en"
            ),
            "1 root · 2 files · 3 folders · 4 MB"
        )

        XCTAssertEqual(TransferStatusText.viewItems(1, language: "en"), "View 1 Item")
        XCTAssertEqual(TransferStatusText.viewItems(2, language: "en"), "View 2 Items")
        XCTAssertEqual(TransferStatusText.viewItems(-1, language: "en"), "View 0 Items")
        XCTAssertEqual(TransferStatusText.receivedItems(1, language: "en"), "1 received item")
        XCTAssertEqual(TransferStatusText.receivedItems(2, language: "zh-Hans"), "已接收 2 个项目")
        XCTAssertEqual(TransferStatusText.savedAs("报告.pdf", language: "zh-Hans"), "已保存为 报告.pdf")
    }

    @MainActor
    func testNativeObserverHopsBackgroundCallbacksToMainActor() async {
        let model = TransferViewModel()
        let observer = model.makeTransferObserver(activityID: nil)

        let callbackUsedMainThread = await withCheckedContinuation { continuation in
            DispatchQueue.global().async {
                let usedMainThread = Thread.isMainThread
                observer.onStarted(itemCount: 2, totalBytes: 42)
                observer.onDiagnostic(message: "background callback")
                continuation.resume(returning: usedMainThread)
            }
        }

        for _ in 0..<100 where model.total != 42 || model.eventLog.isEmpty {
            await Task.yield()
        }

        XCTAssertFalse(callbackUsedMainThread)
        XCTAssertEqual(model.fileName, "2 items")
        XCTAssertEqual(model.total, 42)
        XCTAssertEqual(model.eventLog.last, "background callback")
    }

    func testRateTrackerFirstSampleOnlyEstablishesBaseline() {
        var tracker = RateTracker()
        let start = Date(timeIntervalSinceReferenceDate: 1_000)

        XCTAssertEqual(tracker.update(bytes: 512, now: start), 0)
        XCTAssertEqual(tracker.samples, 0)
        XCTAssertFalse(tracker.isStable)
        XCTAssertEqual(tracker.averageBytesPerSecond, 0)
    }

    func testRateTrackerSmoothsCurrentRateAndBecomesStableAfterTwoIntervals() {
        var tracker = RateTracker()
        let start = Date(timeIntervalSinceReferenceDate: 1_000)
        _ = tracker.update(bytes: 0, now: start)

        XCTAssertEqual(
            tracker.update(bytes: 100, now: start.addingTimeInterval(1)),
            100,
            accuracy: 0.000_001
        )
        XCTAssertFalse(tracker.isStable)
        XCTAssertEqual(
            tracker.update(bytes: 300, now: start.addingTimeInterval(2)),
            130,
            accuracy: 0.000_001
        )
        XCTAssertTrue(tracker.isStable)
        XCTAssertEqual(tracker.averageBytesPerSecond, 150, accuracy: 0.000_001)
    }

    func testRateTrackerCoalescesShortIntervalsWithoutLosingBytes() {
        var tracker = RateTracker()
        let start = Date(timeIntervalSinceReferenceDate: 1_000)
        _ = tracker.update(bytes: 0, now: start)

        XCTAssertEqual(
            tracker.update(bytes: 50, now: start.addingTimeInterval(0.05)),
            0
        )
        XCTAssertEqual(tracker.samples, 0)
        XCTAssertEqual(tracker.averageBytesPerSecond, 0)
        XCTAssertEqual(
            tracker.update(bytes: 150, now: start.addingTimeInterval(1.05)),
            150.0 / 1.05,
            accuracy: 0.000_001
        )
        XCTAssertEqual(
            tracker.averageBytesPerSecond,
            150.0 / 1.05,
            accuracy: 0.000_001
        )
    }

    func testRateTrackerSamplesAtTheSharedOneHundredMillisecondBoundary() {
        var tracker = RateTracker()
        let start = Date(timeIntervalSinceReferenceDate: 1_000)
        _ = tracker.update(bytes: 0, now: start)

        XCTAssertEqual(
            tracker.update(bytes: 100, now: start.addingTimeInterval(0.1)),
            1_000,
            accuracy: 0.000_001
        )
        XCTAssertEqual(tracker.samples, 1)
    }

    func testRateTrackerForceSamplesFastCompletion() {
        var tracker = RateTracker()
        let start = Date(timeIntervalSinceReferenceDate: 1_000)
        _ = tracker.update(bytes: 0, now: start)

        XCTAssertEqual(
            tracker.update(
                bytes: 100,
                now: start.addingTimeInterval(0.05),
                forceSample: true
            ),
            2_000,
            accuracy: 0.000_001
        )
        XCTAssertEqual(tracker.samples, 1)
        XCTAssertEqual(tracker.averageBytesPerSecond, 2_000, accuracy: 0.000_001)
    }

    func testRateTrackerByteRollbackResetsBaselineWithoutNegativeThroughput() {
        var tracker = RateTracker()
        let start = Date(timeIntervalSinceReferenceDate: 1_000)
        _ = tracker.update(bytes: 100, now: start)
        _ = tracker.update(bytes: 200, now: start.addingTimeInterval(1))

        XCTAssertEqual(tracker.update(bytes: 50, now: start.addingTimeInterval(2)), 0)
        XCTAssertEqual(tracker.samples, 0)
        XCTAssertEqual(tracker.averageBytesPerSecond, 0)
        XCTAssertEqual(
            tracker.update(bytes: 150, now: start.addingTimeInterval(3)),
            100,
            accuracy: 0.000_001
        )
        XCTAssertEqual(tracker.samples, 1)
        XCTAssertEqual(tracker.averageBytesPerSecond, 100, accuracy: 0.000_001)
    }

    func testRateTrackerAverageIncludesZeroByteStallIntervals() {
        var tracker = RateTracker()
        let start = Date(timeIntervalSinceReferenceDate: 1_000)
        _ = tracker.update(bytes: 0, now: start)
        _ = tracker.update(bytes: 100, now: start.addingTimeInterval(1))

        XCTAssertEqual(
            tracker.update(bytes: 100, now: start.addingTimeInterval(3)),
            70,
            accuracy: 0.000_001
        )
        XCTAssertFalse(tracker.isStable)
        XCTAssertEqual(tracker.averageBytesPerSecond, 100.0 / 3.0, accuracy: 0.000_001)
    }

    func testEstimatedRemainingSecondsRejectsUnstableAndInvalidBoundaries() {
        XCTAssertNil(estimatedRemainingSeconds(
            total: 100,
            transferred: 50,
            bytesPerSecond: 10,
            isStable: false
        ))
        XCTAssertNil(estimatedRemainingSeconds(
            total: 100,
            transferred: 100,
            bytesPerSecond: 10,
            isStable: true
        ))
        XCTAssertNil(estimatedRemainingSeconds(
            total: 100,
            transferred: 50,
            bytesPerSecond: 0,
            isStable: true
        ))
        XCTAssertNil(estimatedRemainingSeconds(
            total: 100,
            transferred: 50,
            bytesPerSecond: .infinity,
            isStable: true
        ))
        XCTAssertNil(estimatedRemainingSeconds(
            total: UInt64.max,
            transferred: 0,
            bytesPerSecond: .leastNonzeroMagnitude,
            isStable: true
        ))
        XCTAssertEqual(
            estimatedRemainingSeconds(
                total: 100,
                transferred: 25,
                bytesPerSecond: 15,
                isStable: true
            ),
            5
        )
    }

    func testByteAndRateFormattingClampInvalidAndExtremeInputs() {
        XCTAssertFalse(byteString(UInt64.max).isEmpty)
        XCTAssertEqual(rateString(999), "999 B/s")
        XCTAssertEqual(rateString(1_000), "1 KB/s")
        XCTAssertEqual(rateString(1_250_000), "1.3 MB/s")
        XCTAssertEqual(rateString(1_250_000_000), "1.25 GB/s")
        XCTAssertEqual(rateString(.infinity), "0 B/s")
        XCTAssertEqual(rateString(.nan), "0 B/s")
    }

    func testCurrentTransferMetricsExpireAfterAStall() {
        let sampledAt = Date(timeIntervalSinceReferenceDate: 1_000)

        XCTAssertTrue(TransferMetricFreshnessPolicy.isFresh(
            sampledAt: sampledAt,
            now: sampledAt.addingTimeInterval(2.5)
        ))
        XCTAssertFalse(TransferMetricFreshnessPolicy.isFresh(
            sampledAt: sampledAt,
            now: sampledAt.addingTimeInterval(2.501)
        ))
        XCTAssertFalse(TransferMetricFreshnessPolicy.isFresh(
            sampledAt: nil,
            now: sampledAt
        ))
    }

    func testStageTimingProjectionPreservesTypedFieldsAndStableFormat() {
        let sample = ActivityStageTimingSample(FfiTransferStageTiming(
            stage: .firstPayload,
            direction: .receive,
            attemptId: 7,
            transferId: "job-1",
            elapsedUs: 42_000,
            deltaUs: 9_000
        ))

        XCTAssertEqual(sample.stage, .firstPayload)
        XCTAssertEqual(sample.direction, .receive)
        XCTAssertEqual(sample.attemptID, 7)
        XCTAssertEqual(sample.transferID, "job-1")
        XCTAssertEqual(sample.elapsedMicroseconds, 42_000)
        XCTAssertEqual(sample.deltaMicroseconds, 9_000)
        XCTAssertEqual(
            sample.diagnosticLine,
            "stage_timing stage=first_payload direction=receive attempt_id=7 " +
                "transfer_id=job-1 elapsed_us=42000 delta_us=9000"
        )
        XCTAssertFalse(sample.diagnosticLine.contains("\n"))
    }

    func testStageTimingProjectionRetainsNewestAttemptsInCanonicalOrder() {
        var samples: [ActivityStageTimingSample] = []
        for attemptID in UInt64(0)..<66 {
            let sample = ActivityStageTimingSample(FfiTransferStageTiming(
                stage: .sessionStarted,
                direction: .send,
                attemptId: attemptID,
                transferId: nil,
                elapsedUs: attemptID,
                deltaUs: 1
            ))
            samples = ActivityStageTimingProjection.appending(sample, to: samples)
        }

        XCTAssertEqual(samples.count, ActivityStageTimingProjection.maximumSamplesPerActivity)
        XCTAssertEqual(samples.first?.attemptID, 2)
        XCTAssertEqual(samples.last?.attemptID, 65)
    }

    func testStageTimingProjectionReordersLateCallbacksAndDeduplicatesStages() {
        let later = ActivityStageTimingSample(FfiTransferStageTiming(
            stage: .authenticationComplete,
            direction: .send,
            attemptId: 9,
            transferId: nil,
            elapsedUs: 40,
            deltaUs: 10
        ))
        let earlier = ActivityStageTimingSample(FfiTransferStageTiming(
            stage: .authenticationStarted,
            direction: .send,
            attemptId: 9,
            transferId: nil,
            elapsedUs: 30,
            deltaUs: 20
        ))

        var samples = ActivityStageTimingProjection.appending(later, to: [])
        samples = ActivityStageTimingProjection.appending(earlier, to: samples)
        samples = ActivityStageTimingProjection.appending(earlier, to: samples)

        XCTAssertEqual(samples.map(\.stage), [.authenticationStarted, .authenticationComplete])
        XCTAssertEqual(
            earlier.diagnosticLine,
            "stage_timing stage=authentication_started direction=send attempt_id=9 " +
                "transfer_id=- elapsed_us=30 delta_us=20"
        )
    }

    func testStageTimingPresentationUsesOnlyLatestAttemptInElapsedOrder() {
        let samples = [
            stageTiming(.deliveryComplete, attemptID: 4, elapsedMicroseconds: 3_000_000),
            stageTiming(.firstPayload, attemptID: 3, elapsedMicroseconds: 900_000),
            stageTiming(.connectionReady, attemptID: 4, elapsedMicroseconds: 125_000),
        ]

        let latest = ActivityStageTimingPresentationPolicy.latestAttempt(from: samples)

        XCTAssertEqual(latest.map(\.attemptID), [4, 4])
        XCTAssertEqual(latest.map(\.stage), [.connectionReady, .deliveryComplete])
    }

    func testStageTimingElapsedFormattingCoversDisplayBoundaries() {
        XCTAssertEqual(
            ActivityStageTimingPresentationPolicy.elapsedString(microseconds: 0),
            "<1 ms"
        )
        XCTAssertEqual(
            ActivityStageTimingPresentationPolicy.elapsedString(microseconds: 1_000),
            "1 ms"
        )
        XCTAssertEqual(
            ActivityStageTimingPresentationPolicy.elapsedString(microseconds: 1_500),
            "2 ms"
        )
        XCTAssertEqual(
            ActivityStageTimingPresentationPolicy.elapsedString(microseconds: 1_234_000),
            "1.23 s"
        )
        XCTAssertEqual(
            ActivityStageTimingPresentationPolicy.elapsedString(microseconds: 12_340_000),
            "12.3 s"
        )
        XCTAssertEqual(
            ActivityStageTimingPresentationPolicy.elapsedString(microseconds: 61_000_000),
            "1m 1s"
        )
    }

    func testNativeDeliveryIsDeferredUntilTheOwningFutureReturns() {
        XCTAssertFalse(NativeTerminalDeliveryPolicy.shouldForwardPhase(
            .delivered,
            defersUntilNativeReturn: true
        ))
        XCTAssertTrue(NativeTerminalDeliveryPolicy.shouldForwardPhase(
            .finalizingDelivery,
            defersUntilNativeReturn: true
        ))
        XCTAssertFalse(
            NativeTerminalDeliveryPolicy.shouldForwardObserverCompletion(
                defersUntilNativeReturn: true
            )
        )
        XCTAssertTrue(
            NativeTerminalDeliveryPolicy.shouldForwardObserverCompletion(
                defersUntilNativeReturn: false
            )
        )
    }

    func testActionContractForEveryState() {
        let retryable = failure(retryable: true)
        let cases: [(TransferActivityState, ActivityActionAvailability)] = [
            (.preparing, actions(cancel: true)),
            (.waitingForPeer, actions(cancel: true)),
            (.pairing, actions(cancel: true)),
            (.connecting, actions(cancel: true)),
            (.awaitingDecision, actions(cancel: true, approve: true)),
            (.transferring, actions(cancel: true)),
            (.verifying, actions(cancel: true)),
            (.saving, actions(finalizing: true)),
            (.waitingForReceiverSave, actions(finalizing: true)),
            (.finalizingDelivery, actions(finalizing: true)),
            (.paused, actions(cancel: true)),
            (.delivered, actions(delete: true)),
            (.failed, actions(delete: true)),
            (.canceled, actions(delete: true)),
        ]

        for (state, expected) in cases {
            XCTAssertEqual(
                TransferPresentationPolicy.actions(
                    for: state,
                    failure: state == .failed ? retryable : nil
                ),
                expected,
                "Unexpected actions for \(state)"
            )
        }
        XCTAssertFalse(
            TransferPresentationPolicy.actions(
                for: .failed,
                failure: failure(retryable: false)
            ).canResume
        )
        XCTAssertFalse(
            TransferPresentationPolicy.actions(
                for: .failed,
                failure: failure(retryable: true, recoveryAction: .rePair)
            ).canResume
        )
    }

    func testTypedFailureOutcomeAndSessionDispositionAreAuthoritative() {
        let retained = failure(retryable: true)
        XCTAssertEqual(TransferPresentationPolicy.terminalState(for: retained), .failed)
        XCTAssertFalse(TransferPresentationPolicy.shouldReleaseSession(after: retained))

        let canceled = FfiTransferFailure(
            code: .userCanceled,
            category: .user,
            phase: .transferring,
            origin: .local,
            direction: .send,
            retryable: false,
            recoveryAction: .none,
            outcome: .canceled,
            sessionDisposition: .release,
            userMessageKey: "transfer.user_canceled",
            diagnosticMessage: "test"
        )
        XCTAssertEqual(TransferPresentationPolicy.terminalState(for: canceled), .canceled)
        XCTAssertTrue(TransferPresentationPolicy.shouldReleaseSession(after: canceled))
    }

    func testProgressContractKeepsPostPayloadStagesComplete() {
        XCTAssertEqual(TransferPresentationPolicy.progress(for: .connecting), .hidden)
        XCTAssertEqual(TransferPresentationPolicy.progress(for: .awaitingDecision), .hidden)
        XCTAssertEqual(TransferPresentationPolicy.progress(for: .transferring), .active)
        XCTAssertEqual(TransferPresentationPolicy.progress(for: .paused), .retained)
        XCTAssertEqual(TransferPresentationPolicy.progress(for: .failed), .retained)
        XCTAssertEqual(TransferPresentationPolicy.progress(for: .verifying), .complete)
        XCTAssertEqual(TransferPresentationPolicy.progress(for: .saving), .complete)
        XCTAssertEqual(TransferPresentationPolicy.progress(for: .waitingForReceiverSave), .complete)
        XCTAssertEqual(TransferPresentationPolicy.progress(for: .finalizingDelivery), .complete)
        XCTAssertEqual(TransferPresentationPolicy.progress(for: .delivered), .hidden)
    }

    func testNewDraftDetachesOnlyTerminalActivity() {
        let terminalStates: [TransferActivityState] = [.delivered, .failed, .canceled]
        for state in terminalStates {
            XCTAssertTrue(
                TransferDraftLifecyclePolicy.shouldDetachActivityBeforePreparation(state),
                "Expected a new draft to detach \(state)"
            )
        }

        let liveStates: [TransferActivityState] = [
            .preparing,
            .waitingForPeer,
            .pairing,
            .connecting,
            .awaitingDecision,
            .transferring,
            .verifying,
            .saving,
            .waitingForReceiverSave,
            .finalizingDelivery,
            .paused,
        ]
        for state in liveStates {
            XCTAssertFalse(
                TransferDraftLifecyclePolicy.shouldDetachActivityBeforePreparation(state),
                "A new draft must not detach live activity \(state)"
            )
        }
        XCTAssertFalse(
            TransferDraftLifecyclePolicy.shouldDetachActivityBeforePreparation(nil)
        )
    }

    func testReceiverSuppressesPerEntryVerificationUntilAllBytesAreObserved() {
        XCTAssertFalse(TransferPhasePresentationPolicy.shouldSurface(
            .verifying,
            direction: .receive,
            currentState: .transferring,
            observedBytes: 40,
            totalBytes: 100
        ))
        XCTAssertFalse(TransferPhasePresentationPolicy.shouldSurface(
            .transferring,
            direction: .receive,
            currentState: .transferring,
            observedBytes: 40,
            totalBytes: 100
        ))
        XCTAssertTrue(TransferPhasePresentationPolicy.shouldSurface(
            .verifying,
            direction: .receive,
            currentState: .transferring,
            observedBytes: 100,
            totalBytes: 100
        ))
        XCTAssertFalse(TransferPhasePresentationPolicy.shouldSurface(
            .verifying,
            direction: .receive,
            currentState: .verifying,
            observedBytes: 100,
            totalBytes: 100
        ))
        XCTAssertFalse(TransferPhasePresentationPolicy.shouldSurface(
            .transferring,
            direction: .receive,
            currentState: .verifying,
            observedBytes: 100,
            totalBytes: 100
        ))
    }

    func testSenderPhasePresentationRemainsAnExactCoreProjection() {
        XCTAssertTrue(TransferPhasePresentationPolicy.shouldSurface(
            .verifying,
            direction: .send,
            currentState: .transferring,
            observedBytes: 40,
            totalBytes: 100
        ))
        XCTAssertTrue(TransferPhasePresentationPolicy.shouldSurface(
            .transferring,
            direction: .send,
            currentState: .verifying,
            observedBytes: 40,
            totalBytes: 100
        ))
    }

    func testActivityGroupsOnlyStableGroupIDsAndKeepsDirectTransfersSeparate() throws {
        let date = Date(timeIntervalSince1970: 100)
        let groups = activityRoomGroups([
            activity("room-b", state: .delivered, date: date, groupID: "stable-room"),
            activity("direct-b", state: .delivered, date: date),
            activity("room-a", state: .transferring, date: date, groupID: "stable-room"),
            activity("direct-a", state: .delivered, date: date),
        ])

        XCTAssertEqual(groups.count, 3)
        let room = try XCTUnwrap(groups.first { $0.activityGroupID == "stable-room" })
        XCTAssertEqual(room.records.map(\.activityId), ["room-a", "room-b"])
        XCTAssertEqual(
            groups.filter { $0.activityGroupID == nil }.flatMap(\.records).map(\.activityId).sorted(),
            ["direct-a", "direct-b"]
        )
    }

    func testActivityGroupSummaryPrefersCurrentWorkOverHistoricalFailure() throws {
        let group = try XCTUnwrap(ActivityRoomGroup(
            id: "group:stable-room",
            activityGroupID: "stable-room",
            records: [
                activity("old-failure", state: .failed, date: Date(timeIntervalSince1970: 200)),
                activity("current", state: .transferring, date: Date(timeIntervalSince1970: 100)),
            ]
        ))

        XCTAssertEqual(group.summaryRecord.activityId, "current")
        XCTAssertEqual(group.summaryRecord.state, .transferring)
    }

    func testActivityGroupSummaryUsesNewestRecordWhenNoWorkIsPending() throws {
        let group = try XCTUnwrap(ActivityRoomGroup(
            id: "group:stable-room",
            activityGroupID: "stable-room",
            records: [
                activity("old-failure", state: .failed, date: Date(timeIntervalSince1970: 100)),
                activity("new-success", state: .delivered, date: Date(timeIntervalSince1970: 200)),
            ]
        ))

        XCTAssertEqual(group.summaryRecord.activityId, "new-success")
        XCTAssertEqual(group.summaryRecord.state, .delivered)
    }

    func testActivityGroupProgressUsesOnlyCurrentActiveRecords() throws {
        let group = try XCTUnwrap(ActivityRoomGroup(
            id: "group:stable-room",
            activityGroupID: "stable-room",
            records: [
                activity(
                    "active",
                    state: .transferring,
                    date: Date(timeIntervalSince1970: 300),
                    bytes: 40,
                    total: 100
                ),
                activity(
                    "waiting",
                    state: .waitingForPeer,
                    date: Date(timeIntervalSince1970: 200),
                    bytes: 0,
                    total: 1_000
                ),
                activity(
                    "delivered",
                    state: .delivered,
                    date: Date(timeIntervalSince1970: 100),
                    bytes: 2_000,
                    total: 2_000
                ),
            ]
        ))

        XCTAssertEqual(group.progressRecords.map(\.activityId), ["active"])
        XCTAssertEqual(group.progressBytesTransferred, 40)
        XCTAssertEqual(group.progressTotalBytes, 100)
    }

    func testActivityGroupByteAggregationSaturates() throws {
        let group = try XCTUnwrap(ActivityRoomGroup(
            id: "group:stable-room",
            activityGroupID: "stable-room",
            records: [
                activity(
                    "maximum",
                    state: .paused,
                    date: Date(timeIntervalSince1970: 200),
                    bytes: UInt64.max,
                    total: UInt64.max
                ),
                activity(
                    "overflow",
                    state: .paused,
                    date: Date(timeIntervalSince1970: 100),
                    bytes: 1,
                    total: 1
                ),
            ]
        ))

        XCTAssertEqual(group.totalBytes, UInt64.max)
        XCTAssertEqual(group.progressTotalBytes, UInt64.max)
        XCTAssertEqual(group.progressBytesTransferred, UInt64.max)
    }

    func testActivityGroupProgressHidesWaitingAndTerminalHistory() throws {
        let waiting = try XCTUnwrap(ActivityRoomGroup(
            id: "group:waiting",
            activityGroupID: "waiting",
            records: [
                activity(
                    "waiting",
                    state: .waitingForPeer,
                    date: Date(timeIntervalSince1970: 100),
                    bytes: 0,
                    total: 100
                ),
            ]
        ))
        let delivered = try XCTUnwrap(ActivityRoomGroup(
            id: "group:delivered",
            activityGroupID: "delivered",
            records: [
                activity(
                    "delivered",
                    state: .delivered,
                    date: Date(timeIntervalSince1970: 100),
                    bytes: 100,
                    total: 100
                ),
            ]
        ))

        XCTAssertTrue(waiting.progressRecords.isEmpty)
        XCTAssertEqual(waiting.progressTotalBytes, 0)
        XCTAssertTrue(delivered.progressRecords.isEmpty)
        XCTAssertEqual(delivered.progressTotalBytes, 0)
    }

    private func actions(
        pause: Bool = false,
        resume: Bool = false,
        cancel: Bool = false,
        approve: Bool = false,
        delete: Bool = false,
        finalizing: Bool = false
    ) -> ActivityActionAvailability {
        ActivityActionAvailability(
            canPause: pause,
            canResume: resume,
            canCancel: cancel,
            canApprove: approve,
            canDelete: delete,
            isFinalizing: finalizing
        )
    }

    private func failure(
        retryable: Bool,
        recoveryAction: FfiRecoveryAction? = nil
    ) -> FfiTransferFailure {
        let recoveryAction = recoveryAction ?? (retryable ? .resume : .none)
        return FfiTransferFailure(
            code: .networkLost,
            category: .network,
            phase: .transferring,
            origin: .unknown,
            direction: .send,
            retryable: retryable,
            recoveryAction: recoveryAction,
            outcome: .failed,
            sessionDisposition: retryable && recoveryAction != .rePair
                ? .retainForRecovery
                : .release,
            userMessageKey: "transfer.network_lost",
            diagnosticMessage: "test"
        )
    }

    private func stageTiming(
        _ stage: FfiTransferStage,
        attemptID: UInt64,
        elapsedMicroseconds: UInt64
    ) -> ActivityStageTimingSample {
        ActivityStageTimingSample(FfiTransferStageTiming(
            stage: stage,
            direction: .send,
            attemptId: attemptID,
            transferId: nil,
            elapsedUs: elapsedMicroseconds,
            deltaUs: elapsedMicroseconds
        ))
    }

    private func activity(
        _ id: String,
        state: TransferActivityState,
        date: Date,
        groupID: String? = nil,
        bytes: UInt64 = 0,
        total: UInt64 = 0
    ) -> TransferActivityRecord {
        TransferActivityRecord(
            activityId: id,
            direction: .send,
            mode: .invite,
            attemptCount: 1,
            itemCount: 1,
            totalBytes: total,
            bytesTransferred: bytes,
            state: state,
            diagnosticMessage: "",
            failure: nil,
            savedPaths: [],
            roomID: "diagnostic-only",
            connectionPath: nil,
            updatedAt: date,
            activityGroupID: groupID,
            activityGroupLabel: nil
        )
    }
}
