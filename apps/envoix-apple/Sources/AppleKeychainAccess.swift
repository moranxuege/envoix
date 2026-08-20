import Foundation
import Security

protocol AppleKeychainAccessing {
    func update(
        _ query: [CFString: Any],
        attributes: [CFString: Any]
    ) -> OSStatus
    func add(_ item: [CFString: Any]) -> OSStatus
    func copyMatching(_ query: [CFString: Any]) -> (OSStatus, Data?)
    func delete(_ query: [CFString: Any]) -> OSStatus
}

final class SystemAppleKeychainAccess: AppleKeychainAccessing {
    func update(
        _ query: [CFString: Any],
        attributes: [CFString: Any]
    ) -> OSStatus {
        SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
    }

    func add(_ item: [CFString: Any]) -> OSStatus {
        SecItemAdd(item as CFDictionary, nil)
    }

    func copyMatching(_ query: [CFString: Any]) -> (OSStatus, Data?) {
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        return (status, result as? Data)
    }

    func delete(_ query: [CFString: Any]) -> OSStatus {
        SecItemDelete(query as CFDictionary)
    }
}
