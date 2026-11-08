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
        fun tab(name:String){instrumentation.runOnMainSync{val text=all().filterIsInstance<TextView>().last{it.text.toString()==name};assertTrue((text.parent as View).performClick())};instrumentation.waitForIdleSync()}
        try{
            tab("Серверы")
            instrumentation.runOnMainSync{assertTrue(all().filterIsInstance<Button>().any{it.text.toString().contains("Добавить сервер")})}
            tab("Прокси")
            instrumentation.runOnMainSync{assertTrue(all().filterIsInstance<Button>().any{it.text.toString().contains("Добавить прокси")})}
            tab("Журнал")
            instrumentation.runOnMainSync{assertTrue(all().filterIsInstance<Button>().any{it.text.toString()=="Копировать журнал"})}
            tab("Прилож.")
            instrumentation.runOnMainSync{all().filterIsInstance<EditText>().first().setText("Tunnel Traffic Probe")}
            var found=false
            for(i in 0..49){instrumentation.runOnMainSync{found=all().any{it is Switch}};if(found)break;Thread.sleep(100)}
            assertTrue("Probe should appear in installed application list",found)
            val expected="dev.secrettunnel.probe" !in original
            instrumentation.runOnMainSync{all().filterIsInstance<Switch>().first().performClick()}
            assertEquals(expected,"dev.secrettunnel.probe" in store.packages)
            instrumentation.runOnMainSync{all().filterIsInstance<EditText>().first().setText("no-such-application")}
            instrumentation.waitForIdleSync()
            instrumentation.runOnMainSync{all().filterIsInstance<EditText>().first().setText("Tunnel Traffic Probe")}
            instrumentation.waitForIdleSync()
            instrumentation.runOnMainSync{assertEquals(expected,all().filterIsInstance<Switch>().first().isChecked)}
            assertEquals(original-filterOfProbe(),store.packages-filterOfProbe())
        }finally{
            store.prefs.edit().putStringSet("packages",original).commit()
            instrumentation.runOnMainSync{activity.finish()}
        }
    }
    private fun filterOfProbe()=setOf("dev.secrettunnel.probe")
}
