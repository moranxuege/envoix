#if os(iOS)
import Foundation
import Network

final class AppleBonjourDiscoveryProvider: NearbyDiscoveryProvider {
    let source = NearbyDiscoverySource.mdns

    private enum OperationState {
        case setup
        case ready
        case waiting
        case failed
        case cancelled

        var isSettledUnavailable: Bool {
            switch self {
            case .waiting, .failed, .cancelled: return true
            case .setup, .ready: return false
            }
        }
    }

    private static let observationRefreshInterval: TimeInterval = 5

    private let identity: LocalNearbyDiscoveryIdentity
    private var sink: ((NearbyDiscoveryEvent) -> Void)?
    private var browser: NWBrowser?
    private var listener: NWListener?
    private var refreshTimer: Timer?
    private var recordsByResult: [NWBrowser.Result: NearbyDiscoveryBonjourRecord] = [:]
    private var browserState = OperationState.setup
    private var listenerState = OperationState.setup
    private var active = false

    init(identity: LocalNearbyDiscoveryIdentity) {
        self.identity = identity
    }

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {
        self.sink = sink
        guard !active else {
            emitOperationalStatus()
            return
        }

        active = true
        browserState = .setup
        listenerState = .setup
        recordsByResult.removeAll()
        emitStatus(.starting, .startingLocalNetwork)

        let browserParameters = NWParameters.udp
        browserParameters.includePeerToPeer = true
        let browser = NWBrowser(
            for: .bonjourWithTXTRecord(type: NearbyDiscoveryBonjourRecord.serviceType, domain: nil),
            using: browserParameters
        )
        browser.stateUpdateHandler = { [weak self] state in
            self?.handleBrowserState(state)
        }
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            self?.accept(results: results)
        }
        self.browser = browser

        do {
            let listenerParameters = NWParameters.udp
            listenerParameters.includePeerToPeer = true
            let listener = try NWListener(using: listenerParameters, on: .any)
            let record = NearbyDiscoveryBonjourRecord(identity: identity)
            listener.service = NWListener.Service(
                name: "Envoix-\(identity.peerKey.prefix(8))",
                type: NearbyDiscoveryBonjourRecord.serviceType,
                domain: nil,
                txtRecord: NWTXTRecord(record.dictionary)
            )
            listener.newConnectionHandler = { connection in
                connection.cancel()
            }
            listener.stateUpdateHandler = { [weak self] state in
                self?.handleListenerState(state)
            }
            self.listener = listener
        } catch {
            listenerState = .failed
        }

        browser.start(queue: .main)
        listener?.start(queue: .main)
        startRefreshTimer()
        emitOperationalStatus()
    }

    func stop() {
        guard active else {
            sink = nil
            return
        }
        active = false
        refreshTimer?.invalidate()
        refreshTimer = nil
        recordsByResult.removeAll()
        browser?.stateUpdateHandler = nil
        browser?.browseResultsChangedHandler = nil
        listener?.stateUpdateHandler = nil
        listener?.newConnectionHandler = nil
        browser?.cancel()
        listener?.cancel()
        browser = nil
        listener = nil
        browserState = .cancelled
        listenerState = .cancelled
        emitStatus(.stopped, .discoveryStopped)
        sink = nil
    }

    private func startRefreshTimer() {
        refreshTimer?.invalidate()
        let timer = Timer(timeInterval: Self.observationRefreshInterval, repeats: true) { [weak self] _ in
            self?.emitCurrentObservations()
        }
        RunLoop.main.add(timer, forMode: .common)
        refreshTimer = timer
    }

    private func handleBrowserState(_ state: NWBrowser.State) {
        guard active else { return }
        switch state {
        case .setup:
            browserState = .setup
        case .ready:
            browserState = .ready
        case .waiting:
            browserState = .waiting
            recordsByResult.removeAll()
        case .failed:
            browserState = .failed
            recordsByResult.removeAll()
        case .cancelled:
            browserState = .cancelled
            recordsByResult.removeAll()
        @unknown default:
            browserState = .failed
            recordsByResult.removeAll()
        }
        emitOperationalStatus()
    }

    private func handleListenerState(_ state: NWListener.State) {
        guard active else { return }
        switch state {
        case .setup:
            listenerState = .setup
        case .ready:
            listenerState = .ready
        case .waiting:
            listenerState = .waiting
        case .failed:
            listenerState = .failed
        case .cancelled:
            listenerState = .cancelled
        @unknown default:
            listenerState = .failed
        }
        emitOperationalStatus()
    }

    private func accept(results: Set<NWBrowser.Result>) {
        guard active else { return }
        var accepted: [NWBrowser.Result: NearbyDiscoveryBonjourRecord] = [:]
        for result in results {
            guard case .bonjour(let txtRecord) = result.metadata,
                  let record = NearbyDiscoveryBonjourRecord(dictionary: txtRecord.dictionary),
                  record.peerKey != identity.peerKey else {
                continue
            }
            accepted[result] = record
        }
        recordsByResult = accepted
        emitCurrentObservations()
    }

    private func emitCurrentObservations() {
        guard active else { return }
        let now = Int64(ProcessInfo.processInfo.systemUptime * 1_000)
        for record in recordsByResult.values {
            sink?(.observation(NearbyDiscoveryObservation(
                peerKey: record.peerKey,
                source: source,
                seenAtMilliseconds: now,
                displayName: record.displayName
            )))
        }
    }

    private func emitOperationalStatus() {
        guard active else { return }
        if browserState == .ready && listenerState == .ready {
            emitStatus(.ready, .localNetworkReady)
        } else if browserState == .ready && listenerState.isSettledUnavailable {
            emitStatus(.degraded, .localNetworkScanningOnly)
        } else if listenerState == .ready && browserState.isSettledUnavailable {
            emitStatus(.degraded, .localNetworkVisibleOnly)
        } else if browserState == .setup || listenerState == .setup {
            emitStatus(.starting, .startingLocalNetwork)
        } else {
            emitStatus(.temporarilyUnavailable, .localNetworkPermissionOrUnavailable)
        }
    }

    private func emitStatus(_ availability: NearbyProviderAvailability, _ detail: NearbyProviderDetail) {
        sink?(.status(NearbyProviderStatus(source: source, availability: availability, detail: detail)))
    }
}
#endif
