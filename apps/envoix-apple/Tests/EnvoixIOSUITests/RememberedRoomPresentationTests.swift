import EnvoixCore
import XCTest
@testable import Envoix_iOS

final class RememberedRoomPresentationTests: XCTestCase {
    func testEveryStaticCatalogEntryResolvesInBothLanguages() {
        for copy in RememberedRoomCopy.allCases {
            assertResolves(copy.rawValue) {
                RememberedRoomPresentationText.value(copy, language: $0)
            }
        }
        for copy in RememberedRoomActivityStateCopy.allCases {
            assertResolves("remembered_room.activity.state.\(copy.rawValue)") {
                AppText.localized(
                    "remembered_room.activity.state.\(copy.rawValue)",
                    language: $0
                )
            }
        }
        for copy in RememberedRoomOutboxStateCopy.allCases {
            assertResolves("remembered_room.outbox.state.\(copy.rawValue)") {
                AppText.localized(
                    "remembered_room.outbox.state.\(copy.rawValue)",
                    language: $0
                )
            }
        }
        for copy in RememberedRoomConnectionCopy.allCases {
            assertResolves("remembered_room.connection.\(copy.rawValue)") {
                AppText.localized(
                    "remembered_room.connection.\(copy.rawValue)",
                    language: $0
                )
            }
        }
        for copy in AgentRoomConnectionCopy.allCases {
            assertResolves("remembered_room.agent.connection.\(copy.rawValue)") {
                RememberedRoomPresentationText.agentRoomConnection(copy, language: $0)
            }
        }
        for copy in AgentTransferStateCopy.allCases {
            assertResolves("remembered_room.agent.transfer.state.\(copy.rawValue)") {
                AgentTransferPresentationText.state(copy, language: $0)
            }
        }
        for copy in AgentTransferDetailCopy.allCases {
            assertResolves("remembered_room.agent.transfer.detail.\(copy.rawValue)") {
                AgentTransferPresentationText.detail(copy, language: $0)
            }
        }
        for copy in AgentTransferPathCopy.allCases {
            assertResolves("remembered_room.agent.transfer.path.\(copy.rawValue)") {
                AgentTransferPresentationText.path(copy, language: $0)
            }
        }
    }

    func testPairedDeviceCopyDoesNotExposeItsProcessOwner() {
        let values = [
            RememberedRoomPresentationText.value(.helperUnavailable, language: "en"),
            RememberedRoomPresentationText.value(.helperKeepsRoom, language: "en"),
            RememberedRoomPresentationText.value(.helperRefreshUnavailable, language: "en"),
            RememberedRoomPresentationText.value(.noHelperTransfers, language: "en"),
            RememberedRoomPresentationText.value(.loadingHelperActivity, language: "en"),
            AgentTransferPresentationText.detail(.paused, language: "en"),
            AgentTransferPresentationText.detail(.queued, language: "en"),
        ]

        for value in values {
            XCTAssertFalse(value.lowercased().contains("helper"), value)
            XCTAssertFalse(value.lowercased().contains("agent"), value)
        }
    }

    func testRoomStateProjectionPreservesEveryExistingLabel() {
        let activityStates: [(TransferActivityState, String, String)] = [
            (.preparing, "Preparing", "正在准备"),
            (.pairing, "Pairing", "正在配对"),
            (.connecting, "Connecting", "正在连接"),
            (.waitingForPeer, "Waiting", "正在等待"),
            (.transferring, "Transferring", "正在传输"),
            (.verifying, "Verifying", "正在校验"),
            (.saving, "Saving", "正在保存"),
            (.waitingForReceiverSave, "Finalizing", "正在完成"),
            (.finalizingDelivery, "Finalizing", "正在完成"),
            (.awaitingDecision, "Needs attention", "需要处理"),
            (.paused, "Paused", "已暂停"),
            (.delivered, "Delivered", "已送达"),
            (.failed, "Failed", "失败"),
            (.canceled, "Canceled", "已取消"),
        ]
        for (state, english, chinese) in activityStates {
            XCTAssertEqual(
                RememberedRoomPresentationText.activityState(state, language: "en"),
                english
            )
            XCTAssertEqual(
                RememberedRoomPresentationText.activityState(state, language: "zh-Hans"),
                chinese
            )
        }

        let outboxStates: [(RememberedRoomOutboxState, String, String)] = [
            (.queued, "Queued", "等待发送"),
            (.offering, "Offering", "正在邀请"),
            (.transferring, "Sending", "正在发送"),
            (.needsAttention, "Check", "需处理"),
        ]
        for (state, english, chinese) in outboxStates {
            XCTAssertEqual(
                RememberedRoomPresentationText.outboxState(state, language: "en"),
                english
            )
            XCTAssertEqual(
                RememberedRoomPresentationText.outboxState(state, language: "zh-Hans"),
                chinese
            )
        }
    }

    func testConnectionAndDynamicCopyValidateInputs() {
        XCTAssertEqual(
            RememberedRoomPresentationText.connectionStatus(.offline, language: "en"),
            "Offline · waiting for the other app"
        )
        XCTAssertEqual(
            RememberedRoomPresentationText.connectionStatus(
                .needsRepair("repair detail"),
                language: "zh-Hans"
            ),
            "repair detail"
        )
        XCTAssertEqual(
            RememberedRoomPresentationText.forgetDetail(
                hasQueuedFiles: true,
                language: "zh-Hans"
            ),
            "此房间的待发送文件会被移除，之后需要重新配对。"
        )
        XCTAssertEqual(
            RememberedRoomPresentationText.itemCount(-4, language: "en"),
            "0 items"
        )
        XCTAssertEqual(
            RememberedRoomPresentationText.savedIn("Downloads", language: "zh-Hans"),
            "已保存到 Downloads"
        )
        XCTAssertEqual(
            RememberedRoomPresentationText.agentActivityTitle(
                hasLoadedSnapshot: false,
                language: "en"
            ),
            "Loading transfer activity…"
        )
        XCTAssertEqual(
            RememberedRoomPresentationText.agentTransferTitle(
                direction: .send,
                deviceLabel: "  WSL  ",
                language: "en"
            ),
            "Send · WSL"
        )
        XCTAssertEqual(
            RememberedRoomPresentationText.agentTransferTitle(
                direction: .receive,
                deviceLabel: " ",
                language: "zh-Hans"
            ),
            "接收文件"
        )
    }

    private func assertResolves(
        _ key: String,
        file: StaticString = #filePath,
        line: UInt = #line,
        value: (String) -> String
    ) {
        for language in ["en", "zh-Hans"] {
            let localized = value(language)
            XCTAssertFalse(localized.isEmpty, file: file, line: line)
            XCTAssertNotEqual(localized, key, "Missing \(key) for \(language)", file: file, line: line)
        }
    }
}
