import EnvoixCore
import Foundation
import LocalAuthentication
import Security

final class AppleApplicationVault: FfiApplicationVault, @unchecked Sendable {
    static let helperAccessGroup = "6638TTB2SF.com.envoix.engine.credentials"

    private static let keychainService = "com.envoix.application-vault.v1"
    private static let maximumReferenceByteCount = 128
    private static let allowedReferenceBytes = Set(
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-".utf8
    )

    private let configuration: AppleApplicationVaultConfiguration
    private let keychain: AppleKeychainAccessing

    init(
        configuration: AppleApplicationVaultConfiguration,
        keychain: AppleKeychainAccessing? = nil
    ) {
        self.configuration = configuration
        self.keychain = keychain ?? SystemAppleKeychainAccess()
    }

    func contains(reference: String) throws -> Bool {
        var query = try baseQuery(reference: reference)
        query[kSecMatchLimit] = kSecMatchLimitOne
        let (status, _) = keychain.copyMatching(query)
        switch status {
        case errSecSuccess:
            return true
        case errSecItemNotFound:
            return false
        default:
            throw Self.vaultError(for: status)
        }
    }

    func store(reference: String, opaqueCredential: Data) throws {
        guard !opaqueCredential.isEmpty else {
            throw FfiApplicationVaultError.InvalidRequest
        }
        let query = try baseQuery(reference: reference)
        let attributes: [CFString: Any] = [
            kSecValueData: opaqueCredential,
            kSecAttrAccessible: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
        ]
        let updateStatus = keychain.update(query, attributes: attributes)
        if updateStatus == errSecSuccess {
            return
        }
        guard updateStatus == errSecItemNotFound else {
            throw Self.vaultError(for: updateStatus)
        }

        var item = query
        attributes.forEach { item[$0] = $1 }
        let addStatus = keychain.add(item)
        guard addStatus == errSecSuccess else {
            throw Self.vaultError(for: addStatus)
        }
    }

    func load(reference: String) throws -> Data? {
        var query = try baseQuery(reference: reference)
        query[kSecReturnData] = true
        query[kSecMatchLimit] = kSecMatchLimitOne
        let (status, data) = keychain.copyMatching(query)
        if status == errSecItemNotFound {
            return nil
        }
        guard status == errSecSuccess else {
            throw Self.vaultError(for: status)
        }
        guard let data, !data.isEmpty else {
            throw FfiApplicationVaultError.CorruptData
        }
        return data
    }

    func delete(reference: String) throws {
        let status = keychain.delete(try baseQuery(reference: reference))
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw Self.vaultError(for: status)
        }
    }

    private func baseQuery(reference: String) throws -> [CFString: Any] {
        let bytes = reference.utf8
        guard !bytes.isEmpty,
              bytes.count <= Self.maximumReferenceByteCount,
              bytes.allSatisfy(Self.allowedReferenceBytes.contains)
        else {
            throw FfiApplicationVaultError.InvalidRequest
        }

        let authenticationContext = LAContext()
        authenticationContext.interactionNotAllowed = true
        var query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: Self.keychainService,
            kSecAttrAccount: reference,
            kSecAttrSynchronizable: false,
            kSecUseAuthenticationContext: authenticationContext,
            kSecUseDataProtectionKeychain: true,
        ]
        if let accessGroup = configuration.accessGroup {
            query[kSecAttrAccessGroup] = accessGroup
        }
        return query
    }

    private static func vaultError(for status: OSStatus) -> FfiApplicationVaultError {
        switch status {
        case errSecInteractionNotAllowed, errSecInteractionRequired:
            return .InteractionRequired
        case errSecAuthFailed, errSecNoAccessForItem, errSecMissingEntitlement:
            return .PermissionDenied
        case errSecUserCanceled:
            return .Canceled
        case errSecDecode:
            return .CorruptData
        case errSecParam:
            return .InvalidRequest
        case errSecNotAvailable:
            return .Unavailable
        default:
            return .Unavailable
        }
    }
}

enum AppleApplicationVaultConfiguration: Equatable, Sendable {
    case iOSApplication
    case macOSHelper(accessGroup: String = AppleApplicationVault.helperAccessGroup)

    fileprivate var accessGroup: String? {
        switch self {
        case .iOSApplication:
            return nil
        case let .macOSHelper(accessGroup):
            return accessGroup
        }
    }
}
