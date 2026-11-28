package dev.secrettunnel.android

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.graphics.Typeface
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.text.Editable
import android.text.InputType
import android.text.TextWatcher
import android.view.View
import android.widget.*
import androidx.core.view.WindowCompat
import org.json.JSONObject
import java.io.File
import java.util.UUID

internal enum class SetupStep { SERVER, ACCESS, DETAILS, TRUST, FINISH }

/** Retained only in memory. The worker never retains an Activity or writes a password. */
internal class SshSetupSession(private val context: Context) {
    var step = SetupStep.SERVER
    var hasServer = true
    var hasKey = false
    var name = ""
    var host = ""
    var port = "22"
    var user = "root"
    var password = ""
    var keyBytes: ByteArray? = null
    var keyLabel = ""
    var fingerprint = ""
    var saved = false
    var done = false
    @Volatile var busy = false
        private set
    var error = ""
        private set
    val log = mutableListOf<String>()
    var changed: (() -> Unit)? = null
    val id = "setup-${UUID.randomUUID()}"
    private val main = Handler(Looper.getMainLooper())
    private val store = SettingsStore(context)
    private val version = MobileUpdates.installedVersion(context)

    private fun notifyChange() { main.post { changed?.invoke() } }
    private fun progress(text: String) {
        synchronized(log) { if (log.size == 25) log.removeAt(0); log.add(text) }
        notifyChange()
    }
    private fun worker(action: () -> Unit) {
        if (busy) return
        busy = true
        error = ""
        notifyChange()
        Thread {
            try { NativeBridge.requireAvailable(); action() }
            catch (e: Exception) { error = ConnectionMessages.describe(e.message ?: tr("Не удалось завершить настройку")); progress(tr("Настройка остановлена. Исправьте данные и повторите попытку.")) }
            finally { busy = false; notifyChange() }
        }.start()
    }

    fun probe() = worker {
        require(host.isNotBlank() && user.isNotBlank() && (1..65535).contains(port.toIntOrNull() ?: 0)) { tr("Укажите адрес, SSH-порт и пользователя.") }
        require(if (hasKey) keyBytes != null else password.isNotEmpty()) { if (hasKey) tr("Выберите приватный SSH-ключ.") else tr("Введите пароль сервера.") }
        progress(tr("Подключаемся к SSH-серверу…"))
        val response = JSONObject(NativeBridge.probe(JSONObject().put("host",host.trim()).put("port",port.toInt()).toString()))
        require(!response.has("error")) { response.optString("error") }
        fingerprint = response.getString("fingerprint")
        require(isTrustedSshPin(fingerprint)) { tr("Сервер вернул некорректный отпечаток.") }
        progress(tr("Сервер ответил. Получен отпечаток ключа."))
        step = SetupStep.TRUST
    }

    fun importKey(bytes: ByteArray, label: String) {
        require(bytes.size in 1..1_048_576 && bytes.toString(Charsets.UTF_8).contains("PRIVATE KEY")) { tr("Выберите приватный SSH-ключ, файл без .pub.") }
        keyBytes?.fill(0)
        keyBytes = bytes.copyOf()
        keyLabel = label
        notifyChange()
    }

