package dev.envoix.app.discovery

internal class WifiAwareDiscoveryProvider : DiscoveryProvider {
    override val source = DiscoverySource.WifiAware

    override fun start(listener: DiscoveryListener) {
        listener.onStatus(
            ProviderStatus(
                source = source,
                availability = ProviderAvailability.Reserved,
                detail = "Provider interface reserved for a later phase",
            ),
        )
    }

    override fun stop() = Unit
}
