import EnvoixCore
import Foundation

enum MenuBarPresentationText {
    static func transferTitle(_ direction: FfiTransferDirection, language: String) -> String {
        TransferActivityText.direction(direction, language: language)
    }

    static func openAppAction(language: String) -> String {
        AppText.localized("menu_bar.action.open", language: language)
    }

    static func quitAppAction(language: String) -> String {
        AppText.localized("menu_bar.action.quit", language: language)
    }

    static func summary(
        state: TransferActivityState?,
        progressFraction: Double,
        bytesPerSecond: Double,
        language: String
    ) -> String {
        guard let state else {
            return AppText.localized("menu_bar.status.idle", language: language)
        }
        if state == .transferring {
            let finiteFraction = progressFraction.isFinite ? progressFraction : 0
            let percentage = Int((min(max(finiteFraction, 0), 1) * 100).rounded())
            guard bytesPerSecond.isFinite, bytesPerSecond > 0 else {
                return "\(percentage)%"
            }
            return "\(percentage)% · \(rateString(bytesPerSecond))"
        }

        let key: String
        switch state {
        case .preparing: key = "menu_bar.status.preparing"
        case .waitingForPeer: key = "menu_bar.status.waiting"
        case .pairing: key = "menu_bar.status.pairing"
        case .connecting: key = "menu_bar.status.connecting"
        case .awaitingDecision: key = "menu_bar.status.review"
        case .verifying: key = "menu_bar.status.verifying"
        case .saving: key = "menu_bar.status.saving"
        case .waitingForReceiverSave: key = "menu_bar.status.receiver_saving"
        case .finalizingDelivery: key = "menu_bar.status.finalizing"
        case .paused: key = "menu_bar.status.paused"
        case .delivered: key = "menu_bar.status.delivered"
        case .canceled: key = "menu_bar.status.canceled"
        case .failed: key = "menu_bar.status.failed"
        case .transferring:
            preconditionFailure("Transferring is handled before the localized state lookup")
        }
        return AppText.localized(key, language: language)
    }
}

#if os(macOS)
import AppKit
import SwiftUI

/// Compact status shown in the menu-bar popover. Mirrors the live transfer state
/// and offers a one-click way to bring the main window forward.
struct MenuBarView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.openWindow) private var openWindow
    @Environment(\.appLanguage) private var language

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Envoix").font(.headline)

            TransferMenuRow(
                title: MenuBarPresentationText.transferTitle(.receive, language: language),
                viewModel: model.receive,
                language: language
            )
            TransferMenuRow(
                title: MenuBarPresentationText.transferTitle(.send, language: language),
                viewModel: model.send,
                language: language
            )

            Divider()

            Button(MenuBarPresentationText.openAppAction(language: language)) {
                openWindow(id: "main")
                NSApp.activate(ignoringOtherApps: true)
            }
            Button(MenuBarPresentationText.quitAppAction(language: language)) { NSApp.terminate(nil) }
        }
        .padding(14)
        .frame(width: 240)
    }

}

private struct TransferMenuRow: View {
    let title: String
    @ObservedObject var viewModel: TransferViewModel
    let language: String

    var body: some View {
        HStack {
            Text(title).foregroundStyle(.secondary)
            Spacer()
            Text(summary).font(.callout.monospacedDigit())
        }
    }

    private var summary: String {
        MenuBarPresentationText.summary(
            state: viewModel.presentationState,
            progressFraction: viewModel.progressFraction,
            bytesPerSecond: viewModel.bytesPerSec,
            language: language
        )
    }
}
#endif
