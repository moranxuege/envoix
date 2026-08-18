#if os(macOS)
import AppKit
import UniformTypeIdentifiers
import XCTest

@MainActor
final class ClipboardIntakeTests: XCTestCase {
    private var pasteboard: NSPasteboard!

    override func setUp() {
        super.setUp()
        pasteboard = NSPasteboard(name: NSPasteboard.Name(
            "com.envoix.tests.clipboard.\(UUID().uuidString)"
        ))
        pasteboard.clearContents()
    }

    override func tearDown() {
        pasteboard.clearContents()
        pasteboard = nil
        super.tearDown()
    }

    func testReadsPNGWithoutReencoding() throws {
        let png = try makePNG()
        XCTAssertTrue(pasteboard.setData(png, forType: .png))

        let payload = try XCTUnwrap(clipboardImagePayload(from: pasteboard))

        XCTAssertEqual(payload.data, png)
        XCTAssertEqual(payload.contentTypeIdentifier, UTType.png.identifier)
        XCTAssertEqual(payload.preferredFileName, "Clipboard Image.png")
    }

    func testConvertsTIFFToPNG() throws {
        let tiff = try makeImage().tiffRepresentation
        XCTAssertTrue(pasteboard.setData(try XCTUnwrap(tiff), forType: .tiff))

        let payload = try XCTUnwrap(clipboardImagePayload(from: pasteboard))

        XCTAssertEqual(payload.contentTypeIdentifier, UTType.png.identifier)
        XCTAssertNotNil(NSBitmapImageRep(data: payload.data))
    }

    func testExistingFileTakesPrecedenceOverClipboardImage() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let source = root.appendingPathComponent("payload.txt", isDirectory: false)
        try Data("payload".utf8).write(to: source, options: .atomic)

        XCTAssertTrue(pasteboard.writeObjects([source as NSURL]))
        XCTAssertTrue(pasteboard.setData(try makePNG(), forType: .png))

        XCTAssertEqual(clipboardSendContent(from: pasteboard), .file(source))
    }

    func testReadsEveryFinderFileInPasteboardOrder() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let first = root.appendingPathComponent("first.txt", isDirectory: false)
        let second = root.appendingPathComponent("second.txt", isDirectory: false)
        try Data("first".utf8).write(to: first, options: .atomic)
        try Data("second".utf8).write(to: second, options: .atomic)

        XCTAssertTrue(pasteboard.writeObjects([first as NSURL, second as NSURL]))

        XCTAssertEqual(pastedFileURLs(from: pasteboard), [first, second])
        XCTAssertEqual(pastedFileURL(from: pasteboard), first)
    }

    func testRejectsUnsupportedClipboardContent() {
        XCTAssertTrue(pasteboard.setString("not a local path", forType: .string))

        XCTAssertNil(clipboardSendContent(from: pasteboard))
    }

    func testStagesClipboardBytesAsDurableImageDraft() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = ShareDraftStore(
            rootDirectory: root,
            availableCapacity: { _ in Int64.max }
        )
        let png = try makePNG()

        let draft = try store.stage(
            data: png,
            contentTypeIdentifier: UTType.png.identifier,
            mediaKind: .image,
            preferredFileName: "Clipboard Image.png"
        )

        XCTAssertEqual(draft.descriptor.items.count, 1)
        XCTAssertEqual(draft.descriptor.mediaKind, .image)
        XCTAssertEqual(draft.descriptor.fileName, "Clipboard Image.png")
        XCTAssertEqual(try Data(contentsOf: try XCTUnwrap(draft.fileURLs.first)), png)
        XCTAssertEqual(try store.load(id: draft.descriptor.id), draft)
    }

    func testRejectsEmptyClipboardImageData() {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = ShareDraftStore(
            rootDirectory: root,
            availableCapacity: { _ in Int64.max }
        )

        XCTAssertThrowsError(try store.stage(
            data: Data(),
            contentTypeIdentifier: UTType.png.identifier,
            mediaKind: .image,
            preferredFileName: "Clipboard Image.png"
        )) { error in
            XCTAssertEqual(error as? ShareDraftStoreError, .invalidDraft)
        }
    }

    private func makePNG() throws -> Data {
        let image = try makeImage()
        let tiff = try XCTUnwrap(image.tiffRepresentation)
        let representation = try XCTUnwrap(NSBitmapImageRep(data: tiff))
        return try XCTUnwrap(representation.representation(using: .png, properties: [:]))
    }

    private func makeImage() throws -> NSImage {
        let representation = try XCTUnwrap(NSBitmapImageRep(
            bitmapDataPlanes: nil,
            pixelsWide: 2,
            pixelsHigh: 2,
            bitsPerSample: 8,
            samplesPerPixel: 4,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: .deviceRGB,
            bytesPerRow: 0,
            bitsPerPixel: 0
        ))
        representation.setColor(
            NSColor(deviceRed: 0.1, green: 0.4, blue: 0.9, alpha: 1),
            atX: 0,
            y: 0
        )
        let image = NSImage(size: NSSize(width: 2, height: 2))
        image.addRepresentation(representation)
        return image
    }
}
#endif