    fun configure(install: Boolean = false) = worker {
        step = SetupStep.FINISH
        if (!saved) {
            if (!hasKey && keyBytes == null) {
                progress(tr("Создаём отдельный ключ приложения…"))
                val pair = JSONObject(NativeBridge.generateKey())
                require(!pair.has("error")) { pair.optString("error") }
                keyBytes = pair.getString("private").toByteArray(Charsets.UTF_8)
            }
            require(isTrustedSshPin(fingerprint)) { tr("Подтвердите отпечаток сервера.") }
            val temporary = File.createTempFile("ssh-setup-", ".runtime", context.cacheDir)
            try {
                temporary.writeBytes(requireNotNull(keyBytes))
                val config = JSONObject().put("host",host.trim()).put("port",port.toInt())
                    .put("name",name.ifBlank { tr("Мой сервер") }).put("user",user.trim())
                    .put("host_key_sha256",fingerprint).put("key_path",temporary.path)
                if (!hasKey) {
                    progress(tr("Входим по паролю и добавляем публичный ключ…"))
                    val error = NativeBridge.enrollPassword(config.toString(),password)
                    require(error.isEmpty()) { error }
                    progress(tr("Повторный вход по новому ключу прошёл."))
                } else {
                    progress(tr("Проверяем вход по выбранному ключу…"))
                    val result = JSONObject(NativeBridge.checkServer(config.put("check_agent",false).toString()))
                    require(result.optBoolean("ok")) { result.optString("error") }
                }
                progress(tr("Сохраняем подключение…"))
                val scoped = SettingsStore(context,id)
                scoped.importKey(requireNotNull(keyBytes))
                store.saveServer(ServerProfile(id,name.trim().ifBlank { tr("Мой сервер") },host.trim(),port.toInt(),user.trim()))
                scoped.fingerprint = fingerprint
                store.saveProxy(ProxyProfile(id,tr("Мой прокси"),id,1080))
                if (!TunnelVpnService.active) store.activeProxyId = id
                saved = true
                password = ""
                keyBytes?.fill(0)
                keyBytes = null
            } finally { temporary.delete() }
        }
        val route = store.route(id)
        if (install) {
            progress(tr("Скачиваем агент и сверяем SHA-256…"))
            val proxy = MobileUpdates.proxy()
            val release = ReleaseUpdates.exact(version,proxy)
            val asset = release.agent ?: error(tr("В релизе нет агента для Linux x86_64."))
            val archive = File.createTempFile("ssh-setup-agent-", ".tar.gz",context.cacheDir)
            try {
                ReleaseUpdates.download(release,asset,archive,proxy)
                progress(tr("Устанавливаем агент в домашнюю папку пользователя…"))
                store.withSshConfig(route) { config ->
                    val result = JSONObject(NativeBridge.manageAgent(config.toString(),version,archive.path))
                    require(!result.has("error")) { result.optString("error") }
                }
            } finally { archive.delete() }
        }
        progress(tr("Проверяем серверный агент…"))
        store.withSshConfig(route) { config ->
            val result = JSONObject(NativeBridge.checkServer(config.put("check_agent",true).toString()))
            require(result.optBoolean("ok")) { result.optString("error") }
        }
        progress(tr("Подключение проверено и сохранено."))
        done = true
    }

    fun back() {
        if (busy || saved) return
        error = ""
        step = when(step) {
            SetupStep.SERVER -> SetupStep.SERVER
            SetupStep.ACCESS -> SetupStep.SERVER
            SetupStep.DETAILS -> SetupStep.ACCESS
            SetupStep.TRUST, SetupStep.FINISH -> SetupStep.DETAILS
        }
        fingerprint = ""
        notifyChange()
    }
    fun clear() { password = ""; keyBytes?.fill(0); keyBytes = null }
}

