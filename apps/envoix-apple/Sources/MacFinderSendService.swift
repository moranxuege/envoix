#if os(macOS)
import AppKit
import SwiftUI

@MainActor
final class MacApplicationDelegate: NSObject, NSApplicationDelegate {
    private let finderSendService = MacFinderSendService()

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.servicesProvider = finderSendService
        NSUpdateDynamicServices()
        Task {
            await AppleApplicationRuntime.shared.helperService.refresh()
        }
    }

    func applicationShouldHandleReopen(
        _ sender: NSApplication,
        hasVisibleWindows flag: Bool
    ) -> Bool {
        MacMainWindowPresenter.shared.present()
        return true
    }
}

@MainActor
final class MacFinderSendService: NSObject {
    @objc(sendWithEnvoix:userData:error:)
    func sendWithEnvoix(
        _ pasteboard: NSPasteboard,
        userData: String?,
        error errorPointer: AutoreleasingUnsafeMutablePointer<NSString?>
    ) {
        errorPointer.pointee = nil
        do {
            let urls = pastedFileURLs(from: pasteboard)
            guard !urls.isEmpty else { throw OpenedSendFileError.unsupportedItem }
            let outcome = try AppModel.shared.importOpenedSendFiles(urls)
            MacMainWindowPresenter.shared.present()
            if case .queued = outcome {
                ToastCenter.shared.show(
                    "Files are ready and will open after the current send finishes."
                )
            }
        } catch {
            errorPointer.pointee = error.localizedDescription as NSString
            MacMainWindowPresenter.shared.present()
            ToastCenter.shared.show(error.localizedDescription)
        }
    }
}

@MainActor
final class MacMainWindowPresenter {
    static let shared = MacMainWindowPresenter()

    private var openWindow: OpenWindowAction?

    private init() {}

    func register(openWindow: OpenWindowAction) {
        self.openWindow = openWindow
    }

    func present() {
        if let window = NSApp.windows.first(where: { $0.canBecomeMain && !($0 is NSPanel) }) {
            window.makeKeyAndOrderFront(nil)
        } else {
            openWindow?(id: "main")
        }
        NSApp.activate(ignoringOtherApps: true)
    }
}

struct MacMainWindowRegistrationView: View {
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Color.clear
            .frame(width: 0, height: 0)
            .accessibilityHidden(true)
            .onAppear {
                MacMainWindowPresenter.shared.register(openWindow: openWindow)
            }
    }
}
#endif
