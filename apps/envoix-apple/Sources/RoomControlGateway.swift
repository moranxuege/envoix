import Foundation
import EnvoixCore

struct RoomControlInvitation: Equatable {
    let code: String
    let payload: String
    let expiresAt: Date
}

struct RoomControlTransferOffer: Equatable, Identifiable {
    let id: String
    let transferInvite: String
    let rootNames: [String]
    let itemCount: UInt32
    let totalBytes: UInt64
}

enum RoomControlLifetimePolicy: Equatable {
    case idleFifteenMinutes
    case untilForegroundEnds
}

struct RoomControlLifetimeState: Equatable {
    let revision: UInt64
    let policy: RoomControlLifetimePolicy
    let idleDeadline: Date?
}

enum RoomControlCloseReason: Equatable {
    case userEnded
    case idleExpired
    case invitationExpired
    case peerEnded
    case backgrounded
    case networkLost
    case protocolFailure
}

enum RoomControlEvent: Equatable {
    case connected(
        peerDisplayName: String,
        creator: Bool,
        lifetime: RoomControlLifetimeState
    )
    case incomingOffer(RoomControlTransferOffer)
    case offerAccepted(id: String)
    case offerRejected(id: String)
    case lifetimeChanged(RoomControlLifetimeState)
    case closed(RoomControlCloseReason)
}

/// Native boundary for the shared room-control implementation. A room may be
/// rendered as connected only after the gateway emits `.connected`.
@MainActor
protocol RoomControlGateway: AnyObject {
    func makeInvitation(broker: String, relay: String, now: Date) throws -> RoomControlInvitation
    func parseInvitation(
        _ input: String,
        broker: String,
        relay: String,
        now: Date
    ) throws -> RoomControlInvitation
    func host(
        invitation: RoomControlInvitation,
        displayName: String,
        identityPath: String,
        onEvent: @escaping (RoomControlEvent) -> Void
    ) async throws
    func join(
        invitation: RoomControlInvitation,
        displayName: String,
        identityPath: String,
        onEvent: @escaping (RoomControlEvent) -> Void
    ) async throws
    func offerTransfer(_ offer: RoomControlTransferOffer) async throws -> RoomControlLifetimeState?
    func acceptOffer(id: String) async throws -> RoomControlLifetimeState?
    func rejectOffer(id: String) async throws -> RoomControlLifetimeState?
    func setLifetimePolicy(
        _ policy: RoomControlLifetimePolicy
    ) async throws -> RoomControlLifetimeState?
    func setLocalTransferActive(_ active: Bool) async throws -> RoomControlLifetimeState?
    func lifetimeSnapshot() -> RoomControlLifetimeState?
    func expireIdleDeadline() async throws
    func close(reason: RoomControlCloseReason)
}

struct RoomControlUnavailableError: LocalizedError {
    var errorDescription: String? {
        "Room control is unavailable in this build."
    }
}

@MainActor
final class UnavailableRoomControlGateway: RoomControlGateway {
    func makeInvitation(broker: String, relay: String, now: Date) throws -> RoomControlInvitation {
        throw RoomControlUnavailableError()
    }

    func parseInvitation(
        _ input: String,
        broker: String,
        relay: String,
        now: Date
    ) throws -> RoomControlInvitation {
        throw RoomControlUnavailableError()
    }

    func host(
        invitation: RoomControlInvitation,
        displayName: String,
        identityPath: String,
        onEvent: @escaping (RoomControlEvent) -> Void
    ) async throws {
        throw RoomControlUnavailableError()
    }

    func join(
        invitation: RoomControlInvitation,
        displayName: String,
        identityPath: String,
        onEvent: @escaping (RoomControlEvent) -> Void
    ) async throws {
        throw RoomControlUnavailableError()
    }

    func offerTransfer(
        _ offer: RoomControlTransferOffer
    ) async throws -> RoomControlLifetimeState? {
        throw RoomControlUnavailableError()
    }

    func acceptOffer(id: String) async throws -> RoomControlLifetimeState? {
        throw RoomControlUnavailableError()
    }

