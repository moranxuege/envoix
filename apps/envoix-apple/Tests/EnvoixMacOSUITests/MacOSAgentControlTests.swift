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
            clientFactory: { endpoint in
                try factory.make(controlEndpoint: endpoint)
            }
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
    func testRegistrationFailureDoesNotContactHelper() async {
        let service = FakeAgentService(
            registrationState: .notRegistered,
            registerError: FakeAgentServiceError.registration
        )
        let client = FakeMacOSAgentControlClient(response: .status(status: status()))
        let controller = MacOSAgentServiceController(
            service: service,
            controlClient: client
        )

        await controller.setEnabled(true)

        let callCount = await client.callCount
        XCTAssertEqual(service.registerCallCount, 1)
        XCTAssertEqual(controller.registrationState, .failed)
        XCTAssertEqual(controller.connectionState, .unavailable(nil))
        XCTAssertEqual(callCount, 0)
    }

    @MainActor
    func testUnregistrationFailureDoesNotContactHelper() async {
        let service = FakeAgentService(
            registrationState: .enabled,
            unregisterError: FakeAgentServiceError.unregistration
        )
        let client = FakeMacOSAgentControlClient(response: .status(status: status()))
        let controller = MacOSAgentServiceController(
            service: service,
            controlClient: client
        )

        await controller.setEnabled(false)

        let callCount = await client.callCount
        XCTAssertEqual(service.unregisterCallCount, 1)
        XCTAssertEqual(controller.registrationState, .failed)
        XCTAssertEqual(controller.connectionState, .unavailable(nil))
        XCTAssertEqual(callCount, 0)
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

    @MainActor
    func testTransferControllerLoadsSortedHelperDevices() async {
        let client = FakeMacOSAgentControlClient(response: .devices(devices: [
            device(id: "dev_wsl", label: "WSL"),
            device(id: "dev_alpha", label: "Alpha"),
        ]))
        let controller = MacOSAgentTransferController(controlClient: client)

        await controller.refreshDevices()

        XCTAssertEqual(controller.devices, [
            MacOSAgentDevice(id: "dev_alpha", label: "Alpha"),
            MacOSAgentDevice(id: "dev_wsl", label: "WSL"),
        ])
        XCTAssertNil(controller.loadError)
        let requests = await client.requests
        XCTAssertEqual(requests, [.listDevices])
    }

    @MainActor
    func testTransferControllerCreatesTransferThroughHelper() async throws {
        let source = FileManager.default.temporaryDirectory
            .appendingPathComponent("envoix-agent-ui-\(UUID().uuidString).txt")
        try Data("helper transfer".utf8).write(to: source, options: .atomic)
        defer { try? FileManager.default.removeItem(at: source) }
        let transfer = FfiApplicationTransfer(
            id: "transfer_fixture",
            relationshipId: "dev_wsl",
            roomId: nil,
            contentId: "content_fixture",
            direction: .send,
            state: .queued,
            transferredBytes: 0,
            totalBytes: 15,
            failure: nil,
            rejection: nil
        )
        let client = FakeMacOSAgentControlClient(
            response: .transferCreated(transfer: transfer)
        )
        let controller = MacOSAgentTransferController(controlClient: client)

        let transferID = try await controller.createTransfer(
            deviceID: "dev_wsl",
            urls: [source]
        )

        XCTAssertEqual(transferID, "transfer_fixture")
        XCTAssertFalse(controller.isPreparing(deviceID: "dev_wsl"))
        XCTAssertEqual(controller.transfers, [transfer])
        XCTAssertTrue(controller.hasPendingTransfers)
        let requests = await client.requests
        XCTAssertEqual(requests, [
            .createTransfer(device: "dev_wsl", paths: [source.standardizedFileURL.path]),
        ])
    }

    @MainActor
    func testTransferControllerRejectsEmptySelectionBeforeCallingHelper() async {
        let client = FakeMacOSAgentControlClient(response: .devices(devices: []))
        let controller = MacOSAgentTransferController(controlClient: client)

        do {
            _ = try await controller.createTransfer(deviceID: "dev_wsl", urls: [])
            XCTFail("an empty transfer selection must fail")
        } catch OpenedSendFileError.unsupportedItem {
            // Expected: the helper must never see an empty CreateTransfer request.
        } catch {
            XCTFail("unexpected error: \(error)")
        }

        XCTAssertFalse(controller.isPreparing(deviceID: "dev_wsl"))
        let requests = await client.requests
        XCTAssertTrue(requests.isEmpty)
    }

    @MainActor
    func testTransferControllerLoadsAgentSnapshotAndPrioritizesPendingTransfers() async {
        let delivered = transfer(
            id: "transfer_delivered",
            state: .delivered,
            transferredBytes: 10,
            totalBytes: 10
        )
        let queued = transfer(
            id: "transfer_queued",
            state: .queued,
            transferredBytes: 0,
            totalBytes: 20
        )
        let path = FfiAgentTransferPath(
            transferId: queued.id,
            direction: .send,
            path: .relay
        )
        let telemetry = FfiAgentTransferTelemetry(
            transferId: queued.id,
            relationshipId: "dev_wsl",
            direction: .send,
            rootNames: ["fixture.bin"],
            itemCount: 1,
            directoryCount: 0,
            phase: .transferring,
            transferredBytes: 5,
            totalBytes: 20,
            currentBytesPerSecond: 10,
            averageBytesPerSecond: 8,
            etaSeconds: 2,
            sampledAtUnixMs: 1_757_066_400_000
        )
        let client = FakeMacOSAgentControlClient(response: .snapshot(
            snapshot: snapshot(
                transfers: [delivered, queued],
                activePaths: [path],
                telemetry: [telemetry]
            )
        ))
        let controller = MacOSAgentTransferController(controlClient: client)

        await controller.refreshSnapshot()

        XCTAssertTrue(controller.hasLoadedSnapshot)
        XCTAssertNil(controller.loadError)
        XCTAssertEqual(controller.transfers.map(\.id), [queued.id, delivered.id])
        XCTAssertEqual(controller.transfers(deviceID: "dev_wsl").count, 2)
        XCTAssertEqual(controller.activePath(transferID: queued.id), .relay)
        XCTAssertEqual(controller.transferTelemetry(transferID: queued.id), telemetry)
        XCTAssertEqual(controller.inboxDirectory, "/private/tmp/inbox")
        XCTAssertTrue(controller.hasPendingTransfers)
        let requests = await client.requests
        XCTAssertEqual(requests, [.snapshot(inboxLimit: 20)])
    }

    @MainActor
    func testTransferControllerUsesTypedHelperControlsAndAppliesResponses() async throws {
        let controls: [(FfiAgentRequest, FfiApplicationTransferState)] = [
            (.pauseTransfer(transferId: "transfer_fixture"), .paused),
            (.resumeTransfer(transferId: "transfer_fixture"), .connecting),
            (.recoverTransfer(transferId: "transfer_fixture"), .connecting),
            (.cancelTransfer(transferId: "transfer_fixture"), .canceled),
        ]

        for (request, expectedState) in controls {
            let updated = transfer(
                id: "transfer_fixture",
                state: expectedState,
                transferredBytes: 5,
                totalBytes: 20
            )
            let client = FakeMacOSAgentControlClient(response: .transfer(transfer: updated))
            let controller = MacOSAgentTransferController(controlClient: client)

            switch request {
            case .pauseTransfer:
                try await controller.pauseTransfer(id: updated.id)
            case .resumeTransfer:
                try await controller.resumeTransfer(id: updated.id)
            case .recoverTransfer:
                try await controller.retryTransfer(id: updated.id)
            case .cancelTransfer:
                try await controller.cancelTransfer(id: updated.id)
            default:
                return XCTFail("fixture contains an unsupported Transfer control")
            }

            XCTAssertEqual(controller.transfers, [updated])
            let requests = await client.requests
            XCTAssertEqual(requests, [request])
        }

        let removedID = "transfer_fixture"
        let removeClient = FakeMacOSAgentControlClient(
            response: .transferRemoved(transferId: removedID)
        )
        let removeController = MacOSAgentTransferController(controlClient: removeClient)

        try await removeController.removeTransfer(id: removedID)

        XCTAssertTrue(removeController.transfers.isEmpty)
        let removeRequests = await removeClient.requests
        XCTAssertEqual(removeRequests, [.removeTransfer(transferId: removedID)])
    }

    @MainActor
    func testTransferControllerChangesInboxThroughHelper() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("envoix-inbox-\(UUID().uuidString)")
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        defer { try? FileManager.default.removeItem(at: directory) }
        let client = FakeMacOSAgentControlClient(response: .preferencesUpdated(
            preferences: FfiAgentPreferences(
                version: 1,
                inboxDirectory: directory.path
            )
        ))
        let controller = MacOSAgentTransferController(controlClient: client)

        try await controller.setInboxDirectory(directory)

        XCTAssertEqual(controller.inboxDirectory, directory.path)
        let requests = await client.requests
        XCTAssertEqual(requests, [.setInboxDirectory(path: directory.path)])
    }

    func testAgentTransferPresentationExplainsQueuedAndDeliveryStates() {
        let queued = transfer(
            id: "transfer_queued",
            state: .queued,
            transferredBytes: 0,
            totalBytes: 20
        )
        let awaitingProof = transfer(
            id: "transfer_proof",
            state: .awaitingDeliveryProof,
            transferredBytes: 20,
            totalBytes: 20
        )

        XCTAssertEqual(
            MacOSAgentTransferPresentationPolicy.stateText(queued, language: "zh-Hans"),
            "等待发送"
        )
        XCTAssertTrue(
            MacOSAgentTransferPresentationPolicy.detail(queued, language: "en")?
                .contains("retry in the background") == true
        )
        XCTAssertTrue(MacOSAgentTransferPresentationPolicy.showsProgress(awaitingProof.state))
        XCTAssertFalse(
            MacOSAgentTransferPresentationPolicy.isTerminal(awaitingProof.state)
        )
        XCTAssertEqual(
            MacOSAgentTransferPresentationPolicy.pathText(.lan, language: "zh-Hans"),
            "局域网"
        )
        XCTAssertEqual(
            MacOSAgentTransferPresentationPolicy.phaseText(.saving, language: "zh-Hans"),
            "正在保存"
        )
    }

    private func device(id: String, label: String) -> FfiAgentDeviceSummary {
        FfiAgentDeviceSummary(
            id: id,
            label: label,
            generation: 0,
            previousGeneration: nil,
            broker: "fixture-broker",
            relay: nil
        )
    }

    private func transfer(
        id: String,
        state: FfiApplicationTransferState,
        transferredBytes: UInt64,
        totalBytes: UInt64
    ) -> FfiApplicationTransfer {
        FfiApplicationTransfer(
            id: id,
            relationshipId: "dev_wsl",
            roomId: nil,
            contentId: "content_\(id)",
            direction: .send,
            state: state,
            transferredBytes: transferredBytes,
            totalBytes: totalBytes,
            failure: nil,
            rejection: nil
        )
    }

    private func snapshot(
        transfers: [FfiApplicationTransfer],
        activePaths: [FfiAgentTransferPath],
        telemetry: [FfiAgentTransferTelemetry] = []
    ) -> FfiAgentSnapshot {
        FfiAgentSnapshot(
            status: status(),
            engine: FfiApplicationSnapshot(
                contractVersion: expectedApplicationContractVersion,
                lastSequence: 2,
                capabilities: FfiPlatformCapabilities(values: []),
                devices: [],
                relationships: [],
                rooms: [],
                transfers: transfers
            ),
            inbox: [],
            activePaths: activePaths,
            telemetry: telemetry,
            pendingOffers: [],
            eventCursor: FfiAgentEventCursor(instanceId: "agent_fixture", sequence: 2)
        )
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
    private let registerError: Error?
    private let unregisterError: Error?

    init(
        registrationState: MacOSAgentRegistrationState,
        registerError: Error? = nil,
        unregisterError: Error? = nil
    ) {
        self.registrationState = registrationState
        self.registerError = registerError
        self.unregisterError = unregisterError
    }

    func register() throws {
        registerCallCount += 1
        if let registerError { throw registerError }
        registrationState = .enabled
    }

    func unregister() throws {
        unregisterCallCount += 1
        if let unregisterError { throw unregisterError }
        registrationState = .notRegistered
    }
}

private enum FakeAgentServiceError: Error {
    case registration
    case unregistration
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
