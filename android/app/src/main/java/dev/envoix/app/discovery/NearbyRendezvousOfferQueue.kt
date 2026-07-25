package dev.envoix.app.discovery

/**
 * Small in-memory inbox for unauthenticated nearby invitations. Duplicate
 * advertisements do not refresh their lifetime, so a peer cannot keep a stale
 * prompt alive indefinitely.
 */
internal class NearbyRendezvousOfferQueue(
    private val maxSize: Int = 4,
    private val ttlMs: Long = 30_000L,
) {
    init {
        require(maxSize > 0)
        require(ttlMs > 0)
    }

    private data class Key(
        val senderPeerKey: String,
        val invite: String,
    )

    private data class Entry(
        val offer: NearbyRendezvousOffer,
        val receivedAtMs: Long,
    )

    private val entries = LinkedHashMap<Key, Entry>()

    fun add(
        offer: NearbyRendezvousOffer,
        nowMs: Long,
    ): Boolean {
        expire(nowMs)
        val key = Key(offer.senderPeerKey, offer.invite)
        if (key in entries) return false
        while (entries.size >= maxSize) {
            entries.remove(entries.keys.first())
        }
        entries[key] = Entry(offer, nowMs)
        return true
    }

    fun snapshot(nowMs: Long): List<NearbyRendezvousOffer> {
        expire(nowMs)
        return entries.values.map(Entry::offer)
    }

    fun remove(requestId: String): Boolean {
        val key = entries.entries.firstOrNull { it.value.offer.requestId == requestId }?.key ?: return false
        entries.remove(key)
        return true
    }

    fun retainSender(peerKey: String) {
        entries.entries.removeAll { it.value.offer.senderPeerKey != peerKey }
    }

    fun clear() {
        entries.clear()
    }

    private fun expire(nowMs: Long) {
        entries.entries.removeAll { nowMs - it.value.receivedAtMs >= ttlMs }
    }
}
