import SwiftUI

struct ContentView: View {
    #if os(iOS)
    @Environment(\.scenePhase) private var scenePhase
    #endif
    @AppStorage("envoix.appearance") private var appearance: Appearance = .system

    var body: some View {
        ZStack {
            Theme.bg.ignoresSafeArea()
            MobileConnectionFlowView()
                #if os(macOS)
                .frame(minWidth: 760, idealWidth: 920, minHeight: 620, idealHeight: 680)
                #endif

            #if os(iOS)
            if scenePhase != .active {
                Theme.bg.ignoresSafeArea()
            }
            #endif
        }
        .toastHost()
        .preferredColorScheme(appearance.colorScheme)
    }
}
