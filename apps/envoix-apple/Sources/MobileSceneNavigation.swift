#if os(iOS) || os(macOS)
import Combine

enum MobilePage: String, CaseIterable, Identifiable {
    case connect
    case room
    case activity
    case settings

    var id: String { rawValue }
}

/// Navigation state owned by one window scene.
///
/// The process-wide `AppModel` owns transfer state; opening another iPad
/// window must not make its selected page or back destination global.
@MainActor
final class MobileSceneNavigationState: ObservableObject {
    @Published var page: MobilePage
    private(set) var returnPage: MobilePage = .connect

    init(initialPage: MobilePage = .connect) {
        page = initialPage
    }

    func show(_ destination: MobilePage) {
        if page == .connect || page == .room {
            returnPage = page
        }
        page = destination
    }

    func returnToContext(hasActiveRoom: Bool) {
        page = returnPage == .room && !hasActiveRoom ? .connect : returnPage
    }
}
#endif
