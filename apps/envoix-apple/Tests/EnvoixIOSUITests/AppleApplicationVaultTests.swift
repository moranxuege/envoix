import EnvoixCore
import Foundation
import LocalAuthentication
import Security
import XCTest

#if os(iOS)
@testable import Envoix_iOS
#elseif os(macOS)
@testable import Envoix
#endif

final class AppleApplicationVaultTests: XCTestCase {
    func testContainsDoesNotRequestCredentialData() throws {
        let keychain = RecordingApplicationVaultKeychainAccess()
        keychain.copyStatus = errSecSuccess
        keychain.copyData = Data([1, 2, 3, 4])
        let vault = AppleApplicationVault(
            configuration: .iOSApplication,
            keychain: keychain
        )

        XCTAssertTrue(try vault.contains(reference: "credential_1"))

        let query = try XCTUnwrap(keychain.copyQueries.first)
        XCTAssertNil(query[kSecReturnData])
        XCTAssertEqual(query[kSecMatchLimit] as? String, kSecMatchLimitOne as String)
    }

    func testContainsReturnsFalseWhenItemIsMissing() throws {
        let keychain = RecordingApplicationVaultKeychainAccess()
        keychain.copyStatus = errSecItemNotFound
        let vault = AppleApplicationVault(
            configuration: .iOSApplication,
            keychain: keychain
        )

        XCTAssertFalse(try vault.contains(reference: "credential_1"))
    }

    func testLoadReturnsNilWhenItemIsMissing() throws {
        let keychain = RecordingApplicationVaultKeychainAccess()
        keychain.copyStatus = errSecItemNotFound
        let vault = AppleApplicationVault(
            configuration: .iOSApplication,
            keychain: keychain
        )

        XCTAssertNil(try vault.load(reference: "credential_1"))
    }

    func testLoadReturnsOpaqueCredentialWithoutTransformingIt() throws {
        let credential = Data([0, 1, 2, 255])
        let keychain = RecordingApplicationVaultKeychainAccess()
        keychain.copyStatus = errSecSuccess
        keychain.copyData = credential
        let vault = AppleApplicationVault(
            configuration: .iOSApplication,
            keychain: keychain
        )

        XCTAssertEqual(try vault.load(reference: "credential_1"), credential)
        let query = try XCTUnwrap(keychain.copyQueries.first)
        XCTAssertEqual(query[kSecReturnData] as? Bool, true)
    }

    func testLoadRejectsMissingOrEmptySuccessPayloadAsCorrupt() {
        for payload in [nil, Data()] as [Data?] {
            let keychain = RecordingApplicationVaultKeychainAccess()
            keychain.copyStatus = errSecSuccess
            keychain.copyData = payload
            let vault = AppleApplicationVault(
                configuration: .iOSApplication,
                keychain: keychain
            )

            assertVaultError(.CorruptData) {
                try vault.load(reference: "credential_1")
            }
        }
    }

    func testStoreUpdatesBeforeAddingAndOnlyAddsWhenMissing() throws {
        let credential = Data([1, 2, 3, 4])

        let existingKeychain = RecordingApplicationVaultKeychainAccess()
        let existingVault = AppleApplicationVault(
            configuration: .iOSApplication,
            keychain: existingKeychain
        )
        try existingVault.store(
            reference: "credential_existing",
            opaqueCredential: credential
        )
        XCTAssertEqual(existingKeychain.updateQueries.count, 1)
        XCTAssertTrue(existingKeychain.addItems.isEmpty)

        let missingKeychain = RecordingApplicationVaultKeychainAccess()
        missingKeychain.updateStatus = errSecItemNotFound
        let missingVault = AppleApplicationVault(
            configuration: .iOSApplication,
            keychain: missingKeychain
        )
        try missingVault.store(
            reference: "credential_missing",
            opaqueCredential: credential
        )
        XCTAssertEqual(missingKeychain.updateQueries.count, 1)
        XCTAssertEqual(missingKeychain.addItems.count, 1)
        XCTAssertEqual(missingKeychain.addItems[0][kSecValueData] as? Data, credential)
    }

