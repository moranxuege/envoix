import XCTest
import EnvoixCore
@testable import Envoix_iOS

final class TransferPresentationPolicyTests: XCTestCase {
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
        FfiTransferFailure(
            code: .networkLost,
            category: .network,
            phase: .transferring,
            origin: .unknown,
            direction: .send,
            retryable: retryable,
            recoveryAction: recoveryAction ?? (retryable ? .resume : .none),
            userMessageKey: "transfer.network_lost",
            diagnosticMessage: "test"
        )
    }
}
