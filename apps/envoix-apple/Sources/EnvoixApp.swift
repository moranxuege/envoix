import SwiftUI

@main
struct EnvoixApp: App {
    @StateObject private var model = AppModel.shared
    @AppStorage("envoix.language") private var language = "en"
    #if os(macOS)
    @NSApplicationDelegateAdaptor
    private var applicationDelegate: MacApplicationDelegate
    #endif

    init() {
        UserDefaults.standard.removeObject(forKey: "envoix.token")
    }

    var body: some Scene {
        #if os(macOS)
        WindowGroup(id: "main") {
            rootView
        }
        .windowResizability(.contentMinSize)
        .defaultSize(width: 980, height: 720)

        // Menu-bar presence: keeps the app alive after the window is closed and
        // gives a quick status popover. `.window` style shows SwiftUI content.
        MenuBarExtra {
            MenuBarView()
                .environmentObject(model)
                .environment(\.appLanguage, language)
        } label: {
            Image(systemName: model.isActive ? "arrow.up.arrow.down.circle.fill"
                                              : "arrow.up.arrow.down.circle")
        }
        .menuBarExtraStyle(.window)
        #else
        WindowGroup(id: "main") {
            rootView
        }
        #endif
    }

    @ViewBuilder private var rootView: some View {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--ui-testing-accessibility-text") {
            appContent.dynamicTypeSize(.accessibility5)
        } else {
            appContent
        }
        #else
        appContent
        #endif
    }

    private var appContent: some View {
        ContentView()
            .environmentObject(model)
            .environment(\.appLanguage, language)
            #if os(macOS)
            .background(MacMainWindowRegistrationView())
            #endif
    }
}
