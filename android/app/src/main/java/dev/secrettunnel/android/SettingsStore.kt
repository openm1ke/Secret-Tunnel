package dev.secrettunnel.android

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import java.security.KeyStore

data class ServerProfile(val id:String, val name:String, val host:String, val port:Int, val user:String,
    val jumpServerId:String?=null)
data class SshEndpoint(val profile: ServerProfile, val fingerprint: String)

internal fun resolveServerRoute(id: String, servers: List<ServerProfile>): List<ServerProfile> {
    val byId = servers.associateBy { it.id }
    val route = mutableListOf<ServerProfile>()
    val visited = mutableSetOf<String>()
    var next: String? = id
    while (next != null) {
        require(visited.add(next)) { "В цепочке SSH обнаружен цикл" }
        require(route.size < 4) { "В цепочке допустимо до трёх промежуточных серверов" }
        val server = byId[next] ?: error("Промежуточный SSH-сервер не найден")
        route.add(server)
        next = server.jumpServerId
    }
    return route.reversed()
}
data class UdpWindow(val kib: Int, val slots: Int) {
    init {
        require(kib in 64..1024) { "Размер окна UDP должен быть от 64 до 1024 КиБ" }
        require(slots in 4..192) { "Число слотов UDP должно быть от 4 до 192" }
    }

    fun toJson(): JSONObject = JSONObject().put("bytes", kib * 1024).put("slots", slots)

    companion object {
        fun fromJson(value: JSONObject): UdpWindow {
            val bytes = value.getInt("bytes")
            require(bytes % 1024 == 0) { "Размер окна UDP должен быть кратен 1 КиБ" }
            return UdpWindow(bytes / 1024, value.getInt("slots"))
        }
    }
}
data class SshSettings(val cipher:String="auto", val smallQueue:Boolean=false, val qos:String="balanced") {
    init {
        require(cipher in listOf("auto","aes128_gcm","chacha20_poly1305")) { "Неизвестный шифр SSH" }
        require(qos in listOf("balanced","voice","throughput")) { "Неизвестный профиль трафика" }
    }
    fun toJson():JSONObject=JSONObject().put("cipher",cipher).put("smallQueue",smallQueue).put("qos",qos)
    fun toNativeJson():JSONObject=JSONObject().put("cipher",cipher)
        .put("channel_buffer_messages",if(smallQueue)8 else 32)
        .put("channel_receive_window_bytes",if(smallQueue)1024*1024 else 2*1024*1024)
        .put("tcp_interactive_bytes", when (qos) {
            "voice" -> 64 * 1024
            "balanced" -> 96 * 1024
            else -> JSONObject.NULL
        })
    companion object {
        fun fromJson(value:JSONObject)=SshSettings(value.optString("cipher","auto"),value.optBoolean("smallQueue",false),value.optString("qos","balanced"))
    }
}

data class ProxyProfile(val id:String, val name:String, val serverId:String, val port:Int,
    val udpReceive:UdpWindow?=null, val udpAgentReceive:UdpWindow?=null, val ssh:SshSettings=SshSettings())
data class TunnelSession(val route:List<SshEndpoint>, val socksPort:Int, val packages:Set<String>,
    val udpReceive:UdpWindow?, val udpAgentReceive:UdpWindow?, val ssh:SshSettings)

