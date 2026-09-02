import EnvoixCore
import Foundation
import XCTest

final class AgentHostBoundaryTests: XCTestCase {
    func testProductionConfigurationUsesStablePrivateBoundary() throws {
        let configuration = try MacOSAgentBoundary.hostConfiguration(
            localizedDeviceName: "  Test Mac  "
        )

        XCTAssertTrue(configuration.stateDirectory.hasPrefix("/"))
        XCTAssertTrue(configuration.stateDirectory.hasSuffix("/com.envoix.app/agent-v1"))
        XCTAssertEqual(configuration.inboxDirectory, configuration.stateDirectory + "/inbox")
        XCTAssertEqual(configuration.controlEndpoint, configuration.stateDirectory + "/agent.sock")
        XCTAssertEqual(configuration.deviceName, "Test Mac")
        XCTAssertEqual(configuration.credentialProtection, .appleKeychain)
        XCTAssertEqual(
            MacOSAgentBoundary.helperKeychainAccessGroup,
            AppleApplicationVault.helperAccessGroup
        )
    }

    func testDeviceNameIsBoundedAndRejectsControlCharacters() {
        let longName = String(repeating: "M", count: 80) + "\n"
        let normalized = MacOSAgentBoundary.deviceName(from: longName)

        XCTAssertEqual(normalized.count, 64)
        XCTAssertFalse(normalized.contains("\n"))
        XCTAssertEqual(MacOSAgentBoundary.deviceName(from: " \n "), "Mac")
    }

    func testHostWrapperRequiresAPI24AgentCapability() throws {
        let configuration = isolatedConfiguration()
        let core = envoixCoreInfo()
        XCTAssertEqual(core.ffiApiVersion, 24)
        XCTAssertTrue(core.capabilities.contains(expectedAgentHostControlCapability))

        let incompatible = FfiCoreInfo(
            ffiApiVersion: 24,
            coreVersion: core.coreVersion,
            capabilities: core.capabilities.filter {
                $0 != expectedAgentHostControlCapability
            }
        )
        XCTAssertThrowsError(
            try EnvoixEngineHelperHost.start(
                configuration: configuration,
                vault: MemoryAgentVault(),
                core: incompatible
            )
        ) { error in
            XCTAssertEqual(error as? MacOSAgentBoundaryError, .incompatibleCore)
        }
    }

    func testReadinessValidationAndShutdownUseTypedHost() async throws {
        let configuration = isolatedConfiguration()
        let ready = FfiAgentHostReady(
            controlEndpoint: configuration.controlEndpoint,
            agentProtocolVersion: expectedAgentProtocolVersion,
            applicationContractVersion: expectedApplicationContractVersion
        )
        let fake = FakeAgentHost(readiness: ready)
        let owner = EnvoixEngineHelperHost(host: fake, configuration: configuration)

        let observedReady = try await owner.waitUntilReady()
        let firstFakeShutdown = try await owner.shutdown()
        let secondFakeShutdown = try await owner.shutdown()
        XCTAssertEqual(observedReady, ready)
        XCTAssertEqual(firstFakeShutdown, .stopped)
        XCTAssertEqual(secondFakeShutdown, .stopped)
        XCTAssertEqual(fake.shutdownCallCount, 2)

        let incompatible = FakeAgentHost(readiness: FfiAgentHostReady(
            controlEndpoint: configuration.controlEndpoint,
            agentProtocolVersion: expectedAgentProtocolVersion - 1,
            applicationContractVersion: expectedApplicationContractVersion
        ))
        let incompatibleOwner = EnvoixEngineHelperHost(
            host: incompatible,
            configuration: configuration
        )
        do {
            _ = try await incompatibleOwner.waitUntilReady()
            XCTFail("incompatible readiness should fail")
        } catch let error as MacOSAgentBoundaryError {
            XCTAssertEqual(
                error,
                .incompatibleReadiness(
                    agentProtocol: expectedAgentProtocolVersion - 1,
                    applicationContract: expectedApplicationContractVersion
                )
            )
        }
    }

