import XCTest
#if os(macOS)
@testable import Envoix
#else
@testable import Envoix_iOS
#endif

final class AppleWifiAwareControlRoleStoreTests: XCTestCase {
    func testCanonicalRolesAreComplementary() {
        let lower = "0011223344556677"
        let higher = "8899aabbccddeeff"

        XCTAssertEqual(
            AppleWifiAwareControlRoleStore.canonicalRole(
                localPeerKey: lower,
                remotePeerKey: higher
            ),
            .subscriber
        )
        XCTAssertEqual(
            AppleWifiAwareControlRoleStore.canonicalRole(
                localPeerKey: higher,
                remotePeerKey: lower
            ),
            .publisher
        )
    }

    func testRoleRoundTripAndRemovedDevicePruning() {
        let suite = "AppleWifiAwareControlRoleStoreTests.\(UUID().uuidString)"
        guard let defaults = UserDefaults(suiteName: suite) else {
            return XCTFail("Could not create isolated defaults")
        }
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = AppleWifiAwareControlRoleStore(
            defaults: defaults,
            defaultsKey: "roles"
        )

        store.set(.publisher, for: 1)
        store.set(.subscriber, for: 2)
        store.setIfAbsent(.publisher, for: 2)
        XCTAssertEqual(
            store.setCanonicalRoleIfAbsent(
                localPeerKey: "0011223344556677",
                remotePeerKey: "8899aabbccddeeff",
                for: 2
            ),
            .subscriber
        )
        XCTAssertEqual(store.role(for: 1), .publisher)
        XCTAssertEqual(store.role(for: 2), .subscriber)

        store.retain(deviceIDs: [2])
        XCTAssertNil(store.role(for: 1))
        XCTAssertEqual(store.role(for: 2), .subscriber)
    }

    func testExplicitSubscriberSelectionOverridesEarlyPublisherSnapshot() {
        let suite = "AppleWifiAwareControlRoleStoreTests.\(UUID().uuidString)"
        guard let defaults = UserDefaults(suiteName: suite) else {
            return XCTFail("Could not create isolated defaults")
        }
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = AppleWifiAwareControlRoleStore(
            defaults: defaults,
            defaultsKey: "roles"
        )

        store.setIfAbsent(.publisher, for: 7)
        store.set(.subscriber, for: 7)

        XCTAssertEqual(store.role(for: 7), .subscriber)
    }

    func testPersistedPublisherCollisionRecoversToComplementaryRoles() {
        assertSameRoleRecoveryConverges(initialRole: .publisher)
    }

    func testPersistedSubscriberCollisionRecoversToComplementaryRoles() {
        assertSameRoleRecoveryConverges(initialRole: .subscriber)
    }

    func testWrongDirectionHelloNarrowsBootstrapToCanonicalRole() {
        var state = AppleWifiAwareControlRoleState(persistedRole: nil)

        XCTAssertEqual(
            state.authenticate(
                localPeerKey: "0011223344556677",
                remotePeerKey: "8899aabbccddeeff",
                direction: .inboundPublisher
            ),
            .roleMismatch(role: .subscriber, roleChanged: true)
        )
        XCTAssertTrue(state.startsBrowser)
        XCTAssertFalse(state.startsListener)
    }

    func testReadyChannelPreventsRecoveryAndCloseAllowsIt() {
        var state = AppleWifiAwareControlRoleState(persistedRole: .publisher)
        XCTAssertEqual(
            state.authenticate(
                localPeerKey: "8899aabbccddeeff",
                remotePeerKey: "0011223344556677",
                direction: .inboundPublisher
            ),
            .accepted(role: .publisher, roleChanged: false)
        )
        XCTAssertFalse(state.beginRecovery(hasReadyChannel: true))

        state.channelClosed(hasReadyChannel: false)
        XCTAssertTrue(state.beginRecovery(hasReadyChannel: false))
        XCTAssertTrue(state.startsBrowser)
        XCTAssertTrue(state.startsListener)
    }

    func testInvalidOrSelfIdentityCannotChooseARole() {
        XCTAssertNil(AppleWifiAwareControlRoleStore.canonicalRole(
            localPeerKey: "invalid",
            remotePeerKey: "8899aabbccddeeff"
        ))
        XCTAssertNil(AppleWifiAwareControlRoleStore.canonicalRole(
            localPeerKey: "0011223344556677",
            remotePeerKey: "0011223344556677"
        ))
    }

