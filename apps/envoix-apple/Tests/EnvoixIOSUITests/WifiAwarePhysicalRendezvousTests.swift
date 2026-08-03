import EnvoixCore
import Foundation
import XCTest
@testable import Envoix_iOS

final class WifiAwarePhysicalRendezvousTests: XCTestCase {
    private static let invitationCount = 6

    func testPhysicalWifiAwareRendezvousControlPlane() async throws {
#if targetEnvironment(simulator)
        throw XCTSkip("Wi-Fi Aware rendezvous requires two physical Apple devices")
#else
        guard #available(iOS 26.4, *) else {
            throw XCTSkip("Wi-Fi Aware rendezvous requires iOS or iPadOS 26.4")
        }
        let context = try Self.requireContext()
        let networkingMode: AppleWifiAwareRendezvousNetworkingMode =
            context.role == .receiver ? .publisherOnly : .subscriberOnly
        try await Self.run(
            context,
            networkingMode: networkingMode,
            modeMarker: "directed"
        )
#endif
    }

    func testPhysicalWifiAwareSymmetricRendezvousControlPlane() async throws {
#if targetEnvironment(simulator)
        throw XCTSkip("Wi-Fi Aware rendezvous requires two physical Apple devices")
#else
        guard #available(iOS 26.4, *) else {
            throw XCTSkip("Wi-Fi Aware rendezvous requires iOS or iPadOS 26.4")
        }
        let context = try Self.requireContext()
        let roleHints = Self.controlRoleHintsSnapshot()
        defer { Self.restoreControlRoleHints(roleHints) }
        try await Self.run(
            context,
            networkingMode: .automatic,
            modeMarker: "automatic"
        )
