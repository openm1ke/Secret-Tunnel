package dev.secrettunnel.android

import android.app.*
import android.content.*
import android.content.pm.PackageManager
import android.content.res.ColorStateList
import android.graphics.Typeface
import android.graphics.drawable.Drawable
import android.net.VpnService
import android.os.*
import android.text.*
import android.view.*
import android.widget.*
import org.json.JSONObject
import java.util.UUID

class MainActivity:Activity() {
    private lateinit var store:SettingsStore
    private lateinit var content:FrameLayout
    private lateinit var body:LinearLayout
    private lateinit var badge:TextView
    private var page="home"
    private var editingId=""
    private var importId=""
    private var pendingProxy=""
    private var checking=false
    private var checkingDetail="Проверяем доступ к серверу…"
    private var pendingTrust=false
    private var nameField:EditText?=null
    private var hostField:EditText?=null
    private var portField:EditText?=null
    private var userField:EditText?=null
    private var serverSpinner:Spinner?=null
    private var proxyUdpReceive: UdpWindow? = null
    private var proxyUdpAgentReceive: UdpWindow? = null
    private var proxySsh=SshSettings()
    private var formSnapshot=""
    private var formDraft:Bundle?=null
    private var heroTitle:TextView?=null
    private var heroDetail:TextView?=null
    private var connectButton:Button?=null
    private var downSpeed:TextView?=null
    private var upSpeed:TextView?=null
    private var downTotal:TextView?=null
    private var upTotal:TextView?=null
    private var chart:NetworkChart?=null
    private var chartRange=ChartRange.MINUTE
    private var chartPeaks:TextView?=null
    private var appTrafficList:LinearLayout?=null
    private var appTrafficEmpty:TextView?=null
    private var appTrafficTitle:TextView?=null
    private var appTrafficRows=linkedMapOf<Int, TextView>()
    private var logView:TextView?=null
    private var appCount:TextView?=null
    private var appEmpty:TextView?=null
    private var appAdapter:ApplicationAdapter?=null
    private var searchQuery=""
    private val main=Handler(Looper.getMainLooper())
    private val meter=TrafficMeter()
    private var lastStatus=JSONObject()
    private var catalog=listOf<AppEntry>()
    @Volatile private var catalogLoaded=false
    private val selected=mutableSetOf<String>()
    private val iconCache=android.util.LruCache<String,Drawable>(32)
    private data class AppEntry(val pkg:String,val label:String)
    private val poll=object:Runnable {
        override fun run(){
            try{
                lastStatus=JSONObject(NativeBridge.status())
                val rx=lastStatus.optLong("rx");val tx=lastStatus.optLong("tx")
                val rate=meter.sample(SystemClock.elapsedRealtime(),rx,tx)
                val phase=TunnelVpnService.phase
                val state=when{checking->"Проверка";pendingTrust->"Подтверждение";phase=="Работает"->"Подключено";TunnelVpnService.active->phase;phase.contains("Ошибка")->"Ошибка";else->"Остановлен"}
                update(badge,state);badge.setTextColor(when{state=="Подключено"->Palette.accent;state=="Ошибка"->Palette.error;TunnelVpnService.active||checking->Palette.warning;else->Palette.muted})
                update(heroTitle,when{checking->"Проверяем сервер";pendingTrust->"Подтвердите ключ сервера";phase=="Работает"->"Туннель работает";TunnelVpnService.active->phase;phase.contains("Ошибка")->"Не удалось подключиться";store.activeProxy==null->"Добавьте подключение";else->if(readinessAction()=="Подключить")"Готов к подключению" else "Настройте подключение"})
                update(heroDetail,when{checking->checkingDetail;pendingTrust->"Сверьте отпечаток в открытом диалоге";phase=="Работает"->activeTunnelDetails();phase.contains("Ошибка")->friendlyError(TunnelVpnService.detail);TunnelVpnService.active->TunnelVpnService.detail;else->"Выберите профиль и приложения. Остальной трафик идёт напрямую."})
                listOf(nameField,hostField,portField,userField,serverSpinner).forEach{it?.isEnabled=!checking&&!pendingTrust&&!TunnelVpnService.active}
                connectButton?.apply{val value=if(TunnelVpnService.active)"Остановить" else readinessAction();if(text.toString()!=value)text=value;isEnabled=!checking&&!pendingTrust&&phase!="Остановка";alpha=if(isEnabled)1f else .45f}
                update(downSpeed,TrafficMeter.bytes(rate.down)+"/с");update(upSpeed,TrafficMeter.bytes(rate.up)+"/с")
                update(downTotal,"Всего: "+TrafficMeter.bytes(rx.toDouble()));update(upTotal,"Всего: "+TrafficMeter.bytes(tx.toDouble()))
                if(page=="home")refreshTraffic()
                if(page=="logs")update(logView,diagnostics())
            }catch(e:Exception){update(heroDetail,e.message.orEmpty())}
            main.postDelayed(this,1000)
        }
    }
    override fun onCreate(savedInstanceState:Bundle?){
        super.onCreate(savedInstanceState);window.statusBarColor=Palette.background;window.navigationBarColor=Palette.background
        store=SettingsStore(this);selected.addAll(store.packages)
        page=savedInstanceState?.getString("page")?:"home";editingId=savedInstanceState?.getString("editingId").orEmpty()
        chartRange=ChartRange.entries.firstOrNull{it.name==savedInstanceState?.getString("chartRange")}?:ChartRange.MINUTE
        importId=savedInstanceState?.getString("importId").orEmpty();pendingProxy=savedInstanceState?.getString("pendingProxy").orEmpty()
        searchQuery=savedInstanceState?.getString("search").orEmpty();formDraft=savedInstanceState?.getBundle("draft")
        listOf("enrollment-key.runtime","device-key.runtime").forEach{java.io.File(filesDir,it).delete()}
        if(Build.VERSION.SDK_INT>=33)onBackInvokedDispatcher.registerOnBackInvokedCallback(android.window.OnBackInvokedDispatcher.PRIORITY_DEFAULT){handleBack()}
        render();scanApps()
        if(Build.VERSION.SDK_INT>=33&&checkSelfPermission("android.permission.POST_NOTIFICATIONS")!=PackageManager.PERMISSION_GRANTED)requestPermissions(arrayOf("android.permission.POST_NOTIFICATIONS"),101)
    }
    override fun onResume(){super.onResume();main.removeCallbacks(poll);main.post(poll)}
    override fun onPause(){main.removeCallbacks(poll);super.onPause()}
    override fun onSaveInstanceState(out:Bundle){
        out.putString("page",page);out.putString("editingId",editingId);out.putString("importId",importId);out.putString("pendingProxy",pendingProxy);out.putString("search",searchQuery)
        out.putString("chartRange",chartRange.name)
        if(page.endsWith("Editor"))out.putBundle("draft",Bundle().apply{
            putString("name",nameField?.text.toString());putString("host",hostField?.text.toString())
            putString("port",portField?.text.toString());putString("user",userField?.text.toString())
            putInt("serverPosition",serverSpinner?.selectedItemPosition?:0)
            putString("udpReceive", proxyUdpReceive?.toJson()?.toString())
            putString("udpAgentReceive", proxyUdpAgentReceive?.toJson()?.toString())
            putString("ssh",proxySsh.toJson().toString())
        })
        super.onSaveInstanceState(out)
    }
    @Deprecated("Platform back callback")
    override fun onBackPressed(){handleBack()}
    private fun handleBack(){if(page.endsWith("Editor"))go(if(page=="serverEditor")"servers" else "proxies")else if(page!="home")go("home")else finish()}
    private fun go(destination:String){
        if(checking||pendingTrust){toast("Дождитесь завершения проверки");return}
        if(page.endsWith("Editor")&&formSignature()!=formSnapshot)AlertDialog.Builder(this).setTitle("Сохранить изменения?").setMessage("В форме есть несохранённые данные.")
            .setNegativeButton("Остаться",null).setNeutralButton("Не сохранять"){_,_->navigate(destination)}.setPositiveButton("Сохранить"){_,_->attempt{saveEditor();navigate(destination)}}.show()
        else navigate(destination)
    }
    private fun navigate(destination:String){page=destination;formDraft=null;render()}
    private fun render(){
        heroTitle=null;heroDetail=null;connectButton=null;downSpeed=null;upSpeed=null;downTotal=null;upTotal=null;chart=null;logView=null
        chartPeaks=null;appTrafficList=null;appTrafficEmpty=null;appTrafficTitle=null;appTrafficRows.clear()
        nameField=null;hostField=null;portField=null;userField=null;serverSpinner=null
        proxyUdpReceive=null;proxyUdpAgentReceive=null
        val root=column().apply{setBackgroundColor(Palette.background);isFocusableInTouchMode=true}
        val header=row().apply{setPadding(dp(20),dp(15),dp(20),dp(12));gravity=Gravity.CENTER_VERTICAL}
        header.addView(label("Secret Tunnel",21,true),LinearLayout.LayoutParams(0,-2,1f))
        badge=label("Остановлен",11).apply{setPadding(dp(12),dp(7),dp(12),dp(7));background=Palette.panel(Palette.raised,dp(20).toFloat())};header.addView(badge);root.addView(header)
        content=FrameLayout(this);root.addView(content,LinearLayout.LayoutParams(-1,0,1f))
        val nav=row().apply{setPadding(dp(8),dp(8),dp(8),dp(9));setBackgroundColor(Palette.surface)}
        for((id,title) in listOf("home" to "Главная","servers" to "Серверы","proxies" to "Прокси","apps" to "Приложения","logs" to "Журнал")){
            val active=page==id||(page=="serverEditor"&&id=="servers")||(page=="proxyEditor"&&id=="proxies")
            nav.addView(column().apply{gravity=Gravity.CENTER;minimumHeight=dp(56);contentDescription=title
                if(active)background=Palette.panel(Palette.raised,dp(15).toFloat())
                addView(NavigationIcon(this@MainActivity,id,if(active)Palette.accent else Palette.muted),LinearLayout.LayoutParams(dp(23),dp(23)))
                addView(label(title,11,active,if(active)Palette.accent else Palette.muted).apply{
                    gravity=Gravity.CENTER;maxLines=1;setPadding(dp(2),dp(5),dp(2),0)
                    setAutoSizeTextTypeUniformWithConfiguration(9,11,1,android.util.TypedValue.COMPLEX_UNIT_SP)
                },LinearLayout.LayoutParams(-1,dp(24)))
                setOnClickListener{go(id)}
            },LinearLayout.LayoutParams(0,-2,1f))
        };root.addView(nav);setContentView(root)
        if(page=="apps")appsPage()else{
            val scroll=ScrollView(this).apply{isFillViewport=true;clipToPadding=false};body=column().apply{setPadding(dp(20),dp(8),dp(20),dp(20))};scroll.addView(body);content.addView(scroll)
            when(page){"servers"->serversPage();"proxies"->proxiesPage();"logs"->logsPage();"serverEditor"->serverEditor();"proxyEditor"->proxyEditor();else->{page="home";homePage()}}
        };root.requestFocus()
    }
    private fun homePage(){
        val hero=card(Palette.raised)
        heroTitle=label("Готов к подключению",23,true).also{hero.addView(it)}
        heroDetail=label("Выберите профиль и приложения",14,false,Palette.muted).also{hero.addView(it,spaced(8))}
        connectButton=button("Подключить",true){connectOrStop()};hero.addView(connectButton,spaced(18));body.addView(hero,spaced(12))
        val proxy=store.activeProxy;val server=proxy?.let{store.server(it.serverId)};val profile=card()
        profile.addView(label((proxy?.name?:"Профиль не выбран")+"  ›",16,true,Palette.accent))
        profile.addView(label(server?.let{"${it.name} · ${it.host.ifBlank{"адрес не указан"}}"}?:"Добавьте сервер и прокси",13,false,Palette.muted),spaced(4))
        if (server?.jumpServerId != null) profile.addView(label(routeLabel(server.id),13,false,Palette.muted),spaced(5))
        profile.setOnClickListener{go("proxies")};profile.contentDescription="Выбрать профиль подключения";body.addView(profile,spaced(12))
        val metrics=row()
        fun metric(title:String,color:Int,down:Boolean):LinearLayout{
            val card=card();card.addView(label(title,12,true,color));val speed=label("0 B/с",21,true).apply{setSingleLine(true);setAutoSizeTextTypeUniformWithConfiguration(14,21,1,android.util.TypedValue.COMPLEX_UNIT_SP)};card.addView(speed,spaced(10));val total=label("Всего: 0 B",11,false,Palette.muted);card.addView(total,spaced(5))
            if(down){downSpeed=speed;downTotal=total}else{upSpeed=speed;upTotal=total};return card
        }
        metrics.addView(metric("↓  Загрузка",Palette.accent,true),LinearLayout.LayoutParams(0,-1,1f).apply{marginEnd=dp(6)})
        metrics.addView(metric("↑  Отправка",Palette.upload,false),LinearLayout.LayoutParams(0,-1,1f).apply{marginStart=dp(6)});body.addView(metrics,spaced(12))
        val graph=card()
        graph.addView(label("График скорости",14,true))
        val period=Spinner(this).apply {
            contentDescription="Период графика скорости"
            minimumHeight=dp(48)
            adapter=ArrayAdapter(this@MainActivity,android.R.layout.simple_spinner_dropdown_item,ChartRange.entries.map{it.label})
            setSelection(chartRange.ordinal)
            onItemSelectedListener=object:AdapterView.OnItemSelectedListener{
                override fun onNothingSelected(parent:AdapterView<*>?){}
                override fun onItemSelected(parent:AdapterView<*>?,view:View?,position:Int,id:Long){
                    chartRange=ChartRange.entries[position]
                    refreshTraffic()
                }
            }
        }
        graph.addView(period,LinearLayout.LayoutParams(-1,dp(48)))
        chart=NetworkChart(this).also{graph.addView(it,LinearLayout.LayoutParams(-1,dp(126)))}
        chartPeaks=label("",12,false,Palette.muted).also{graph.addView(it,spaced(8))}
        body.addView(graph,spaced(12))
        val apps=card()
        appTrafficTitle=label("Трафик приложений",14,true).also{apps.addView(it)}
        apps.addView(label("Скорость сейчас и объём за сеанс VPN",12,false,Palette.muted),spaced(5))
        appTrafficEmpty=label("",12,false,Palette.muted).also{apps.addView(it,spaced(8))}
        appTrafficList=column().also{apps.addView(it)}
        body.addView(apps,spaced(12))
        refreshTraffic()
        body.addView(button(if(store.packages.isEmpty())"Выбрать приложения" else "Выбрать приложения · ${store.packages.size}"){go("apps")},spaced(12))
    }

