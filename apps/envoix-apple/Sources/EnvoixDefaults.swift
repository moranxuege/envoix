import EnvoixCore
import Foundation

private let deploymentEndpoints = envoixDeploymentEndpoints()
let defaultRendezvousBroker = deploymentEndpoints.broker
let defaultRelayURL = deploymentEndpoints.relay

private let retiredDefaultBrokers: Set<String> = [
    "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445",
]
private let retiredDefaultRelays: Set<String> = [
    "https://envoix.chkxwlyh.us:8444",
]

func migrateRetiredDeploymentDefaults(_ defaults: UserDefaults = .standard) {
    if let broker = defaults.string(forKey: "envoix.serverURL"),
       retiredDefaultBrokers.contains(broker.trimmingCharacters(in: .whitespacesAndNewlines)) {
        defaults.removeObject(forKey: "envoix.serverURL")
    }
    if let relay = defaults.string(forKey: "envoix.relayURL"),
       retiredDefaultRelays.contains(relay.trimmingCharacters(in: .whitespacesAndNewlines)) {
        defaults.removeObject(forKey: "envoix.relayURL")
    }
}
