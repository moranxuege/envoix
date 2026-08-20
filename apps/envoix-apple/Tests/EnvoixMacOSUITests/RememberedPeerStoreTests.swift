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

    func testMacOSUsesKeychainCredentialStoreByDefault() {
        XCTAssertTrue(
            RememberedPeerStore.makeDefaultCredentialStore() is AppleCredentialStore
        )
    }

    #if DEBUG
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
    #endif

    func testKeychainCredentialStoreAlwaysDisablesAuthenticationUI() throws {
        let keychain = RecordingAppleKeychainAccess()
        let store = AppleCredentialStore(keychain: keychain)
        let credential = Data([1, 2, 3, 4])

        try store.put(UUID().uuidString, credential)
        XCTAssertEqual(try store.get(UUID().uuidString), credential)
        try store.delete(UUID().uuidString)

        XCTAssertEqual(keychain.queries.count, 3)
        for query in keychain.queries {
            let context = try XCTUnwrap(query[kSecUseAuthenticationContext] as? LAContext)
            XCTAssertTrue(context.interactionNotAllowed)
            XCTAssertEqual(query[kSecUseDataProtectionKeychain] as? Bool, true)
        }
    }

    func testKeychainInteractionFailuresFailClosedWithoutAddFallback() {
        let statuses = [
            errSecInteractionNotAllowed,
            errSecInteractionRequired,
            errSecAuthFailed,
            errSecUserCanceled,
        ]

        for status in statuses {
            let keychain = RecordingAppleKeychainAccess()
            keychain.updateStatus = status
            let store = AppleCredentialStore(keychain: keychain)

            assertCredentialInteractionRequired {
                try store.put(UUID().uuidString, Data([1]))
            }
            XCTAssertEqual(keychain.updateCallCount, 1)
            XCTAssertEqual(keychain.addCallCount, 0)
        }
    }

    func testKeychainAddInteractionFailureDoesNotRetry() {
        let keychain = RecordingAppleKeychainAccess()
        keychain.updateStatus = errSecItemNotFound
        keychain.addStatus = errSecInteractionNotAllowed
        let store = AppleCredentialStore(keychain: keychain)

        assertCredentialInteractionRequired {
            try store.put(UUID().uuidString, Data([1]))
        }
        XCTAssertEqual(keychain.updateCallCount, 1)
        XCTAssertEqual(keychain.addCallCount, 1)
    }

    func testKeychainReadAndDeleteInteractionFailuresFailClosed() {
        let keychain = RecordingAppleKeychainAccess()
        keychain.copyStatus = errSecAuthFailed
        keychain.deleteStatus = errSecInteractionRequired
        let store = AppleCredentialStore(keychain: keychain)

        assertCredentialInteractionRequired {
            _ = try store.get(UUID().uuidString)
        }
        assertCredentialInteractionRequired {
            try store.delete(UUID().uuidString)
        }
        XCTAssertEqual(keychain.copyCallCount, 1)
        XCTAssertEqual(keychain.deleteCallCount, 1)
    }

    private func assertCredentialInteractionRequired(
        _ operation: () throws -> Void,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        XCTAssertThrowsError(try operation(), file: file, line: line) { error in
            guard case RememberedPeerStoreError.credentialInteractionRequired = error else {
                return XCTFail("Unexpected error: \(error)", file: file, line: line)
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

private final class RecordingAppleKeychainAccess: AppleKeychainAccessing {
    var updateStatus = errSecSuccess
    var addStatus = errSecSuccess
    var copyStatus = errSecSuccess
    var copyData = Data([1, 2, 3, 4])
    var deleteStatus = errSecSuccess

    private(set) var queries: [[CFString: Any]] = []
    private(set) var updateCallCount = 0
    private(set) var addCallCount = 0
    private(set) var copyCallCount = 0
    private(set) var deleteCallCount = 0

    func update(
        _ query: [CFString: Any],
        attributes _: [CFString: Any]
    ) -> OSStatus {
        queries.append(query)
        updateCallCount += 1
        return updateStatus
    }

    func add(_ item: [CFString: Any]) -> OSStatus {
        queries.append(item)
        addCallCount += 1
        return addStatus
    }

    func copyMatching(_ query: [CFString: Any]) -> (OSStatus, Data?) {
        queries.append(query)
        copyCallCount += 1
        return (copyStatus, copyData)
    }

    func delete(_ query: [CFString: Any]) -> OSStatus {
        queries.append(query)
        deleteCallCount += 1
        return deleteStatus
    }
}
