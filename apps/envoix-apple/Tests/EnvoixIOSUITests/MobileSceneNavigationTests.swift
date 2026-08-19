import XCTest
@testable import Envoix_iOS

@MainActor
final class MobileSceneNavigationTests: XCTestCase {
    func testHostDeclaresUniversalResizableIPadSupport() throws {
        let families = try XCTUnwrap(
            Bundle.main.object(forInfoDictionaryKey: "UIDeviceFamily") as? [Int]
        )
        let sourceInfo = try sourceInfoDictionary()
        let iPadOrientations = try XCTUnwrap(
            sourceInfo["UISupportedInterfaceOrientations~ipad"]
                as? [String]
        )
        let sceneManifest = try XCTUnwrap(
            Bundle.main.infoDictionary?["UIApplicationSceneManifest"]
                as? [String: Any]
        )

        XCTAssertEqual(Set(families), Set([1, 2]))
        XCTAssertNotEqual(
            Bundle.main.object(forInfoDictionaryKey: "UIRequiresFullScreen") as? Bool,
            true
        )
        XCTAssertEqual(sceneManifest["UIApplicationSupportsMultipleScenes"] as? Bool, true)
        XCTAssertEqual(
            Set(iPadOrientations),
            Set([
                "UIInterfaceOrientationPortrait",
                "UIInterfaceOrientationPortraitUpsideDown",
                "UIInterfaceOrientationLandscapeLeft",
                "UIInterfaceOrientationLandscapeRight",
            ])
        )
    }

    func testEachSceneStartsOnItsOwnConnectionPage() {
        let first = MobileSceneNavigationState()
        let second = MobileSceneNavigationState(initialPage: .activity)

        first.show(.settings)

        XCTAssertEqual(first.page, .settings)
        XCTAssertEqual(second.page, .activity)
    }

    func testAuxiliaryPageReturnsToTheRoomThatOpenedIt() {
        let navigation = MobileSceneNavigationState()
        navigation.show(.room)
        navigation.show(.activity)

        navigation.returnToContext(hasActiveRoom: true)

        XCTAssertEqual(navigation.page, .room)
    }

    func testMissingRoomReturnsAuxiliaryPageToConnectionHub() {
        let navigation = MobileSceneNavigationState(initialPage: .room)
        navigation.show(.settings)

        navigation.returnToContext(hasActiveRoom: false)

        XCTAssertEqual(navigation.page, .connect)
    }

    func testRuntimeKeepsCurrentActivePresentationOwner() {
        let first = UUID()
        let second = UUID()
        let requests = [
            first: runtimeRequest(order: 0, isActive: true),
            second: runtimeRequest(order: 1, isActive: true),
        ]

        XCTAssertEqual(
            AppleApplicationRuntimePolicy.presentationOwner(
                current: second,
                requests: requests
            ),
            second
        )
    }

    func testRuntimeTransfersPresentationOwnershipAndAggregatesSceneLeases() {
        let closed = UUID()
        let active = UUID()
        let requests = [
            closed: runtimeRequest(
                order: 0,
                isActive: false,
                requestsDiscovery: false,
                keepsRememberedConnected: true
            ),
            active: runtimeRequest(
                order: 1,
                isActive: true,
                requestsDiscovery: true,
                keepsRememberedConnected: false
            ),
        ]

        XCTAssertEqual(
            AppleApplicationRuntimePolicy.presentationOwner(
                current: closed,
                requests: requests
            ),
            active
        )
        XCTAssertTrue(AppleApplicationRuntimePolicy.requestsDiscovery(requests))
        XCTAssertTrue(AppleApplicationRuntimePolicy.keepsRememberedConnected(requests))
    }

    func testRuntimeHasNoPromptOwnerWithoutAnActiveScene() {
        let scene = UUID()
        let requests = [scene: runtimeRequest(order: 0, isActive: false)]

        XCTAssertNil(
            AppleApplicationRuntimePolicy.presentationOwner(
                current: scene,
                requests: requests
            )
        )
        XCTAssertFalse(AppleApplicationRuntimePolicy.requestsDiscovery(requests))
        XCTAssertFalse(AppleApplicationRuntimePolicy.keepsRememberedConnected(requests))
    }

    private func runtimeRequest(
        order: Int,
        isActive: Bool,
        requestsDiscovery: Bool = false,
        keepsRememberedConnected: Bool = false
    ) -> AppleSceneRuntimeRequest {
        AppleSceneRuntimeRequest(
            order: order,
            isActive: isActive,
            requestsDiscovery: requestsDiscovery,
            keepsRememberedConnected: keepsRememberedConnected,
            displayName: "Test device",
            identityPath: "/tmp/test-identity"
        )
    }

    private func sourceInfoDictionary() throws -> [String: Any] {
        let projectDirectory = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let data = try Data(
            contentsOf: projectDirectory
                .appendingPathComponent("Resources/Info.plist")
        )
        return try XCTUnwrap(
            PropertyListSerialization.propertyList(from: data, format: nil)
                as? [String: Any]
        )
    }
}