    func rejectOffer(id: String) async throws -> RoomControlLifetimeState? {
        throw RoomControlUnavailableError()
    }

    func setLifetimePolicy(
        _ policy: RoomControlLifetimePolicy
    ) async throws -> RoomControlLifetimeState? {
        throw RoomControlUnavailableError()
    }

    func setLocalTransferActive(_ active: Bool) async throws -> RoomControlLifetimeState? {
        throw RoomControlUnavailableError()
    }

    func lifetimeSnapshot() -> RoomControlLifetimeState? { nil }

    func expireIdleDeadline() async throws {
        throw RoomControlUnavailableError()
    }

    func close(reason: RoomControlCloseReason) {}
}

@MainActor
final class LiveRoomControlGateway: RoomControlGateway {
    private var session: FfiRoomControlSession?
    private var cancellation: FfiRoomControlCancellation?

    func makeInvitation(broker: String, relay: String, now: Date) throws -> RoomControlInvitation {
        project(try makeRoomControlInvite(broker: broker, relay: relay))
    }

    func parseInvitation(
        _ input: String,
        broker: String,
        relay: String,
        now: Date
    ) throws -> RoomControlInvitation {
        let invitation = project(try parseRoomControlInvite(
            input: input,
            fallbackBroker: broker,
            fallbackRelay: relay
        ))
        guard invitation.expiresAt > now else {
            throw RuntimeSettingsError("This room invitation has expired.")
        }
        return invitation
    }

    func host(
        invitation: RoomControlInvitation,
        displayName: String,
        identityPath: String,
        onEvent: @escaping (RoomControlEvent) -> Void
    ) async throws {
        try await connect(
            invitation: invitation,
            displayName: displayName,
            mode: .host,
            identityPath: identityPath,
            onEvent: onEvent
        )
    }

    func join(
        invitation: RoomControlInvitation,
        displayName: String,
        identityPath: String,
        onEvent: @escaping (RoomControlEvent) -> Void
    ) async throws {
        try await connect(
            invitation: invitation,
            displayName: displayName,
            mode: .join,
            identityPath: identityPath,
            onEvent: onEvent
        )
    }

    func offerTransfer(
        _ offer: RoomControlTransferOffer
    ) async throws -> RoomControlLifetimeState? {
        guard let session else { throw RoomControlUnavailableError() }
        return try await session.offerTransfer(offer: FfiRoomTransferOffer(
            offerId: offer.id,
            transferInvite: offer.transferInvite,
            rootNames: Array(offer.rootNames.prefix(3)),
            itemCount: offer.itemCount,
            totalBytes: offer.totalBytes
        )).map(project)
    }

    func acceptOffer(id: String) async throws -> RoomControlLifetimeState? {
        guard let session else { throw RoomControlUnavailableError() }
        return try await session.acceptOffer(offerId: id).map(project)
    }

    func rejectOffer(id: String) async throws -> RoomControlLifetimeState? {
        guard let session else { throw RoomControlUnavailableError() }
        return try await session.rejectOffer(
            offerId: id,
            reason: .declined
        ).map(project)
    }

    func setLifetimePolicy(
        _ policy: RoomControlLifetimePolicy
    ) async throws -> RoomControlLifetimeState? {
        guard let session else { throw RoomControlUnavailableError() }
        return try await session.setPolicy(policy: ffiPolicy(policy)).map(project)
    }

    func setLocalTransferActive(_ active: Bool) async throws -> RoomControlLifetimeState? {
        guard let session else { throw RoomControlUnavailableError() }
        return try await session.setLocalTransferActive(active: active).map(project)
    }

    func lifetimeSnapshot() -> RoomControlLifetimeState? {
        session.map { project($0.snapshot().lifetime) }
    }

    func expireIdleDeadline() async throws {
        guard let activeSession = session else { throw RoomControlUnavailableError() }
        let activeCancellation = cancellation
        try await activeSession.close(reason: .idleExpired)
        activeCancellation?.cancel()
        if let activeCancellation {
            clearIfCurrent(activeCancellation)
        } else if session === activeSession {
            session = nil
        }
    }

