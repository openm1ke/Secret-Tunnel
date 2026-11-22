package dev.secrettunnel.android

import android.content.Intent
import android.os.SystemClock
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.TextView
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Explicitly supplied disposable SSH account. No credentials ship with the tests. */
@RunWith(AndroidJUnit4::class)
class SetupWizardTest {
    private val instrumentation get()=InstrumentationRegistry.getInstrumentation()
    private val context get()=instrumentation.targetContext
    private fun fixture():JSONObject {
        assumeTrue(InstrumentationRegistry.getArguments().getString("runWizardIntegration")=="true")
        assumeTrue(android.os.Build.MODEL.contains("sdk",ignoreCase=true))
        return JSONObject(File(context.filesDir,"wizard-fixture.json").readText())
    }
    private fun waitFor(reason:String,seconds:Long=45,ready:()->Boolean) {
        val deadline=SystemClock.elapsedRealtime()+seconds*1000
        while(SystemClock.elapsedRealtime()<deadline) {
            if(ready())return
            Thread.sleep(100)
        }
        fail(reason)
    }
    @Test fun passwordWizardHandlesErrorsAndInstallsAgent() {
        val values=fixture()
        val store=SettingsStore(context)
        val previous=store.activeProxyId
        val before=store.servers().map{it.id}.toSet()
        val activity=instrumentation.startActivitySync(Intent(context,SetupActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK))
        fun views(v:View):List<View> = listOf(v)+if(v is ViewGroup)(0 until v.childCount).flatMap{views(v.getChildAt(it))}else emptyList()
        fun ui(action:(List<View>)->Unit) = instrumentation.runOnMainSync { action(views(activity.window.decorView)) }
        fun click(caption:String) {
            waitFor("Button unavailable: $caption") {
                var ready=false
                ui { nodes->ready=nodes.filterIsInstance<Button>().any{it.text.toString()==caption&&it.isEnabled} }
                ready
            }
            ui { nodes -> assertTrue(nodes.filterIsInstance<Button>().first{it.text.toString()==caption&&it.isEnabled}.performClick()) }
        }
        fun contains(text:String):Boolean {
            var found=false
            ui { nodes->found=nodes.filterIsInstance<TextView>().any{it.text.contains(text)} }
            return found
        }
        fun fill(port:Int,password:String) = ui { nodes ->
            mapOf(2001 to values.getString("host"),2002 to port.toString(),2003 to values.getString("user"),2004 to password,2005 to "Ubuntu · мастер").forEach { (id,value)->
                nodes.filterIsInstance<EditText>().first{it.id==id}.setText(value)
            }
        }
        try {
            click("Да, сервер уже куплен")
            click("Логин и пароль от хостера")
            fill(1,"wrong")
            click("Проверить адрес")
            waitFor("No closed-port error"){contains("Сервер отклонил")}
            fill(values.getInt("port"),"wrong")
            click("Проверить адрес")
            waitFor("No fingerprint"){contains(values.getString("fingerprint"))}
            click("Доверять и настроить")
            waitFor("No password error"){contains("Сервер не принял")}
            click("Назад")
            fill(values.getInt("port"),values.getString("password"))
            click("Проверить адрес")
            waitFor("Retry failed to probe"){contains(values.getString("fingerprint"))}
            click("Доверять и настроить")
            waitFor("No result after key login"){contains("Подключение готово")||contains("Установить агент и проверить")}
            if(!contains("Подключение готово")) {
                click("Установить агент и проверить")
                waitFor("Agent installation failed",180){contains("Подключение готово")}
            }
            val added=store.servers().single{it.id !in before}
            val scoped=SettingsStore(context,added.id)
            assertTrue(scoped.hasKey())
            assertEquals(values.getString("fingerprint"),scoped.fingerprint)
            assertFalse(store.prefs.all.values.any{it is String&&(it.contains("PRIVATE KEY")||it.contains(values.getString("password")))})
            click("Готово")
        } finally {
            instrumentation.runOnMainSync{activity.finish()}
            if(store.proxies().any{it.id==previous})store.activeProxyId=previous
        }
    }
    @Test fun existingKeyRejectsChangedFingerprint() {
        val values=fixture()
        val session=SshSetupSession(context.applicationContext)
        session.hasKey=true
        session.host=values.getString("host");session.port=values.getInt("port").toString()
        session.user=values.getString("user");session.name="Ubuntu · готовый ключ"
        val original=File(context.filesDir,"wizard-existing.key").readBytes()
        session.importKey(original,"fixture")
        original.fill(0)
        session.probe()
        waitFor("Probe failed"){!session.busy}
        assertEquals(values.getString("fingerprint"),session.fingerprint)
        session.fingerprint="SHA256:"+"A".repeat(43)
        session.configure()
        waitFor("Pin rejection stalled"){!session.busy}
        assertFalse(session.saved)
        session.fingerprint=values.getString("fingerprint")
        session.configure()
        waitFor("Key login failed"){!session.busy}
        assertTrue(session.error,session.saved)
        if(!session.done) {
            session.configure(true)
            waitFor("Agent installation failed",180){!session.busy}
        }
        assertTrue(session.error,session.done)
        assertEquals("",session.password)
        assertNull(session.keyBytes)
        assertTrue(context.cacheDir.listFiles()?.none{it.name.startsWith("ssh-setup-")}?:true)
        session.clear()
    }
}
