package dev.secrettunnel.android

/** Physical default-network transitions, independent of VPN interface lifetime. */
internal class UnderlyingNetworkTracker<N : Any>(private val settleMs: Long = 750) {
    enum class Action { READY, WAIT, OFFLINE, RECONNECT }
    class Link<N>(val network: N, val availableAt: Long)
    private var current: Link<N>? = null
    private var connected: Link<N>? = null

    @Synchronized fun available(network: N, now: Long): Boolean {
        if (current?.network == network) return false
        current = Link(network, now)
        return true
    }
    @Synchronized fun lost(network: N): Boolean {
        if (current?.network != network) return false
        current = null
        return true
    }
    @Synchronized fun currentLink(): Link<N>? = current
    @Synchronized fun connectedOn(link: Link<N>) { connected = link }
    @Synchronized fun action(now: Long): Action {
        val link = current ?: return Action.OFFLINE
        return when {
            link === connected -> Action.READY
            now - link.availableAt < settleMs -> Action.WAIT
            connected != null -> Action.RECONNECT
            else -> Action.READY
        }
    }
}
