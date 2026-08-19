import Foundation
import XCTest
@testable import Envoix_iOS

final class RoomPresentationTests: XCTestCase {
    func testRoomCatalogProvidesStaticScreenCopy() {
        let cases: [(String, String, String)] = [
            ("common.decline", "Decline", "拒绝"),
            ("common.done", "Done", "完成"),
            ("room.action.add_files", "Add files", "添加文件"),
            ("room.action.end", "End room", "结束房间"),
            ("room.action.keep_open", "Keep open", "保持开启"),
            ("room.action.keep_open_accessibility", "Keep room open", "保持房间开启"),
            ("room.activity.all", "All Activity", "全部活动"),
            ("room.activity.empty", "Transfers started here will appear in this timeline.", "从这里开始的传输会显示在此时间线中。"),
            ("room.activity.title", "Room activity", "房间活动"),
            ("room.destination.default", "Envoix / Downloads", "Envoix / Downloads"),
            ("room.offer.contents", "Contents", "内容"),
            ("room.offer.destination", "Destination", "保存位置"),
            ("room.offer.incoming", "Incoming transfer", "收到传输邀请"),
            ("room.offer.preparing_receiver", "Preparing receiver…", "正在准备接收…"),
            ("room.offer.summary", "Offer summary", "内容摘要"),
        ]

        for (key, english, chinese) in cases {
            XCTAssertEqual(AppText.localized(key, language: "en"), english)
            XCTAssertEqual(AppText.localized(key, language: "zh-Hans"), chinese)
        }
    }

    func testRoomStatusProjectsEveryControlPhaseAndIdleOrigin() {
        let phaseCases: [(RoomControlPhase, String, String)] = [
            (.hosting, "Waiting for another device", "正在等待另一台设备"),
            (.joining, "Joining room", "正在加入房间"),
            (.connectingRemembered, "Connecting", "正在连接"),
            (.waitingRemembered, "Waiting for the other device", "正在等待另一台设备"),
            (.connected, "Connected for this room", "已连接此房间"),
            (.ended(.userEnded), "Room ended", "房间已结束"),
            (.failed("private detail"), "Connection needs attention", "连接需要处理"),
        ]
        for (phase, english, chinese) in phaseCases {
            XCTAssertEqual(
                RoomPresentationText.status(
                    phase: phase,
                    origin: .roomControl,
                    selectedPeerIsVisible: false,
                    discoveryIsActive: false,
                    language: "en"
                ),
                english
            )
            XCTAssertEqual(
                RoomPresentationText.status(
                    phase: phase,
                    origin: .roomControl,
                    selectedPeerIsVisible: false,
                    discoveryIsActive: false,
                    language: "zh-Hans"
                ),
                chinese
            )
        }

        let nearby = OneTimeRoomOrigin.nearby(NearbyPairingSelection(
            discoveryPeerKey: "peer",
            displayName: "Phone",
            sources: [.bluetooth]
        ))
        XCTAssertEqual(idleStatus(origin: nearby, visible: true, discovery: true), "Nearby now")
        XCTAssertEqual(idleStatus(origin: nearby, visible: false, discovery: true), "Looking for this device")
        XCTAssertEqual(idleStatus(origin: nearby, visible: false, discovery: false), "Nearby discovery paused")
        XCTAssertEqual(idleStatus(origin: .pairingCode, visible: false, discovery: false), "Invite loaded")
        XCTAssertEqual(idleStatus(origin: .showCode, visible: false, discovery: false), "Ready to show a room QR")
        XCTAssertEqual(idleStatus(origin: .roomControl, visible: false, discovery: false), "Connecting")
    }

