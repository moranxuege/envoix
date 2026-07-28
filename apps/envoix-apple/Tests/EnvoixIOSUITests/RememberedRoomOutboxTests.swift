import XCTest
@testable import Envoix_iOS

final class RememberedRoomOutboxTests: XCTestCase {
    func testDeliveredCleanupWaitsForNativeSendOwnershipRelease() {
        XCTAssertFalse(RememberedRoomOutboxDeliveryCleanupPolicy.canFinalize(
            state: .delivered,
            activityMatches: true,
            senderOwnsNativeOperation: true
        ))
        XCTAssertTrue(RememberedRoomOutboxDeliveryCleanupPolicy.canFinalize(
            state: .delivered,
            activityMatches: true,
            senderOwnsNativeOperation: false
        ))
        XCTAssertFalse(RememberedRoomOutboxDeliveryCleanupPolicy.canFinalize(
            state: .finalizingDelivery,
            activityMatches: true,
            senderOwnsNativeOperation: false
        ))
        XCTAssertFalse(RememberedRoomOutboxDeliveryCleanupPolicy.canFinalize(
            state: .delivered,
            activityMatches: false,
            senderOwnsNativeOperation: false
        ))
    }

    func testFailureAttentionWaitsForNativeSendOwnershipRelease() {
        XCTAssertFalse(RememberedRoomOutboxDeliveryCleanupPolicy.canPublishNeedsAttention(
            state: .failed,
            activityMatches: true,
            senderOwnsNativeOperation: true
        ))
        XCTAssertFalse(RememberedRoomOutboxDeliveryCleanupPolicy.canPublishNeedsAttention(
            state: .canceled,
            activityMatches: true,
            senderOwnsNativeOperation: true
        ))
        XCTAssertTrue(RememberedRoomOutboxDeliveryCleanupPolicy.canPublishNeedsAttention(
            state: .failed,
            activityMatches: true,
            senderOwnsNativeOperation: false
        ))
        XCTAssertFalse(RememberedRoomOutboxDeliveryCleanupPolicy.canPublishNeedsAttention(
            state: .delivered,
            activityMatches: true,
            senderOwnsNativeOperation: false
        ))
        XCTAssertFalse(RememberedRoomOutboxDeliveryCleanupPolicy.canPublishNeedsAttention(
            state: .failed,
            activityMatches: false,
            senderOwnsNativeOperation: false
        ))
    }

    func testClaimPersistsAndInterruptedOfferRequiresExplicitRetry() throws {
        let root = temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        var now: Int64 = 1_000
        let file = root.appendingPathComponent("outbox.json")
        let store = RememberedRoomOutboxStore(
            fileURL: file,
            clockEpochMilliseconds: { now }
        )
        let queued = try store.enqueue(
            relationshipID: "relationship-a",
            jobID: "0123456789abcdef0123456789abcdef",
            rootNames: ["Photos"],
            itemCount: 2,
            directoryCount: 0,
            totalBytes: 4_096
        )

        now = 2_000
        let claimed = try XCTUnwrap(store.claimNext(relationshipID: "relationship-a"))
        XCTAssertEqual(claimed.id, queued.id)
        XCTAssertEqual(claimed.state, .offering)
        XCTAssertNil(try store.claimNext(relationshipID: "relationship-a"))

        let restored = RememberedRoomOutboxStore(
            fileURL: file,
            clockEpochMilliseconds: { 3_000 }
        )
        XCTAssertEqual(try restored.reconcileInterruptedAttempts(), 1)
        let interrupted = try XCTUnwrap(restored.entries().first)
        XCTAssertEqual(interrupted.state, .needsAttention)
        XCTAssertTrue(interrupted.lastError?.contains("interrupted") == true)
        XCTAssertTrue(try restored.retry(id: interrupted.id))
        XCTAssertEqual(try restored.entries().first?.state, .queued)
    }

    func testOfferIdentityRejectsStaleCompletion() throws {
        let root = temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RememberedRoomOutboxStore(
            fileURL: root.appendingPathComponent("outbox.json"),
            clockEpochMilliseconds: { 1_000 }
        )
        let queued = try store.enqueue(
            relationshipID: "relationship-a",
            jobID: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            rootNames: ["one"],
            itemCount: 1,
            directoryCount: 0,
            totalBytes: 1
        )
        let first = try XCTUnwrap(store.claimNext(relationshipID: "relationship-a"))

        XCTAssertFalse(try store.requeue(id: queued.id, offerID: "stale"))
        XCTAssertTrue(
            try store.requeue(
                id: queued.id,
                offerID: try XCTUnwrap(first.offerID)
            )
        )
        let second = try XCTUnwrap(store.claimNext(relationshipID: "relationship-a"))
        XCTAssertFalse(
            try store.markTransferring(
                id: queued.id,
                offerID: try XCTUnwrap(first.offerID),
                activityID: "old"
            )
        )
        XCTAssertTrue(
            try store.markTransferring(
                id: queued.id,
                offerID: try XCTUnwrap(second.offerID),
                activityID: "new"
            )
        )
        XCTAssertEqual(try store.entries().first?.activityID, "new")
    }

    func testSameManifestJobIsDeduplicatedWithinRoom() throws {
        let root = temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = RememberedRoomOutboxStore(
            fileURL: root.appendingPathComponent("outbox.json"),
            clockEpochMilliseconds: { 1_000 }
        )
        let first = try store.enqueue(
            relationshipID: "relationship-a",
            jobID: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            rootNames: ["first"],
            itemCount: 1,
            directoryCount: 0,
            totalBytes: 2
        )
        let duplicate = try store.enqueue(
            relationshipID: "relationship-a",
            jobID: first.jobID,
            rootNames: ["different projection"],
            itemCount: 9,
            directoryCount: 3,
            totalBytes: 99
        )

        XCTAssertEqual(first, duplicate)
        XCTAssertEqual(try store.entries().count, 1)
    }

    private func temporaryDirectory() -> URL {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("envoix-outbox-\(UUID().uuidString)", isDirectory: true)
        try? FileManager.default.createDirectory(
            at: root,
            withIntermediateDirectories: true
        )
        return root
    }
}
