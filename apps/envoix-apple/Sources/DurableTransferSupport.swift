import Foundation
import EnvoixCore

private let receiptMailboxPath = "receipts"

/// Native HTTPS courier for opaque receipt blobs. Rust owns the mailbox key,
/// payload authentication, polling schedule, and state transition.
final class AppleMailboxObserver: MailboxObserverV2, @unchecked Sendable {
    private weak var model: AppModel?
    private let session: URLSession

    init(model: AppModel, session: URLSession = .shared) {
        self.model = model
        self.session = session
    }

    func onFetchReceipt(activityId: String, key: String, server: String?) {
        guard let url = mailboxURL(key: key, server: server) else {
            deliverReceipt(Data(), activityID: activityId)
            return
        }
        var request = URLRequest(url: url)
        request.httpMethod = "GET"
        request.timeoutInterval = 15
        session.dataTask(with: request) { [weak self] data, response, _ in
            let status = (response as? HTTPURLResponse)?.statusCode
            self?.deliverReceipt(status == 200 ? (data ?? Data()) : Data(), activityID: activityId)
        }.resume()
    }

    func onPostReceipt(activityId: String, key: String, blob: Data, server: String?) {
        postReceipt(activityID: activityId, key: key, blob: blob, server: server, attempt: 0)
    }

    private func postReceipt(
        activityID: String,
        key: String,
        blob: Data,
        server: String?,
        attempt: Int
    ) {
        guard let url = mailboxURL(key: key, server: server) else { return }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.httpBody = blob
        request.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
        request.timeoutInterval = 15
        session.dataTask(with: request) { [weak self] _, response, _ in
            guard let self else { return }
            let status = (response as? HTTPURLResponse)?.statusCode ?? 0
            if (200..<300).contains(status) {
                DispatchQueue.main.async { [weak self] in
                    self?.model?.acknowledgeReceiptPost(activityID: activityID)
                }
                return
            }
            let delays: [TimeInterval] = [1, 3, 10, 30]
            guard attempt < delays.count else { return }
            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + delays[attempt]) { [weak self] in
                self?.postReceipt(
                    activityID: activityID,
                    key: key,
                    blob: blob,
                    server: server,
                    attempt: attempt + 1
                )
            }
        }.resume()
    }

    private func deliverReceipt(_ data: Data, activityID: String) {
        DispatchQueue.main.async { [weak self] in
            self?.model?.deliverReceipt(data, activityID: activityID)
        }
    }

    private func mailboxURL(key: String, server: String?) -> URL? {
        let frozenServer = server?.trimmed ?? ""
        let effectiveServer = frozenServer.isEmpty ? currentReceiptServer() : frozenServer
        guard key.range(of: "^[0-9a-f]+$", options: .regularExpression) != nil,
              var components = URLComponents(string: effectiveServer),
              components.scheme == "http" || components.scheme == "https",
              components.host != nil else { return nil }
        let basePath = components.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        components.path = "/" + [basePath, receiptMailboxPath, key]
            .filter { !$0.isEmpty }
            .joined(separator: "/")
        return components.url
    }
}

/// Pre-field records intentionally follow the current configured endpoint;
/// V2 sessions receive and retain this value from their Rust context instead.
func currentReceiptServer() -> String {
    let configured = UserDefaults.standard.string(forKey: "envoix.logServer")?.trimmed ?? ""
    if configured.isEmpty || deprecatedLogServers.contains(configured) {
        return defaultLogServer
    }
    return configured
}

struct ReceivePublicationTarget: Codable {
    let destinationPath: String
    let bookmark: Data?

    init(destinationPath: String, bookmark: Data?) {
        self.destinationPath = destinationPath
        self.bookmark = bookmark
    }

    init(ffiTarget: FfiNativePublicationTarget) {
        destinationPath = ffiTarget.destinationPath
        bookmark = ffiTarget.bookmark.isEmpty ? nil : Data(ffiTarget.bookmark)
    }

    var ffiTarget: FfiNativePublicationTarget {
        FfiNativePublicationTarget(
            destinationPath: destinationPath,
            bookmark: bookmark ?? Data()
        )
    }
}

enum ReceivePublicationStore {
    private static let key = "envoix.receivePublicationTargets.v1"

    static func loadAll() -> [String: ReceivePublicationTarget] {
        guard let data = UserDefaults.standard.data(forKey: key),
              let values = try? JSONDecoder().decode([String: ReceivePublicationTarget].self, from: data) else {
            return [:]
        }
        return values
    }

    static func save(_ target: ReceivePublicationTarget, activityID: String) {
        var values = loadAll()
        values[activityID] = target
        persist(values)
    }

    static func remove(activityID: String) {
        var values = loadAll()
        values.removeValue(forKey: activityID)
        persist(values)
    }

    private static func persist(_ values: [String: ReceivePublicationTarget]) {
        if values.isEmpty {
            UserDefaults.standard.removeObject(forKey: key)
        } else if let data = try? JSONEncoder().encode(values) {
            UserDefaults.standard.set(data, forKey: key)
        }
    }
}
