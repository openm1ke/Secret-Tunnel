package dev.secrettunnel.android

/** Physical default-network transitions, independent of VPN interface lifetime. */
internal class UnderlyingNetworkTracker(private val settleMs:Long=750) {
    enum class Action { READY, WAIT, OFFLINE, RECONNECT }
    private var current:Long?=null
    private var connected:Long?=null
    private var connectedOnce=false
    private var changedAt=0L

    @Synchronized fun available(handle:Long,now:Long) {
        if(current!=handle){current=handle;changedAt=now}
    }
    @Synchronized fun lost(handle:Long,now:Long) {
        if(current==handle){current=null;changedAt=now}
    }
    @Synchronized fun currentHandle():Long?=current
    @Synchronized fun connectedOn(handle:Long?) {connected=handle;connectedOnce=true}
    @Synchronized fun action(now:Long):Action=when {
        current==null->Action.OFFLINE
        connectedOnce&&current==connected->Action.READY
        now-changedAt<settleMs->Action.WAIT
        connectedOnce->Action.RECONNECT
        else->Action.READY
    }
}