class SetupActivity : LocalizedActivity() {
    private lateinit var session: SshSetupSession
    private lateinit var palette: Palette
    private lateinit var body: LinearLayout
    private val density get() = resources.displayMetrics.density
    private fun dp(value: Int) = (value * density).toInt()
    override fun onCreate(state: Bundle?) {
        val dark=SettingsStore(this).appearance.isDark(resources.configuration)
        setTheme(if(dark)R.style.Theme_SecretTunnel_Dark else R.style.Theme_SecretTunnel_Light)
        super.onCreate(state)
        session = lastNonConfigurationInstance as? SshSetupSession ?: SshSetupSession(applicationContext)
        if (lastNonConfigurationInstance == null && intent.getBooleanExtra("server_purchased", false)) session.step = SetupStep.ACCESS
        palette = if (dark) Palette.dark else Palette.light
        window.statusBarColor = palette.background
        window.navigationBarColor = palette.background
        WindowCompat.getInsetsController(window,window.decorView).apply {
            isAppearanceLightStatusBars=!dark
            isAppearanceLightNavigationBars=!dark
        }
        session.changed = { if (!isFinishing && !isDestroyed) render() }
        render()
    }
    @Deprecated("Retained setup session")
    override fun onRetainNonConfigurationInstance(): Any = session
    override fun onDestroy() {
        session.changed = null
        if (isFinishing) session.clear()
        super.onDestroy()
    }
    @Deprecated("Platform back callback")
    override fun onBackPressed() {
        if (session.busy) return
        if (session.step == SetupStep.SERVER || session.saved) finish() else session.back()
    }
    private fun render() {
        val scroll = ScrollView(this).apply { isFillViewport = true; setBackgroundColor(palette.background) }
        body = LinearLayout(this).apply { orientation=LinearLayout.VERTICAL; setPadding(dp(20),dp(24),dp(20),dp(24)) }
        scroll.addView(body)
        text(tr("Настройка подключения"),24,true)
        when(session.step) {
            SetupStep.SERVER -> {
                text(tr("У вас уже есть сервер?"),20,true)
                text(tr("Для подключения нужен ваш VPS с доступом по SSH."))
                button(tr("Да, сервер уже куплен"),true) { session.step=SetupStep.ACCESS; render() }
                button(tr("Пока нет сервера")) { startActivityForResult(Intent(this, HostingCatalogActivity::class.java).putExtra("from_wizard", true), 209) }
                if (!session.hasServer) text(tr("Выберите VPS с Linux x86_64 у своего хостера. После покупки понадобятся адрес, порт SSH, логин и пароль или готовый ключ."))
            }
            SetupStep.ACCESS -> {
                text(tr("Как вы входите на сервер?"),20,true)
                button(tr("Логин и пароль от хостера"),true) { session.clear(); session.hasKey=false; session.step=SetupStep.DETAILS; render() }
                button(tr("У меня есть SSH-ключ")) { session.clear(); session.keyLabel=""; session.hasKey=true; session.step=SetupStep.DETAILS; render() }
            }
            SetupStep.DETAILS -> {
                text(tr("Данные из письма хостера"),20,true)
                field(tr("Адрес сервера"),session.host,2001) { session.host=it; session.fingerprint="" }
                field(tr("Порт SSH"),session.port,2002,InputType.TYPE_CLASS_NUMBER) { session.port=it; session.fingerprint="" }
                field(tr("Пользователь"),session.user,2003) { session.user=it }
                if (session.hasKey) {
                    text(if(session.keyLabel.isBlank())tr("Выберите приватный SSH-ключ, файл без .pub.") else session.keyLabel)
                    button(tr("Выбрать файл")) {
                        startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).setType("*/*").addCategory(Intent.CATEGORY_OPENABLE),106)
                    }
                } else {
                    field(tr("Пароль сервера"),session.password,2004,InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD) { session.password=it }
                    text(tr("Приложение создаст отдельный ключ и проверит вход по нему."))
                }
                field(tr("Название сервера · необязательно"),session.name,2005) { session.name=it }
                button(tr("Проверить адрес"),true) { session.probe() }
            }
            SetupStep.TRUST -> {
                text(tr("Подтвердите сервер"),20,true)
                text(tr("Сверьте отпечаток ключа SSH в панели хостера. Он помогает убедиться, что вы подключаетесь к своему серверу."))
                text(session.fingerprint).apply { setTextIsSelectable(true); typeface=Typeface.MONOSPACE }
                text(if(session.hasKey)tr("Проверим вход по ключу и серверный агент.") else tr("После подтверждения добавим публичный ключ приложения и проверим вход по нему."))
                button(tr("Доверять и настроить"),true) { session.configure() }
            }
            SetupStep.FINISH -> {
                text(if(session.done)tr("Подключение готово") else if(session.busy)tr("Настраиваем подключение") else tr("Настройка не завершена"),20,true)
                if(session.done) {
                    text(tr("Ключ и сервер проверены. Выберите приложения для туннеля на главной."))
                    button(tr("Готово"),true) { setResult(RESULT_OK); finish() }
                } else if(session.saved && !session.busy) {
                    text(tr("Доступ по SSH сохранён. Агент передаёт TCP и UDP через туннель. Установка добавит его в домашнюю папку пользователя."))
                    button(tr("Установить агент и проверить"),true) { session.configure(true) }
                    button(tr("Повторить проверку")) { session.configure() }
                }
            }
        }
        if(session.error.isNotEmpty()) text(tr(session.error)).apply { setTextColor(palette.error); accessibilityLiveRegion=View.ACCESSIBILITY_LIVE_REGION_POLITE }
        if(session.log.isNotEmpty()) {
            text(tr("Ход настройки"),14,true)
            val lines=synchronized(session.log){session.log.joinToString("\n")}
            val log=ScrollView(this).apply { setBackgroundColor(palette.surface); setPadding(dp(12),dp(12),dp(12),dp(12)) }
            log.addView(TextView(this).apply { text=lines; textSize=12f; setTextColor(palette.muted); accessibilityLiveRegion=View.ACCESSIBILITY_LIVE_REGION_POLITE })
            body.addView(log,LinearLayout.LayoutParams(-1,dp(136)).apply { topMargin=dp(12) })
        }
        if(session.step!=SetupStep.SERVER && !session.done && !session.saved) button(tr("Назад")) { session.back() }
        if(!session.done) button(tr("Закрыть мастер")) { finish() }
        setContentView(scroll)
    }
    private fun text(value:String,size:Int=14,bold:Boolean=false): TextView = TextView(this).apply {
        text=value; textSize=size.toFloat(); setTextColor(if(bold)palette.text else palette.muted)
        if(bold)typeface=Typeface.create("sans-serif-medium",Typeface.NORMAL)
        body.addView(this,LinearLayout.LayoutParams(-1,-2).apply { topMargin=dp(12) })
    }
    private fun button(caption:String,primary:Boolean=false,action:()->Unit) {
        body.addView(Button(this).apply {
            text=caption; isAllCaps=false; textSize=14f; minimumHeight=dp(50)
            background=palette.panel(if(primary)palette.accent else palette.raised,dp(14).toFloat())
            setTextColor(if(primary)palette.onAccent else palette.accent)
            isEnabled=!session.busy; alpha=if(isEnabled)1f else .45f
            setOnClickListener { action() }
        },LinearLayout.LayoutParams(-1,-2).apply { topMargin=dp(16) })
    }
    private fun field(caption:String,value:String,fieldId:Int,type:Int=InputType.TYPE_CLASS_TEXT,change:(String)->Unit) {
        text(caption,12,true)
        body.addView(EditText(this).apply {
            id=fieldId; inputType=type; setSingleLine(); setText(value); textSize=16f
            setTextColor(palette.text); setHintTextColor(palette.muted)
            background=palette.panel(palette.surface,dp(12).toFloat(),palette.border)
            setPadding(dp(12),0,dp(12),0); isEnabled=!session.busy
            isSaveEnabled=false; importantForAutofill=View.IMPORTANT_FOR_AUTOFILL_NO
            addTextChangedListener(object:TextWatcher {
                override fun beforeTextChanged(s:CharSequence?,start:Int,count:Int,after:Int) = Unit
                override fun onTextChanged(s:CharSequence?,start:Int,before:Int,count:Int) { change(s.toString()) }
                override fun afterTextChanged(s:Editable?) = Unit
            })
        },LinearLayout.LayoutParams(-1,dp(50)).apply { topMargin=dp(8) })
    }
    @Deprecated("Platform activity result")
    override fun onActivityResult(request:Int,result:Int,data:Intent?) {
        super.onActivityResult(request,result,data)
        if (request == 209 && result == RESULT_OK) { session.step = SetupStep.ACCESS; render(); return }
        if(request!=106 || result!=RESULT_OK)return
        val uri=data?.data ?: return
        try {
            val bytes=contentResolver.openInputStream(uri)?.use { it.readBytesLimited() } ?: error(tr("Не удалось открыть ключ."))
            try { session.importKey(bytes,tr("Ключ выбран")) } finally { bytes.fill(0) }
        } catch(e:Exception) { Toast.makeText(this,e.message,Toast.LENGTH_LONG).show() }
    }
    private fun java.io.InputStream.readBytesLimited():ByteArray {
        val out=java.io.ByteArrayOutputStream()
        val buffer=ByteArray(8192)
        while(out.size()<=1_048_576) { val count=read(buffer); if(count<0)break; out.write(buffer,0,count) }
        return out.toByteArray()
    }
}
