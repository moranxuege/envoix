import EnvoixCore
import XCTest
@testable import Envoix

final class MacOSAgentControlTests: XCTestCase {
    @MainActor
    func testDisabledServiceDoesNotContactControlEndpoint() async {
        let service = FakeAgentService(registrationState: .notRegistered)
        let client = FakeMacOSAgentControlClient(response: .status(status: status()))
        let controller = MacOSAgentServiceController(
            service: service,
            controlClient: client
        )

        await controller.refresh()

        let callCount = await client.callCount
        XCTAssertEqual(controller.registrationState, .notRegistered)
        XCTAssertEqual(controller.connectionState, .idle)
        XCTAssertEqual(callCount, 0)
    }

    @MainActor
    func testExplicitEnablementRegistersAndUsesTypedStatus() async {
        let service = FakeAgentService(registrationState: .notRegistered)
        let client = FakeMacOSAgentControlClient(response: .status(status: status()))
        let controller = MacOSAgentServiceController(
            service: service,
            controlClient: client
        )

        await controller.setEnabled(true)

        let requestsAfterEnable = await client.requests
        XCTAssertEqual(service.registerCallCount, 1)
        XCTAssertEqual(controller.registrationState, .enabled)
        XCTAssertEqual(controller.connectionState, .ready(pairedDevices: 3))
        XCTAssertEqual(requestsAfterEnable, [.status])

        await controller.setEnabled(false)

        let requestsAfterDisable = await client.requests
        XCTAssertEqual(service.unregisterCallCount, 1)
        XCTAssertEqual(controller.registrationState, .notRegistered)
        XCTAssertEqual(controller.connectionState, .idle)
        XCTAssertEqual(requestsAfterDisable, [.status])
    }

    @MainActor
    func testApprovalAndCompatibilityFailuresRemainFailClosed() async {
        let approvalService = FakeAgentService(registrationState: .requiresApproval)
        let approvalClient = FakeMacOSAgentControlClient(response: .status(status: status()))
        let approvalController = MacOSAgentServiceController(
            service: approvalService,
            controlClient: approvalClient
        )

        await approvalController.refresh()

        let approvalCallCount = await approvalClient.callCount
        XCTAssertEqual(approvalController.registrationState, .requiresApproval)
        XCTAssertEqual(approvalController.connectionState, .idle)
        XCTAssertEqual(approvalCallCount, 0)

        let enabledService = FakeAgentService(registrationState: .enabled)
        let incompatibleClient = FakeMacOSAgentControlClient(
            error: FfiAgentControlError.Failed(
                code: .incompatibleProtocol,
                reason: "fixture"
            )
        )
        let incompatibleController = MacOSAgentServiceController(
            service: enabledService,
            controlClient: incompatibleClient
        )

        await incompatibleController.refresh()

        let incompatibleCallCount = await incompatibleClient.callCount
        XCTAssertEqual(incompatibleController.connectionState, .incompatible)
        XCTAssertEqual(incompatibleCallCount, 1)
    }

    @MainActor
    func testUnexpectedTypedResponseIsRejected() async {
        let service = FakeAgentService(registrationState: .enabled)
        let client = FakeMacOSAgentControlClient(response: .devices(devices: []))
        let controller = MacOSAgentServiceController(
            service: service,
            controlClient: client
        )

        await controller.refresh()

        XCTAssertEqual(controller.connectionState, .incompatible)
    }

    private func status() -> FfiAgentStatus {
        FfiAgentStatus(
            protocolVersion: expectedAgentProtocolVersion,
            pid: 42,
            deviceName: "Mac",
            stateDirectory: "/private/tmp/state",
            inboxDirectory: "/private/tmp/inbox",
            broker: defaultRendezvousBroker,
            relay: defaultRelayURL,
            pairedDevices: 3,
            activeReceivers: 0,
            activePairings: 0,
            activePaths: 0,
            pendingOffers: 0
        )
    }
}

@MainActor
private final class FakeAgentService: MacOSAgentServiceRegistering {
    var registrationState: MacOSAgentRegistrationState
    private(set) var registerCallCount = 0
    private(set) var unregisterCallCount = 0

    init(registrationState: MacOSAgentRegistrationState) {
        self.registrationState = registrationState
    }

    func register() throws {
        registerCallCount += 1
        registrationState = .enabled
    }

    func unregister() throws {
        unregisterCallCount += 1
        registrationState = .notRegistered
    }
}

private actor FakeMacOSAgentControlClient: MacOSHelperControlClient {
    private let response: FfiAgentResponse?
    private let error: Error?
    private(set) var requests: [FfiAgentRequest] = []

    init(response: FfiAgentResponse) {
        self.response = response
        error = nil
    }

    init(error: Error) {
        response = nil
        self.error = error
    }

    var callCount: Int {
        requests.count
    }

    func call(request: FfiAgentRequest) async throws -> FfiAgentResponse {
        requests.append(request)
        if let error {
            throw error
        }
        return try XCTUnwrap(response)
    }
}