    func close(reason: RoomControlCloseReason) {
        let activeCancellation = cancellation
        let activeSession = session
        activeCancellation?.cancel()
        if let activeCancellation {
            clearIfCurrent(activeCancellation)
        } else if activeSession != nil {
            session = nil
        }
        let ffiReason = ffiCloseReason(reason)
        Task {
            try? await activeSession?.close(reason: ffiReason)
        }
    }

    private func connect(
        invitation: RoomControlInvitation,
        displayName: String,
        mode: FfiRoomConnectMode,
        identityPath: String,
        onEvent: @escaping (RoomControlEvent) -> Void
    ) async throws {
        cancellation?.cancel()
        let token = FfiRoomControlCancellation()
        cancellation = token
        do {
            let connectedSession = try await connectRoomControlSession(
                input: invitation.payload,
                displayName: displayName,
                mode: mode,
                identityPath: identityPath,
                fallbackBroker: "",
                fallbackRelay: "",
                cancellation: token
            )
            guard cancellation === token, !Task.isCancelled else {
                token.cancel()
                try? await connectedSession.close(reason: .userEnded)
                clearIfCurrent(token)
                return
            }
            session = connectedSession
            let snapshot = connectedSession.snapshot()
            onEvent(.connected(
                peerDisplayName: snapshot.peerName,
                creator: snapshot.creator,
                lifetime: project(snapshot.lifetime)
            ))

            while cancellation === token, !Task.isCancelled {
                let event = try await connectedSession.nextEvent()
                if let projected = project(event) {
                    onEvent(projected)
                    if case .closed = projected {
                        clearIfCurrent(token)
                        return
                    }
                }
            }
            token.cancel()
            try? await connectedSession.close(reason: .userEnded)
            clearIfCurrent(token)
        } catch {
            token.cancel()
            clearIfCurrent(token)
            throw error
        }
    }

    private func clearIfCurrent(_ token: FfiRoomControlCancellation) {
        guard cancellation === token else { return }
        session = nil
        cancellation = nil
    }

    private func project(_ invitation: FfiRoomControlInvite) -> RoomControlInvitation {
        RoomControlInvitation(
            code: invitation.code,
            payload: invitation.payload,
            expiresAt: Date(
                timeIntervalSince1970: TimeInterval(invitation.expiresAtEpochMs) / 1_000
            )
        )
    }

    private func project(_ event: FfiRoomControlEvent) -> RoomControlEvent? {
        switch event.kind {
        case .incomingOffer:
            guard let offer = event.offer else { return nil }
            return .incomingOffer(RoomControlTransferOffer(
                id: offer.offerId,
                transferInvite: offer.transferInvite,
                rootNames: Array(offer.rootNames.prefix(3)),
                itemCount: offer.itemCount,
                totalBytes: offer.totalBytes
            ))
        case .offerAccepted:
            return .offerAccepted(id: event.offerId)
        case .offerRejected:
            return .offerRejected(id: event.offerId)
        case .lifetimeChanged:
            guard let lifetime = event.lifetime else { return nil }
            return .lifetimeChanged(project(lifetime))
        case .peerClosed:
            let reason = project(event.closeReason ?? .protocolFailure)
            return .closed(reason == .userEnded ? .peerEnded : reason)
        case .pong:
            return nil
        }
    }

    private func ffiPolicy(_ policy: RoomControlLifetimePolicy) -> FfiRoomLifetimePolicy {
        switch policy {
        case .idleFifteenMinutes: return .idle15Minutes
        case .untilForegroundEnds: return .untilForegroundEnds
        }
    }

    private func project(_ policy: FfiRoomLifetimePolicy) -> RoomControlLifetimePolicy {
        switch policy {
        case .idle15Minutes: return .idleFifteenMinutes
        case .untilForegroundEnds: return .untilForegroundEnds
        }
    }

    private func project(_ state: FfiRoomLifetimeState) -> RoomControlLifetimeState {
        RoomControlLifetimeState(
            revision: state.revision,
            policy: project(state.policy),
            idleDeadline: state.idleDeadlineEpochMs.map {
                Date(timeIntervalSince1970: TimeInterval($0) / 1_000)
            }
        )
    }

