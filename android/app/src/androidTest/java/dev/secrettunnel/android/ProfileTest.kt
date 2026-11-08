package dev.secrettunnel.android

import android.content.Context
import android.content.ContextWrapper
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.Assert.*
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class ProfileTest {
    @Test fun migrationAndProfilesKeepKeysPinsAndPortsSeparate(){
        val target=InstrumentationRegistry.getInstrumentation().targetContext
        val prefsName="profile-test-"+UUID.randomUUID()
        val isolated=object:ContextWrapper(target){
            override fun getSharedPreferences(name:String,mode:Int)=super.getSharedPreferences(prefsName,mode)
        }
        val prefs=isolated.getSharedPreferences("tunnel",Context.MODE_PRIVATE)
        try{
            val original=SettingsStore(isolated)
            val pair=JSONObject(NativeBridge.generateKey())
            val privateKey=pair.getString("private").toByteArray()
            original.importKey(privateKey)
            val cipher=prefs.getString("server.legacy.key_cipher",null)
            val iv=prefs.getString("server.legacy.key_iv",null)
            val pin="SHA256:"+"A".repeat(43)
            prefs.edit().clear().putString("host","old.example.com").putInt("port",2222)
                .putString("user","alice").putString("key_cipher",cipher).putString("key_iv",iv)
                .putString("fingerprint",pin).putStringSet("packages",setOf("dev.secrettunnel.probe")).commit()
            val migrated=SettingsStore(isolated)
            assertEquals("old.example.com",migrated.host);assertEquals(2222,migrated.port)
            assertEquals(pin,migrated.fingerprint);assertTrue(migrated.decryptKey().contentEquals(privateKey))
            assertFalse(prefs.contains("key_cipher"));assertEquals(1,migrated.packages.size)
            migrated.saveServer(ServerProfile("second","Второй","second.example.com",22,"bob"))
            val second=SettingsStore(isolated,"second")
            assertFalse(second.hasKey());assertEquals("",second.fingerprint)
            val other=JSONObject(NativeBridge.generateKey()).getString("private").toByteArray()
            second.importKey(other);second.fingerprint="SHA256:"+"B".repeat(43)
            assertTrue(migrated.decryptKey().contentEquals(privateKey));assertTrue(second.decryptKey().contentEquals(other))
            migrated.saveProxy(ProxyProfile("second-proxy","Другой порт","second",1081))
            migrated.activeProxyId="second-proxy"
            val running=migrated.snapshot()
            assertEquals(1081,running.socksPort);assertEquals("second",running.serverId)
            migrated.activeProxyId="local"
            assertEquals(1080,migrated.snapshot().socksPort);assertEquals(1081,running.socksPort)
            var rejected=false
            try{migrated.deleteServer("second")}catch(_:IllegalArgumentException){rejected=true}
            assertTrue("A referenced server must not disappear",rejected)
            migrated.saveServer(ServerProfile("second","Второй","changed.example.com",22,"bob"))
            assertEquals("",second.fingerprint);assertEquals(pin,migrated.fingerprint)
            migrated.deleteProxy("second-proxy");migrated.deleteServer("second")
            assertTrue(migrated.decryptKey().contentEquals(privateKey))
            assertFalse(prefs.all.values.any{it is String&&it.contains("PRIVATE KEY")})
            privateKey.fill(0);other.fill(0)
        }finally{target.deleteSharedPreferences(prefsName)}
    }
}
