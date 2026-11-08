package dev.secrettunnel.android
import org.junit.Assert.*
import org.junit.Test
class TrafficMeterTest {
    @Test fun ratesUseElapsedTimeAndIgnoreResetsAndGaps(){
        val meter=TrafficMeter()
        assertEquals(0.0,meter.sample(1000,100,200).down,0.001)
        val next=meter.sample(1500,1124,712)
        assertEquals(2048.0,next.down,0.001);assertEquals(1024.0,next.up,0.001)
        assertEquals(0.0,meter.sample(2000,0,0).down,0.001)
        assertEquals(0.0,meter.sample(15000,90000,90000).down,0.001)
    }
    @Test fun historySurvivesNewViewsAndDiscardsOldSamples(){
        val history=TrafficHistory()
        history.sample(1000,0,0);history.sample(2500,3000,1500)
        val captured=history.snapshot(3000)
        assertEquals(2,captured.size);assertEquals(2000.0,captured.last().rate.down,0.001)
        history.sample(4000,6000,3000)
        assertEquals(2,captured.size)
        assertTrue(history.snapshot(65001).isEmpty())
    }
}
