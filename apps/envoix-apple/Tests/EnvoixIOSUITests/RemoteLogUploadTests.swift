import XCTest
@testable import Envoix_iOS

final class RemoteLogUploadTests: XCTestCase {
    private let target = RemoteLogUpload.Target(roomID: "room1", side: "send")

    func testRequestRequiresHTTPS() {
        XCTAssertThrowsError(
            try RemoteLogUpload.request(
                server: "http://logs.example",
                target: target,
                body: "report",
                bearerToken: "upload-token"
            )
        ) { error in
            XCTAssertEqual(error as? RemoteLogUpload.UploadError, .invalidServer)
        }
    }

    func testRequestRequiresBearerToken() {
        XCTAssertThrowsError(
            try RemoteLogUpload.request(
                server: "https://logs.example",
                target: target,
                body: "report",
                bearerToken: "  "
            )
        ) { error in
            XCTAssertEqual(error as? RemoteLogUpload.UploadError, .authenticationRequired)
        }
    }

    func testRequestCarriesBoundedBodyAndBearerToken() throws {
        let request = try RemoteLogUpload.request(
            server: "https://logs.example",
            target: target,
            body: "report",
            bearerToken: " upload-token "
        )

        XCTAssertEqual(request.url?.absoluteString, "https://logs.example/logs/room1?side=send")
        XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer upload-token")
        XCTAssertEqual(request.httpBody, Data("report".utf8))
    }

    func testRequestRejectsOversizedBody() {
        XCTAssertThrowsError(
            try RemoteLogUpload.request(
                server: "https://logs.example",
                target: target,
                body: String(repeating: "x", count: RemoteLogUpload.bodyMaxBytes + 1),
                bearerToken: "upload-token"
            )
        ) { error in
            XCTAssertEqual(error as? RemoteLogUpload.UploadError, .bodyTooLarge)
        }
    }
}
