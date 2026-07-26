import Combine
import Foundation

struct NearbyInviteOffer {
    let perform: (
        _ invite: String,
        _ completion: @escaping (_ error: String?) -> Void
    ) -> Void
}

final class NearbyInviteDeliveryController: ObservableObject {
    @Published private(set) var isDelivering = false
    @Published private(set) var error: String?

    private var deliveryID: UUID?

    func deliver(
        invite: String,
        using offer: NearbyInviteOffer?,
        onSuccess: @escaping () -> Void
    ) {
        guard deliveryID == nil else { return }
        guard let offer else {
            onSuccess()
            return
        }

        let id = UUID()
        deliveryID = id
        isDelivering = true
        error = nil
        offer.perform(invite) { [weak self] error in
            DispatchQueue.main.async {
                guard let self, self.deliveryID == id else { return }
                self.deliveryID = nil
                self.isDelivering = false
                if let error {
                    self.error = error
                } else {
                    onSuccess()
                }
            }
        }
    }

    func cancel() {
        deliveryID = nil
        isDelivering = false
        error = nil
    }
}
