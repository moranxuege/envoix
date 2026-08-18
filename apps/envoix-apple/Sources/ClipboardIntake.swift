#if os(macOS)
import AppKit
import Foundation
import UniformTypeIdentifiers

struct ClipboardImagePayload: Equatable {
    let data: Data
    let contentTypeIdentifier: String
    let preferredFileName: String
}

enum ClipboardSendContent: Equatable {
    case file(URL)
    case image(ClipboardImagePayload)
}

func clipboardSendContent(
    from pasteboard: NSPasteboard = .general
) -> ClipboardSendContent? {
    if let url = pastedFileURL(from: pasteboard) {
        return .file(url)
    }
    return clipboardImagePayload(from: pasteboard).map(ClipboardSendContent.image)
}

func clipboardImagePayload(
    from pasteboard: NSPasteboard = .general
) -> ClipboardImagePayload? {
    if let data = pasteboard.data(forType: .png), !data.isEmpty {
        return ClipboardImagePayload(
            data: data,
            contentTypeIdentifier: UTType.png.identifier,
            preferredFileName: "Clipboard Image.png"
        )
    }

    guard let image = NSImage(pasteboard: pasteboard),
          let tiff = image.tiffRepresentation,
          let representation = NSBitmapImageRep(data: tiff),
          let data = representation.representation(using: .png, properties: [:]),
          !data.isEmpty else { return nil }
    return ClipboardImagePayload(
        data: data,
        contentTypeIdentifier: UTType.png.identifier,
        preferredFileName: "Clipboard Image.png"
    )
}
#endif