    func testStoreRejectsEmptyCredentialBeforeAccessingKeychain() {
        let keychain = RecordingApplicationVaultKeychainAccess()
        let vault = AppleApplicationVault(
            configuration: .iOSApplication,
            keychain: keychain
        )

        assertVaultError(.InvalidRequest) {
            try vault.store(reference: "credential_1", opaqueCredential: Data())
        }
        XCTAssertTrue(keychain.updateQueries.isEmpty)
        XCTAssertTrue(keychain.addItems.isEmpty)
    }

    func testStoreDoesNotFallBackToAddForTypedFailures() {
        let keychain = RecordingApplicationVaultKeychainAccess()
        keychain.updateStatus = errSecInteractionNotAllowed
        let vault = AppleApplicationVault(
            configuration: .iOSApplication,
            keychain: keychain
        )

        assertVaultError(.InteractionRequired) {
            try vault.store(reference: "credential_1", opaqueCredential: Data([1]))
        }
        XCTAssertEqual(keychain.updateQueries.count, 1)
        XCTAssertTrue(keychain.addItems.isEmpty)
    }

    func testStorePropagatesAddFailureWithoutRetrying() {
        let keychain = RecordingApplicationVaultKeychainAccess()
        keychain.updateStatus = errSecItemNotFound
        keychain.addStatus = errSecUserCanceled
        let vault = AppleApplicationVault(
            configuration: .iOSApplication,
            keychain: keychain
        )

        assertVaultError(.Canceled) {
            try vault.store(reference: "credential_1", opaqueCredential: Data([1]))
        }
        XCTAssertEqual(keychain.updateQueries.count, 1)
        XCTAssertEqual(keychain.addItems.count, 1)
    }

    func testDeleteTreatsMissingItemAsSuccess() throws {
        let keychain = RecordingApplicationVaultKeychainAccess()
        keychain.deleteStatus = errSecItemNotFound
        let vault = AppleApplicationVault(
            configuration: .iOSApplication,
            keychain: keychain
        )

        XCTAssertNoThrow(try vault.delete(reference: "credential_1"))
        XCTAssertEqual(keychain.deleteQueries.count, 1)
    }

    func testOSStatusFailuresMapToTypedVaultErrors() {
        let mappings: [(OSStatus, FfiApplicationVaultError)] = [
            (errSecInteractionNotAllowed, .InteractionRequired),
            (errSecInteractionRequired, .InteractionRequired),
            (errSecAuthFailed, .PermissionDenied),
            (errSecNoAccessForItem, .PermissionDenied),
            (errSecMissingEntitlement, .PermissionDenied),
            (errSecUserCanceled, .Canceled),
            (errSecDecode, .CorruptData),
            (errSecParam, .InvalidRequest),
            (errSecNotAvailable, .Unavailable),
            (OSStatus(-424_242), .Unavailable),
        ]

        for (status, expectedError) in mappings {
            let keychain = RecordingApplicationVaultKeychainAccess()
            keychain.copyStatus = status
            let vault = AppleApplicationVault(
                configuration: .iOSApplication,
                keychain: keychain
            )

            assertVaultError(expectedError) {
                try vault.contains(reference: "credential_1")
            }
        }
    }

