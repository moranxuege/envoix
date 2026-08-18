import Foundation
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

    func testMacOSUsesFileCredentialStoreByDefault() {
        XCTAssertTrue(
            RememberedPeerStore.makeDefaultCredentialStore() is MacOSFileCredentialStore
        )
    }

    func testFileCredentialStoreRoundTripsWithRestrictedPermissions() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = MacOSFileCredentialStore(directoryURL: directory)
        let reference = UUID().uuidString
        let credential = Data([1, 2, 3, 4])

        try store.put(reference, credential)

        let reopenedStore = MacOSFileCredentialStore(directoryURL: directory)
        XCTAssertEqual(try reopenedStore.get(reference), credential)
        let directoryPermissions = try XCTUnwrap(
            FileManager.default.attributesOfItem(atPath: directory.path)[.posixPermissions]
                as? NSNumber
        )
        let credentialPermissions = try XCTUnwrap(
            FileManager.default.attributesOfItem(
                atPath: directory.appendingPathComponent(reference).path
            )[.posixPermissions] as? NSNumber
        )
        XCTAssertEqual(directoryPermissions.intValue & 0o777, 0o700)
        XCTAssertEqual(credentialPermissions.intValue & 0o777, 0o600)

        try reopenedStore.delete(reference)
        XCTAssertThrowsError(try reopenedStore.get(reference)) { error in
            guard case RememberedPeerStoreError.missingCredential = error else {
                return XCTFail("Unexpected error: \(error)")
            }
        }
    }

    func testFileCredentialStoreRejectsPathTraversal() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = MacOSFileCredentialStore(directoryURL: directory)

        XCTAssertThrowsError(try store.put("../credential", Data([1]))) { error in
            guard case RememberedPeerStoreError.corruptMetadata = error else {
                return XCTFail("Unexpected error: \(error)")
            }
        }
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
