package dev.secrettunnel.android

import org.junit.Assert.assertEquals
import org.junit.Test

class UnderlyingNetworkTrackerTest {
    @Test fun handoverIgnoresLateLossAndRepeatedCapabilitiesOfTheNewNetwork() {
        val tracker=UnderlyingNetworkTracker<Int>()
        tracker.available(1,0);tracker.connectedOn(tracker.currentLink()!!)
        tracker.available(2,1000)
        tracker.lost(1)
        tracker.available(2,1400)
        assertEquals(UnderlyingNetworkTracker.Action.WAIT,tracker.action(1500))
        assertEquals(UnderlyingNetworkTracker.Action.RECONNECT,tracker.action(1800))
        tracker.connectedOn(tracker.currentLink()!!)
        assertEquals(UnderlyingNetworkTracker.Action.READY,tracker.action(1801))
    }
    @Test fun offlineDoesNotBecomeReadyAndAChangeDuringConnectIsNotForgotten() {
        val tracker=UnderlyingNetworkTracker<Int>()
        tracker.available(1,0);tracker.connectedOn(tracker.currentLink()!!)
        tracker.lost(1)
        assertEquals(UnderlyingNetworkTracker.Action.OFFLINE,tracker.action(6000))
        tracker.available(2,6001)
        val attempting = tracker.currentLink()!!
        tracker.available(3,6200)
        tracker.connectedOn(attempting)
        assertEquals(UnderlyingNetworkTracker.Action.RECONNECT,tracker.action(7000))
        tracker.connectedOn(tracker.currentLink()!!)
        assertEquals(UnderlyingNetworkTracker.Action.READY,tracker.action(7001))
    }
    @Test fun returningToTheSameNetworkAfterLossRequiresANewConnection() {
        val tracker = UnderlyingNetworkTracker<Int>()
        tracker.available(1, 0)
        tracker.connectedOn(tracker.currentLink()!!)
        tracker.lost(1)
        tracker.available(1, 1000)
        assertEquals(UnderlyingNetworkTracker.Action.RECONNECT, tracker.action(1800))
    }
}
