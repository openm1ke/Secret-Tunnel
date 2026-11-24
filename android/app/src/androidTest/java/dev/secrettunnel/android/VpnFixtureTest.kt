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
import android.net.VpnService
import android.os.SystemClock
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.TextView

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
            val observed = JSONObject(NativeBridge.probe(JSONObject().put("host",host).put("port",port).toString()))
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
            assertEquals("emulator-test",store.snapshot().route.last().profile.id)
            assertFalse(store.prefs.all.values.any { it is String && it.contains("PRIVATE KEY") })
        } finally { key.delete(); fixture.delete() }
    }

    @Test fun checkSavedServerBeforeAndDuringVpn() {
        val instrumentation=InstrumentationRegistry.getInstrumentation()
        assumeTrue("Run against an isolated Android emulator", android.os.Build.MODEL.contains("sdk", ignoreCase=true))
        val store=SettingsStore(context)
        val server=store.server("emulator-test")
        assumeTrue("Install the verified SSH fixture first",server!=null)
        requireNotNull(server)
        assertEquals(server.id,store.snapshot().route.last().profile.id)
        assertNull("Grant VPN permission in the test emulator first",VpnService.prepare(context))

        val failed=ServerProfile("qa-refused","Закрытый SSH-порт","127.0.0.1",1,server.user)
        val source=SettingsStore(context,server.id)
        val fingerprint=source.fingerprint
        store.saveServer(failed)
        val bad=SettingsStore(context,failed.id)
        val key=source.decryptKey()
        try { bad.importKey(key) } finally { key.fill(0) }
        bad.fingerprint=fingerprint

        val activity=instrumentation.startActivitySync(Intent(context,MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK))
        fun views(root:View):List<View> = listOf(root) +
            if(root is ViewGroup)(0 until root.childCount).flatMap{views(root.getChildAt(it))}else emptyList()
        fun ui(action:(List<View>)->Unit) {
            instrumentation.runOnMainSync { action(views(activity.window.decorView)) }
        }
        fun tab(name:String) = ui { nodes ->
            assertTrue(nodes.last{it.isClickable&&it.contentDescription?.toString()==name}.performClick())
        }
        fun click(text:String) = ui { nodes ->
            assertTrue(nodes.filterIsInstance<Button>().first{it.text.toString()==text&&it.isEnabled}.performClick())
        }
        fun waitUntil(description:String, timeout:Long=45_000, ready:()->Boolean) {
            val deadline=SystemClock.elapsedRealtime()+timeout
            while(SystemClock.elapsedRealtime()<deadline) {
                if(ready())return
                Thread.sleep(100)
            }
            fail(description)
        }
        fun openServer(name:String) {
            tab("Ещё")
            tab("Серверы")
            ui { nodes ->
                val title=nodes.filterIsInstance<TextView>().first{it.text.toString()==name}
                assertTrue(views(title.parent as View).filterIsInstance<Button>()
                    .first{it.text.toString()=="Настроить"}.performClick())
            }
        }
        fun checkResult(expected:String) {
            waitUntil("No result or check button stayed disabled: $expected") {
                var ready=false
                ui { nodes ->
                    ready=nodes.filterIsInstance<TextView>().any{it.text.contains(expected)} &&
                        nodes.filterIsInstance<Button>().any{it.text.toString()=="Проверить SSH и агент"&&it.isEnabled}
                }
                ready
            }
        }
        try {
            openServer(server.name)
            click("Проверить SSH и агент")
            checkResult("Проверка STMUX003: OK.")
            tab("Главная")
            click("Подключить")
            waitUntil("VPN did not connect"){TunnelVpnService.phase=="Работает"}

            openServer(server.name)
            click("Проверить отпечаток")
            checkResult("Отпечаток сервера подтверждён.")
            click("Проверить SSH и агент")
            checkResult("Проверка STMUX003: OK.")

            openServer(failed.name)
            repeat(2) {
                click("Проверить SSH и агент")
                checkResult("Сервер отклонил TCP-соединение")
            }
            assertTrue(TunnelVpnService.active)
            assertEquals("Работает",TunnelVpnService.phase)
            assertTrue(JSONObject(NativeBridge.status()).optBoolean("running"))
            assertEquals(server,store.server(server.id))
            assertEquals(fingerprint,SettingsStore(context,server.id).fingerprint)
        } finally {
            if(TunnelVpnService.active) {
                context.startService(Intent(context,TunnelVpnService::class.java).setAction("STOP"))
                waitUntil("VPN did not stop",15_000){!TunnelVpnService.active}
            }
            store.deleteServer(failed.id)
            instrumentation.runOnMainSync { activity.finish() }
        }
    }

    @Test fun runVpnTrafficSession() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        assumeTrue("Explicit opt-in is required for a real VPN session", InstrumentationRegistry.getArguments().getString("runVpnIntegration") == "true")
        val ready = File(context.filesDir, "integration-ready.json")
        val finish = File(context.filesDir, "integration-finish")
        ready.delete(); finish.delete()
        val store = SettingsStore(context)
        val previousProtection = store.protectedPackages
        val previousPriority = store.priorityUdpPackages
        val previousTuning = store.tuning
        val protect = InstrumentationRegistry.getArguments().getString("protectProbe") == "true"
        val priority = InstrumentationRegistry.getArguments().getString("priorityProbe") == "true"
        assumeTrue(!priority || android.os.Build.VERSION.SDK_INT >= 29)
        store.snapshot() // fail with an actionable fixture error
        val activity = instrumentation.startActivitySync(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)) as MainActivity
        fun views(root: View): List<View> = listOf(root) + if(root is ViewGroup) (0 until root.childCount).flatMap { views(root.getChildAt(it)) } else emptyList()
        fun snapshot() = JSONObject().put("active", TunnelVpnService.active).put("phase", TunnelVpnService.phase)
            .put("detail", TunnelVpnService.detail).put("protected", TunnelVpnService.protectedCount)
            .put("native", JSONObject(NativeBridge.status()))
        try {
            if (protect) store.protectedPackages = store.packages
            if (priority) {
                store.priorityUdpPackages = store.packages
                store.tuning = previousTuning.copy(ssh = previousTuning.ssh.copy(qos = "voice", priorityUdpSsh = true))
            }
            instrumentation.runOnMainSync {
                views(activity.window.decorView).filterIsInstance<Button>().first { it.text.toString() == "Подключить" }.performClick()
            }
            // The host accepts the real Android VPN consent dialog through UI.
            val startDeadline = System.currentTimeMillis() + 90_000
            while (TunnelVpnService.phase != "Работает" && System.currentTimeMillis() < startDeadline) {
                if (!protect) check(!TunnelVpnService.phase.startsWith("Ошибка")) { TunnelVpnService.detail }
                if (protect) File(context.filesDir,"integration-live.json").writeText(snapshot().toString())
                Thread.sleep(200)
            }
            assertEquals("VPN failed to start", "Работает", TunnelVpnService.phase)
            if (protect) assertEquals(store.packages.size, TunnelVpnService.protectedCount)
            if (priority) assertTrue("Second SSH failed to start",
                JSONObject(NativeBridge.status()).optJSONObject("priority_udp_ssh")?.optBoolean("separate") == true)
            if(InstrumentationRegistry.getArguments().getString("injectEngineCrash")=="true"){
                val engine=TunnelVpnService()
                assertTrue(engine.TProxyIsRunning())
                engine.TProxyStopService()
                val recoveryDeadline=System.currentTimeMillis()+30000
                while(!engine.TProxyIsRunning()&&System.currentTimeMillis()<recoveryDeadline)Thread.sleep(100)
                assertTrue("Packet engine did not recover",engine.TProxyIsRunning())
                assertTrue("Recovery must retain the VPN session",TunnelVpnService.active)
            }
            val before = JSONObject(NativeBridge.status())
            ready.writeText(snapshot().toString())
            val sessionSeconds = InstrumentationRegistry.getArguments().getString("sessionSeconds")
                ?.toLongOrNull()?.coerceIn(30, 7200) ?: 180
            val deadline = System.currentTimeMillis() + sessionSeconds * 1000
            var nextSnapshot=0L
            while (!finish.isFile && System.currentTimeMillis() < deadline) {
                if(System.currentTimeMillis()>=nextSnapshot){File(context.filesDir,"integration-live.json").writeText(snapshot().toString());nextSnapshot=System.currentTimeMillis()+1000}
                Thread.sleep(200)
            }
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
            if (protect) store.protectedPackages = previousProtection
            if (priority) {
                store.priorityUdpPackages = previousPriority
                store.tuning = previousTuning
            }
            instrumentation.runOnMainSync { activity.finish() }
            ready.delete(); finish.delete()
        }
    }
}