    func testEndpointAttemptBecomesIdleWhenEndpointDisappears() throws {
        var state = WifiAwareEndpointAttemptState(endpoint: "endpoint-a")
        let attempt = try XCTUnwrap(state.beginAttempt())

        state.updateEndpoint(nil)

        XCTAssertTrue(state.finishAttempt(attempt))
        XCTAssertNil(state.currentEndpoint)
        XCTAssertFalse(state.hasActiveAttempt)
        XCTAssertNil(state.beginAttempt())
    }

    func testEndpointAttemptUsesReplacementEndpointAfterFinishing() throws {
        var state = WifiAwareEndpointAttemptState(endpoint: "endpoint-a")
        let attempt = try XCTUnwrap(state.beginAttempt())

        state.updateEndpoint("endpoint-b")

        XCTAssertTrue(state.finishAttempt(attempt))
        XCTAssertEqual(state.currentEndpoint, "endpoint-b")
        XCTAssertFalse(state.hasActiveAttempt)
    }

    func testEndpointAttemptUsesEndpointThatReappearsBeforeFinishing() throws {
        var state = WifiAwareEndpointAttemptState(endpoint: "endpoint-a")
        let attempt = try XCTUnwrap(state.beginAttempt())

        state.updateEndpoint(nil)
        state.updateEndpoint("endpoint-b")

        XCTAssertTrue(state.finishAttempt(attempt))
        XCTAssertEqual(state.currentEndpoint, "endpoint-b")
        XCTAssertFalse(state.hasActiveAttempt)
    }

    func testStaleEndpointAttemptCannotClearReplacementAttempt() throws {
        var state = WifiAwareEndpointAttemptState(endpoint: "endpoint-a")
        let staleAttempt = try XCTUnwrap(state.beginAttempt())
        state.cancelAttempt()
        let replacementAttempt = try XCTUnwrap(state.beginAttempt())

        XCTAssertNotEqual(staleAttempt, replacementAttempt)
        XCTAssertFalse(state.finishAttempt(staleAttempt))
        XCTAssertTrue(state.hasActiveAttempt)
        XCTAssertTrue(state.finishAttempt(replacementAttempt))
        XCTAssertFalse(state.hasActiveAttempt)
    }

    func testAdmissionResetIsolatedFromLateReleaseOfOldTokens() {
        var admission = WifiAwareInboundConnectionAdmission()
        let maximum = WifiAwareInboundConnectionAdmission
            .maximumConcurrentConnections
        let staleTokens = (0..<maximum).compactMap { _ in
            admission.acquire()
        }
        XCTAssertEqual(staleTokens.count, maximum)

        admission.reset()
        XCTAssertEqual(admission.activeConnectionCount, 0)

        let replacementTokens = (0..<maximum).compactMap { _ in
            admission.acquire()
        }
        XCTAssertEqual(replacementTokens.count, maximum)
        XCTAssertNil(admission.acquire())

        for token in staleTokens {
            admission.release(token)
        }
        XCTAssertEqual(admission.activeConnectionCount, maximum)
        XCTAssertNil(admission.acquire())

        for token in replacementTokens {
            admission.release(token)
        }
        XCTAssertEqual(admission.activeConnectionCount, 0)
    }

    private func assertSameRoleRecoveryConverges(
        initialRole: AppleWifiAwareControlRole,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        var lower = AppleWifiAwareControlRoleState(
            persistedRole: initialRole
        )
        var higher = AppleWifiAwareControlRoleState(
            persistedRole: initialRole
        )

        XCTAssertTrue(lower.beginRecovery(hasReadyChannel: false))
        XCTAssertTrue(higher.beginRecovery(hasReadyChannel: false))
        XCTAssertTrue(lower.startsBrowser && lower.startsListener)
        XCTAssertTrue(higher.startsBrowser && higher.startsListener)

        XCTAssertEqual(
            lower.authenticate(
                localPeerKey: "0011223344556677",
                remotePeerKey: "8899aabbccddeeff",
                direction: .outboundSubscriber
            ),
            .accepted(role: .subscriber, roleChanged: true),
            file: file,
            line: line
        )
        XCTAssertEqual(
            higher.authenticate(
                localPeerKey: "8899aabbccddeeff",
                remotePeerKey: "0011223344556677",
                direction: .inboundPublisher
            ),
            .accepted(role: .publisher, roleChanged: true),
            file: file,
            line: line
        )
        XCTAssertTrue(lower.startsBrowser)
        XCTAssertFalse(lower.startsListener)
        XCTAssertFalse(higher.startsBrowser)
        XCTAssertTrue(higher.startsListener)
    }
}
