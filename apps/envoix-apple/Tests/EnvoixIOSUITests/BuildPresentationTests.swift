import XCTest
@testable import Envoix_iOS

final class BuildPresentationTests: XCTestCase {
    func testDebugLabelIncludesAppBuildAndSecondPrecisionTimestamp() {
        let label = AppleBuildPresentation.label(
            infoDictionary: [
                "CFBundleShortVersionString": "0.3.0",
                "CFBundleVersion": "5",
                AppleBuildPresentation.timestampInfoKey: "2026-08-20T21:30:45+0800",
            ],
            coreVersion: "0.3.0",
            apiVersion: 22,
            configuration: .debug
        )

        XCTAssertEqual(
            label,
            "Debug · App 0.3.0 (5) · Core 0.3.0 · API 22"
                + " · Built 2026-08-20T21:30:45+0800"
        )
    }

    func testReleaseLabelIncludesDayPrecisionTimestamp() {
        let label = AppleBuildPresentation.label(
            infoDictionary: [
                "CFBundleShortVersionString": "0.3.0",
                "CFBundleVersion": "5",
                AppleBuildPresentation.timestampInfoKey: "2026-08-20",
            ],
            coreVersion: "0.3.0",
            apiVersion: 22,
            configuration: .release
        )

        XCTAssertEqual(
            label,
            "Release · App 0.3.0 (5) · Core 0.3.0 · API 22 · Built 2026-08-20"
        )
    }

    func testMissingOrUnexpandedMetadataIsReportedAsUnavailable() {
        let label = AppleBuildPresentation.label(
            infoDictionary: [
                "CFBundleShortVersionString": " ",
                "CFBundleVersion": "$(CURRENT_PROJECT_VERSION)",
                AppleBuildPresentation.timestampInfoKey: "$(ENVOIX_BUILD_TIMESTAMP)",
            ],
            coreVersion: "0.3.0",
            apiVersion: 22,
            configuration: .debug
        )

        XCTAssertEqual(
            label,
            "Debug · App unavailable (unavailable) · Core 0.3.0 · API 22"
                + " · Built unavailable"
        )
    }
}
