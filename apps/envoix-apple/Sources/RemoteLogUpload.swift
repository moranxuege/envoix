import Foundation

/// Debug-only client for the rendezvous server's per-room diagnostic endpoint.
enum RemoteLogUpload {
    static let bodyMaxBytes = 480 * 1024
    private static let developerModeKey = "envoix.developerMode"

    struct Target {
        let roomID: String
        let side: String
    }

    enum UploadError: LocalizedError {
        case invalidServer
        case invalidRoomID
        case developerModeRequired
        case bodyTooLarge
        case unexpectedResponse(Int)

        var errorDescription: String? {
            switch self {
            case .invalidServer:
                return "Invalid diagnostic log server URL."
            case .invalidRoomID:
                return "Invalid room ID for diagnostic upload."
            case .developerModeRequired:
                return "Enable Developer Mode to upload diagnostics."
            case .bodyTooLarge:
                return "Diagnostic report exceeds the upload limit."
            case let .unexpectedResponse(status):
                return "Diagnostic upload failed (HTTP \(status))."
            }
        }
    }

    static var isEnabledInCurrentBuild: Bool {
        #if DEBUG
        true
        #else
        false
        #endif
    }

    /// Matches Android's `room.substringBefore('-')` upload key without retaining the secret suffix.
    static func roomID(from code: String) -> String? {
        let roomID = code.trimmed.split(separator: "-", maxSplits: 1).first.map(String.init) ?? ""
        guard
            !roomID.isEmpty,
            roomID.utf8.count <= 64,
            roomID.unicodeScalars.allSatisfy({ $0.isASCII && CharacterSet.alphanumerics.contains($0) })
        else {
            return nil
        }
        return roomID
    }

    /// Uses an alphanumeric collection key so an app-level report can be sent
    /// even before a Room transfer exists.
    static func appTarget() -> Target {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyyMMddHHmmss"
        return Target(roomID: "app\(formatter.string(from: Date()))", side: "app")
    }

    static func upload(server: String, target: Target, body: String) async throws {
        #if DEBUG
        guard UserDefaults.standard.bool(forKey: developerModeKey) else {
            throw UploadError.developerModeRequired
        }
        guard body.utf8.count <= bodyMaxBytes else { throw UploadError.bodyTooLarge }
        let servers = uploadServers(startingWith: server)
        for (index, server) in servers.enumerated() {
            do {
                try await uploadOnce(server: server, target: target, body: body)
                return
            } catch {
                guard index + 1 < servers.count, shouldFallback(after: error) else {
                    throw error
                }
            }
        }
        throw UploadError.invalidServer
        #else
        throw UploadError.invalidServer
        #endif
    }

    private static func uploadOnce(server: String, target: Target, body: String) async throws {
        let url = try uploadURL(server: server, target: target)
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.httpBody = Data(body.utf8)
        request.timeoutInterval = 8
        request.setValue("text/plain; charset=utf-8", forHTTPHeaderField: "Content-Type")

        let (_, response) = try await URLSession.shared.data(for: request)
        guard let response = response as? HTTPURLResponse else {
            throw UploadError.unexpectedResponse(-1)
        }
        guard (200...299).contains(response.statusCode) else {
            throw UploadError.unexpectedResponse(response.statusCode)
        }
    }

    /// Prefer HTTPS even when an older HTTP address remains in user settings.
    private static func uploadServers(startingWith server: String) -> [String] {
        let configuredServer = server.trimmed
        let preferredServer = deprecatedLogServers.contains(configuredServer) ? defaultLogServer : configuredServer
        guard
            var components = URLComponents(string: preferredServer),
            let scheme = components.scheme?.lowercased(),
            scheme == "http" || scheme == "https"
        else {
            return [preferredServer]
        }

        components.scheme = "https"
        let https = components.url?.absoluteString
        components.scheme = "http"
        let http = components.url?.absoluteString
        return [https, http].compactMap { $0 }.reduce(into: []) { servers, candidate in
            if !servers.contains(candidate) { servers.append(candidate) }
        }
    }

    private static func shouldFallback(after error: Error) -> Bool {
        guard let uploadError = error as? UploadError else { return true }
        guard case let .unexpectedResponse(status) = uploadError else { return false }
        return status < 0 || status >= 500
    }

    private static func uploadURL(server: String, target: Target) throws -> URL {
        guard
            target.roomID.utf8.count <= 64,
            !target.roomID.isEmpty,
            target.roomID.unicodeScalars.allSatisfy({ $0.isASCII && CharacterSet.alphanumerics.contains($0) })
        else {
            throw UploadError.invalidRoomID
        }
        guard
            var components = URLComponents(string: server.trimmed),
            let scheme = components.scheme?.lowercased(),
            scheme == "http" || scheme == "https",
            components.host != nil
        else {
            throw UploadError.invalidServer
        }
        guard !target.side.trimmed.isEmpty else { throw UploadError.invalidRoomID }

        let basePath = components.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        components.path = "/" + ([basePath, "logs", target.roomID].filter { !$0.isEmpty }.joined(separator: "/"))
        components.queryItems = [URLQueryItem(name: "side", value: target.side.trimmed)]
        guard let url = components.url else { throw UploadError.invalidServer }
        return url
    }
}
