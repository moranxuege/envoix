import EnvoixCore
import XCTest
@testable import Envoix

final class MacOSAgentControlTests: XCTestCase {
    func testControlEndpointOpeningIsDeferredAndRetried() async throws {
        let responseClient = FakeMacOSAgentControlClient(
            response: .status(status: status())
        )
        let factory = SequencedAgentControlFactory(client: responseClient)
        let client = try MacOSAgentControlClient(
            controlEndpoint: URL(fileURLWithPath: "/private/tmp/envoix-agent.sock"),
            clientFactory: factory.make
        )

        XCTAssertEqual(factory.attemptCount, 0)
        do {
            _ = try await client.call(request: .status)
            XCTFail("the first deferred connection should fail")
        } catch MacOSAgentControlClientError.unavailable {
            // The next refresh must construct a fresh control client.
        }

        let response = try await client.call(request: .status)
        guard case let .status(actual) = response else {
            return XCTFail("expected typed Agent status")
        }
        XCTAssertEqual(actual.pairedDevices, 3)
        XCTAssertEqual(factory.attemptCount, 2)
    }

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
    func testExplicitEnablementRetriesWhileHelperStarts() async {
        let service = FakeAgentService(registrationState: .notRegistered)
        let client = StartingMacOSAgentControlClient(
            response: .status(status: status())
        )
        let controller = MacOSAgentServiceController(
            service: service,
            controlClient: client,
            startupRetryDelaysNanoseconds: [0]
        )

        await controller.setEnabled(true)

        let callCount = await client.callCount
        XCTAssertEqual(controller.registrationState, .enabled)
        XCTAssertEqual(controller.connectionState, .ready(pairedDevices: 3))
        XCTAssertEqual(callCount, 2)
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

    @MainActor
    func testPairingCoordinatorUsesTypedHelperRequest() async throws {
        let client = FakeMacOSAgentControlClient(response: .devicePaired(
            device: FfiAgentDeviceSummary(
                id: "dev_wsl",
                label: "WSL",
                generation: 0,
                previousGeneration: nil,
                broker: "fixture-broker",
                relay: nil
            )
        ))
        let coordinator = MacOSAgentPairingCoordinator(controlClient: client)

        let device = try await coordinator.joinPairing(
            label: "WSL",
            invitation: "123456-fixture-room",
            verificationCode: "654321"
        )

        XCTAssertEqual(device, DurablePairedDevice(id: "dev_wsl", label: "WSL"))
        let requests = await client.requests
        XCTAssertEqual(requests, [
            .joinPairing(pairing: FfiAgentPairingInput(
                label: "WSL",
                invitation: "123456-fixture-room",
                verificationCode: "654321"
            )),
        ])
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

private actor FakeMacOSAgentControlClient:
    MacOSHelperControlClient, FfiAgentControlClientProtocol
{
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

private actor StartingMacOSAgentControlClient: MacOSHelperControlClient {
    private let response: FfiAgentResponse
    private(set) var callCount = 0

    init(response: FfiAgentResponse) {
        self.response = response
    }

    func call(request: FfiAgentRequest) async throws -> FfiAgentResponse {
        callCount += 1
        if callCount == 1 {
            throw MacOSAgentControlClientError.unavailable
        }
        return response
    }
}

private final class SequencedAgentControlFactory: @unchecked Sendable {
    private let lock = NSLock()
    private let client: FfiAgentControlClientProtocol
    private var attempts = 0

    init(client: FfiAgentControlClientProtocol) {
        self.client = client
    }

    var attemptCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return attempts
    }

    func make(controlEndpoint: String) throws -> FfiAgentControlClientProtocol {
        lock.lock()
        defer { lock.unlock() }
        attempts += 1
        if attempts == 1 {
            throw MacOSAgentControlClientError.unavailable
        }
        return client
    }
}
