package dev.secrettunnel.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.json.JSONObject

@RunWith(AndroidJUnit4::class)
class ConnectionTest {
    private val context get()=InstrumentationRegistry.getInstrumentation().targetContext
    private val fixture get()=InstrumentationRegistry.getArguments()
    private fun requireServer(){assumeTrue("Supply sshHost, sshPort, sshUser and sshFingerprint to run the real-server checks",fixture.containsKey("sshHost")&&fixture.containsKey("sshFingerprint"))}
    @Test fun testProbeDoesNotNeedCredentials() {
        requireServer()
        val result=JSONObject(NativeBridge.probe(fixture.getString("sshHost")!!,fixture.getString("sshPort","22")!!.toInt()))
        assertFalse(result.toString(),result.has("error"))
        assertTrue(result.getString("fingerprint").matches(Regex("SHA256:[A-Za-z0-9+/]{43}")))
    }
    @Test fun testInvalidPortRejected() {
        assertTrue(JSONObject(NativeBridge.probe("localhost",0)).has("error"))
    }
    @Test fun testDeviceKeyEncryptedRoundTrip() {
        val pair=JSONObject(NativeBridge.generateKey())
        assertTrue(pair.toString(),pair.has("private"))
        assertTrue(pair.getString("public").startsWith("ssh-ed25519 "))
        val storage=SettingsStore(context)
        val old=if(storage.hasKey()) storage.decryptKey() else null
        val bytes=pair.getString("private").toByteArray()
        try {
            storage.importKey(bytes)
            assertTrue("Encrypted key round trip failed",pair.getString("private")==storage.decryptKey().toString(Charsets.UTF_8))
            assertFalse(storage.prefs.all.values.any{it is String&&it.contains("PRIVATE KEY")})
        } finally {
            if(old!=null) {storage.importKey(old);old.fill(0)}
            else {val prefix="server.${storage.activeProxy!!.serverId}.";storage.prefs.edit().remove(prefix+"key_iv").remove(prefix+"key_cipher").commit()}
            bytes.fill(0)
        }
    }

    @Test fun testRegistrationAndWrongPin() {
        requireServer()
        val bootstrap=java.io.File(context.filesDir,"bootstrap-test.key")
        assumeTrue("Install a dedicated temporary bootstrap key",bootstrap.isFile)
        val pair=JSONObject(NativeBridge.generateKey())
        val device=java.io.File(context.filesDir,"test-device.key")
        val public=pair.getString("public")
        java.io.File(context.filesDir,"test-device.pub").writeText(public)
        device.writeText(pair.getString("private"))
        try {
            val host=fixture.getString("sshHost")!!
            val port=fixture.getString("sshPort","22")!!.toInt()
            val user=fixture.getString("sshUser")?:error("Supply sshUser")
            val pin=JSONObject(NativeBridge.probe(host,port)).getString("fingerprint")
            assertEquals("Server fingerprint must match the independently verified fixture",fixture.getString("sshFingerprint"),pin)
            val cfg=JSONObject().put("host",host).put("port",port).put("user",user)
                .put("key_path",bootstrap.absolutePath).put("device_key_path",device.absolutePath)
                .put("socks_port",1080).put("host_key_sha256","SHA256:"+"A".repeat(43))
            assertTrue("Wrong pin was accepted",NativeBridge.registerKey(cfg.toString(),public).isNotEmpty())
            cfg.put("host_key_sha256",pin)
            assertEquals("",NativeBridge.registerKey(cfg.toString(),public))
            // Retry must not duplicate the authorized_keys entry.
            assertEquals("",NativeBridge.registerKey(cfg.toString(),public))
            val storage=SettingsStore(context)
            storage.host=host;storage.port=port;storage.user=user;storage.fingerprint=pin
            val bytes=device.readBytes()
            try {storage.importKey(bytes)} finally {bytes.fill(0)}
            storage.saveProxy(storage.activeProxy!!.copy(name="Проверка SSH · 1081",port=1081))
            storage.prefs.edit().putStringSet("packages",setOf("dev.secrettunnel.probe")).commit()
        } finally {device.delete()}
    }
}