    private func ffiCloseReason(_ reason: RoomControlCloseReason) -> FfiRoomCloseReason {
        switch reason {
        case .userEnded: return .userEnded
        case .idleExpired: return .idleExpired
        case .invitationExpired: return .invitationExpired
        case .peerEnded: return .peerEnded
        case .backgrounded: return .backgrounded
        case .networkLost: return .networkLost
        case .protocolFailure: return .protocolFailure
        }
    }

    private func project(_ reason: FfiRoomCloseReason) -> RoomControlCloseReason {
        switch reason {
        case .userEnded: return .userEnded
        case .idleExpired: return .idleExpired
        case .invitationExpired: return .invitationExpired
        case .peerEnded: return .peerEnded
        case .backgrounded: return .backgrounded
        case .networkLost: return .networkLost
        case .protocolFailure: return .protocolFailure
        }
    }
}

@MainActor
enum RoomControlGatewayFactory {
    static func make() -> RoomControlGateway {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--ui-testing-discovery-fixtures") {
            return FixtureRoomControlGateway()
        }
        #endif
        return LiveRoomControlGateway()
    }
}

#if DEBUG
@MainActor
private final class FixtureRoomControlGateway: RoomControlGateway {
    private var onEvent: ((RoomControlEvent) -> Void)?
    private var lifetime = RoomControlLifetimeState(
        revision: 0,
        policy: .idleFifteenMinutes,
        idleDeadline: Date().addingTimeInterval(15 * 60)
    )

    func makeInvitation(broker: String, relay: String, now: Date) throws -> RoomControlInvitation {
        RoomControlInvitation(
            code: "R123456-test-room",
            payload: "envoix://room/R123456-test-room",
            expiresAt: now.addingTimeInterval(5 * 60)
        )
    }

    func parseInvitation(
        _ input: String,
        broker: String,
        relay: String,
        now: Date
    ) throws -> RoomControlInvitation {
        try makeInvitation(broker: "", relay: "", now: now)
    }

    func host(
        invitation: RoomControlInvitation,
        displayName: String,
        identityPath: String,
        onEvent: @escaping (RoomControlEvent) -> Void
    ) async throws {
        self.onEvent = onEvent
        onEvent(.connected(
            peerDisplayName: "Nearby test device",
            creator: true,
            lifetime: lifetime
        ))
    }

    func join(
        invitation: RoomControlInvitation,
        displayName: String,
        identityPath: String,
        onEvent: @escaping (RoomControlEvent) -> Void
    ) async throws {
        self.onEvent = onEvent
        onEvent(.connected(
            peerDisplayName: "Nearby test device",
            creator: false,
            lifetime: lifetime
        ))
    }

    func offerTransfer(
        _ offer: RoomControlTransferOffer
    ) async throws -> RoomControlLifetimeState? {
        onEvent?(.offerAccepted(id: offer.id))
        return nil
    }

    func acceptOffer(id: String) async throws -> RoomControlLifetimeState? { nil }
    func rejectOffer(id: String) async throws -> RoomControlLifetimeState? { nil }
    func setLifetimePolicy(
        _ policy: RoomControlLifetimePolicy
    ) async throws -> RoomControlLifetimeState? {
        lifetime = RoomControlLifetimeState(
            revision: lifetime.revision + 1,
            policy: policy,
            idleDeadline: policy == .idleFifteenMinutes
                ? Date().addingTimeInterval(15 * 60)
                : nil
        )
        return lifetime
    }
    func setLocalTransferActive(_ active: Bool) async throws -> RoomControlLifetimeState? {
        lifetime = RoomControlLifetimeState(
            revision: lifetime.revision + 1,
            policy: lifetime.policy,
            idleDeadline: active ? nil : Date().addingTimeInterval(15 * 60)
        )
        return lifetime
    }
    func lifetimeSnapshot() -> RoomControlLifetimeState? { lifetime }
    func expireIdleDeadline() async throws {}
    func close(reason: RoomControlCloseReason) {}
}
#endif
