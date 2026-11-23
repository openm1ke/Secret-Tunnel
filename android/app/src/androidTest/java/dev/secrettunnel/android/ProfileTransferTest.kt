package dev.secrettunnel.android

import android.app.Activity
import android.content.Intent
import android.graphics.BitmapFactory
import android.os.SystemClock
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.TextView
import androidx.core.content.FileProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.google.zxing.BinaryBitmap
import com.google.zxing.RGBLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class ProfileTransferTest {
    private val instrumentation get()=InstrumentationRegistry.getInstrumentation()
    private val context get()=instrumentation.targetContext
    private fun fixture():JSONObject {
        assumeTrue(InstrumentationRegistry.getArguments().getString("runProfileTransfer")=="true")
        assumeTrue(android.os.Build.MODEL.contains("sdk",ignoreCase=true))
        return JSONObject(File(context.filesDir,"profile-transfer-fixture.json").readText())
    }
    private fun waitFor(reason:String,condition:()->Boolean){
        val deadline=SystemClock.elapsedRealtime()+30_000
        while(SystemClock.elapsedRealtime()<deadline){if(condition())return;Thread.sleep(50)}
        fail(reason)
    }
    @Test fun desktopQrDecodesToTheEncryptedLink(){
        val values=fixture()
        val bitmap=BitmapFactory.decodeFile(File(context.filesDir,"profile-transfer-qr.png").path)
        assertNotNull(bitmap)
        val pixels=IntArray(bitmap.width*bitmap.height);bitmap.getPixels(pixels,0,bitmap.width,0,0,bitmap.width,bitmap.height)
        val decoded=QRCodeReader().decode(BinaryBitmap(HybridBinarizer(RGBLuminanceSource(bitmap.width,bitmap.height,pixels))))
        assertEquals(values.getString("link"),decoded.text)
        val preview=JSONObject(NativeBridge.transferUnlock(decoded.text,values.getString("code"),true))
        assertFalse(preview.has("error"));assertFalse(preview.toString().contains("private_key"))
        bitmap.recycle()
    }
    @Test fun encryptedFileImportsAtomicallyAfterConfirmation(){
        val values=fixture();val store=SettingsStore(context)
        val before=store.servers().associateBy{it.id};val previous=store.activeProxyId
        val activity=instrumentation.startActivitySync(Intent(context,ProfileTransferActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK))
        fun views(v:View):List<View> = listOf(v)+if(v is ViewGroup)(0 until v.childCount).flatMap{views(v.getChildAt(it))}else emptyList()
        fun ui(action:(List<View>)->Unit)=instrumentation.runOnMainSync{action(views(activity.window.decorView))}
        fun contains(text:String):Boolean{var found=false;ui{found=it.filterIsInstance<TextView>().any{v->v.text.contains(text)}};return found}
        fun click(caption:String){
            waitFor("Unavailable button $caption"){var ready=false;ui{ready=it.filterIsInstance<Button>().any{v->v.text.toString()==caption&&v.isEnabled}};ready}
            ui{assertTrue(it.filterIsInstance<Button>().first{v->v.text.toString()==caption&&v.isEnabled}.performClick())}
        }
        fun enterCode(value:String)=ui{nodes->nodes.filterIsInstance<EditText>().first{it.id==2102}.setText(value)}
        try{
            val directory=File(context.cacheDir,"profile-transfers").apply{mkdirs()}
            val file=File(directory,"desktop-test.stprofile").apply{writeText(values.getString("link"))}
            assertFalse(file.readText().contains("PRIVATE KEY"))
            val uri=FileProvider.getUriForFile(context,"${context.packageName}.updates",file)
            val callback=ProfileTransferActivity::class.java.getDeclaredMethod("onActivityResult",Int::class.javaPrimitiveType,Int::class.javaPrimitiveType,Intent::class.java).apply{isAccessible=true}
            instrumentation.runOnMainSync{callback.invoke(activity,201,Activity.RESULT_OK,Intent().setData(uri))}
            waitFor("File was not loaded"){var ready=false;ui{ready=it.filterIsInstance<EditText>().any{v->v.id==2101&&v.text.toString()==values.getString("link")}};ready}
            var incorrect=values.getString("code");incorrect=(if(incorrect[0]=='A')"B" else "A")+incorrect.substring(1)
            enterCode(incorrect);click("Расшифровать и проверить")
            waitFor("No wrong-code error"){contains("Неверный код")}
            assertEquals(before.keys,store.servers().map{it.id}.toSet())
            enterCode(values.getString("code"));val started=SystemClock.elapsedRealtime();click("Расшифровать и проверить")
            waitFor("No decrypted preview"){contains("Будут добавлены серверы")}
            println("Profile transfer preview_ms=${SystemClock.elapsedRealtime()-started}")
            ui{nodes->
                val heading=nodes.filterIsInstance<TextView>().first{it.text.toString()=="Будут добавлены серверы"}
                val position=IntArray(2);heading.getLocationInWindow(position)
                assertTrue("Preview is clipped horizontally",position[0]>=0&&position[0]+heading.width<=activity.window.decorView.width)
            }
            instrumentation.runOnMainSync{activity.window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_SECURE)}
            instrumentation.waitForIdleSync()
            instrumentation.uiAutomation.takeScreenshot()?.let{image->
                File(context.filesDir,"qa-profile-transfer-preview.png").outputStream().use{image.compress(android.graphics.Bitmap.CompressFormat.PNG,100,it)}
                image.recycle()
            }
            instrumentation.runOnMainSync{activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_SECURE)}
            assertEquals(before.keys,store.servers().map{it.id}.toSet())
            click("Добавить серверы");waitFor("Import failed"){contains("Серверы добавлены")}
            val added=store.servers().filter{it.id !in before}
            assertEquals(2,added.size)
            assertTrue(before.all{(id,profile)->store.server(id)==profile})
            val destination=added.single{it.name=="Demo FR"};val route=store.route(destination.id)
            assertEquals(listOf("Demo RU","Demo FR"),route.map{it.profile.name})
            assertTrue(route.all{isTrustedSshPin(it.fingerprint)})
            val key=SettingsStore(context,destination.id).decryptKey()
            try{assertEquals(values.getString("private_key"),key.toString(Charsets.UTF_8))}finally{key.fill(0)}
            assertFalse(store.prefs.all.values.any{it is String&&it.contains("PRIVATE KEY-----")})
            val exported=JSONObject(NativeBridge.transferExport(store.transferDocument(listOf(destination.id)).toString()))
            assertFalse(exported.has("error"))
            val back=JSONObject(NativeBridge.transferUnlock(exported.getString("link"),exported.getString("code"),true))
            assertEquals(2,back.getJSONArray("servers").length())
            click("Готово")
        }finally{
            instrumentation.runOnMainSync{activity.finish()}
            if(store.proxies().any{it.id==previous})store.activeProxyId=previous
        }
    }
}