    func testRoomTrustAndCloseReasonsUseCatalogWhileFailuresStayDynamic() {
        XCTAssertEqual(RoomPresentationText.trust(authenticated: true, language: "en"), "Authenticated for this room")
        XCTAssertEqual(RoomPresentationText.trust(authenticated: false, language: "zh-Hans"), "未经验证")

        let cases: [(RoomControlCloseReason, String, String)] = [
            (.userEnded, "This room was ended.", "此房间已结束。"),
            (.idleExpired, "This room ended after 15 minutes without transfer activity.", "此房间在 15 分钟无传输活动后结束。"),
            (.invitationExpired, "The room invitation expired.", "房间邀请已过期。"),
            (.peerEnded, "The other device ended this room.", "另一台设备结束了此房间。"),
            (.backgrounded, "The room ended when Envoix left the foreground.", "Envoix 离开前台后房间已结束。"),
            (.networkLost, "The room connection was lost.", "房间连接已断开。"),
            (.protocolFailure, "The room ended because of a connection error.", "房间因连接错误而结束。"),
        ]
        for (reason, english, chinese) in cases {
            XCTAssertEqual(
                RoomPresentationText.endedMessage(phase: .ended(reason), language: "en"),
                english
            )
            XCTAssertEqual(
                RoomPresentationText.endedMessage(phase: .ended(reason), language: "zh-Hans"),
                chinese
            )
        }
        XCTAssertEqual(
            RoomPresentationText.endedMessage(phase: .failed("Gateway detail"), language: "en"),
            "Gateway detail"
        )
        XCTAssertNil(RoomPresentationText.endedMessage(phase: .connected, language: "en"))
    }

    func testRoomLifetimeAndOfferFormatsUseExplicitLocale() {
        let now = Date(timeIntervalSince1970: 1_000)
        XCTAssertEqual(
            lifetime(origin: .pairingCode, phase: .idle, deadline: nil, now: now),
            "One-time transfer"
        )
        XCTAssertEqual(
            lifetime(origin: .roomControl, phase: .ended(.userEnded), deadline: nil, now: now),
            "Room closed"
        )
        XCTAssertEqual(
            lifetime(
                origin: .roomControl,
                phase: .connected,
                policy: .untilForegroundEnds,
                deadline: nil,
                now: now
            ),
            "Kept open while Envoix is open"
        )
        XCTAssertEqual(
            lifetime(origin: .roomControl, phase: .connected, deadline: nil, now: now),
            "Idle timer paused during transfer"
        )
        XCTAssertEqual(
            lifetime(
                origin: .roomControl,
                phase: .connected,
                deadline: now.addingTimeInterval(61.2),
                now: now
            ),
            "Ends in 1:02 if idle"
        )
        XCTAssertEqual(
            lifetime(
                origin: .roomControl,
                phase: .connected,
                deadline: now.addingTimeInterval(-1),
                now: now
            ),
            "Ends in 0:00 if idle"
        )

        XCTAssertEqual(RoomPresentationText.additionalItems(3, language: "en"), "+3 more")
        XCTAssertEqual(RoomPresentationText.additionalItems(3, language: "zh-Hans"), "另有 3 项")
        XCTAssertEqual(TransferContentText.fileCount(1, language: "en"), "1 file")
        XCTAssertEqual(TransferContentText.fileCount(2, language: "en"), "2 files")
        XCTAssertEqual(TransferContentText.folderCount(2, language: "zh-Hans"), "2 个文件夹")
        XCTAssertEqual(
            RoomPresentationText.offerSummary(
                fileCount: 1,
                folderCount: 2,
                byteDescription: "3 MB",
                language: "en"
            ),
            "1 file · 2 folders · 3 MB"
        )
    }

    private func idleStatus(
        origin: OneTimeRoomOrigin,
        visible: Bool,
        discovery: Bool
    ) -> String {
        RoomPresentationText.status(
            phase: .idle,
            origin: origin,
            selectedPeerIsVisible: visible,
            discoveryIsActive: discovery,
            language: "en"
        )
    }

    private func lifetime(
        origin: OneTimeRoomOrigin,
        phase: RoomControlPhase,
        policy: RoomControlLifetimePolicy = .idleFifteenMinutes,
        deadline: Date?,
        now: Date
    ) -> String {
        RoomPresentationText.lifetime(
            origin: origin,
            phase: phase,
            policy: policy,
            idleDeadline: deadline,
            now: now,
            language: "en"
        )
    }
}
