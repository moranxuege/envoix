import Foundation
import CryptoKit

/// Secret-free BLE locator. The six-digit PAKE input stays on the two screens.
struct BleVerificationInvitation: Equatable, Identifiable {
    static let urlPrefix = "envoix://ble/v1/"
    private static let lifetime: TimeInterval = 300

    let verificationCode: String
    let privateInvitation: String
    let publicOffer: String
    let expiresAt: Date
    var id: String { publicOffer }

    static func make(broker: String, relay: String, now: Date = Date()) throws -> Self {
        let configuredBroker = broker.trimmed
        let broker = configuredBroker.isEmpty ? defaultRendezvousBroker : configuredBroker
        let configuredRelay = relay.trimmed
        let relay = configuredRelay.isEmpty ? defaultRelayURL : configuredRelay
        let code = digits()
        var locator = digits()
        while locator == code { locator = digits() }
        let expiry = now.addingTimeInterval(lifetime)
        let offer = url("ble", ["v1", locator], broker, relay, expiry)
        return Self(
            verificationCode: code,
            privateInvitation: privateURL(locator, code, offer, broker, relay, expiry),
            publicOffer: offer,
            expiresAt: expiry
        )
    }

    static func resolve(
        publicOffer: String,
        verificationCode: String,
        now: Date = Date()
    ) -> String? {
        guard isDigits(verificationCode), let offer = parse(publicOffer, now: now) else { return nil }
        return privateURL(
            offer.locator,
            verificationCode,
            publicOffer,
            offer.broker,
            offer.relay,
            offer.expiry
        )
    }

    static func isPublicOffer(_ value: String, now: Date = Date()) -> Bool {
        parse(value, now: now) != nil
    }

    private static func privateURL(
        _ locator: String,
        _ code: String,
        _ publicOffer: String,
        _ broker: String,
        _ relay: String,
        _ expiry: Date
    ) -> String {
        let material = Data("envoix BLE verification v1\0\(publicOffer)\0\(code)".utf8)
        let secret = SHA256.hash(data: material).prefix(4)
            .map { String(format: "%02x", $0) }
            .joined()
        return url(
            "room",
            ["\(locator)-\(secret.prefix(4))-\(secret.suffix(4))"],
            broker,
            relay,
            expiry
        )
    }

    private static func url(
        _ host: String,
        _ path: [String],
        _ broker: String,
        _ relay: String,
        _ expiry: Date
    ) -> String {
        var value = URLComponents()
        value.scheme = "envoix"
        value.host = host
        value.path = "/" + path.joined(separator: "/")
        value.queryItems = [URLQueryItem(name: "broker", value: broker)]
        if !relay.isEmpty { value.queryItems?.append(URLQueryItem(name: "relay", value: relay)) }
        value.queryItems?.append(URLQueryItem(
            name: "expires",
            value: String(Int64(expiry.timeIntervalSince1970))
        ))
        return value.string!
    }

    private static func parse(
        _ input: String,
        now: Date
    ) -> (locator: String, broker: String, relay: String, expiry: Date)? {
        guard input == input.trimmed, input.utf8.count <= 2_048,
              let value = URLComponents(string: input),
              value.scheme == "envoix", value.host == "ble", value.fragment == nil,
              value.user == nil, value.password == nil, value.port == nil,
              let rawItems = value.percentEncodedQuery?.split(
                  separator: "&",
                  omittingEmptySubsequences: false
              ), rawItems.allSatisfy({ $0.contains("=") }) else { return nil }
        let path = value.path.split(separator: "/")
        let items = value.queryItems ?? []
        guard path.count == 2, path[0] == "v1", isDigits(String(path[1])),
              items.allSatisfy({ ["broker", "relay", "expires"].contains($0.name) }),
              let broker = one("broker", items)?.trimmed, !broker.isEmpty,
              let seconds = one("expires", items).flatMap(Int64.init),
              items.filter({ $0.name == "relay" }).count <= 1 else { return nil }
        let expiry = Date(timeIntervalSince1970: TimeInterval(seconds))
        let relay = one("relay", items)?.trimmed ?? ""
        guard broker.utf8.count <= 1_024, relay.utf8.count <= 1_024,
              expiry > now, expiry.timeIntervalSince(now) <= lifetime * 2 else { return nil }
        return (String(path[1]), broker, relay, expiry)
    }

    private static func one(_ name: String, _ items: [URLQueryItem]) -> String? {
        let matches = items.filter { $0.name == name }
        return matches.count == 1 ? matches[0].value : nil
    }

    private static func digits() -> String { String(format: "%06d", Int.random(in: 0..<1_000_000)) }
    private static func isDigits(_ value: String) -> Bool {
        value.utf8.count == 6 && value.utf8.allSatisfy { (48...57).contains($0) }
    }
}
