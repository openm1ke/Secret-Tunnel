package dev.secrettunnel.android

import org.junit.Assert.assertEquals
import org.junit.Test

class UnderlyingNetworkTrackerTest {
    @Test fun handoverIgnoresLateLossAndRepeatedCapabilitiesOfTheNewNetwork() {
        val tracker=UnderlyingNetworkTracker()
        tracker.available(1,0);tracker.connectedOn(1)
        tracker.available(2,1000)
        tracker.lost(1,1100)
        tracker.available(2,1400)
        assertEquals(UnderlyingNetworkTracker.Action.WAIT,tracker.action(1500))
        assertEquals(UnderlyingNetworkTracker.Action.RECONNECT,tracker.action(1800))
        tracker.connectedOn(2)
        assertEquals(UnderlyingNetworkTracker.Action.READY,tracker.action(1801))
    }
    @Test fun offlineDoesNotBecomeReadyAndAChangeDuringConnectIsNotForgotten() {
        val tracker=UnderlyingNetworkTracker()
        tracker.available(1,0);tracker.connectedOn(1)
        tracker.lost(1,1000)
        assertEquals(UnderlyingNetworkTracker.Action.OFFLINE,tracker.action(6000))
        tracker.available(2,6001)
        tracker.available(3,6200)
        tracker.connectedOn(2)
        assertEquals(UnderlyingNetworkTracker.Action.RECONNECT,tracker.action(7000))
        tracker.connectedOn(3)
        assertEquals(UnderlyingNetworkTracker.Action.READY,tracker.action(7001))
    }
}
