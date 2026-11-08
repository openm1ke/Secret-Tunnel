package dev.secrettunnel.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import android.content.Intent
import android.view.View
import android.view.ViewGroup
import android.widget.Button

/** Explicit fixture for emulator integration; not included in the shipping APK. */
@RunWith(AndroidJUnit4::class)
class VpnFixtureTest {
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext

    @Test fun importVerifiedEmulatorFixture() {
        val fixture = File(context.filesDir, "integration-fixture.json")
        val key = File(context.filesDir, "bootstrap-test.key")
        assumeTrue("Push an isolated SSH fixture and bootstrap key with run-as", fixture.isFile && key.isFile)
        try {
            val values = JSONObject(fixture.readText())
            val host = values.getString("host"); val port = values.getInt("port")
            val pin = values.getString("fingerprint")
            val observed = JSONObject(NativeBridge.probe(host, port))
            assertFalse("SSH key probe failed", observed.has("error"))
            assertTrue("Probe must match independently trusted OpenSSH key", pin == observed.getString("fingerprint"))
            val store = SettingsStore(context)
            store.saveServer(ServerProfile("emulator-test", "SSH · эмулятор", host, port, values.getString("user")))
            val scoped = SettingsStore(context, "emulator-test")
            scoped.fingerprint = pin
            val bytes = key.readBytes()
            try { scoped.importKey(bytes) } finally { bytes.fill(0) }
            store.saveProxy(ProxyProfile("emulator-test", "SSH · локальный 1080", "emulator-test", 1080))
            store.activeProxyId = "emulator-test"
            check(store.prefs.edit().putStringSet("packages",setOf("dev.secrettunnel.probe")).commit())
            assertTrue(scoped.hasKey())
            assertEquals("emulator-test",store.snapshot().serverId)
            assertFalse(store.prefs.all.values.any { it is String && it.contains("PRIVATE KEY") })
        } finally { key.delete(); fixture.delete() }
    }

    @Test fun runVpnTrafficSession() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        assumeTrue("Explicit opt-in is required for a real VPN session", InstrumentationRegistry.getArguments().getString("runVpnIntegration") == "true")
        val ready = File(context.filesDir, "integration-ready.json")
        val finish = File(context.filesDir, "integration-finish")
        ready.delete(); finish.delete()
        SettingsStore(context).snapshot() // fail with an actionable fixture error
        val activity = instrumentation.startActivitySync(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)) as MainActivity
        fun views(root: View): List<View> = listOf(root) + if(root is ViewGroup) (0 until root.childCount).flatMap { views(root.getChildAt(it)) } else emptyList()
        fun snapshot() = JSONObject().put("active", TunnelVpnService.active).put("phase", TunnelVpnService.phase)
            .put("detail", TunnelVpnService.detail).put("native", JSONObject(NativeBridge.status()))
        try {
            instrumentation.runOnMainSync {
                views(activity.window.decorView).filterIsInstance<Button>().first { it.text.toString() == "Подключить" }.performClick()
            }
            // The host accepts the real Android VPN consent dialog through UI.
            val startDeadline = System.currentTimeMillis() + 90_000
            while (TunnelVpnService.phase != "Работает" && System.currentTimeMillis() < startDeadline) {
                check(!TunnelVpnService.phase.startsWith("Ошибка")) { TunnelVpnService.detail }
                Thread.sleep(200)
            }
            assertEquals("VPN failed to start", "Работает", TunnelVpnService.phase)
            val before = JSONObject(NativeBridge.status())
            ready.writeText(snapshot().toString())
            val deadline = System.currentTimeMillis() + 180_000
            while (!finish.isFile && System.currentTimeMillis() < deadline) Thread.sleep(200)
            assertTrue("Host did not complete the independent traffic probes", finish.isFile)
            val after = JSONObject(NativeBridge.status())
            assertTrue("SSH tunnel stopped during traffic check", after.optBoolean("running"))
            assertTrue("Selected TCP/UDP did not increase received bytes", after.getLong("rx") > before.getLong("rx"))
            assertTrue("Selected TCP/UDP did not increase sent bytes", after.getLong("tx") > before.getLong("tx"))
            File(context.filesDir, "integration-status.json").writeText(snapshot().put("before",before).toString())
        } finally {
            context.startService(Intent(context, TunnelVpnService::class.java).setAction("STOP"))
            val deadline = System.currentTimeMillis() + 15_000
            while (TunnelVpnService.active && System.currentTimeMillis() < deadline) Thread.sleep(100)
            File(context.filesDir, "integration-stop.json").writeText(snapshot().toString())
            instrumentation.runOnMainSync { activity.finish() }
            ready.delete(); finish.delete()
        }
    }
}
