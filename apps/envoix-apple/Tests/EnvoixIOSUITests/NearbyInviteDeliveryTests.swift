import XCTest
@testable import Envoix_iOS

final class NearbyInviteDeliveryTests: XCTestCase {
    func testOneInFlightDeliveryStartsExactlyOnce() {
        let controller = NearbyInviteDeliveryController()
        var completions: [(String?) -> Void] = []
        var startCount = 0
        let started = expectation(description: "transfer started")

        let offer = NearbyInviteOffer { _, completion in
            completions.append(completion)
        }
        controller.deliver(invite: "first", using: offer) {
            startCount += 1
            started.fulfill()
        }
        controller.deliver(invite: "duplicate", using: offer) {
            XCTFail("A second delivery must not start while the first is pending")
        }

        XCTAssertTrue(controller.isDelivering)
        XCTAssertEqual(completions.count, 1)

        completions[0](nil)
        wait(for: [started], timeout: 1)
        XCTAssertEqual(startCount, 1)
        XCTAssertFalse(controller.isDelivering)

        completions[0](nil)
        let duplicateCallbackSettled = expectation(description: "duplicate callback settled")
        DispatchQueue.main.async {
            duplicateCallbackSettled.fulfill()
        }
        wait(for: [duplicateCallbackSettled], timeout: 1)
        XCTAssertEqual(startCount, 1)
    }

    func testFailureCanBeRetried() {
        let controller = NearbyInviteDeliveryController()
        var completions: [(String?) -> Void] = []
        let offer = NearbyInviteOffer { _, completion in
            completions.append(completion)
        }

        controller.deliver(invite: "first", using: offer) {
            XCTFail("A failed delivery must not start a transfer")
        }
        completions[0]("Bluetooth peer unavailable")
        drainMainQueue()

        XCTAssertFalse(controller.isDelivering)
        XCTAssertEqual(controller.error, "Bluetooth peer unavailable")

        let started = expectation(description: "retry started transfer")
        controller.deliver(invite: "retry", using: offer) {
            started.fulfill()
        }
        XCTAssertEqual(completions.count, 2)
        completions[1](nil)
        wait(for: [started], timeout: 1)
        XCTAssertNil(controller.error)
    }

    func testCanceledDeliveryIgnoresLateCompletion() {
        let controller = NearbyInviteDeliveryController()
        var completion: ((String?) -> Void)?
        var startCount = 0
        let offer = NearbyInviteOffer { _, callback in
            completion = callback
        }

        controller.deliver(invite: "invite", using: offer) {
            startCount += 1
        }
        controller.cancel()
        completion?(nil)
        drainMainQueue()

        XCTAssertFalse(controller.isDelivering)
        XCTAssertEqual(startCount, 0)
    }

    func testMissingOfferStartsImmediately() {
        let controller = NearbyInviteDeliveryController()
        var startCount = 0

        controller.deliver(invite: "unused", using: nil) {
            startCount += 1
        }

        XCTAssertEqual(startCount, 1)
        XCTAssertFalse(controller.isDelivering)
    }

    private func drainMainQueue() {
        let settled = expectation(description: "main queue settled")
        DispatchQueue.main.async {
            settled.fulfill()
        }
        wait(for: [settled], timeout: 1)
    }
}
