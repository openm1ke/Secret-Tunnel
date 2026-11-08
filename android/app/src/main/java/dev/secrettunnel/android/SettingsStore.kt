package dev.secrettunnel.android

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import java.security.KeyStore

class SettingsStore(context: Context) {
    val prefs = context.getSharedPreferences("tunnel", Context.MODE_PRIVATE)
    var host: String get() = prefs.getString("host", "")!!; set(v) { prefs.edit().putString("host", v).apply() }
    var port: Int get() = prefs.getInt("port", 22); set(v) { prefs.edit().putInt("port", v).apply() }
    var user: String get() = prefs.getString("user", "")!!; set(v) { prefs.edit().putString("user", v).apply() }
    var fingerprint: String get() = prefs.getString("fingerprint", "")!!; set(v) { prefs.edit().putString("fingerprint", v).apply() }
    var packages: Set<String> get() = prefs.getStringSet("packages", emptySet())!!.toSet(); set(v) { prefs.edit().putStringSet("packages", v.toSet()).apply() }
    fun hasKey() = prefs.contains("key_cipher")
    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        return (store.getKey("secret-tunnel-key", null) as? SecretKey) ?: KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(KeyGenParameterSpec.Builder("secret-tunnel-key", KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
            generateKey()
        }
    }
    fun importKey(bytes: ByteArray) {
        require(bytes.size in 1..1_048_576) { "Ключ слишком большой или пустой" }
        val text = bytes.toString(Charsets.UTF_8)
        require(text.contains("PRIVATE KEY-----")) { "Ожидался приватный SSH-ключ в формате OpenSSH/PEM" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key()) }
        prefs.edit().putString("key_iv", Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
            .putString("key_cipher", Base64.encodeToString(cipher.doFinal(bytes), Base64.NO_WRAP)).apply()
    }
    fun decryptKey(): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
            init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, Base64.decode(prefs.getString("key_iv", ""), Base64.NO_WRAP)))
        }
        return cipher.doFinal(Base64.decode(prefs.getString("key_cipher", ""), Base64.NO_WRAP))
    }
}