#endif
    }

    @available(iOS 26.4, *)
    private static func run(
        _ context: WifiAwarePhysicalRendezvousContext,
        networkingMode: AppleWifiAwareRendezvousNetworkingMode,
        modeMarker: String
    ) async throws {
        marker(
            "pairing=preexisting pin_automation=not_covered " +
                "mode=\(modeMarker) run=\(context.runID)"
        )
        let provider = AppleWifiAwarePairingProvider(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: context.localPeerKey,
                displayName: "Envoix rendezvous test"
            ),
            networkingMode: networkingMode
        )
        let (events, continuation) =
            AsyncStream<NearbyDiscoveryEvent>.makeStream(
                bufferingPolicy: .bufferingNewest(32)
            )
        provider.setRendezvousOfferAdmission { offer in
            continuation.yield(.rendezvousOffer(offer))
            return true
        }
        provider.start { event in
            continuation.yield(event)
        }
        do {
            if context.role == .receiver {
                marker(
                    "role=receiver listener_state=waiting run=\(context.runID)"
                )
            }
            try await withProbeTimeout(.seconds(60)) {
                switch context.role {
                case .sender:
                    try await sendInvite(
                        context: context,
                        provider: provider,
                        events: events
                    )
                    try await receiveInvite(context: context, events: events)
                case .receiver:
                    try await receiveInvite(context: context, events: events)
                    try await sendInvite(
                        context: context,
                        provider: provider,
                        events: events
                    )
                }
            }
        } catch {
            await stop(provider, continuation: continuation)
            throw error
        }
        marker(
            "completion_marker=success test=rendezvous_control_plane " +
                "mode=\(modeMarker) role=\(context.role.rawValue) " +
                "sent=\(invitationCount) received=\(invitationCount) " +
                "run=\(context.runID)"
        )
        await stop(provider, continuation: continuation)
    }

    @available(iOS 26.0, *)
    private static func stop(
        _ provider: AppleWifiAwarePairingProvider,
        continuation: AsyncStream<NearbyDiscoveryEvent>.Continuation
    ) async {
        continuation.finish()
        provider.stop()
        await provider.waitUntilStopped()
    }

    @available(iOS 26.4, *)
    private static func sendInvite(
        context: WifiAwarePhysicalRendezvousContext,
        provider: AppleWifiAwarePairingProvider,
        events: AsyncStream<NearbyDiscoveryEvent>
    ) async throws {
        let observation = try await targetObservation(
            peerKey: context.expectedPeerKey,
            events: events
        )
        guard let deviceID = exactDeviceID(
            observation.nearbyWifiAwareDeviceID
        ) else {
            throw WifiAwarePhysicalRendezvousError.invalidTargetDeviceID
        }
        let selection = NearbyPairingSelection(
            discoveryPeerKey: observation.peerKey,
            displayName: observation.displayName,
            sources: [.wifiAware],
            nearbyWifiAwareDeviceID: deviceID
        )
        guard provider.canOfferInvite(to: selection) else {
            throw WifiAwarePhysicalRendezvousError.targetRouteUnavailable
        }
        marker(
            "local_role=\(context.role.rawValue) phase=outbound " +
                "target_observed run=\(context.runID)"
        )

        for sequence in 1...invitationCount {
            let deliveryError = await withCheckedContinuation { continuation in
                provider.offerInvite(
                    to: selection,
                    invite: context.invite
                ) { error in
                    continuation.resume(returning: error)
                }
            }
            guard deliveryError == nil else {
                throw WifiAwarePhysicalRendezvousError.inviteNotAcknowledged
            }
            marker(
                "local_role=\(context.role.rawValue) phase=outbound " +
                    "invite_acknowledged sequence=\(sequence) " +
                    "count=\(invitationCount) run=\(context.runID)"
            )
        }
    }

    private static func targetObservation(
        peerKey: String,
        events: AsyncStream<NearbyDiscoveryEvent>
    ) async throws -> NearbyDiscoveryObservation {
        for await event in events {
            try Task.checkCancellation()
            guard case let .observation(observation) = event,
                  observation.peerKey == peerKey else {
                continue
            }
            guard observation.source == .wifiAware else {
                throw WifiAwarePhysicalRendezvousError.invalidTargetSource
            }
            return observation
        }
        throw WifiAwarePhysicalRendezvousError.eventStreamEnded
    }

    private static func receiveInvite(
        context: WifiAwarePhysicalRendezvousContext,
        events: AsyncStream<NearbyDiscoveryEvent>
    ) async throws {
        var requestIDs = Set<String>()
        for await event in events {
            try Task.checkCancellation()
            guard case let .rendezvousOffer(offer) = event,
                  offer.senderPeerKey == context.expectedPeerKey else {
                continue
            }
            guard offer.source == .wifiAware,
                  exactDeviceID(offer.senderWifiAwareDeviceID) != nil,
                  offer.invite.hasPrefix(inviteV2URLPrefix),
                  let parsedInvite = try? parsePairingInvite(
                      input: offer.invite
                  ),
                  parsedInvite.creatorRole == context.role.remoteInviteRole
            else {
                throw WifiAwarePhysicalRendezvousError.invalidOffer
            }
            guard requestIDs.insert(offer.requestID).inserted else { continue }
            marker(
                "local_role=\(context.role.rawValue) phase=inbound " +
                    "rendezvous_offer_received " +
                    "sequence=\(requestIDs.count) count=\(invitationCount) " +
                    "run=\(context.runID)"
            )
            if requestIDs.count == invitationCount {
                return
            }
        }
        throw WifiAwarePhysicalRendezvousError.eventStreamEnded
    }

    private static func requireContext() throws
        -> WifiAwarePhysicalRendezvousContext {
        let environment = ProcessInfo.processInfo.environment
        guard environment[enabledEnvironment] == "1" else {
            throw XCTSkip(
                "Wi-Fi Aware rendezvous requires \(enabledEnvironment)=1"
            )
        }
        guard let role = WifiAwarePhysicalRendezvousRole(
            rawValue: environment[roleEnvironment] ?? ""
        ) else {
            throw WifiAwarePhysicalRendezvousError.invalidRole
        }
        guard let localPeerKey = strictPeerKey(
            environment[localPeerKeyEnvironment]
        ) else {
            throw WifiAwarePhysicalRendezvousError.invalidLocalPeerKey
        }
        guard let expectedPeerKey = strictPeerKey(
            environment[expectedPeerKeyEnvironment]
        ) else {
            throw WifiAwarePhysicalRendezvousError.invalidExpectedPeerKey
        }
        guard localPeerKey != expectedPeerKey else {
            throw WifiAwarePhysicalRendezvousError.duplicatePeerKeys
        }
        guard let runID = environment[runIDEnvironment],
              runID.range(
                of: #"^[A-Za-z0-9_-]{1,48}$"#,
                options: .regularExpression
              ) != nil else {
            throw WifiAwarePhysicalRendezvousError.invalidRunID
        }
        let invite = try makePairingInvite(
            role: role.localInviteRole,
            broker: defaultRendezvousBroker,
            relay: defaultRelayURL
        ).payload
        guard invite.hasPrefix(inviteV2URLPrefix),
              (try? parsePairingInvite(input: invite)) != nil,
              BleRendezvousProtocol.isSupportedInvite(invite)
        else {
            throw WifiAwarePhysicalRendezvousError.invalidGeneratedInvite
        }
        return WifiAwarePhysicalRendezvousContext(
            role: role,
            localPeerKey: localPeerKey,
            expectedPeerKey: expectedPeerKey,
            runID: runID,
            invite: invite
        )
    }

    private static func strictPeerKey(_ value: String?) -> String? {
        guard let value,
              NearbyDiscoveryPeerRegistry.normalizePeerKey(value) == value else {
            return nil
        }
        return value
    }

    private static func exactDeviceID(_ value: String?) -> String? {
        guard let value,
              value.count == 16,
              value == value.lowercased(),
              UInt64(value, radix: 16) != nil else {
            return nil
        }
        return value
    }

    private static func controlRoleHintsSnapshot() -> [String: String]? {
        UserDefaults.standard.dictionary(
            forKey: AppleWifiAwareControlRoleStore.defaultsKey
        ) as? [String: String]
    }

    private static func restoreControlRoleHints(
        _ snapshot: [String: String]?
    ) {
        if let snapshot {
            UserDefaults.standard.set(
                snapshot,
                forKey: AppleWifiAwareControlRoleStore.defaultsKey
            )
        } else {
            UserDefaults.standard.removeObject(
                forKey: AppleWifiAwareControlRoleStore.defaultsKey
            )
        }
    }

    private static func marker(_ message: String) {
        FileHandle.standardError.write(
            Data("[wifi-aware-rendezvous-physical] \(message)\n".utf8)
        )
    }

    private static let enabledEnvironment =
        "ENVOIX_WIFI_AWARE_RENDEZVOUS_PHYSICAL"
    private static let roleEnvironment =
        "ENVOIX_WIFI_AWARE_RENDEZVOUS_ROLE"
    private static let localPeerKeyEnvironment =
        "ENVOIX_WIFI_AWARE_RENDEZVOUS_LOCAL_PEER_KEY"
    private static let expectedPeerKeyEnvironment =
        "ENVOIX_WIFI_AWARE_RENDEZVOUS_EXPECTED_PEER_KEY"
    private static let runIDEnvironment =
        "ENVOIX_WIFI_AWARE_RENDEZVOUS_RUN_ID"
}

private enum WifiAwarePhysicalRendezvousRole: String {
    case sender
    case receiver

    var localInviteRole: FfiInviteRole {
        switch self {
        case .sender: .send
        case .receiver: .receive
        }
    }

    var remoteInviteRole: FfiInviteRole {
        switch self {
        case .sender: .receive
        case .receiver: .send
        }
    }
}

private struct WifiAwarePhysicalRendezvousContext: Sendable {
    let role: WifiAwarePhysicalRendezvousRole
    let localPeerKey: String
    let expectedPeerKey: String
    let runID: String
    let invite: String
}

private enum WifiAwarePhysicalRendezvousError: Error {
    case invalidRole
    case invalidLocalPeerKey
    case invalidExpectedPeerKey
    case duplicatePeerKeys
    case invalidRunID
    case invalidGeneratedInvite
    case invalidTargetSource
    case invalidTargetDeviceID
    case targetRouteUnavailable
    case inviteNotAcknowledged
    case invalidOffer
    case eventStreamEnded
}
