import Foundation
import LocalAuthentication
import Security
import XCTest

final class RememberedPeerStoreTests: XCTestCase {
    func testListingPeersDoesNotReadProtectedCredentials() throws {
        let credentials = RecordingCredentialStore()
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = RememberedPeerStore(
            credentialStore: credentials,
            metadataFileURL: directory.appendingPathComponent("remembered.json")
        )
        let pending = try store.prepare(
            label: "WSL Agent",
            broker: "udp://broker.example:8555",
            relay: ""
        )
        try store.acquireSession(pending.relationshipID)
        try store.create(
            pending,
            opaqueCredential: Data([1, 2, 3, 4]),
            generation: 0
        )
        store.releaseSession(pending.relationshipID)

        XCTAssertEqual(try store.peers().map(\.label), ["WSL Agent"])
        XCTAssertEqual(credentials.getCallCount, 0)
    }

    func testCredentialReadDisallowsAuthenticationInteraction() throws {
        let query = AppleCredentialStore().readQuery("relationship")
        let context = try XCTUnwrap(
            query[kSecUseAuthenticationContext] as? LAContext
        )

        XCTAssertTrue(context.interactionNotAllowed)
    }

    func testCredentialAuthorizationFailuresRequireRepair() {
        XCTAssertTrue(
            AppleCredentialStore.requiresRepair(errSecInteractionNotAllowed)
        )
        XCTAssertTrue(AppleCredentialStore.requiresRepair(errSecAuthFailed))
        XCTAssertFalse(AppleCredentialStore.requiresRepair(errSecNotAvailable))
    }
}

private final class RecordingCredentialStore: RememberedCredentialStoring {
    private var values: [String: Data] = [:]
    private(set) var getCallCount = 0

    func put(_ reference: String, _ credential: Data) throws {
        values[reference] = credential
    }

    func get(_ reference: String) throws -> Data {
        getCallCount += 1
        guard let value = values[reference] else {
            throw RememberedPeerStoreError.missingCredential
        }
        return value
    }

    func delete(_ reference: String) throws {
        values.removeValue(forKey: reference)
    }
}