class SettingsStore(private val context:Context, private val profileId:String?=null) {
    val prefs = context.getSharedPreferences("tunnel", Context.MODE_PRIVATE)
    init { migrate() }
    private fun migrate() = synchronized(SettingsStore::class.java) {
        if(!prefs.contains("profile_schema")) {
            val id="legacy"
            val editor=prefs.edit()
            for(field in listOf("host","user","fingerprint","key_iv","key_cipher"))
                if(prefs.contains(field)) editor.putString("server.$id.$field",prefs.getString(field,""))
            editor.putInt("server.$id.port",prefs.getInt("port",22))
                .putString("server.$id.name","Основной сервер")
                .putString("server_ids",org.json.JSONArray().put(id).toString())
                .putString("proxy_profiles",org.json.JSONArray().put(org.json.JSONObject()
                    .put("id","local").put("name","Основной прокси").put("serverId",id).put("port",1080)).toString())
                .putString("active_proxy","local").putInt("profile_schema",1)
            // Transfer encrypted blobs verbatim under the same Keystore master key.
            for(field in listOf("host","port","user","fingerprint","key_iv","key_cipher")) editor.remove(field)
            check(editor.commit()) {"Не удалось перенести настройки"}
        }
    }
    private fun ids():List<String> {val a=org.json.JSONArray(prefs.getString("server_ids","[]"));return (0 until a.length()).map{a.getString(it)}}
    private fun currentId()=profileId ?: activeProxy?.serverId ?: ids().firstOrNull().orEmpty()
    private fun scoped(field:String)="server.${currentId()}.$field"
    private fun value(field:String,default:String="")=prefs.getString(scoped(field),default)!!
    private fun put(field:String,value:String) {prefs.edit().putString(scoped(field),value).apply()}
    var host:String get()=value("host");set(v){put("host",v)}
    var port:Int get()=prefs.getInt(scoped("port"),22);set(v){prefs.edit().putInt(scoped("port"),v).apply()}
    var user:String get()=value("user");set(v){put("user",v)}
    var fingerprint:String get()=value("fingerprint");set(v){put("fingerprint",v)}
    var packages:Set<String> get()=prefs.getStringSet("packages",emptySet())!!.toSet();set(v){prefs.edit().putStringSet("packages",v.toSet()).apply()}
    var activeProxyId:String get()=prefs.getString("active_proxy","")!!;set(v){require(proxies().any{it.id==v});prefs.edit().putString("active_proxy",v).apply()}
    val activeProxy:ProxyProfile? get()=proxies().firstOrNull{it.id==prefs.getString("active_proxy","")}
    fun servers():List<ServerProfile> = ids().map { id ->
        val prefix="server.$id."
        ServerProfile(id,prefs.getString(prefix+"name","")!!,prefs.getString(prefix+"host","")!!,
            prefs.getInt(prefix+"port",22),prefs.getString(prefix+"user","")!!,
            prefs.getString(prefix+"jumpServerId",null))
    }
    fun server(id:String)=servers().firstOrNull{it.id==id}
    fun route(id: String): List<SshEndpoint> = resolveServerRoute(id, servers()).map {
        SshEndpoint(it, SettingsStore(context, it.id).fingerprint)
    }

