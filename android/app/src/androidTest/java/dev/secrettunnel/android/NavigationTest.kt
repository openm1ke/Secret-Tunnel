package dev.secrettunnel.android

import android.content.Intent
import android.view.View
import android.view.ViewGroup
import android.widget.*
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.Assert.*
import org.junit.Assume.assumeTrue

@RunWith(AndroidJUnit4::class)
class NavigationTest {
    @Test fun destinationsAndRecycledSwitchesPreserveSelection(){
        val instrumentation=InstrumentationRegistry.getInstrumentation()
        val context=instrumentation.targetContext
        val hasProbe=try{context.packageManager.getPackageInfo("dev.secrettunnel.probe",0);true}catch(_:Exception){false}
        assumeTrue("Install the isolated traffic probe for the UI integration test",hasProbe)
        val store=SettingsStore(context);val original=store.packages
        val activity=instrumentation.startActivitySync(Intent(context,MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) as MainActivity
        fun views(root:View):List<View> = listOf(root)+if(root is ViewGroup)(0 until root.childCount).flatMap{views(root.getChildAt(it))}else emptyList()
        fun all()=views(activity.window.decorView)
        fun tab(name:String){
            instrumentation.runOnMainSync {
                val destination = all().last { it.isClickable && it.contentDescription?.toString() == name }
                assertTrue(destination.performClick())
            }
            instrumentation.waitForIdleSync()
        }
        try{
            tab("Ещё")
            tab("Серверы")
            instrumentation.runOnMainSync{assertTrue(all().filterIsInstance<Button>().any{it.text.toString().contains("Добавить сервер")})}
            tab("Ещё")
            tab("Прокси")
            instrumentation.runOnMainSync{assertTrue(all().filterIsInstance<Button>().any{it.text.toString().contains("Добавить прокси")})}
            tab("Ещё")
            tab("Журнал")
            instrumentation.runOnMainSync{assertTrue(all().filterIsInstance<Button>().any{it.text.toString()=="Копировать журнал"})}
            tab("Приложения")
            instrumentation.runOnMainSync{all().filterIsInstance<EditText>().first().setText("dev.secrettunnel.probe")}
            var found=false
            for(i in 0..49){instrumentation.runOnMainSync{found=all().any{it is Switch}};if(found)break;Thread.sleep(100)}
            assertTrue("Probe should appear in installed application list",found)
            val expected="dev.secrettunnel.probe" !in original
            instrumentation.runOnMainSync{all().filterIsInstance<Switch>().first().performClick()}
            assertEquals(expected,"dev.secrettunnel.probe" in store.packages)
            instrumentation.runOnMainSync{all().filterIsInstance<EditText>().first().setText("no-such-application")}
            instrumentation.waitForIdleSync()
            instrumentation.runOnMainSync{all().filterIsInstance<EditText>().first().setText("dev.secrettunnel.probe")}
            instrumentation.waitForIdleSync()
            instrumentation.runOnMainSync{assertEquals(expected,all().filterIsInstance<Switch>().first().isChecked)}
            assertEquals(original-filterOfProbe(),store.packages-filterOfProbe())
        }finally{
            store.prefs.edit().putStringSet("packages",original).commit()
            instrumentation.runOnMainSync{activity.finish()}
        }
    }
    private fun filterOfProbe()=setOf("dev.secrettunnel.probe")

    @Test fun priorityUdpCheckboxPersistsOnlyForSelectedApps() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val probe = "dev.secrettunnel.probe"
        assumeTrue(android.os.Build.VERSION.SDK_INT >= 29)
        assumeTrue(runCatching { context.packageManager.getApplicationInfo(probe, 0) }.isSuccess)
        val store = SettingsStore(context)
        val packages = store.packages
        val priority = store.priorityUdpPackages
        val protection = store.protectedPackages
        val tuning = store.tuning
        store.packages = packages + probe
        store.priorityUdpPackages = priority - probe
        store.protectedPackages = protection - probe
        store.tuning = tuning.copy(ssh = tuning.ssh.copy(qos = "voice", priorityUdpSsh = true))
        val activity = instrumentation.startActivitySync(Intent(context, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)) as MainActivity
        fun views(root: View): List<View> =
            listOf(root) + if (root is ViewGroup) (0 until root.childCount).flatMap { views(root.getChildAt(it)) } else emptyList()
        fun all() = views(activity.window.decorView)
        fun option(caption: String) = all().filterIsInstance<CheckBox>()
            .first { it.isShown && it.text.toString() == caption }
        val udpOption = "UDP через второй SSH"
        val protectOption = "Блокировать при обрыве туннеля"
        try {
            instrumentation.runOnMainSync {
                all().last { it.isClickable && it.contentDescription?.toString() == "Приложения" }.performClick()
                all().filterIsInstance<EditText>().first().setText(probe)
            }
            var loaded = false
            repeat(50) {
                if (!loaded) {
                    instrumentation.runOnMainSync { loaded = all().any { it is CheckBox && it.isShown } }
                    if (!loaded) Thread.sleep(100)
                }
            }
            assertTrue(loaded)
            instrumentation.runOnMainSync {
                val checkbox = option(udpOption)
                assertFalse(checkbox.isChecked)
                checkbox.performClick()
                assertFalse(option(protectOption).isChecked)
                option(protectOption).performClick()
            }
            assertTrue(probe in store.priorityUdpPackages)
            assertTrue(probe in store.protectedPackages)
            instrumentation.runOnMainSync {
                all().filterIsInstance<Switch>().first().performClick()
                assertFalse(all().any { it is CheckBox && it.isShown })
            }
            assertFalse(probe in store.priorityUdpPackages)
        } finally {
            instrumentation.runOnMainSync { activity.finish() }
            store.packages = packages
            store.priorityUdpPackages = priority
            store.protectedPackages = protection
            store.tuning = tuning
        }
    }
}
