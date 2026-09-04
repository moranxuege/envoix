import Foundation

/// Debug-only client for the rendezvous server's per-room diagnostic endpoint.
enum RemoteLogUpload {
    static let bodyMaxBytes = 480 * 1024
    private static let developerModeKey = "envoix.developerMode"
    private static let uploadTokenEnvironment = "ENVOIX_DIAGNOSTIC_UPLOAD_TOKEN"
    private static let tokenMaxBytes = 1024

    struct Target {
        let roomID: String
        let side: String
    }

    enum UploadError: LocalizedError, Equatable {
        case invalidServer
        case invalidRoomID
        case authenticationRequired
        case developerModeRequired
        case bodyTooLarge
        case unexpectedResponse(Int)

        var errorDescription: String? {
            switch self {
            case .invalidServer:
                return "Invalid diagnostic log server URL."
            case .invalidRoomID:
                return "Invalid room ID for diagnostic upload."
            case .authenticationRequired:
                return "A diagnostic upload token is required."
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
        let token = ProcessInfo.processInfo.environment[uploadTokenEnvironment]
        let request = try request(
            server: server,
            target: target,
            body: body,
            bearerToken: token ?? ""
        )
        try await upload(request)
        #else
        throw UploadError.invalidServer
        #endif
    }

    static func request(
        server: String,
        target: Target,
        body: String,
        bearerToken: String
    ) throws -> URLRequest {
        guard body.utf8.count <= bodyMaxBytes else { throw UploadError.bodyTooLarge }
        let token = bearerToken.trimmed
        guard
            !token.isEmpty,
            token.utf8.count <= tokenMaxBytes,
            token.utf8.allSatisfy({ $0 >= 0x21 && $0 <= 0x7e })
        else {
            throw UploadError.authenticationRequired
        }

        let url = try uploadURL(server: server, target: target)
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.httpBody = Data(body.utf8)
        request.timeoutInterval = 8
        request.setValue("text/plain; charset=utf-8", forHTTPHeaderField: "Content-Type")
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return request
    }

    private static func upload(_ request: URLRequest) async throws {
        let (_, response) = try await URLSession.shared.data(for: request)
        guard let response = response as? HTTPURLResponse else {
            throw UploadError.unexpectedResponse(-1)
        }
        guard (200...299).contains(response.statusCode) else {
            throw UploadError.unexpectedResponse(response.statusCode)
        }
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
            scheme == "https",
            components.host != nil,
            components.user == nil,
            components.password == nil,
            components.query == nil,
            components.fragment == nil
        else {
            throw UploadError.invalidServer
        }
        let side = target.side.trimmed
        guard
            !side.isEmpty,
            side.utf8.count <= 16,
            side.unicodeScalars.allSatisfy({ $0.isASCII && CharacterSet.alphanumerics.contains($0) })
        else {
            throw UploadError.invalidRoomID
        }

        let basePath = components.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        components.path = "/" + ([basePath, "logs", target.roomID].filter { !$0.isEmpty }.joined(separator: "/"))
        components.queryItems = [URLQueryItem(name: "side", value: side)]
        guard let url = components.url else { throw UploadError.invalidServer }
        return url
    }
}