    fun <T> withSshConfig(route: List<SshEndpoint>, includeTargetKey: Boolean = true, action: (JSONObject) -> T): T {
        val files = mutableListOf<File>()
        fun encode(endpoint: SshEndpoint, withKey: Boolean): JSONObject {
            val server = endpoint.profile
            val config = JSONObject().put("name", server.name).put("host", server.host).put("port", server.port)
                .put("user", server.user).put("host_key_sha256", endpoint.fingerprint)
            if (withKey) {
                require(endpoint.fingerprint.matches(Regex("SHA256:[A-Za-z0-9+/]{43}"))) {
                    "Подтвердите отпечаток сервера «${server.name}»"
                }
                val store = SettingsStore(context, server.id)
                require(store.hasKey()) { "Добавьте SSH-ключ для сервера «${server.name}»" }
                val file = File.createTempFile("ssh-route-", ".runtime", context.cacheDir)
                files.add(file)
                val bytes = store.decryptKey()
                try { file.writeBytes(bytes) } finally { bytes.fill(0) }
                config.put("key_path", file.absolutePath)
            }
            return config
        }
        try {
            val hops = JSONArray(route.dropLast(1).map { encode(it, true) })
            return action(encode(route.last(), includeTargetKey).put("ssh_jumps", hops))
        } finally {
            files.forEach { it.delete() }
        }
    }
    fun proxies():List<ProxyProfile> {
        val a=org.json.JSONArray(prefs.getString("proxy_profiles","[]"))
        return (0 until a.length()).map {
            val o = a.getJSONObject(it)
            ProxyProfile(o.getString("id"), o.getString("name"), o.getString("serverId"), o.getInt("port"),
                o.optJSONObject("udpReceive")?.let { UdpWindow.fromJson(it) },
                o.optJSONObject("udpAgentReceive")?.let { UdpWindow.fromJson(it) },
                o.optJSONObject("ssh")?.let { SshSettings.fromJson(it) }?:SshSettings())
        }
    }
    fun saveServer(profile:ServerProfile) {
        require(profile.name.isNotBlank()&&profile.name.length<=64){"Укажите название до 64 символов"}
        require(profile.host.isNotBlank()&&!Regex("[\\s/@?#]").containsMatchIn(profile.host)){"Укажите имя хоста или IP без схемы и пробелов"}
        require(profile.port in 1..65535){"SSH-порт должен быть от 1 до 65535"}
        require(profile.user.isNotBlank()&&!profile.user.any{it.isWhitespace()}){"Укажите SSH-пользователя без пробелов"}
        val profiles = servers().filterNot { it.id == profile.id } + profile
        profiles.forEach { resolveServerRoute(it.id, profiles) }
        val previous=server(profile.id);val prefix="server.${profile.id}."
        val editor=prefs.edit().putString(prefix+"name",profile.name).putString(prefix+"host",profile.host)
            .putInt(prefix+"port",profile.port).putString(prefix+"user",profile.user)
            .putString(prefix+"jumpServerId",profile.jumpServerId)
            .putString("server_ids",org.json.JSONArray((ids()+profile.id).distinct()).toString())
        if(previous==null||previous.host!=profile.host||previous.port!=profile.port) editor.remove(prefix+"fingerprint")
        check(editor.commit()) {"Не удалось сохранить сервер"}
    }
    private fun writeProxies(items:List<ProxyProfile>) {
        val a=org.json.JSONArray()
        items.forEach {
            a.put(JSONObject().put("id", it.id).put("name", it.name).put("serverId", it.serverId)
                .put("port", it.port).put("udpReceive", it.udpReceive?.toJson())
                .put("udpAgentReceive", it.udpAgentReceive?.toJson()).put("ssh",it.ssh.toJson()))
        }
        check(prefs.edit().putString("proxy_profiles",a.toString()).commit()) {"Не удалось сохранить прокси"}
    }
    fun saveProxy(profile:ProxyProfile) {
        require(profile.name.isNotBlank()&&profile.name.length<=64){"Укажите название до 64 символов"}
        require(profile.port in 1024..65535){"Локальный порт должен быть от 1024 до 65535"}
        require(server(profile.serverId)!=null){"Выберите существующий сервер"}
        writeProxies(proxies().filterNot{it.id==profile.id}+profile)
        if(activeProxy==null) activeProxyId=profile.id
    }
    fun deleteProxy(id:String) {
        val remaining=proxies().filterNot{it.id==id};writeProxies(remaining)
        if(activeProxy==null) prefs.edit().putString("active_proxy",remaining.firstOrNull()?.id.orEmpty()).apply()
    }
    fun deleteServer(id:String) {
        require(proxies().none{it.serverId==id}) {"Сервер используется в прокси. Сначала измените или удалите этот прокси."}
        require(servers().none { it.jumpServerId == id }) { "Сервер используется в цепочке SSH. Сначала измените её." }
        val editor=prefs.edit().putString("server_ids",org.json.JSONArray(ids().filterNot{it==id}).toString())
        prefs.all.keys.filter{it.startsWith("server.$id.")}.forEach{editor.remove(it)}
        check(editor.commit()) {"Не удалось удалить сервер"}
    }
    fun snapshot():TunnelSession {
        val proxy=activeProxy ?: error("Выберите прокси")
        val route = route(proxy.serverId)
        route.forEach { endpoint ->
            val server = endpoint.profile
            require(server.host.isNotBlank() && server.user.isNotBlank()) { "Настройте сервер «${server.name}»" }
            require(SettingsStore(context, server.id).hasKey()) { "Добавьте SSH-ключ для сервера «${server.name}»" }
        }
        require(packages.isNotEmpty()) {"Выберите приложения"}
        return TunnelSession(route,proxy.port,packages,
            proxy.udpReceive,proxy.udpAgentReceive,proxy.ssh)
    }
    fun hasKey()=prefs.contains(scoped("key_cipher"))
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
        prefs.edit().putString(scoped("key_iv"), Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
            .putString(scoped("key_cipher"), Base64.encodeToString(cipher.doFinal(bytes), Base64.NO_WRAP)).apply()
    }
    fun decryptKey(): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
            init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, Base64.decode(prefs.getString(scoped("key_iv"), ""), Base64.NO_WRAP)))
        }
        return cipher.doFinal(Base64.decode(prefs.getString(scoped("key_cipher"), ""), Base64.NO_WRAP))
    }
}