    private fun refreshTraffic() {
        val window=TunnelVpnService.traffic.snapshot(chartRange)
        chart?.show(window)
        val wallOffset=System.currentTimeMillis()-SystemClock.elapsedRealtime()
        fun peak(down:Boolean):String {
            val point=window.points.maxByOrNull{if(down)it.rate.down else it.rate.up}?:return "—"
            val rate=if(down)point.rate.down else point.rate.up
            val time=TransportDiagnostics.timestamp(point.timeMillis+wallOffset)
            return if(rate<=0) "—" else TrafficMeter.bytes(rate)+"/с · "+
                if(chartRange==ChartRange.SECOND||chartRange==ChartRange.FIVE_SECONDS)time else time.substringBefore('.')
        }
        update(chartPeaks,"Пик ↓ ${peak(true)}\nПик ↑ ${peak(false)}")

        val list=appTrafficList?:return
        val entries=TunnelVpnService.appTraffic?.entries.orEmpty()
        val active=if(TunnelVpnService.active)entries.count{it.uid>=0&&(it.rate.down>0||it.rate.up>0)} else 0
        update(appTrafficTitle,if(active>0)"Трафик приложений · $active сейчас" else "Трафик приложений")
        update(appTrafficEmpty,when {
            Build.VERSION.SDK_INT<29 -> "Разбивка по приложениям доступна с Android 10."
            entries.isNotEmpty() -> ""
            TunnelVpnService.active -> "Ожидаем трафик выбранных приложений…"
            else -> "Подключите туннель, чтобы увидеть трафик приложений."
        })
        appTrafficEmpty?.visibility=if(entries.isEmpty())View.VISIBLE else View.GONE
        if(appTrafficRows.keys.toList()!=entries.map{it.uid}) {
            list.removeAllViews()
            appTrafficRows.clear()
            for(entry in entries) {
                val details=label("",12,false,Palette.muted)
                list.addView(column().apply{
                    addView(label(entry.name,14,true))
                    addView(details,spaced(4))
                },spaced(12))
                appTrafficRows[entry.uid]=details
            }
        }
        for(entry in entries) {
            val rate=if(TunnelVpnService.active)entry.rate else TrafficMeter.Rate(0.0,0.0)
            update(appTrafficRows[entry.uid],"↓ ${TrafficMeter.bytes(rate.down)}/с · ↑ ${TrafficMeter.bytes(rate.up)}/с\nЗа сеанс ↓ ${TrafficMeter.bytes(entry.down.toDouble())} · ↑ ${TrafficMeter.bytes(entry.up.toDouble())}")
            appTrafficRows[entry.uid]?.setTextColor(if(rate.down>0||rate.up>0)Palette.text else Palette.muted)
        }
    }