    func testReferencesAreBoundedASCIIIdentifiers() throws {
        let invalidReferences = [
            "",
            "credential/1",
            "credential 1",
            "credential.1",
            "crédential_1",
            String(repeating: "a", count: 129),
        ]

        for reference in invalidReferences {
            let keychain = RecordingApplicationVaultKeychainAccess()
            let vault = AppleApplicationVault(
                configuration: .iOSApplication,
                keychain: keychain
            )

            assertVaultError(.InvalidRequest) {
                try vault.contains(reference: reference)
            }
            XCTAssertTrue(keychain.copyQueries.isEmpty)
        }

        let keychain = RecordingApplicationVaultKeychainAccess()
        let vault = AppleApplicationVault(
            configuration: .iOSApplication,
            keychain: keychain
        )
        XCTAssertTrue(
            try vault.contains(reference: String(repeating: "A", count: 128))
        )
        XCTAssertTrue(try vault.contains(reference: "AZaz09_-"))
    }

    func testEveryKeychainQueryIsNonInteractiveAndUsesDataProtection() throws {
        let keychain = RecordingApplicationVaultKeychainAccess()
        keychain.updateStatus = errSecItemNotFound
        keychain.copyStatus = errSecSuccess
        keychain.copyData = Data([1])
        let vault = AppleApplicationVault(
            configuration: .iOSApplication,
            keychain: keychain
        )

        try vault.store(reference: "credential_1", opaqueCredential: Data([1]))
        _ = try vault.contains(reference: "credential_1")
        _ = try vault.load(reference: "credential_1")
        try vault.delete(reference: "credential_1")

        let queries = keychain.updateQueries
            + keychain.addItems
            + keychain.copyQueries
            + keychain.deleteQueries
        XCTAssertEqual(queries.count, 5)
        for query in queries {
            let context = try XCTUnwrap(query[kSecUseAuthenticationContext] as? LAContext)
            XCTAssertTrue(context.interactionNotAllowed)
            XCTAssertEqual(query[kSecUseDataProtectionKeychain] as? Bool, true)
            XCTAssertEqual(query[kSecAttrSynchronizable] as? Bool, false)
            XCTAssertNil(query[kSecAttrAccessGroup])
        }
    }

    func testMacOSHelperConfigurationUsesOnlyExplicitAccessGroup() throws {
        let keychain = RecordingApplicationVaultKeychainAccess()
        let vault = AppleApplicationVault(
            configuration: .macOSHelper(),
            keychain: keychain
        )

        _ = try vault.contains(reference: "credential_1")

        let query = try XCTUnwrap(keychain.copyQueries.first)
        XCTAssertEqual(
            query[kSecAttrAccessGroup] as? String,
            AppleApplicationVault.helperAccessGroup
        )
    }

    private func assertVaultError<T>(
        _ expectedError: FfiApplicationVaultError,
        operation: () throws -> T,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        XCTAssertThrowsError(try operation(), file: file, line: line) { error in
            XCTAssertEqual(
                error as? FfiApplicationVaultError,
                expectedError,
                file: file,
                line: line
            )
        }
    }
}

private final class RecordingApplicationVaultKeychainAccess: AppleKeychainAccessing {
    var updateStatus = errSecSuccess
    var addStatus = errSecSuccess
    var copyStatus = errSecSuccess
    var copyData: Data?
    var deleteStatus = errSecSuccess

    private(set) var updateQueries: [[CFString: Any]] = []
    private(set) var updateAttributes: [[CFString: Any]] = []
    private(set) var addItems: [[CFString: Any]] = []
    private(set) var copyQueries: [[CFString: Any]] = []
    private(set) var deleteQueries: [[CFString: Any]] = []

    func update(
        _ query: [CFString: Any],
        attributes: [CFString: Any]
    ) -> OSStatus {
        updateQueries.append(query)
        updateAttributes.append(attributes)
        return updateStatus
    }

    func add(_ item: [CFString: Any]) -> OSStatus {
        addItems.append(item)
        return addStatus
    }

    func copyMatching(_ query: [CFString: Any]) -> (OSStatus, Data?) {
        copyQueries.append(query)
        return (copyStatus, copyData)
    }

    func delete(_ query: [CFString: Any]) -> OSStatus {
        deleteQueries.append(query)
        return deleteStatus
    }
}
