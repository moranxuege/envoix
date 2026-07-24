import XCTest
import EnvoixCore
@testable import Envoix_iOS

final class TransferPresentationPolicyTests: XCTestCase {
    func testActionContractForEveryState() {
        let retryable = failure(retryable: true)
        let cases: [(TransferActivityState, ActivityActionAvailability)] = [
            (.preparing, actions(cancel: true)),
            (.waitingForPeer, actions(pause: true, cancel: true)),
            (.pairing, actions(pause: true, cancel: true)),
            (.connecting, actions(pause: true, cancel: true)),
            (.awaitingDecision, actions(cancel: true, approve: true)),
            (.transferring, actions(pause: true, cancel: true)),
            (.verifying, actions(pause: true, cancel: true)),
            (.saving, actions(finalizing: true)),
            (.waitingForReceiverSave, actions(finalizing: true)),
            (.finalizingDelivery, actions(finalizing: true)),
            (.paused, actions(resume: true, cancel: true)),
            (.delivered, actions(delete: true)),
            (.failed, actions(resume: true, delete: true)),
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
        XCTAssertEqual(TransferPresentationPolicy.progress(for: .delivered), .complete)
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

    private func failure(retryable: Bool) -> FfiTransferFailure {
        FfiTransferFailure(
            code: .networkLost,
            category: .network,
            phase: .transferring,
            origin: .unknown,
            direction: .send,
            retryable: retryable,
            recoveryAction: retryable ? .resume : .none,
            userMessageKey: "transfer.network_lost",
            diagnosticMessage: "test"
        )
    }
}
