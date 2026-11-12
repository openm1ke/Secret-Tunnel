package dev.secrettunnel.android

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class HistoryFormatTest {
    @Test fun speedRunUsesDesktopFieldsAndSurvivesARoundTrip() {
        val results = SpeedResults().apply { pingMs = 41.5; downloadMbps = 87.25; uploadMbps = 22.0; bytesDown = 1234; downloadSource = "Hetzner" }
        val run = SpeedRun("1-tunnel-quick", "quick", "tunnel", "203.0.113.10", "FR", "CDG", "Cloudflare · CDG", 1, 2,
            "France", listOf("RU", "France"), "FR", results, "", false)
        val json = run.toJson()
        // The desktop's speedtests.json reads these names.
        assertEquals("203.0.113.10", json.getString("exit_ip"))
        assertEquals(87.25, json.getJSONObject("results").getDouble("download_mbps"), 0.0)
        assertTrue(json.getJSONObject("results").isNull("jitter_ms"))
        val back = SpeedRun.fromJson(JSONObject(json.toString()))
        assertEquals(listOf("RU", "France"), back.route)
        assertEquals(41.5, back.results.pingMs!!, 0.0)
        assertNull(back.results.jitterMs)
        assertEquals("Hetzner", back.results.downloadSource)
    }

    @Test fun desktopSpeedRunReadsOnThePhone() {
        val desktop = """{"id":"9-direct-full","kind":"full","mode":"direct","started_at":9,"finished_at":60,"server":"","route":[],
            "country":"","results":{"ping_ms":12.0,"download_single_mbps":300.5,"bytes_down":5,"bytes_up":6},"error":"","cancelled":false}"""
        val run = SpeedRun.fromJson(JSONObject(desktop))
        assertTrue(run.direct)
        assertEquals(300.5, run.results.downloadSingleMbps!!, 0.0)
        assertNull(run.results.uploadMbps)
    }

    @Test fun sessionRecordRoundTrip() {
        val record = SessionRecord("7", 7, 70, "France", listOf("RU", "France"), "FR", 100, 50,
            listOf(AppUsage("Telegram", 80, 40)), true)
        val back = SessionRecord.fromJson(JSONObject(record.toJson().toString()))
        assertEquals(record, back)
    }

    @Test fun tuningKeepsUdpWindowsAndSshChoices() {
        val tuning = Tuning(SshSettings("chacha20_poly1305", true, "voice"), UdpWindow(1024, 512), UdpWindow(256, 192))
        assertEquals(tuning, Tuning.fromJson(JSONObject(tuning.toJson().toString())))
        assertEquals(Tuning(), Tuning.fromJson(JSONObject("{}")))
    }
}
