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

/// Resolves an item copied in Finder or a plain-text path, expanding `~`.
func pastedFileURL(from pasteboard: NSPasteboard = .general) -> URL? {
    pastedFileURLs(from: pasteboard).first
}

/// Resolves files copied or supplied by Finder while preserving their order.
func pastedFileURLs(from pasteboard: NSPasteboard = .general) -> [URL] {
    let exists = { FileManager.default.fileExists(atPath: $0) }

    if let urls = pasteboard.readObjects(
        forClasses: [NSURL.self],
        options: [.urlReadingFileURLsOnly: true]
    ) as? [URL],
       !urls.isEmpty,
       urls.allSatisfy({ exists($0.path) }) {
        return urls
    }
    if let raw = pasteboard.string(forType: .string)?.trimmingCharacters(in: .whitespacesAndNewlines),
       !raw.isEmpty {
        let expanded = (raw as NSString).expandingTildeInPath
        if exists(expanded) { return [URL(fileURLWithPath: expanded)] }
    }
    return []
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