    func testRealHostRejectsSecondOwnerAndReopensAfterAwaitedShutdown() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let configuration = isolatedConfiguration(root: root)
        let vault = MemoryAgentVault()
        let first = try FfiAgentHost.start(
            configuration: configuration,
            vault: vault
        )
        let ready = try await first.waitUntilReady()
        XCTAssertEqual(ready.controlEndpoint, configuration.controlEndpoint)

        let client = try FfiAgentControlClient(
            controlEndpoint: configuration.controlEndpoint
        )
        let response = try await client.call(request: .diagnostics)
        guard case let .diagnostics(diagnostics) = response else {
            return XCTFail("expected typed diagnostics, got \(response)")
        }
        XCTAssertEqual(diagnostics.agentProtocolVersion, expectedAgentProtocolVersion)
        XCTAssertEqual(
            diagnostics.applicationContractVersion,
            expectedApplicationContractVersion
        )
        XCTAssertEqual(diagnostics.credentialProtection, .ownerOnlyFile)

        let second = try FfiAgentHost.start(
            configuration: configuration,
            vault: vault
        )
        do {
            _ = try await second.waitUntilReady()
            XCTFail("a second durable owner should be rejected")
        } catch let FfiAgentHostError.Failed(code, _) {
            XCTAssertEqual(code, .stateAlreadyOwned)
        }
        _ = try? await second.shutdown()

        let firstShutdown = try await first.shutdown()
        let repeatedShutdown = try await first.shutdown()
        XCTAssertEqual(firstShutdown, .stopped)
        XCTAssertEqual(repeatedShutdown, .stopped)

        let reopened = try FfiAgentHost.start(
            configuration: configuration,
            vault: vault
        )
        _ = try await reopened.waitUntilReady()
        let reopenedShutdown = try await reopened.shutdown()
        XCTAssertEqual(reopenedShutdown, .stopped)
    }

    private func isolatedConfiguration(
        root: URL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
    ) -> FfiAgentHostConfiguration {
        let state = root.appendingPathComponent("state", isDirectory: true)
        return FfiAgentHostConfiguration(
            stateDirectory: state.path,
            inboxDirectory: root.appendingPathComponent("inbox", isDirectory: true).path,
            controlEndpoint: state.appendingPathComponent("agent.sock").path,
            deviceName: "Envoix Test Mac",
            broker: defaultRendezvousBroker,
            relay: defaultRelayURL,
            credentialProtection: .ownerOnlyFile
        )
    }
}

private final class FakeAgentHost: FfiAgentHostProtocol, @unchecked Sendable {
    private let lock = NSLock()
    private let readiness: FfiAgentHostReady
    private var shutdownCalls = 0

    init(readiness: FfiAgentHostReady) {
        self.readiness = readiness
    }

    var shutdownCallCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return shutdownCalls
    }

    func lifecycle() -> FfiAgentHostLifecycleState {
        .ready
    }

    func waitUntilReady() async throws -> FfiAgentHostReady {
        readiness
    }

    func shutdown() async throws -> FfiAgentHostLifecycleState {
        recordShutdown()
        return .stopped
    }

    private func recordShutdown() {
        lock.lock()
        shutdownCalls += 1
        lock.unlock()
    }
}

private final class MemoryAgentVault: FfiApplicationVault, @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String: Data] = [:]

    func contains(reference: String) throws -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return values[reference] != nil
    }

    func store(reference: String, opaqueCredential: Data) throws {
        lock.lock()
        values[reference] = opaqueCredential
        lock.unlock()
    }

    func load(reference: String) throws -> Data? {
        lock.lock()
        defer { lock.unlock() }
        return values[reference]
    }

    func delete(reference: String) throws {
        lock.lock()
        values.removeValue(forKey: reference)
        lock.unlock()
    }
}
