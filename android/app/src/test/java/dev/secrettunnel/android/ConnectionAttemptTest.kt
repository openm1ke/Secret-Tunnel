package dev.secrettunnel.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ConnectionAttemptTest {
    @Test
    fun historicalCrashWithoutStartupMarkerStaysInJournal() {
        val failure = PreviousConnectionFailure(200L, "Сбой в нативной библиотеке", interruptedStart = false)
        assertNull(failure.consumeStartupWarning(tunnelActive = false, lastHandled = 0L) {
            error("An unrelated crash must not trigger a startup warning")
        })
    }

    @Test
    fun activeTunnelPreventsWarningNowAndAfterReopening() {
        val failure = PreviousConnectionFailure(200L, "Сбой при создании VPN", interruptedStart = true)
        var handled = 0L

        assertNull(failure.consumeStartupWarning(tunnelActive = true, lastHandled = handled) { handled = it })
        assertEquals(200L, handled)
        assertNull(failure.consumeStartupWarning(tunnelActive = false, lastHandled = handled) {
            error("The same warning must not return after stopping the tunnel")
        })
    }

    @Test
    fun interruptedStartupWarnsOnceWhenTunnelIsStopped() {
        val failure = PreviousConnectionFailure(200L, "Сбой при создании VPN", interruptedStart = true)
        var handled = 0L

        assertEquals(failure.message, failure.consumeStartupWarning(tunnelActive = false, lastHandled = handled) {
            handled = it
        })
        assertNull(failure.consumeStartupWarning(tunnelActive = false, lastHandled = handled) {
            error("The same failure must not be shown twice")
        })
    }

    @Test
    fun startupStageRequiresMatchingProcessAndChronology() {
        val marker = ConnectionAttemptMarker(42, 150L, "Создание VPN")
        assertEquals(marker.stage, marker.stageForExit(exitPid = 42, exitTime = 200L))
        assertNull(marker.stageForExit(exitPid = 43, exitTime = 200L))
        assertNull(marker.stageForExit(exitPid = 42, exitTime = 100L))
    }
}
