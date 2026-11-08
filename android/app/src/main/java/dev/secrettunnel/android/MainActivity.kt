package dev.secrettunnel.android

import android.app.*
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.*
import android.net.VpnService
import android.os.*
import android.text.*
import android.view.View
import android.widget.*
import org.json.JSONObject
import java.util.Locale
import kotlin.math.max

class MainActivity : Activity() {
    private lateinit var store: SettingsStore
    private lateinit var body: LinearLayout
    private lateinit var host: EditText
    private lateinit var port: EditText
    private lateinit var user: EditText
    private lateinit var fingerprint: EditText
    private lateinit var keyButton: Button
    private lateinit var status: TextView
    private lateinit var totals: TextView
    private lateinit var chart: TrafficChart
    private lateinit var log: TextView
    private lateinit var connect: Button
    private lateinit var appList: LinearLayout
    private lateinit var appCount: TextView
    private var page = "tunnel"
    private val main = Handler(Looper.getMainLooper())
    private var lastRx = 0L
    private var lastTx = 0L
    private var lastTime = 0L
    private var offered = ""
    private val selected = mutableSetOf<String>()
    private var catalog = listOf<Pair<String, String>>()
    @Volatile private var catalogLoaded = false
    private val poll = object : Runnable {
        override fun run() {
            try {
                val s = JSONObject(NativeBridge.status())
                val rx = s.optLong("rx"); val tx = s.optLong("tx"); val now = SystemClock.elapsedRealtime()
                if (::status.isInitialized) {
                    status.text = getString(R.string.status_lines, TunnelVpnService.phase, TunnelVpnService.detail)
                    connect.text = if (TunnelVpnService.active) "Остановить" else "Подключить"
                    host.isEnabled = !TunnelVpnService.active; port.isEnabled = !TunnelVpnService.active
                    user.isEnabled = !TunnelVpnService.active; fingerprint.isEnabled = !TunnelVpnService.active
                    keyButton.isEnabled = !TunnelVpnService.active
                    totals.text = getString(R.string.traffic_totals, bytes(rx), bytes(tx))
                    val seconds = ((now - lastTime) / 1000.0).coerceAtLeast(0.1)
                    if (lastTime != 0L) chart.add(max(0L, rx-lastRx)/seconds, max(0L, tx-lastTx)/seconds)
                    val lines = s.optJSONArray("logs")
                    log.text = buildString {
                        if (s.optString("error").isNotEmpty()) append(s.optString("error") + "\n")
                        if (lines != null) for (i in max(0, lines.length()-60) until lines.length()) append(lines.optString(i) + "\n")
                        val nativeLog=java.io.File(filesDir,"tun.log")
                        if(nativeLog.isFile) java.io.RandomAccessFile(nativeLog,"r").use { file ->
                            val length=file.length();val count=length.coerceAtMost(8192).toInt()
                            file.seek(length-count);val data=ByteArray(count);file.readFully(data)
                            if(count>0) append("\nTCP/UDP engine:\n"+data.toString(Charsets.UTF_8))
                        }
                        if (isEmpty()) append("Журнал пуст")
                    }
                    val seen = s.optString("observed_host_key").takeIf { it.startsWith("SHA256:") }.orEmpty()
                    if (!TunnelVpnService.active && seen.isNotEmpty() && seen != offered && store.fingerprint.isEmpty()) {
                        offered = seen
                        AlertDialog.Builder(this@MainActivity).setTitle("Проверьте ключ SSH-сервера")
                            .setMessage("Получен отпечаток:\n\n$seen\n\nСверьте его с сервером по доверенному каналу. Соединение пока отклонено. Сохранить этот ключ как доверенный?")
                            .setNegativeButton("Отмена", null).setPositiveButton("Доверять") { _, _ -> store.fingerprint = seen; fingerprint.setText(seen) }.show()
                    }
                }
                lastRx=rx;lastTx=tx;lastTime=now
            } catch (error: Exception) { if (::status.isInitialized) status.text = error.message }
            main.postDelayed(this, 1000)
        }
    }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.statusBarColor = Color.rgb(21, 39, 48)
        store = SettingsStore(this); selected.addAll(store.packages)
        render()
        Thread {
            val intent = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
            catalog = packageManager.queryIntentActivities(intent, 0).map { it.activityInfo.packageName to it.loadLabel(packageManager).toString() }
                .filter { it.first != packageName && packageManager.checkPermission("android.permission.INTERNET", it.first) == PackageManager.PERMISSION_GRANTED }
                .distinctBy { it.first }.sortedBy { it.second.lowercase() }
            catalogLoaded = true
            runOnUiThread { if (page == "apps") showApps("") }
        }.start()
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission("android.permission.POST_NOTIFICATIONS") != PackageManager.PERMISSION_GRANTED) requestPermissions(arrayOf("android.permission.POST_NOTIFICATIONS"), 101)
    }
    override fun onResume() { super.onResume(); main.post(poll) }
    override fun onPause() { main.removeCallbacks(poll); super.onPause() }
    private fun render() {
        val root = LinearLayout(this).apply { orientation=LinearLayout.VERTICAL; setPadding(dp(16),dp(12),dp(16),dp(8)); setBackgroundColor(Color.rgb(242,245,247)) }
        root.addView(text("Secret Tunnel", 25).apply { setTextColor(Color.rgb(21,65,48)) })
        val nav=LinearLayout(this)
        nav.addView(button("Туннель") { if (page=="tunnel") saveFields(); page="tunnel";render() },LinearLayout.LayoutParams(0,dp(52),1f))
        nav.addView(button("Приложения") { if (page=="tunnel") saveFields();page="apps";render() },LinearLayout.LayoutParams(0,dp(52),1f))
        root.addView(nav)
        val scroll=ScrollView(this)
        body=LinearLayout(this).apply { orientation=LinearLayout.VERTICAL }
        scroll.addView(body);root.addView(scroll,LinearLayout.LayoutParams(-1,0,1f));setContentView(root)
        if(page=="tunnel") tunnelPage() else appsPage()
    }
    private fun tunnelPage() {
        status=text(TunnelVpnService.phase,18);body.addView(status)
        host=field("Адрес SSH-сервера",store.host)
        port=field("SSH-порт",store.port.toString()).apply { inputType=android.text.InputType.TYPE_CLASS_NUMBER }
        user=field("Пользователь SSH",store.user)
        fingerprint=field("Отпечаток сервера SHA256:…",store.fingerprint)
        keyButton=button(if(store.hasKey()) "SSH-ключ импортирован · заменить" else "Импортировать приватный SSH-ключ") {
            startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).setType("*/*").addCategory(Intent.CATEGORY_OPENABLE),102)
        };body.addView(keyButton)
        body.addView(text("Ключ хранится зашифрованным через Android Keystore. Сейчас поддерживаются ключи без пароля. При первом подключении нужно проверить отпечаток сервера.",12))
        body.addView(text("Выбрано приложений: ${store.packages.size}. DNS выбранных приложений также идёт через туннель.",14))
        connect=button(if(TunnelVpnService.active) "Остановить" else "Подключить") {
            if(TunnelVpnService.active) startService(Intent(this,TunnelVpnService::class.java).setAction("STOP"))
            else try {
                saveFields();require(store.host.isNotBlank()&&store.user.isNotBlank()) {"Укажите сервер и пользователя"}
                require(store.hasKey()) {"Импортируйте SSH-ключ"};require(store.packages.isNotEmpty()) {"Выберите приложения"}
                val permission=VpnService.prepare(this)
                if(permission!=null) startActivityForResult(permission,103) else startTunnel()
            } catch(e:Exception) {message(e.message.orEmpty())}
        };body.addView(connect)
        totals=text("↓ 0 B   ↑ 0 B",18);body.addView(totals)
        chart=TrafficChart(this);body.addView(chart,LinearLayout.LayoutParams(-1,dp(110)))
        body.addView(text("↓ Получено (зелёный) · ↑ Отправлено (синий)\nСчитаются полезные данные туннеля, без SSH-заголовков.",12))
        body.addView(text("Журнал соединения",18));log=text("Журнал пуст",12).apply { setTextIsSelectable(true);typeface=Typeface.MONOSPACE };body.addView(log)
    }
    private fun appsPage() {
        body.addView(text("Приложения через SSH",20))
        body.addView(text("Галочками выберите приложения с доступом в интернет. Остальные идут напрямую. Изменение выбора применяется после переподключения.",14))
        appCount=text(getString(R.string.selected_count, selected.size),14);body.addView(appCount)
        val search=field("Поиск приложения или package name","")
        body.addView(button("Снять все галочки") {
            selected.clear();store.packages=emptySet();appCount.text=getString(R.string.selected_count,0);showApps(search.text.toString())
        })
        appList=LinearLayout(this).apply { orientation=LinearLayout.VERTICAL };body.addView(appList)
        search.addTextChangedListener(object:TextWatcher {
            override fun beforeTextChanged(s:CharSequence?,start:Int,count:Int,after:Int) {}
            override fun onTextChanged(s:CharSequence?,start:Int,before:Int,count:Int) {showApps(s.toString())}
            override fun afterTextChanged(s:Editable?) {}
        });showApps("")
    }
    private fun showApps(query:String) {
        if(!::appList.isInitialized) return
        appList.removeAllViews()
        val filtered=catalog.filter { it.first.contains(query,true)||it.second.contains(query,true) }
        if(!catalogLoaded) appList.addView(text("Поиск установленных приложений…",14))
        else if(filtered.isEmpty()) appList.addView(text("Ничего не найдено",14))
        for((pkg,name) in filtered) appList.addView(CheckBox(this).apply {
            text=getString(R.string.app_entry,name,pkg);textSize=14f;isChecked=pkg in selected;setPadding(0,dp(7),0,dp(7))
            setOnCheckedChangeListener { _,checked -> if(checked) selected.add(pkg) else selected.remove(pkg);store.packages=selected;appCount.text=getString(R.string.selected_count,selected.size) }
        })
    }
    private fun saveFields() {
        if(!::host.isInitialized) return
        val value=port.text.toString().toIntOrNull()
        require(value!=null&&value in 1..65535) {"Порт должен быть от 1 до 65535"}
        val newHost=host.text.toString().trim();val newPort=value
        var newPin=fingerprint.text.toString().trim()
        if((store.host!=newHost||store.port!=newPort) && newPin==store.fingerprint) {newPin="";fingerprint.setText("")}
        store.host=newHost;store.port=newPort;store.user=user.text.toString().trim();store.fingerprint=newPin
    }
    private fun startTunnel() {startForegroundService(Intent(this,TunnelVpnService::class.java))}
    @Deprecated("Android activity result callback")
    override fun onActivityResult(requestCode:Int,resultCode:Int,data:Intent?) {
        super.onActivityResult(requestCode,resultCode,data)
        if(resultCode!=RESULT_OK) return
        when(requestCode) {
            103 -> startTunnel()
            102 -> try {
                val uri=data?.data?:return
                val bytes=contentResolver.openInputStream(uri)!!.use { input ->
                    val out=java.io.ByteArrayOutputStream();val buffer=ByteArray(8192)
                    while(out.size()<=1_048_576) {val count=input.read(buffer);if(count<0) break;out.write(buffer,0,count)}
                    out.toByteArray()
                }
                try {store.importKey(bytes)} finally {bytes.fill(0)}
                keyButton.text=getString(R.string.key_imported)
            } catch(e:Exception) {message(e.message.orEmpty())}
        }
    }
    private fun field(title:String,value:String):EditText {
        body.addView(text(title,13))
        return EditText(this).apply { setSingleLine(true);setText(value);hint=title;textSize=16f;importantForAutofill=View.IMPORTANT_FOR_AUTOFILL_NO;body.addView(this,LinearLayout.LayoutParams(-1,dp(50))) }
    }
    private fun text(value:String,size:Int)=TextView(this).apply { text=value;textSize=size.toFloat();setTextColor(Color.rgb(31,46,55));setPadding(0,dp(6),0,dp(6)) }
    private fun button(value:String,action:()->Unit)=Button(this).apply { text=value;isAllCaps=false;setOnClickListener {try {action()} catch(e:Exception) {message(e.message.orEmpty())}} }
    private fun message(value:String) {AlertDialog.Builder(this).setMessage(value).setPositiveButton("OK",null).show()}
    private fun dp(value:Int)=(resources.displayMetrics.density*value).toInt()
    private fun bytes(value:Long):String=when {value>=1048576->String.format(Locale.US,"%.1f MiB",value/1048576.0);value>=1024->String.format(Locale.US,"%.1f KiB",value/1024.0);else->"$value B"}
}

private class TrafficChart(context:android.content.Context):View(context) {
    private val points=ArrayDeque<Pair<Double,Double>>()
    private val paint=Paint(Paint.ANTI_ALIAS_FLAG)
    private val rxPath=Path()
    private val txPath=Path()
    fun add(rx:Double,tx:Double) {points.addLast(rx to tx);while(points.size>60) points.removeFirst();invalidate()}
    override fun onDraw(canvas:Canvas) {
        super.onDraw(canvas);canvas.drawColor(Color.WHITE)
        if(points.size<2) return
        val peak=points.maxOf {max(it.first,it.second)}.coerceAtLeast(1024.0)
        fun draw(first:Boolean,color:Int) {
            val path=if(first) rxPath else txPath;path.reset();points.forEachIndexed {i,p->
                val x=i*width/59f;val y=(height-8-(if(first)p.first else p.second)/peak*(height-16)).toFloat()
                if(i==0) path.moveTo(x,y) else path.lineTo(x,y)
            };paint.color=color;paint.style=Paint.Style.STROKE;paint.strokeWidth=3f;canvas.drawPath(path,paint)
        }
        draw(true,Color.rgb(30,143,85));draw(false,Color.rgb(44,105,205))
    }
}