    private fun serversPage(){
        heading("Серверы","SSH-серверы, ключи доступа и доверие")
        body.addView(button("+  Добавить сервер",true){unlocked();editingId=UUID.randomUUID().toString();navigate("serverEditor")},spaced(16))
        for(server in store.servers()){
            val scoped=SettingsStore(this,server.id);val card=card();card.addView(label(server.name,18,true))
            card.addView(label(if(server.host.isBlank())"Укажите адрес и пользователя" else "${server.user}@${server.host}:${server.port}",13,false,Palette.muted),spaced(6))
            if (server.jumpServerId != null) card.addView(label(routeLabel(server.id),13,false,Palette.muted),spaced(6))
            card.addView(label(if(scoped.hasKey())"✓  Ключ доступа добавлен" else "○  Добавьте SSH-ключ",12,false,if(scoped.hasKey())Palette.accent else Palette.warning),spaced(12))
            card.addView(label(if(validPin(scoped.fingerprint))"✓  Ключ сервера сохранён" else "○  Сервер ещё не проверен",12,false,Palette.muted),spaced(6))
            card.addView(button("Настроить"){unlocked();editingId=server.id;navigate("serverEditor")},spaced(14));body.addView(card,spaced(12))
        }
        if(store.servers().isEmpty())emptyState("Пока нет серверов","Добавьте SSH-сервер, чтобы создать подключение.")
    }
    private fun proxiesPage(){
        heading("Прокси","Именованные SOCKS5-подключения через SSH")
        body.addView(button("+  Добавить прокси",true){unlocked();require(store.servers().isNotEmpty()){"Сначала добавьте сервер"};editingId=UUID.randomUUID().toString();navigate("proxyEditor")},spaced(16))
        body.addView(label("Одновременно используется один профиль. Локальный прокси доступен только на этом устройстве.",12,false,Palette.muted),spaced(12))
        for(proxy in store.proxies()){
            val card=card();val title=row().apply{gravity=Gravity.CENTER_VERTICAL}
            title.addView(RadioButton(this).apply{isChecked=proxy.id==store.activeProxyId;isClickable=false;buttonTintList=ColorStateList.valueOf(Palette.accent);importantForAccessibility=View.IMPORTANT_FOR_ACCESSIBILITY_NO})
            title.addView(label(proxy.name,17,true),LinearLayout.LayoutParams(0,-2,1f));card.addView(title)
            card.addView(label("127.0.0.1:${proxy.port}  →  ${store.server(proxy.serverId)?.name?:"Сервер удалён"}",12,false,Palette.muted),spaced(6))
            card.addView(label(if(proxy.id==store.activeProxyId)"Выбран для подключения" else "Нажмите, чтобы выбрать",11,false,if(proxy.id==store.activeProxyId)Palette.accent else Palette.muted),spaced(10))
            card.setOnClickListener{attempt{unlocked();store.activeProxyId=proxy.id;render()}}
            card.addView(button("Изменить"){unlocked();editingId=proxy.id;navigate("proxyEditor")},spaced(12));body.addView(card,spaced(12))
        }
        if(store.proxies().isEmpty())emptyState("Создайте первый прокси","Выберите сервер, задайте название и локальный порт.")
    }
    private fun serverEditor(){
        val server=store.server(editingId)
        heading(if(server==null)"Новый сервер" else "Настройки сервера","Ключ доступа хранится отдельно для каждого сервера")
        nameField=field("Название",server?.name.orEmpty(),"Например, Германия",1001)
        hostField=field("Адрес сервера",server?.host.orEmpty(),"IP или имя хоста",1002)
        portField=field("SSH-порт",(server?.port?:22).toString(),"22",1003,true)
        userField=field("SSH-пользователь",server?.user.orEmpty(),"Имя пользователя",1004)
        val servers = store.servers().filter { it.id != editingId }
        body.addView(label("Подключаться через SSH-сервер",12,true,Palette.muted),spaced(16))
        serverSpinner = serverPicker(listOf(null to "Напрямую") + servers.map { it.id to it.name }, server?.jumpServerId)
            .apply { contentDescription = "Подключаться через SSH-сервер" }
        val preview = label("",13,false,Palette.muted)
        body.addView(preview,spaced(6))
        serverSpinner?.onItemSelectedListener = object : android.widget.AdapterView.OnItemSelectedListener {
            override fun onNothingSelected(parent: android.widget.AdapterView<*>?) = Unit
            override fun onItemSelected(parent: android.widget.AdapterView<*>?, view: View?, position: Int, id: Long) {
                val hop = selectedServerInEditor()
                val destination = nameField?.text.toString().ifBlank { "Конечный сервер" }
                preview.text = if (hop == null) "Прямое подключение" else runCatching {
                    val draft = ServerProfile(editingId, destination, "", 22, "", hop)
                    resolveServerRoute(editingId, servers + draft).joinToString(" → ") { it.name }
                }.getOrElse { it.message.orEmpty() }
            }
        }
        val originalSignature=formSignature();restoreDraft()
        val scoped=SettingsStore(this,editingId);val keyCard=card();keyCard.addView(label("Доступ к серверу",17,true))
        keyCard.addView(label(if(scoped.hasKey())"SSH-ключ добавлен и зашифрован" else "Импортируйте разрешённый на сервере ключ без пароля",13,false,Palette.muted),spaced(10))
        keyCard.addView(button(if(scoped.hasKey())"Заменить SSH-ключ" else "Импортировать SSH-ключ"){
            unlocked();saveServerForm();importId=editingId;startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).setType("*/*").addCategory(Intent.CATEGORY_OPENABLE),102)
        },spaced(14))
        keyCard.addView(button("Создать отдельный ключ телефона"){
            unlocked();saveServerForm();require(scoped.hasKey()){"Сначала импортируйте уже разрешённый SSH-ключ"};verifyServer(editingId){enrollDevice(editingId)}
        },spaced(8));body.addView(keyCard,spaced(20))
        val trust=card();trust.addView(label("Подлинность сервера",17,true))
        trust.addView(label(if(validPin(scoped.fingerprint))scoped.fingerprint else "Отпечаток будет получен автоматически",12,false,Palette.muted).apply{setTextIsSelectable(true)},spaced(10))
        trust.addView(button("Проверить отпечаток"){unlocked();saveServerForm();verifyServer(editingId){toast("Сервер доступен, ключ сохранён");render()}},spaced(14))
        val isRelay = store.servers().any { it.jumpServerId == editingId }
        trust.addView(button(if(isRelay) "Проверить SSH" else "Проверить SSH и агент"){
            unlocked();saveServerForm();val id=editingId
            require(SettingsStore(this,id).hasKey()){"Сначала импортируйте SSH-ключ"}
            verifyServer(id){checkServerAgent(id, !isRelay)}
        },spaced(8));body.addView(trust,spaced(12))
        body.addView(button("Сохранить сервер",true){unlocked();saveServerForm();navigate("servers")},spaced(20))
        if(server!=null)body.addView(button("Удалить сервер",danger=true){unlocked();confirmDelete("Удалить сервер ${server.name}?"){store.deleteServer(server.id);navigate("servers")}},spaced(8))
        formSnapshot=originalSignature
    }
    private fun proxyEditor(){
        val proxy=store.proxies().firstOrNull{it.id==editingId};val servers=store.servers()
        heading(if(proxy==null)"Новый прокси" else "Настройки прокси","SOCKS5 · TCP и UDP через выбранный SSH-сервер")
        nameField=field("Название",proxy?.name.orEmpty(),"Например, Рабочий",1001)
        body.addView(label("SSH-сервер",12,true,Palette.muted),spaced(16))
        serverSpinner = serverPicker(servers.map { it.id to "${it.name} · ${it.host.ifBlank{"не настроен"}}" }, proxy?.serverId)
        portField=field("Локальный порт",(proxy?.port?:1080).toString(),"1024–65535",1003,true)
        proxyUdpReceive = proxy?.udpReceive
        proxyUdpAgentReceive = proxy?.udpAgentReceive
        proxySsh=proxy?.ssh?:SshSettings()
        val originalSignature=formSignature()
        restoreDraft()
        body.addView(label("Адрес: 127.0.0.1. Порт используется локальным SOCKS5-прокси и VPN выбранных приложений.",13,false,Palette.muted),spaced(14))
        body.addView(udpSettingsCard(), spaced(16))
        body.addView(sshSettingsCard(), spaced(16))
        body.addView(button("Сохранить прокси",true){unlocked();saveProxyForm();navigate("proxies")},spaced(20))
        if(proxy!=null)body.addView(button("Удалить прокси",danger=true){unlocked();confirmDelete("Удалить прокси ${proxy.name}?"){store.deleteProxy(proxy.id);navigate("proxies")}},spaced(8))
        formSnapshot=originalSignature
    }
    private fun serverPicker(choices: List<Pair<String?, String>>, selected: String?): Spinner = Spinner(this).apply {
        adapter = object : ArrayAdapter<String>(this@MainActivity, android.R.layout.simple_spinner_item, choices.map { it.second }) {
            override fun getView(position: Int, convertView: View?, parent: ViewGroup): View =
                super.getView(position, convertView, parent).apply { (this as TextView).setTextColor(Palette.text); textSize = 15f }
        }.apply { setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item) }
        tag = choices.map { it.first }
        setSelection(choices.indexOfFirst { it.first == selected }.coerceAtLeast(0))
        backgroundTintList = ColorStateList.valueOf(Palette.accent)
    }.also { body.addView(it, LinearLayout.LayoutParams(-1, dp(56))) }

    private fun selectedServerInEditor(): String? = (serverSpinner?.tag as? List<*>)
        ?.getOrNull(serverSpinner?.selectedItemPosition ?: -1) as? String

    private fun routeLabel(id: String): String = runCatching {
        resolveServerRoute(id, store.servers()).joinToString(" → ") { it.name }
    }.getOrElse { it.message.orEmpty() }

    private fun saveEditor(){unlocked();if(page=="serverEditor")saveServerForm()else saveProxyForm()}
    private fun saveServerForm(){
        store.saveServer(ServerProfile(editingId,nameField?.text.toString().trim(),hostField?.text.toString().trim(),portField?.text.toString().toIntOrNull()?:0,userField?.text.toString().trim(),selectedServerInEditor()));formSnapshot=formSignature()
    }
    private fun saveProxyForm(){
        val serverId=selectedServerInEditor() ?: error("Выберите сервер")
        require(store.server(serverId)!=null){"Этот сервер удалён. Откройте форму заново и выберите другой."}
        store.saveProxy(ProxyProfile(editingId,nameField?.text.toString().trim(),serverId,
            portField?.text.toString().toIntOrNull()?:0,proxyUdpReceive,proxyUdpAgentReceive,proxySsh))
        formSnapshot=formSignature()
    }
    private fun formSignature()=listOf(nameField?.text,hostField?.text,portField?.text,userField?.text,
        serverSpinner?.selectedItemPosition,proxyUdpReceive,proxyUdpAgentReceive,proxySsh).joinToString("\u0000")
    private fun restoreDraft(){
        formDraft?.let{d->
            nameField?.setText(d.getString("name"));hostField?.setText(d.getString("host"))
            portField?.setText(d.getString("port"));userField?.setText(d.getString("user"))
            serverSpinner?.setSelection(d.getInt("serverPosition"))
            proxyUdpReceive = d.getString("udpReceive")?.let { UdpWindow.fromJson(JSONObject(it)) }
            proxyUdpAgentReceive = d.getString("udpAgentReceive")?.let { UdpWindow.fromJson(JSONObject(it)) }
            proxySsh=d.getString("ssh")?.let { SshSettings.fromJson(JSONObject(it)) }?:proxySsh
        }
        formDraft=null
    }

    private fun udpSettingsCard(): LinearLayout = card().apply {
        addView(label("Расширенные · UDP", 16, true))
        addView(label("Параметры применяются при следующем подключении.", 12, false, Palette.muted), spaced(8))
        fun windowButton(title: String, current: UdpWindow?, save: (UdpWindow?) -> Unit) {
            fun caption(value: UdpWindow?) = "$title · ${udpWindowLabel(value)}"
            var selected = current
            lateinit var choice: Button
            choice = button(caption(current)) {
                unlocked()
                chooseUdpWindow(title, selected) { value ->
                    selected = value
                    save(value)
                    choice.text = caption(value)
                }
            }
            addView(choice, spaced(8))
        }
        windowButton("Загрузка", proxyUdpReceive) { proxyUdpReceive = it }
        windowButton("Отправка", proxyUdpAgentReceive) { proxyUdpAgentReceive = it }
        addView(label("Для изменения окна отправки нужен агент с поддержкой этой настройки.",
            12, false, Palette.muted), spaced(8))
    }

    private fun sshSettingsCard():LinearLayout=card().apply {
        addView(label("Расширенные · SSH",16,true))
        addView(label("Применяется ко всем серверам цепочки при следующем подключении.",12,false,Palette.muted),spaced(8))
        val codes=listOf("auto","aes128_gcm","chacha20_poly1305")
        val names=listOf("Авто","AES-128-GCM","ChaCha20-Poly1305")
        fun cipherCaption()="Шифрование · ${names[codes.indexOf(proxySsh.cipher)]}"
        lateinit var cipher:Button
        cipher=button(cipherCaption()){
            unlocked()
            AlertDialog.Builder(this@MainActivity).setTitle("Шифрование SSH")
                .setSingleChoiceItems(names.toTypedArray(),codes.indexOf(proxySsh.cipher)){dialog,index->
                    proxySsh=proxySsh.copy(cipher=codes[index]);cipher.text=cipherCaption();dialog.dismiss()
                }.setNegativeButton("Отмена",null).show()
        }
        addView(cipher,spaced(8))
        fun queueCaption()="Очередь · ${if(proxySsh.smallQueue)"Уменьшенная · тест" else "Стандартная"}"
        lateinit var queue:Button
        queue=button(queueCaption()){
            unlocked()
            AlertDialog.Builder(this@MainActivity).setTitle("Очередь SSH")
                .setSingleChoiceItems(arrayOf("Стандартная · 32 сообщения / 2 МиБ","Уменьшенная · 8 сообщений / 1 МиБ"),if(proxySsh.smallQueue)1 else 0){dialog,index->
                    proxySsh=proxySsh.copy(smallQueue=index==1);queue.text=queueCaption();dialog.dismiss()
                }.setNegativeButton("Отмена",null).show()
        }
        addView(queue,spaced(8))
        val qosCodes=listOf("balanced","voice","throughput")
        val qosNames=listOf("Сбалансированный","Голос","Максимальная скорость")
        fun qosCaption()="Приоритет · ${qosNames[qosCodes.indexOf(proxySsh.qos)]}"
        lateinit var qos:Button
        qos=button(qosCaption()){
            unlocked()
            AlertDialog.Builder(this@MainActivity).setTitle("Профиль трафика")
                .setSingleChoiceItems(qosNames.toTypedArray(),qosCodes.indexOf(proxySsh.qos)){dialog,index->
                    proxySsh=proxySsh.copy(qos=qosCodes[index]);qos.text=qosCaption();dialog.dismiss()
                }.setNegativeButton("Отмена",null).show()
        }
        addView(qos,spaced(8))
        addView(label("«Голос» сильнее ограничивает TCP-загрузки при активном коротком UDP. «Максимальная скорость» отключает это ограничение. Профиль согласуется с конечным сервером для этого сеанса.",12,false,Palette.muted),spaced(8))
        addView(label("Уменьшенная очередь предназначена для сравнения задержек под нагрузкой. Она может ограничить скорость на соединениях с большим RTT. Сжатие выключено.",12,false,Palette.muted),spaced(8))
    }

    private fun udpWindowLabel(window: UdpWindow?): String =
        window?.let { "${it.kib} КиБ / ${it.slots} сл." } ?: "По умолчанию"

    private fun chooseUdpWindow(title: String, current: UdpWindow?, save: (UdpWindow?) -> Unit) {
        val presets = listOf(null, UdpWindow(192, 192), UdpWindow(256, 192))
        val options = (presets.map(::udpWindowLabel) + "Вручную").toTypedArray()
        val selected = presets.indexOf(current).let { if (it < 0) presets.size else it }
        AlertDialog.Builder(this).setTitle("UDP · $title")
            .setSingleChoiceItems(options, selected) { dialog, index ->
                dialog.dismiss()
                if (index == presets.size) editUdpWindow(title, current, save)
                else save(presets[index])
            }
            .setNegativeButton("Отмена", null).show()
    }

    private fun editUdpWindow(title: String, current: UdpWindow?, save: (UdpWindow?) -> Unit) {
        val form = column().apply { setPadding(dp(24), dp(8), dp(24), dp(8)) }
        fun number(caption: String, value: Int): EditText {
            form.addView(label(caption, 14, true), spaced(10))
            return EditText(this).apply {
                setSingleLine(true)
                inputType = InputType.TYPE_CLASS_NUMBER
                contentDescription = caption
                setText(value.toString())
                form.addView(this, LinearLayout.LayoutParams(-1, -2))
            }
        }
        val kib = number("Окно, КиБ (64–1024)", current?.kib ?: 192)
        val slots = number("Слоты (4–192)", current?.slots ?: 192)
        val dialog = AlertDialog.Builder(this).setTitle("UDP · $title")
            .setView(form).setNegativeButton("Отмена", null)
            .setPositiveButton("Применить", null).create()
        dialog.setOnShowListener {
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
                val bytesValue = kib.text.toString().toIntOrNull()
                val slotsValue = slots.text.toString().toIntOrNull()
                when {
                    bytesValue == null || bytesValue !in 64..1024 -> kib.error = "Введите от 64 до 1024 КиБ"
                    slotsValue == null || slotsValue !in 4..192 -> slots.error = "Введите от 4 до 192 слотов"
                    else -> {
                        save(UdpWindow(bytesValue, slotsValue))
                        dialog.dismiss()
                    }
                }
            }
        }
        dialog.show()
    }
    private fun appsPage(){
        val layout=column().apply{setPadding(dp(20),dp(8),dp(20),0)};content.addView(layout)
        layout.addView(label("Приложения",26,true));layout.addView(label("Включите туннель только для нужных приложений.",13,false,Palette.muted),spaced(8))
        val search=EditText(this).apply{setSingleLine(true);hint="Поиск по названию или пакету";setTextColor(Palette.text);setHintTextColor(Palette.muted);textSize=14f;inputType=InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;background=Palette.panel(Palette.surface,dp(14).toFloat(),Palette.border);setPadding(dp(14),0,dp(14),0);setText(searchQuery)}
        layout.addView(search,LinearLayout.LayoutParams(-1,dp(50)).apply{topMargin=dp(16)})
        appCount=label("",11,false,Palette.muted).also{layout.addView(it,spaced(12))}
        val actions=row();actions.addView(button("Выбранные"){searchQuery="";search.setText("");appAdapter?.showSelected=true;refreshApps()},LinearLayout.LayoutParams(0,dp(48),1f).apply{marginEnd=dp(5)})
        actions.addView(button("Все приложения"){appAdapter?.showSelected=false;refreshApps()},LinearLayout.LayoutParams(0,dp(48),1f).apply{marginStart=dp(5)});layout.addView(actions,spaced(8))
        if(TunnelVpnService.active)layout.addView(label("Изменения применятся после переподключения",11,false,Palette.warning),spaced(8))
        val list=ListView(this).apply{divider=null;cacheColorHint=Palette.background;setPadding(0,dp(6),0,dp(12));clipToPadding=false}
        appAdapter=ApplicationAdapter();list.adapter=appAdapter
        val listArea=FrameLayout(this);listArea.addView(list,FrameLayout.LayoutParams(-1,-1))
        appEmpty=label("Ищем приложения…",14,false,Palette.muted).apply{gravity=Gravity.CENTER;setPadding(dp(12),dp(24),dp(12),dp(24))}.also{listArea.addView(it,FrameLayout.LayoutParams(-1,-1));list.emptyView=it}
        layout.addView(listArea,LinearLayout.LayoutParams(-1,0,1f));refreshApps()
        search.addTextChangedListener(object:TextWatcher{
            override fun beforeTextChanged(s:CharSequence?,start:Int,count:Int,after:Int){}
            override fun onTextChanged(s:CharSequence?,start:Int,before:Int,count:Int){searchQuery=s.toString();refreshApps()}
            override fun afterTextChanged(s:Editable?){}
        })
    }
    private fun refreshApps(){if(page!="apps")return;appAdapter?.refresh();update(appEmpty,if(!catalogLoaded)"Ищем приложения…" else "Ничего не найдено.\nИзмените поиск или откройте все приложения.");update(appCount,if(catalogLoaded)"Найдено: ${catalog.size}  ·  Через туннель: ${selected.size}" else "Ищем установленные приложения…")}
    private fun scanApps(){Thread{
        val intent=Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
        catalog=packageManager.queryIntentActivities(intent,0).map{AppEntry(it.activityInfo.packageName,it.loadLabel(packageManager).toString())}
            .filter{it.pkg!=packageName&&packageManager.checkPermission("android.permission.INTERNET",it.pkg)==PackageManager.PERMISSION_GRANTED}
            .distinctBy{it.pkg}.sortedBy{it.label.lowercase()};catalogLoaded=true;runOnUiThread{refreshApps()}
    }.start()}
    private class AppHolder(val icon:ImageView,val name:TextView,val pkg:TextView,val toggle:Switch)
    private inner class ApplicationAdapter:BaseAdapter(){
        private var visible=listOf<AppEntry>();var showSelected=false
        fun refresh(){visible=catalog.filter{(!showSelected||it.pkg in selected)&&(it.label.contains(searchQuery,true)||it.pkg.contains(searchQuery,true))};notifyDataSetChanged()}
        override fun getCount()=visible.size
        override fun getItem(position:Int)=visible[position]
        override fun getItemId(position:Int)=visible[position].pkg.hashCode().toLong()
        override fun getView(position:Int,convertView:View?,parent:ViewGroup):View{
            val container=convertView as? LinearLayout ?: row().apply{
                gravity=Gravity.CENTER_VERTICAL;minimumHeight=dp(80);setPadding(0,dp(12),0,dp(12))
                val icon=ImageView(this@MainActivity);addView(icon,LinearLayout.LayoutParams(dp(40),dp(40)).apply{marginEnd=dp(12)})
                val texts=column();val name=label("",14,true);val pkg=label("",10,false,Palette.muted).apply{maxLines=1;ellipsize=android.text.TextUtils.TruncateAt.END};texts.addView(name);texts.addView(pkg,spaced(4));addView(texts,LinearLayout.LayoutParams(0,-2,1f))
                val toggle=Switch(this@MainActivity).apply{minimumWidth=dp(52);minimumHeight=dp(48);thumbTintList=ColorStateList(arrayOf(intArrayOf(android.R.attr.state_checked),intArrayOf()),intArrayOf(Palette.accent,Palette.muted));trackTintList=ColorStateList(arrayOf(intArrayOf(android.R.attr.state_checked),intArrayOf()),intArrayOf(Palette.raised,Palette.border))};addView(toggle);tag=AppHolder(icon,name,pkg,toggle)
            }
            val item=visible[position];val holder=container.tag as AppHolder;holder.name.text=item.label;holder.pkg.text=item.pkg
            holder.icon.setImageDrawable(iconCache.get(item.pkg)?:try{packageManager.getApplicationIcon(item.pkg).also{iconCache.put(item.pkg,it)}}catch(_:Exception){null})
            holder.toggle.setOnCheckedChangeListener(null);holder.toggle.isChecked=item.pkg in selected;holder.toggle.contentDescription="Через туннель: ${item.label}"
            holder.toggle.setOnCheckedChangeListener{_,enabled->if(enabled)selected.add(item.pkg)else selected.remove(item.pkg);store.packages=selected;update(appCount,"Найдено: ${catalog.size}  ·  Через туннель: ${selected.size}");if(showSelected)refresh()}
            container.setOnClickListener{holder.toggle.isChecked=!holder.toggle.isChecked};return container
        }
    }

    private fun logsPage(){
        heading("Журнал","Диагностика подключения и ошибок")
        body.addView(button("Копировать журнал"){(getSystemService(CLIPBOARD_SERVICE) as android.content.ClipboardManager).setPrimaryClip(ClipData.newPlainText("Secret Tunnel",diagnostics()));toast("Журнал скопирован")},spaced(16))
        logView=label(diagnostics(),11,false,Palette.muted).apply{typeface=Typeface.MONOSPACE;setTextIsSelectable(true)}.also{body.addView(it,spaced(16))}
    }
    private fun activeTunnelDetails(): String {
        val status = "TCP и UDP выбранных приложений идут через туннель"
        val windows = lastStatus.optJSONObject("transport")?.let(TransportDiagnostics::windows)
        val heartbeat = lastStatus.optJSONObject("transport")?.optJSONObject("diagnostics")?.optJSONObject("heartbeat")
        val measuredAt = heartbeat?.optLong("last_ack_at_ms")?:0
        val freshness = (heartbeat?.optLong("interval_ms",5000)?:5000).times(3).coerceAtLeast(15_000)
        val latency = if(measuredAt>0&&System.currentTimeMillis()-measuredAt<=freshness&&!heartbeat!!.isNull("last_rtt_ms"))
            "Задержка туннеля: ${heartbeat.optLong("last_rtt_ms")} мс" else null
        return listOfNotNull(status,windows,latency).joinToString("\n")
    }
    private fun diagnostics():String=buildString{
        append(TunnelVpnService.phase+"\n"+TunnelVpnService.detail+"\n\n")
        lastStatus.optString("agent").takeIf{it.isNotBlank()&&it!="null"}?.let{append(it+"\n\n")}
        lastStatus.optJSONObject("transport")?.let { t->
            append("Транспорт v3 · SSH-каналов: ${t.optInt("ssh_channels")}\n")
            TransportDiagnostics.windows(t)?.let { append("$it\n") }
            t.optJSONObject("ssh")?.let { append(TransportDiagnostics.describeSsh(it)+"\n") }
            append("Соединения: ${t.optInt("active")} / ${t.optInt("limit")} · пик: ${t.optInt("peak")}\n")
            append("Очереди: приём ${t.optLong("receive_queue_bytes")} / ${t.optLong("receive_budget")} B; отправка ${t.optLong("send_queue_bytes")} / ${t.optLong("send_budget")} B\n")
            append("UDP отброшено: ${t.optLong("udp_drops")} · отказов по лимиту: ${t.optLong("rejected")} · ошибок протокола: ${t.optLong("protocol_errors")}\n\n")
            t.optJSONObject("diagnostics")?.let { append("Клиент\n" + TransportDiagnostics.describe(it) + "\n") }
            t.optJSONObject("server")?.let { server->
                append("Отчёт сервера: соединений ${server.optInt("active")} / ${server.optInt("limit")} · пик ${server.optInt("peak")}\n")
                append("Сервер: UDP отброшено ${server.optLong("udp_drops")} · отказов по лимиту ${server.optLong("rejected")}\n")
                if(server.optString("last_udp_error").isNotBlank())append("UDP сервера: ${server.optString("last_udp_error")}\n")
                server.optJSONObject("diagnostics")?.let { append(TransportDiagnostics.describe(it)) }
                append("\n")
            }
        }
        val logs=lastStatus.optJSONArray("logs");if(logs!=null)for(i in 0 until logs.length())append(TransportDiagnostics.formatLog(logs.optString(i))+"\n")
        val file=java.io.File(filesDir,"tun.log")
        if(file.isFile)java.io.RandomAccessFile(file,"r").use{f->val length=f.length();val size=length.coerceAtMost(8192).toInt();f.seek(length-size);val bytes=ByteArray(size);f.readFully(bytes);append(bytes.toString(Charsets.UTF_8))}
        if(logs==null||logs.length()==0)append("Событий пока нет")
    }
    private fun friendlyError(raw:String)=ConnectionMessages.describe(raw)
    private fun readinessAction():String {
        val proxy=store.activeProxy?:return "Добавить прокси"
        val server=store.server(proxy.serverId)?:return "Добавить сервер"
        if(server.host.isBlank()||server.user.isBlank())return "Настроить сервер"
        if(!SettingsStore(this,server.id).hasKey())return "Добавить SSH-ключ"
        if(store.packages.isEmpty())return "Выбрать приложения"
        return "Подключить"
    }
    private fun connectOrStop(){
        if(TunnelVpnService.active){startService(Intent(this,TunnelVpnService::class.java).setAction("STOP"));return}
        val proxy=store.activeProxy
        if(proxy==null){navigate("proxies");return}
        val server=store.server(proxy.serverId)
        if(server==null){navigate("servers");return}
        if(server.host.isBlank()||server.user.isBlank()||!SettingsStore(this,server.id).hasKey()){
            editingId=server.id;navigate("serverEditor");toast("Заполните сервер и добавьте SSH-ключ");return
        }
        if(store.packages.isEmpty()){navigate("apps");toast("Выберите приложения для туннеля");return}
        store.snapshot();pendingProxy=store.activeProxyId
        verifyServer(server.id){val permission=VpnService.prepare(this);if(permission!=null)startActivityForResult(permission,103)else startTunnel()}
    }
    private fun startTunnel(){require(pendingProxy==store.activeProxyId){"Профиль изменился. Подключитесь заново."};startForegroundService(Intent(this,TunnelVpnService::class.java))}
    private fun validPin(value:String)=Regex("SHA256:[A-Za-z0-9+/]{43}").matches(value)
    private fun verifyServer(id:String,after:()->Unit){
        unlocked();require(!pendingTrust){"Проверка уже выполняется"}
        fun verifyNext(index: Int) {
            val route = store.route(id)
            if (index == route.size) { after(); return }
            if (index < route.lastIndex && validPin(route[index].fingerprint)) verifyNext(index + 1)
            else verifyHost(route.take(index + 1)) { verifyNext(index + 1) }
        }
        verifyNext(0)
    }
    private fun verifyHost(route: List<SshEndpoint>, after: () -> Unit) {
        val endpoint = route.last()
        val server = endpoint.profile
        val scoped=SettingsStore(this,server.id);val saved=endpoint.fingerprint
        checkingDetail="Получаем ключ SSH-сервера «${server.name}»…";checking=true
        Thread{
            val result=try{store.withSshConfig(route, includeTargetKey=false) { JSONObject(NativeBridge.probe(it.toString())) }}
                catch(e:Exception){JSONObject().put("error",e.message)}
            runOnUiThread{
                checking=false;if(isFinishing||isDestroyed)return@runOnUiThread
                if(store.server(server.id)!=server)return@runOnUiThread
                val pin=result.optString("fingerprint")
                if(!validPin(pin)){message("Не удалось проверить сервер «${server.name}»: "+result.optString("error"));return@runOnUiThread}
                fun proceed(){scoped.fingerprint=pin;attempt(after)}
                if(saved==pin)proceed()else{
                    pendingTrust=true
                    AlertDialog.Builder(this).setTitle(if(validPin(saved))"Ключ сервера изменился" else "Доверять серверу?")
                        .setMessage("${server.host}:${server.port}\n\n$pin\n\n"+if(validPin(saved))"Сохранён другой ключ. Сверьте замену у администратора перед подтверждением." else "Сверьте отпечаток у администратора. После подтверждения приложение запомнит этот сервер.")
                        .setNegativeButton("Отмена",null).setPositiveButton("Доверять"){dialog,_->
                            dialog.dismiss();pendingTrust=false;proceed()
                        }.setOnDismissListener{pendingTrust=false}.show()
                }
            }
        }.start()
    }
    private fun checkServerAgent(id:String, checkAgent:Boolean){
        val route = store.route(id)
        checkingDetail=if(checkAgent)"Проверяем SSH, ключ и серверный агент…" else "Проверяем SSH и ключ…"
        checking=true
        Thread{
            val result=try{
                store.withSshConfig(route) { config ->
                    config.put("check_agent",checkAgent)
                    val response=JSONObject(NativeBridge.checkServer(config.toString()))
                    if(response.optBoolean("ok"))response.getString("summary") else friendlyError(response.optString("error","Не удалось проверить SSH"))
                }
            }catch(error:Exception){error.message?:"Не удалось проверить SSH"}
            runOnUiThread{checking=false;if(!isFinishing&&!isDestroyed)message(result)}
        }.start()
    }
    private fun enrollDevice(id:String){
        val scoped=SettingsStore(this,id);val route=store.route(id);checking=true
        Thread{
            var device:java.io.File?=null;var bytes:ByteArray?=null
            val error=try{
                val pair=JSONObject(NativeBridge.generateKey());require(!pair.has("error")){pair.optString("error")}
                val file=java.io.File.createTempFile("ssh-device-",".runtime",cacheDir);device=file
                val privateKey=pair.getString("private").toByteArray(Charsets.UTF_8);bytes=privateKey
                file.writeBytes(privateKey)
                store.withSshConfig(route) { config ->
                    config.put("device_key_path",file.absolutePath)
                    val result=NativeBridge.registerKey(config.toString(),pair.getString("public"));require(result.isEmpty()){result}
                }
                scoped.importKey(privateKey);""
            }catch(e:Exception){e.message?:"Не удалось зарегистрировать ключ"}finally{bytes?.fill(0);device?.delete()}
            runOnUiThread{checking=false;if(!isFinishing&&!isDestroyed){if(error.isEmpty()){render();message("Ключ телефона добавлен и проверен. Можно подключаться.")}else message(error)}}
        }.start()
    }
    @Deprecated("Platform activity result")
    override fun onActivityResult(requestCode:Int,resultCode:Int,data:Intent?){
        super.onActivityResult(requestCode,resultCode,data);if(resultCode!=RESULT_OK)return
        if(requestCode==103){attempt{startTunnel()};return}
        if(requestCode==102){val id=importId;val uri=data?.data?:return;checking=true
            Thread{
                val error=try{
                    require(store.server(id)!=null){"Сервер удалён"}
                    val bytes=contentResolver.openInputStream(uri)!!.use{input->val out=java.io.ByteArrayOutputStream();val buffer=ByteArray(8192);while(out.size()<=1048576){val count=input.read(buffer);if(count<0)break;out.write(buffer,0,count)};out.toByteArray()}
                    try{SettingsStore(this,id).importKey(bytes)}finally{bytes.fill(0)};""
                }catch(e:Exception){e.message?:"Не удалось импортировать ключ"}
                runOnUiThread{checking=false;if(!isFinishing&&!isDestroyed){if(error.isEmpty()){render();toast("SSH-ключ добавлен")}else message(error)}}
            }.start()
        }
    }
    private fun unlocked(){require(!TunnelVpnService.active){"Сначала остановите туннель, чтобы изменить подключение"};require(!checking){"Дождитесь завершения операции"}}
    private fun heading(title:String,subtitle:String){body.addView(label(title,26,true));body.addView(label(subtitle,13,false,Palette.muted),spaced(8))}
    private fun emptyState(title:String,detail:String){val card=card();card.addView(label(title,18,true));card.addView(label(detail,14,false,Palette.muted),spaced(10));body.addView(card,spaced(20))}
    private fun field(title:String,value:String,hint:String,idValue:Int,numeric:Boolean=false):EditText{
        body.addView(label(title,12,true,Palette.muted),spaced(16))
        return EditText(this).apply{id=idValue;setSingleLine(true);inputType=if(numeric)InputType.TYPE_CLASS_NUMBER else InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
            importantForAutofill=View.IMPORTANT_FOR_AUTOFILL_NO;setText(value);this.hint=hint;textSize=16f;setTextColor(Palette.text);setHintTextColor(Palette.muted)
            background=Palette.panel(Palette.surface,dp(13).toFloat(),Palette.border);setPadding(dp(14),0,dp(14),0);body.addView(this,LinearLayout.LayoutParams(-1,dp(54)).apply{topMargin=dp(7)})}
    }
    private fun card(color:Int=Palette.surface)=column().apply{setPadding(dp(18),dp(18),dp(18),dp(18));background=Palette.panel(color,dp(22).toFloat())}
    private fun column()=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL}
    private fun row()=LinearLayout(this).apply{orientation=LinearLayout.HORIZONTAL}
    private fun label(value:String,size:Int,bold:Boolean=false,color:Int=Palette.text)=TextView(this).apply{text=value;textSize=size.toFloat();setTextColor(color);if(bold)typeface=Typeface.create("sans-serif-medium",Typeface.NORMAL);includeFontPadding=false}
    private fun button(value:String,primary:Boolean=false,danger:Boolean=false,action:()->Unit)=Button(this).apply{
        text=value;isAllCaps=false;textSize=14f;minimumHeight=dp(50);setPadding(dp(12),dp(9),dp(12),dp(9))
        background=Palette.panel(if(primary)Palette.accent else Palette.raised,dp(14).toFloat(),if(primary)0 else Palette.border)
        setTextColor(if(danger)Palette.error else if(primary)Palette.background else Palette.accent);setOnClickListener{attempt(action)}
    }
    private fun spaced(top:Int)=LinearLayout.LayoutParams(-1,-2).apply{topMargin=dp(top)}
    private fun update(view:TextView?,value:String){if(view!=null&&view.text.toString()!=value)view.text=value}
    private fun confirmDelete(title:String,action:()->Unit){AlertDialog.Builder(this).setTitle(title).setNegativeButton("Отмена",null).setPositiveButton("Удалить"){_,_->attempt(action)}.show()}
    private fun attempt(action:()->Unit){try{action()}catch(e:Exception){message(e.message.orEmpty())}}
    private fun toast(value:String){Toast.makeText(this,value,Toast.LENGTH_SHORT).show()}
    private fun message(value:String){AlertDialog.Builder(this).setMessage(value).setPositiveButton("Понятно",null).show()}
    private fun dp(value:Int)=(resources.displayMetrics.density*value).toInt()
}
