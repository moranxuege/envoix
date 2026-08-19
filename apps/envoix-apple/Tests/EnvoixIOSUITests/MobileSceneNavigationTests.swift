import XCTest
@testable import Envoix_iOS

@MainActor
final class MobileSceneNavigationTests: XCTestCase {
    func testHostDeclaresUniversalResizableIPadSupport() throws {
        let families = try XCTUnwrap(
            Bundle.main.object(forInfoDictionaryKey: "UIDeviceFamily") as? [Int]
        )
        let iPadOrientations = try XCTUnwrap(
            Bundle.main.infoDictionary?["UISupportedInterfaceOrientations~ipad"]
                as? [String]
        )

        XCTAssertEqual(Set(families), Set([1, 2]))
        XCTAssertNotEqual(
            Bundle.main.object(forInfoDictionaryKey: "UIRequiresFullScreen") as? Bool,
            true
        )
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
}
