package dev.secrettunnel.android

import android.app.*
import android.content.*
import android.content.ClipboardManager
import android.content.pm.PackageManager
import android.content.res.ColorStateList
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.Rect
import android.graphics.Typeface
import android.graphics.drawable.Drawable
import android.net.VpnService
import android.os.*
import android.text.*
import android.view.*
import android.widget.*
import androidx.core.view.WindowCompat
import org.json.JSONObject
import java.util.UUID

class MainActivity:LocalizedActivity() {
    private companion object { const val UPDATE_INSTALL_REQUEST = 104 }
    private lateinit var store:SettingsStore
    private lateinit var palette: Palette
    private lateinit var uiContext: Context
    private lateinit var content:FrameLayout
    private lateinit var body:LinearLayout
    private var pageScroll: ScrollView? = null
    private lateinit var badge:TextView
    private lateinit var tunnelLogo: TunnelLogo
    private val isTv by lazy {
        packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK) ||
            resources.configuration.uiMode and Configuration.UI_MODE_TYPE_MASK == Configuration.UI_MODE_TYPE_TELEVISION
    }
    private val pageInset get() = if (isTv) 28 else 20
    private var page="home"
    private var editingId=""
    private var importId=""
    private var pendingProxy=""
    private var checking=false
    private var checkingDetail=tr("Проверяем доступ к серверу…")
    private var pendingTrust=false
    private enum class CheckState { RUNNING, PASSED, FAILED }
    private data class ServerCheck(val id:String, val state:CheckState, val text:String)
    private var serverCheck:ServerCheck?=null
    private var sshCheckButton:Button?=null
    private var fingerprintCheckButton:Button?=null
    private var serverCheckView:TextView?=null
    private var fingerprintView:TextView?=null
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
    private var connectionLayers:TextView?=null
    private var connectionProbes:TextView?=null
    private var checkConnectionButton:Button?=null
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
    private var exitView:TextView?=null
    private var revealIp=false
    private var statsPeriod="week"
    private var speedKind="quick"
    private var speedDirect=false
    private var speedProvider="cloudflare"
    private var speedViews:SpeedViews?=null
    private var speedWasRunning=false
    private var aboutUpdate: AboutUpdateViews? = null
    private var updateNotice: Button? = null
    private var shownUpdate: MobileUpdates.Status? = null
    private var agentServerId: String? = null
    private var agentBusy = false
    private var agentResult = ""
    private var agentError = false
    private var agentViews: AgentViews? = null
    private var helpText: String? = null
    private var helpOnline = false
    private var helpLoading = false
    private var helpQuery = ""
    private var helpSection = "setup"
    private val installedVersion by lazy { MobileUpdates.installedVersion(this) }
    private class AboutUpdateViews(val status: TextView, val check: Button, val actions: LinearLayout)
    private class AgentViews(val server: Button, val check: Button, val install: Button, val status: TextView)
    private class SpeedViews(val status:TextView,val steps:TextView,val down:TextView,val up:TextView,val ping:TextView,val start:Button)
    private val speedTick=object:Runnable{
        override fun run(){
            if(page!="speed")return
            refreshSpeed()
            main.postDelayed(this,300)
        }
    }
    private val main=Handler(Looper.getMainLooper())
    private val meter=TrafficMeter()
    private var lastStatus=JSONObject()
    private var previousConnectionFailure: PreviousConnectionFailure? = null
    private var startupFailed = false
    private var catalog=listOf<AppEntry>()
    private var catalogLoaded=false
    private var catalogError: String? = null
    private var watchingPackages = false
    private var scanningApps = false
    private var rescanApps = false
    private val packageChanges = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action == Intent.ACTION_PACKAGE_REMOVED &&
                intent.getBooleanExtra(Intent.EXTRA_REPLACING, false)) return
            scanApps()
        }
    }
    private val selected=mutableSetOf<String>()
    private val protectedApps = mutableSetOf<String>()
    private val iconCache=android.util.LruCache<AppEntry,Drawable>(32)
    private data class AppEntry(val pkg:String,val label:String,val sourceDir:String)
    private val poll=object:Runnable {
        override fun run(){
            try{
                if (TunnelVpnService.nativeStatusAvailable) {
                    lastStatus=JSONObject(NativeBridge.status())
                }
                val rx=lastStatus.optLong("rx");val tx=lastStatus.optLong("tx")
                val rate=meter.sample(SystemClock.elapsedRealtime(),rx,tx)
                val phase=TunnelVpnService.phase
                refreshHeader(phase)
                update(heroTitle,when{checking->tr("Проверяем сервер");pendingTrust->tr("Подтвердите ключ сервера");phase=="Работает"->tr("Туннель работает");TunnelVpnService.active->phase;phase.contains("Ошибка")->tr("Не удалось подключиться");store.activeProxy==null->tr("Добавьте подключение");else->if(readinessAction()==tr("Подключить"))tr("Готов к подключению") else tr("Настройте подключение")})
                update(heroDetail,when{checking->checkingDetail;pendingTrust->tr("Сверьте отпечаток в открытом диалоге");phase=="Работает"->activeTunnelDetails();TunnelVpnService.active->TunnelVpnService.detail;phase.contains("Ошибка")->friendlyError(TunnelVpnService.detail);else->tr("Выберите профиль и приложения. Остальной трафик идёт напрямую.")})
                listOf(nameField,hostField,portField,userField,serverSpinner).forEach{it?.isEnabled=!checking&&!pendingTrust&&!TunnelVpnService.active}
                refreshServerCheck()
                connectButton?.apply{val value=if(TunnelVpnService.active)tr("Остановить") else readinessAction();if(text.toString()!=value)text=value;isEnabled=!checking&&!pendingTrust&&phase!=tr("Остановка");alpha=if(isEnabled)1f else .45f}
                update(downSpeed,TrafficMeter.bytes(rate.down)+tr("/с"));update(upSpeed,TrafficMeter.bytes(rate.up)+tr("/с"))
                update(downTotal,tr("Всего: ")+TrafficMeter.bytes(rx.toDouble()));update(upTotal,tr("Всего: ")+TrafficMeter.bytes(tx.toDouble()))
                update(exitView,exitLine())
                refreshConnectionHealth()
                if(page=="home")refreshTraffic()
                if(page=="logs")update(logView,diagnostics())
                if(page=="about")refreshUpdateCard()
                updateNotice?.let { notice ->
                    val version = newerRelease()?.version
                    notice.visibility = if (version == null) View.GONE else View.VISIBLE
                    if (version != null) update(notice, tr("Доступна версия {0}", version))
                }
            }catch(e:Exception){update(heroDetail,e.message.orEmpty())}
            main.postDelayed(this,1000)
        }
    }
    override fun onCreate(savedInstanceState:Bundle?){
        super.onCreate(savedInstanceState)
        try {
            initialize(savedInstanceState)
        } catch (error: Exception) {
            showStartupError(error)
        } catch (error: LinkageError) {
            showStartupError(error)
        }
    }

    private fun initialize(savedInstanceState: Bundle?) {
        store=SettingsStore(this);selected.addAll(store.packages);protectedApps.addAll(store.protectedPackages)
        applyAppearance(store.appearance.isDark(resources.configuration))
        page=savedInstanceState?.getString("page")?.takeUnless{isTv&&it=="more"}?:"home"
        editingId=savedInstanceState?.getString("editingId").orEmpty()
        helpSection=savedInstanceState?.getString("helpSection")?:"setup"
        helpQuery=savedInstanceState?.getString("helpQuery").orEmpty()
        agentServerId=savedInstanceState?.getString("agentServerId")
        chartRange=ChartRange.entries.firstOrNull{it.name==savedInstanceState?.getString("chartRange")}?:ChartRange.MINUTE
        importId=savedInstanceState?.getString("importId").orEmpty();pendingProxy=savedInstanceState?.getString("pendingProxy").orEmpty()
        searchQuery=savedInstanceState?.getString("search").orEmpty();formDraft=savedInstanceState?.getBundle("draft")
        statsPeriod=savedInstanceState?.getString("statsPeriod")?:"week";speedKind=savedInstanceState?.getString("speedKind")?:"quick"
        speedDirect=savedInstanceState?.getBoolean("speedDirect")?:false;speedProvider=savedInstanceState?.getString("speedProvider")?:"cloudflare"
        listOf("enrollment-key.runtime","device-key.runtime").forEach{java.io.File(filesDir,it).delete()}
        if(Build.VERSION.SDK_INT>=33)onBackInvokedDispatcher.registerOnBackInvokedCallback(android.window.OnBackInvokedDispatcher.PRIORITY_DEFAULT){handleBack()}
        previousConnectionFailure = ConnectionAttempt.previousFailure(this)
        render()
        if (savedInstanceState == null) previousConnectionFailure?.let { failure ->
            ConnectionAttempt.consumeStartupWarning(this, failure, TunnelVpnService.active)?.let { warning ->
                AlertDialog.Builder(uiContext).setTitle(tr("Предыдущий запуск туннеля прервался"))
                    .setMessage(warning).setPositiveButton(tr("Понятно"), null).show()
            }
        }
        if (store.checkUpdates) MobileUpdates.check(automatic = true)
        if(Build.VERSION.SDK_INT>=33&&checkSelfPermission("android.permission.POST_NOTIFICATIONS")!=PackageManager.PERMISSION_GRANTED)requestPermissions(arrayOf("android.permission.POST_NOTIFICATIONS"),101)
    }

    private fun showStartupError(error: Throwable) {
        startupFailed = true
        android.util.Log.e("SecretTunnelStartup", tr("Не удалось открыть приложение"), error)
        val message = listOfNotNull(error.javaClass.simpleName, error.localizedMessage).joinToString("\n")
        val device = "Android ${Build.VERSION.RELEASE}, API ${Build.VERSION.SDK_INT}\n${Build.SUPPORTED_ABIS.joinToString()}"
        setContentView(ScrollView(this).apply {
            setBackgroundColor(android.graphics.Color.BLACK)
            addView(TextView(this@MainActivity).apply {
                text = tr("Secret Tunnel\n\nНе удалось открыть приложение\n\n{0}\n\n{1}", message, device)
                textSize = 20f
                setTextColor(android.graphics.Color.WHITE)
                setPadding(dp(32), dp(24), dp(32), dp(24))
            })
        })
    }

    override fun onStart() {
        super.onStart()
        if (startupFailed) return
        registerReceiver(packageChanges, IntentFilter().apply {
            addAction(Intent.ACTION_PACKAGE_ADDED)
            addAction(Intent.ACTION_PACKAGE_REMOVED)
            addAction(Intent.ACTION_PACKAGE_CHANGED)
            addDataScheme("package")
        })
        watchingPackages = true
        scanApps()
    }

    override fun onStop() {
        if (watchingPackages) {
            unregisterReceiver(packageChanges)
            watchingPackages = false
        }
        super.onStop()
    }

    override fun onResume(){super.onResume();if(startupFailed)return;main.removeCallbacks(poll);main.post(poll);if(page=="speed"){main.removeCallbacks(speedTick);main.post(speedTick)}}
    override fun onPause(){main.removeCallbacks(poll);main.removeCallbacks(speedTick);super.onPause()}
    override fun onSaveInstanceState(out:Bundle){
        out.putString("page",page);out.putString("editingId",editingId);out.putString("importId",importId);out.putString("pendingProxy",pendingProxy);out.putString("search",searchQuery)
        out.putString("chartRange",chartRange.name)
        out.putString("helpSection",helpSection);out.putString("helpQuery",helpQuery)
        out.putString("agentServerId",agentServerId)
        out.putString("statsPeriod",statsPeriod);out.putString("speedKind",speedKind);out.putBoolean("speedDirect",speedDirect);out.putString("speedProvider",speedProvider)
        if(page.endsWith("Editor")) out.putBundle("draft", captureEditorDraft())
        super.onSaveInstanceState(out)
    }

    private fun captureEditorDraft() = Bundle().apply {
        putString("name",nameField?.text.toString());putString("host",hostField?.text.toString())
        putString("port",portField?.text.toString());putString("user",userField?.text.toString())
        putInt("serverPosition",serverSpinner?.selectedItemPosition?:0)
        putString("udpReceive", proxyUdpReceive?.toJson()?.toString())
        putString("udpAgentReceive", proxyUdpAgentReceive?.toJson()?.toString())
        putString("ssh",proxySsh.toJson().toString())
    }

    private fun applyAppearance(dark: Boolean) {
        palette = if (dark) Palette.dark else Palette.light
        uiContext = ContextThemeWrapper(this, if (dark) R.style.Theme_SecretTunnel_Dark else R.style.Theme_SecretTunnel_Light)
        window.statusBarColor = palette.background
        window.navigationBarColor = palette.background
        WindowCompat.getInsetsController(window, window.decorView).apply {
            isAppearanceLightStatusBars = !dark
            isAppearanceLightNavigationBars = !dark
        }
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        if (startupFailed) return
        val dark = store.appearance.isDark(newConfig)
        if ((palette === Palette.dark) == dark) return
        if (page.endsWith("Editor")) formDraft = captureEditorDraft()
        applyAppearance(dark)
        render(keepTvPosition = true)
    }

    @Deprecated("Platform back callback")
    override fun onBackPressed(){handleBack()}
    private fun handleBack(){when{
        startupFailed->finish()
        page.endsWith("Editor")->go(if(page=="serverEditor")"servers" else "proxies")
        page=="help"->go("about")
        !isTv && page in moreSections->go("more")
        page!="home"->go("home")
        else->finish()
    }}
    private val moreSections=setOf("servers","proxies","settings","logs","about","help","stats")
    private val phoneDestinations=listOf(
        "home" to tr("Главная"), "apps" to tr("Приложения"), "catalog" to tr("Каталог"),
        "speed" to tr("Скорость"), "more" to tr("Ещё"),
    )
    private val tvDestinations=listOf(
        "home" to tr("Главная"), "apps" to tr("Приложения"), "servers" to tr("Серверы"), "proxies" to tr("Прокси"),
        "catalog" to tr("Каталог серверов"),
        "speed" to tr("Скорость"), "stats" to tr("Статистика"), "logs" to tr("Журнал"),
        "settings" to tr("Настройки"), "about" to tr("О программе"), "help" to tr("Справка"),
    )
    private fun go(destination:String){
        if(checking||pendingTrust){toast(tr("Дождитесь завершения проверки"));return}
        if(page.endsWith("Editor")&&formSignature()!=formSnapshot)AlertDialog.Builder(uiContext).setTitle(tr("Сохранить изменения?")).setMessage(tr("В форме есть несохранённые данные."))
            .setNegativeButton(tr("Остаться"),null).setNeutralButton(tr("Не сохранять")){_,_->navigate(destination)}.setPositiveButton(tr("Сохранить")){_,_->attempt{saveEditor();navigate(destination)}}.show()
        else navigate(destination)
    }
    private fun navigate(destination:String){
        if (destination == "catalog") { startActivity(Intent(this, HostingCatalogActivity::class.java)); return }
        val enteringApps = destination == "apps" && page != destination
        page=destination;formDraft=null;render()
        if (enteringApps) scanApps()
    }

    private fun refreshHeader(phase: String) {
        val active = TunnelVpnService.active
        val state = when {
            checking -> tr("Проверка")
            pendingTrust -> tr("Подтверждение")
            phase == "Работает" -> tr("Подключено")
            active -> phase
            phase.contains("Ошибка") -> "Ошибка"
            else -> "Остановлен"
        }
        update(badge, state)
        badge.setTextColor(when {
            state == tr("Подключено") -> palette.accent
            state == "Ошибка" -> palette.error
            active || checking -> palette.warning
            else -> palette.muted
        })
        tunnelLogo.show(when (phase) {
            "Работает" -> TunnelLogo.State.ON
            "Подключение", "Переподключение", "Смена сети", "Восстановление", "Ожидание сети" -> TunnelLogo.State.CONNECTING
            else -> TunnelLogo.State.OFF
        })
    }

    private fun render(keepTvPosition: Boolean = false){
        val oldScroll = pageScroll?.takeIf { isTv && keepTvPosition }
        val oldOffset = oldScroll?.scrollY
        val oldFocus = oldScroll?.findFocus()
        val focusId = oldFocus?.id ?: View.NO_ID
        val focusTag = oldFocus?.tag as? String
        pageScroll = null
        heroTitle=null;heroDetail=null;connectButton=null;downSpeed=null;upSpeed=null;downTotal=null;upTotal=null;chart=null;logView=null
        connectionLayers=null;connectionProbes=null;checkConnectionButton=null
        chartPeaks=null;appTrafficList=null;appTrafficEmpty=null;appTrafficTitle=null;appTrafficRows.clear()
        nameField=null;hostField=null;portField=null;userField=null;serverSpinner=null;exitView=null;speedViews=null
        sshCheckButton=null;fingerprintCheckButton=null;serverCheckView=null;fingerprintView=null
        aboutUpdate=null;shownUpdate=null;updateNotice=null;agentViews=null
        main.removeCallbacks(speedTick)
        proxyUdpReceive=null;proxyUdpAgentReceive=null
        val root=column().apply{
            setBackgroundColor(palette.background)
            isFocusableInTouchMode=!isTv
            if(isTv)setPadding(dp(32),dp(18),dp(32),dp(18))
        }
        val header=row().apply{setPadding(dp(pageInset),dp(15),dp(pageInset),dp(12));gravity=Gravity.CENTER_VERTICAL}
        val logoSize = dp(if (isTv) 36 else 30)
        tunnelLogo = TunnelLogo(this).also {
            header.addView(it, LinearLayout.LayoutParams(logoSize, logoSize).apply { marginEnd = dp(10) })
        }
        header.addView(label("Secret Tunnel",21,true),LinearLayout.LayoutParams(0,-2,1f))
        badge=label("Остановлен",11).apply{setPadding(dp(12),dp(7),dp(12),dp(7));background=palette.panel(palette.raised,dp(20).toFloat())};header.addView(badge);root.addView(header)
        refreshHeader(TunnelVpnService.phase)
        content=FrameLayout(this)
        val nav=(if(isTv)column() else row()).apply{
            setPadding(dp(8),dp(8),dp(8),dp(9))
            if(!isTv)setBackgroundColor(palette.surface)
        }
        val activePage=when(page){
            "serverEditor"->"servers"
            "proxyEditor"->"proxies"
            else->page
        }
        var activeNavigation:View?=null
        for((id,title) in if(isTv)tvDestinations else phoneDestinations){
            val active=activePage==id||(id=="more"&&activePage in moreSections)
            val destination=(if(isTv)row() else column()).apply{
                gravity=if(isTv)Gravity.CENTER_VERTICAL else Gravity.CENTER
                minimumHeight=dp(if(isTv)48 else 56);contentDescription=title
                if(isTv){setPadding(dp(12),0,dp(8),0);background=focusPanel(if(active)palette.raised else palette.surface,15)}
                else if(active)background=palette.panel(palette.raised,dp(15).toFloat())
                addView(NavigationIcon(this@MainActivity,id,if(active)palette.accent else palette.muted),LinearLayout.LayoutParams(dp(23),dp(23)))
                addView(label(title,if(isTv)14 else 11,active,if(active)palette.accent else palette.muted).apply{
                    maxLines=1
                    if(isTv)setPadding(dp(10),0,0,0)
                    else {gravity=Gravity.CENTER;setPadding(dp(2),dp(5),dp(2),0)
                        setAutoSizeTextTypeUniformWithConfiguration(9,11,1,android.util.TypedValue.COMPLEX_UNIT_SP)}
                },if(isTv)LinearLayout.LayoutParams(0,-2,1f) else LinearLayout.LayoutParams(-1,dp(24)))
                setOnClickListener{go(id)}
                if(isTv)isFocusable=true
            }
            nav.addView(destination,if(isTv) LinearLayout.LayoutParams(-1,-2) else LinearLayout.LayoutParams(0,-2,1f))
            if(active)activeNavigation=destination
        }
        if(isTv){
            val workspace=row()
            val menu=ScrollView(this).apply{
                background=palette.panel(palette.surface,dp(16).toFloat())
                isFocusable=false
                isScrollbarFadingEnabled=false
                addView(nav)
            }
            workspace.addView(menu,LinearLayout.LayoutParams(dp(180),-1))
            workspace.addView(content,LinearLayout.LayoutParams(0,-1,1f))
            root.addView(workspace,LinearLayout.LayoutParams(-1,0,1f))
        }else{
            root.addView(content,LinearLayout.LayoutParams(-1,0,1f));root.addView(nav)
        }
        setContentView(root)
        if(page=="apps")appsPage()else{
            body=column().apply{setPadding(dp(pageInset),dp(8),dp(pageInset),dp(20))}
            if(isTv&&page=="logs"){
                content.addView(body, FrameLayout.LayoutParams(-1,-1))
            }else{
                val scroll=ScrollView(this).apply{isFillViewport=true;clipToPadding=false;addView(body)}
                pageScroll=scroll
                content.addView(scroll)
            }
            when(page){"servers"->serversPage();"proxies"->proxiesPage();"logs"->logsPage();"serverEditor"->serverEditor();"proxyEditor"->proxyEditor()
                "speed"->speedPage();"stats"->statsPage();"more"->morePage();"settings"->settingsPage()
                "about"->aboutPage();"help"->helpPage();else->{page="home";homePage()}}
        }
        if(isTv){
            val scroll = pageScroll
            val target = scroll?.let {
                (if (focusId == View.NO_ID) null else it.findViewById<View>(focusId))
                    ?: focusTag?.let { tag -> it.findViewWithTag<View>(tag) }
            }
            if(target?.requestFocus()!=true)activeNavigation?.requestFocus()
            if(scroll!=null && oldOffset!=null){
                scroll.whenLaidOut {
                    scroll.scrollTo(0,oldOffset)
                    if(target?.isFocused==true && target!==scroll){
                        target.requestRectangleOnScreen(Rect(0,0,target.width,target.height),true)
                    }
                }
            }
        }else root.requestFocus()
    }
    private fun homePage(){
        val hero=card(palette.raised)
        heroTitle=label(tr("Готов к подключению"),23,true).also{hero.addView(it)}
        heroDetail=label(tr("Выберите профиль и приложения"),14,false,palette.muted).also{hero.addView(it,spaced(8))}
        if(store.servers().none{it.host.isNotBlank()&&SettingsStore(this,it.id).hasKey()})
            hero.addView(button(tr("Настроить подключение"),true){startActivityForResult(Intent(this,SetupActivity::class.java),105)},spaced(12))
        connectButton=button(tr("Подключить"),true){connectOrStop()};hero.addView(connectButton,spaced(18));body.addView(hero,spaced(12))
        updateNotice = button(tr("Обновления")) { go("about") }.apply {
            visibility = View.GONE
            body.addView(this, spaced(12))
        }
        val health=card()
        connectionLayers=label("",14,true).also{health.addView(it)}
        connectionProbes=label("",13,false,palette.muted).also{health.addView(it,spaced(6))}
        checkConnectionButton=button(tr("Проверить соединение")){
            val error=NativeBridge.checkConnection()
            if(error.isNotBlank())toast(error)
            else { lastStatus=JSONObject(NativeBridge.status());refreshConnectionHealth() }
        }.also{health.addView(it,spaced(8))}
        health.addView(label(tr("TCP и IPv6 проверяют открытие соединения. UDP и DNS используют запрос к 1.1.1.1. Проверки через SOCKS не проверяют захват приложений."),12,false,palette.muted),spaced(6))
        body.addView(health,spaced(12))
        refreshConnectionHealth()
        val proxy=store.activeProxy;val server=proxy?.let{store.server(it.serverId)};val profile=card()
        profile.addView(label((proxy?.name?:tr("Профиль не выбран"))+"  ›",16,true,palette.accent))
        profile.addView(label(server?.let{"${it.name} · ${it.host.ifBlank{tr("адрес не указан")}}"}?:tr("Добавьте сервер и прокси"),13,false,palette.muted),spaced(4))
        if (server?.jumpServerId != null) profile.addView(label(routeLabel(server.id),13,false,palette.muted),spaced(5))
        profile.setOnClickListener{go("proxies")};tvCardFocus(profile);profile.contentDescription=tr("Выбрать профиль подключения");body.addView(profile,spaced(12))
        val exit=card();exit.addView(label(tr("Выход в интернет"),12,true,palette.muted))
        exitView=label(exitLine(),15,true).also{exit.addView(it,spaced(6))}
        if(TunnelVpnService.active&&store.hideSensitive)exit.addView(label(tr("Нажмите, чтобы показать или скрыть IP"),11,false,palette.muted),spaced(6))
        exit.setOnClickListener{revealIp=!revealIp;update(exitView,exitLine())};tvCardFocus(exit);exit.contentDescription=tr("Внешний IP и страна выхода")
        body.addView(exit,spaced(12))
        val metrics=row()
        fun metric(title:String,color:Int,down:Boolean):LinearLayout{
            val card=card();card.addView(label(title,12,true,color));val speed=label(tr("0 B/с"),21,true).apply{setSingleLine(true);setAutoSizeTextTypeUniformWithConfiguration(14,21,1,android.util.TypedValue.COMPLEX_UNIT_SP)};card.addView(speed,spaced(10));val total=label(tr("Всего: 0 B"),11,false,palette.muted);card.addView(total,spaced(5))
            if(down){downSpeed=speed;downTotal=total}else{upSpeed=speed;upTotal=total};return card
        }
        metrics.addView(metric(tr("↓  Загрузка"),palette.accent,true),LinearLayout.LayoutParams(0,-1,1f).apply{marginEnd=dp(6)})
        metrics.addView(metric(tr("↑  Отправка"),palette.upload,false),LinearLayout.LayoutParams(0,-1,1f).apply{marginStart=dp(6)});body.addView(metrics,spaced(12))
        val graph=card()
        graph.addView(label(tr("График скорости"),14,true))
        val period=Spinner(uiContext).apply {
            contentDescription=tr("Период графика скорости")
            minimumHeight=dp(48)
            adapter=ArrayAdapter(uiContext,android.R.layout.simple_spinner_dropdown_item,ChartRange.entries.map{it.label})
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
        chart=NetworkChart(uiContext, palette).also{graph.addView(it,LinearLayout.LayoutParams(-1,dp(126)))}
        chartPeaks=label("",12,false,palette.muted).also{graph.addView(it,spaced(8))}
        body.addView(graph,spaced(12))
        val apps=card()
        appTrafficTitle=label(tr("Трафик приложений"),14,true).also{apps.addView(it)}
        apps.addView(label(tr("Скорость сейчас и объём за сеанс VPN"),12,false,palette.muted),spaced(5))
        appTrafficEmpty=label("",12,false,palette.muted).also{apps.addView(it,spaced(8))}
        appTrafficList=column().also{apps.addView(it)}
        body.addView(apps,spaced(12))
        refreshTraffic()
        body.addView(button(if(store.packages.isEmpty())tr("Выбрать приложения") else tr("Выбрать приложения · {0}", store.packages.size)){go("apps")},spaced(12))
    }

    private fun refreshConnectionHealth() {
        val phase=TunnelVpnService.phase
        val connected=lastStatus.optBoolean("running")
        val ssh=when {
            phase=="Ожидание сети" || phase=="Смена сети" || phase=="Переподключение" -> phase
            phase=="Остановка" -> tr("Отключается")
            connected -> tr("Подключён")
            lastStatus.optBoolean("starting") || phase=="Подключение" -> tr("Подключается")
            else -> tr("Отключён")
        }
        val interception=if(TunnelVpnService.interceptionActive) tr("Включён") else tr("Выключен")
        val priority = lastStatus.optJSONObject("priority_udp_ssh")?.takeIf { TunnelVpnService.interceptionActive }
        val priorityRoute = when (priority?.optBoolean("separate")) {
            true -> tr("\nUDP отмеченных приложений · Отдельный SSH")
            false -> tr("\nUDP отмеченных приложений · Основной SSH")
            null -> ""
        }
        update(connectionLayers, tr("SSH · {0}\nПерехват VPN · {1}{2}", ssh, interception, priorityRoute))
        val checks=lastStatus.optJSONObject("checks")
        val usable=connected && phase=="Работает"
        val running=usable && checks?.optBoolean("running")==true
        val report=if(usable)checks?.optJSONObject("report") else null
        val labels=listOf("tcp" to "TCP","udp" to "UDP","dns" to "DNS","ipv6" to "IPv6")
        val summary=labels.joinToString("\n") { (key,label) ->
            val result=report?.optJSONObject(key)
            val value=if(running)tr("Проверяем…") else when(result?.optString("status")) {
                "passed" -> tr("{0} мс", result.optLong("ms"))
                "failed" -> result.optString("detail").ifBlank{tr("Нет ответа")}
                else -> tr("Нет проверки")
            }
            "$label · $value"
        }
        update(connectionProbes,summary)
        checkConnectionButton?.setActionAvailable(usable && !running)
    }

    private fun refreshTraffic() {
        val window=TunnelVpnService.traffic.snapshot(chartRange)
        chart?.show(window)
        val wallOffset=System.currentTimeMillis()-SystemClock.elapsedRealtime()
        fun peak(down:Boolean):String {
            val point=window.points.maxByOrNull{if(down)it.rate.down else it.rate.up}?:return "—"
            val rate=if(down)point.rate.down else point.rate.up
            val time=TransportDiagnostics.timestamp(point.timeMillis+wallOffset)
            return if(rate<=0) "—" else TrafficMeter.bytes(rate)+tr("/с · ")+
                if(chartRange==ChartRange.SECOND||chartRange==ChartRange.FIVE_SECONDS)time else time.substringBefore('.')
        }
        update(chartPeaks,tr("Пик ↓ {0}\nПик ↑ {1}", peak(true), peak(false)))

        val list=appTrafficList?:return
        val entries=TunnelVpnService.appTraffic?.entries.orEmpty()
        val active=if(TunnelVpnService.active)entries.count{it.uid>=0&&(it.rate.down>0||it.rate.up>0)} else 0
        update(appTrafficTitle,if(active>0)tr("Трафик приложений · {0} сейчас", active) else tr("Трафик приложений"))
        update(appTrafficEmpty,when {
            Build.VERSION.SDK_INT<29 -> tr("Разбивка по приложениям доступна с Android 10.")
            entries.isNotEmpty() -> ""
            TunnelVpnService.active -> tr("Ожидаем трафик выбранных приложений…")
            else -> tr("Подключите туннель, чтобы увидеть трафик приложений.")
        })
        appTrafficEmpty?.visibility=if(entries.isEmpty())View.VISIBLE else View.GONE
        if(appTrafficRows.keys.toList()!=entries.map{it.uid}) {
            list.removeAllViews()
            appTrafficRows.clear()
            for(entry in entries) {
                val details=label("",12,false,palette.muted)
                list.addView(column().apply{
                    addView(label(entry.name,14,true))
                    addView(details,spaced(4))
                },spaced(12))
                appTrafficRows[entry.uid]=details
            }
        }
        for(entry in entries) {
            val rate=if(TunnelVpnService.active)entry.rate else TrafficMeter.Rate(0.0,0.0)
            update(appTrafficRows[entry.uid],tr("↓ {0}/с · ↑ {1}/с\nЗа сеанс ↓ {2} · ↑ {3}", TrafficMeter.bytes(rate.down), TrafficMeter.bytes(rate.up), TrafficMeter.bytes(entry.down.toDouble()), TrafficMeter.bytes(entry.up.toDouble())))
            appTrafficRows[entry.uid]?.setTextColor(if(rate.down>0||rate.up>0)palette.text else palette.muted)
        }
    }

    private fun serversPage(){
        heading(tr("Серверы"),tr("SSH-серверы, ключи доступа и доверие"))
        body.addView(button(tr("Каталог серверов")){go("catalog")},spaced(12))
        body.addView(button(tr("Мастер настройки"),true){startActivityForResult(Intent(this,SetupActivity::class.java),105)},spaced(16))
        body.addView(button(tr("Импортировать")){unlocked();startActivityForResult(Intent(this,ProfileTransferActivity::class.java),107)},spaced(12))
        val transferable=store.servers().filter{it.host.isNotBlank()&&it.user.isNotBlank()&&SettingsStore(this,it.id).hasKey()&&isTrustedSshPin(SettingsStore(this,it.id).fingerprint)}
        if(transferable.size>1)body.addView(button(tr("Поделиться всеми настроенными серверами")){
            startActivity(Intent(this,ProfileTransferActivity::class.java).putStringArrayListExtra("server_ids",ArrayList(transferable.map{it.id})))
        },spaced(12))
        body.addView(button(tr("+  Добавить сервер"),true){unlocked();editingId=UUID.randomUUID().toString();navigate("serverEditor")},spaced(16))
        for(server in store.servers()){
            val scoped=SettingsStore(this,server.id);val card=card();card.addView(label(server.name,18,true))
            card.addView(label(if(server.host.isBlank())tr("Укажите адрес и пользователя") else "${server.user}@${server.host}:${server.port}",13,false,palette.muted),spaced(6))
            if (server.jumpServerId != null) card.addView(label(routeLabel(server.id),13,false,palette.muted),spaced(6))
            card.addView(label(if(scoped.hasKey())tr("✓  Ключ доступа добавлен") else tr("○  Добавьте SSH-ключ"),12,false,if(scoped.hasKey())palette.accent else palette.warning),spaced(12))
            card.addView(label(if(isTrustedSshPin(scoped.fingerprint))tr("✓  Ключ сервера сохранён") else tr("○  Сервер ещё не проверен"),12,false,palette.muted),spaced(6))
            card.addView(button(tr("Настроить")){editingId=server.id;navigate("serverEditor")},spaced(14))
            if(scoped.hasKey())card.addView(button(tr("Поделиться")){
                startActivity(Intent(this,ProfileTransferActivity::class.java).putStringArrayListExtra("server_ids",arrayListOf(server.id)))
            },spaced(8))
            body.addView(card,spaced(12))
        }
        if(store.servers().isEmpty())emptyState(tr("Пока нет серверов"),tr("Добавьте SSH-сервер, чтобы создать подключение."))
    }
    private fun proxiesPage(){
        heading(tr("Прокси"),tr("Именованные SOCKS5-подключения через SSH"))
        body.addView(button(tr("+  Добавить прокси"),true){unlocked();require(store.servers().isNotEmpty()){tr("Сначала добавьте сервер")};editingId=UUID.randomUUID().toString();navigate("proxyEditor")},spaced(16))
        body.addView(label(tr("Одновременно используется один профиль. Локальный прокси доступен только на этом устройстве."),12,false,palette.muted),spaced(12))
        for(proxy in store.proxies()){
            val card=card();val title=row().apply{gravity=Gravity.CENTER_VERTICAL}
            title.addView(RadioButton(uiContext).apply{isChecked=proxy.id==store.activeProxyId;isClickable=false;isFocusable=false;buttonTintList=ColorStateList.valueOf(palette.accent);importantForAccessibility=View.IMPORTANT_FOR_ACCESSIBILITY_NO})
            title.addView(label(proxy.name,17,true),LinearLayout.LayoutParams(0,-2,1f));card.addView(title)
            card.addView(label("127.0.0.1:${proxy.port}  →  ${store.server(proxy.serverId)?.name?:tr("Сервер удалён")}",12,false,palette.muted),spaced(6))
            card.addView(label(if(proxy.id==store.activeProxyId)tr("Выбран для подключения") else tr("Нажмите, чтобы выбрать"),11,false,if(proxy.id==store.activeProxyId)palette.accent else palette.muted),spaced(10))
            card.tag = proxy.id
            card.setOnClickListener{attempt{unlocked();store.activeProxyId=proxy.id;render(keepTvPosition = true)}};tvCardFocus(card)
            card.addView(button(tr("Изменить")){unlocked();editingId=proxy.id;navigate("proxyEditor")},spaced(12));body.addView(card,spaced(12))
        }
        if(store.proxies().isEmpty())emptyState(tr("Создайте первый прокси"),tr("Выберите сервер, задайте название и локальный порт."))
    }
    private fun serverEditor(){
        val server=store.server(editingId)
        heading(if(server==null)tr("Новый сервер") else tr("Настройки сервера"),tr("Ключ доступа хранится отдельно для каждого сервера"))
        nameField=field(tr("Название"),server?.name.orEmpty(),tr("Например, Германия"),1001)
        hostField=field(tr("Адрес сервера"),server?.host.orEmpty(),tr("IP или имя хоста"),1002)
        portField=field(tr("Порт SSH"),(server?.port?:22).toString(),"22",1003,true)
        userField=field(tr("Пользователь"),server?.user.orEmpty(),tr("Имя пользователя"),1004)
        val servers = store.servers().filter { it.id != editingId }
        body.addView(label(tr("Подключаться через"),12,true,palette.muted),spaced(16))
        serverSpinner = serverPicker(listOf(null to tr("Напрямую")) + servers.map { it.id to it.name }, server?.jumpServerId)
            .apply { contentDescription = tr("Подключаться через") }
        val preview = label("",13,false,palette.muted)
        body.addView(preview,spaced(6))
        serverSpinner?.onItemSelectedListener = object : android.widget.AdapterView.OnItemSelectedListener {
            override fun onNothingSelected(parent: android.widget.AdapterView<*>?) = Unit
            override fun onItemSelected(parent: android.widget.AdapterView<*>?, view: View?, position: Int, id: Long) {
                val hop = selectedServerInEditor()
                val destination = nameField?.text.toString().ifBlank { tr("Конечный сервер") }
                preview.text = if (hop == null) tr("Прямое подключение") else runCatching {
                    val draft = ServerProfile(editingId, destination, "", 22, "", hop)
                    resolveServerRoute(editingId, servers + draft).joinToString(" → ") { it.name }
                }.getOrElse { it.message.orEmpty() }
            }
        }
        val originalSignature=formSignature();restoreDraft()
        val scoped=SettingsStore(this,editingId);val keyCard=card();keyCard.addView(label(tr("Доступ к серверу"),17,true))
        keyCard.addView(label(if(scoped.hasKey())tr("SSH-ключ добавлен и зашифрован") else tr("Импортируйте разрешённый на сервере ключ без пароля"),13,false,palette.muted),spaced(10))
        keyCard.addView(button(if(scoped.hasKey())tr("Заменить SSH-ключ") else tr("Импортировать SSH-ключ")){
            unlocked();saveServerForm();importId=editingId;startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).setType("*/*").addCategory(Intent.CATEGORY_OPENABLE),102)
        },spaced(14))
        keyCard.addView(button(if(isTv)tr("Создать ключ для телевизора") else tr("Создать отдельный ключ телефона")){
            unlocked();saveServerForm();require(scoped.hasKey()){tr("Сначала импортируйте уже разрешённый SSH-ключ")};verifyServer(editingId){enrollDevice(editingId)}
        },spaced(8));body.addView(keyCard,spaced(20))
        val trust=card();trust.addView(label(tr("Подлинность сервера"),17,true))
        fingerprintView=label(if(isTrustedSshPin(scoped.fingerprint))scoped.fingerprint else tr("Отпечаток будет получен автоматически"),12,false,palette.muted)
            .apply{setTextIsSelectable(true)}
        trust.addView(fingerprintView,spaced(10))
        fingerprintCheckButton=button(tr("Проверить отпечаток")){startServerCheck(null)}
        trust.addView(fingerprintCheckButton,spaced(14))
        val isRelay = store.servers().any { it.jumpServerId == editingId }
        sshCheckButton=button(if(isRelay) tr("Проверить SSH") else tr("Проверить SSH и агент")){startServerCheck(!isRelay)}
        trust.addView(sshCheckButton,spaced(8))
        serverCheckView=label("",13,false,palette.muted).apply {
            accessibilityLiveRegion=View.ACCESSIBILITY_LIVE_REGION_POLITE
        }
        trust.addView(serverCheckView,spaced(10));body.addView(trust,spaced(12))
        body.addView(button(tr("Сохранить сервер"),true){unlocked();saveServerForm();navigate("servers")},spaced(20))
        if(server!=null)body.addView(button(tr("Удалить сервер"),danger=true){unlocked();confirmDelete(tr("Удалить сервер {0}?", server.name)){store.deleteServer(server.id);navigate("servers")}},spaced(8))
        formSnapshot=originalSignature
        if(TunnelVpnService.active) listOf(nameField,hostField,portField,userField,serverSpinner).forEach{it?.isEnabled=false}
        refreshServerCheck()
    }
    private fun proxyEditor(){
        val proxy=store.proxies().firstOrNull{it.id==editingId};val servers=store.servers()
        heading(if(proxy==null)tr("Новый прокси") else tr("Настройки прокси"),tr("SOCKS5 · TCP и UDP через выбранный SSH-сервер"))
        nameField=field(tr("Название"),proxy?.name.orEmpty(),tr("Например, Рабочий"),1001)
        body.addView(label(tr("SSH-сервер"),12,true,palette.muted),spaced(16))
        serverSpinner = serverPicker(servers.map { it.id to "${it.name} · ${it.host.ifBlank{tr("не настроен")}}" }, proxy?.serverId)
        portField=field(tr("Локальный порт"),(proxy?.port?:1080).toString(),"1024–65535",1003,true)
        proxyUdpReceive = proxy?.udpReceive
        proxyUdpAgentReceive = proxy?.udpAgentReceive
        proxySsh=proxy?.ssh?:SshSettings()
        val originalSignature=formSignature()
        restoreDraft()
        body.addView(label(tr("Адрес: 127.0.0.1. Порт используется локальным SOCKS5-прокси и VPN выбранных приложений."),13,false,palette.muted),spaced(14))
        val settingsPath=if(isTv)tr("«Настройки»") else tr("«Ещё» → «Настройки»")
        body.addView(label(tr("Шифрование, очередь, приоритет звонков и окна UDP — в разделе {0}. Они общие для всех подключений.", settingsPath),12,false,palette.muted),spaced(10))
        body.addView(button(tr("Сохранить прокси"),true){unlocked();saveProxyForm();navigate("proxies")},spaced(20))
        if(proxy!=null)body.addView(button(tr("Удалить прокси"),danger=true){unlocked();confirmDelete(tr("Удалить прокси {0}?", proxy.name)){store.deleteProxy(proxy.id);navigate("proxies")}},spaced(8))
        formSnapshot=originalSignature
    }
    private fun serverPicker(choices: List<Pair<String?, String>>, selected: String?): Spinner = Spinner(uiContext).apply {
        adapter = object : ArrayAdapter<String>(uiContext, android.R.layout.simple_spinner_item, choices.map { it.second }) {
            override fun getView(position: Int, convertView: View?, parent: ViewGroup): View =
                super.getView(position, convertView, parent).apply { (this as TextView).setTextColor(palette.text); textSize = 15f }
        }.apply { setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item) }
        tag = choices.map { it.first }
        setSelection(choices.indexOfFirst { it.first == selected }.coerceAtLeast(0))
        backgroundTintList = ColorStateList.valueOf(palette.accent)
        if (isTv) {
            id = R.id.server_picker
            isFocusable = true
            foreground = focusPanel(Color.TRANSPARENT, 12)
        }
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
        val serverId=selectedServerInEditor() ?: error(tr("Выберите сервер"))
        require(store.server(serverId)!=null){tr("Этот сервер удалён. Откройте форму заново и выберите другой.")}
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

    /** Shared with the desktop's «Для опытных: встроенный SSH»: the same names and choices. */
    private fun tuningCard(): LinearLayout = card().apply {
        var tuning=store.tuning
        fun save(next:Tuning){tuning=next;store.tuning=next}
        addView(label(tr("Для опытных: встроенный SSH"),16,true))
        addView(label(tr("Применяется при следующем подключении ко всем серверам цепочки."),12,false,palette.muted),spaced(8))
        fun choice(caption:()->String,title:String,names:List<String>,selected:()->Int,pick:(Int)->Unit){
            lateinit var view:Button
            view=button(caption()){
                AlertDialog.Builder(uiContext).setTitle(title)
                    .setSingleChoiceItems(names.toTypedArray(),selected()){dialog,index->pick(index);view.text=caption();dialog.dismiss()}
                    .setNegativeButton(tr("Отмена"),null).show()
            }
            addView(view,spaced(8))
        }
        val ciphers=listOf("auto","aes128_gcm","chacha20_poly1305");val cipherNames=listOf(tr("Авто"),"AES-128-GCM","ChaCha20-Poly1305")
        choice({tr("Шифрование · {0}", cipherNames[ciphers.indexOf(tuning.ssh.cipher)])},tr("Шифрование"),cipherNames,{ciphers.indexOf(tuning.ssh.cipher)}){save(tuning.copy(ssh=tuning.ssh.copy(cipher=ciphers[it])))}
        val queues=listOf(tr("Стандартная · 4 МиБ"),tr("Уменьшенная · 1 МиБ · для тестов"))
        choice({tr("Очередь SSH · {0}", queues[if(tuning.ssh.smallQueue)1 else 0])},tr("Очередь SSH"),queues,{if(tuning.ssh.smallQueue)1 else 0}){save(tuning.copy(ssh=tuning.ssh.copy(smallQueue=it==1)))}
        val qos=listOf("balanced","voice","throughput");val qosNames=listOf(tr("Сбалансированный (рекомендуется)"),tr("Голос"),tr("Максимальная скорость"))
        val prioritySettings = column()
        choice({tr("Приоритет звонков · {0}", qosNames[qos.indexOf(tuning.ssh.qos)])},tr("Приоритет звонков"),qosNames,{qos.indexOf(tuning.ssh.qos)}){
            save(tuning.copy(ssh=tuning.ssh.copy(qos=qos[it])))
            prioritySettings.visibility = if (tuning.ssh.qos == "voice") View.VISIBLE else View.GONE
        }
        addView(label(tr("Во время звонков туннель придерживает загрузки, чтобы голос не прерывался. «Голос» придерживает сильнее, «Максимальная скорость» не придерживает. Уменьшенная очередь нужна для сравнения задержек и может ограничить скорость."),12,false,palette.muted),spaced(8))
        if (Build.VERSION.SDK_INT >= 29) {
            settingsToggle(prioritySettings, tr("Отдельный SSH для UDP"),
                tr("Отметьте приложения во вкладке «Приложения», затем переподключите туннель."),
                tuning.ssh.priorityUdpSsh) { save(tuning.copy(ssh = tuning.ssh.copy(priorityUdpSsh = it))) }
            prioritySettings.visibility = if (tuning.ssh.qos == "voice") View.VISIBLE else View.GONE
            addView(prioritySettings)
        }
        fun window(title:String,current:()->UdpWindow?,saveWindow:(UdpWindow?)->Unit){
            lateinit var view:Button
            view=button("$title · ${udpWindowLabel(current())}"){chooseUdpWindow(title,current()){saveWindow(it);view.text="$title · ${udpWindowLabel(it)}"}}
            addView(view,spaced(8))
        }
        window(tr("UDP · загрузка"),{tuning.udpReceive}){save(tuning.copy(udpReceive=it))}
        window(tr("UDP · отправка"),{tuning.udpAgentReceive}){save(tuning.copy(udpAgentReceive=it))}
        addView(label(tr("Окно UDP — сколько данных одного UDP-соединения туннель принимает без подтверждения. Больше окно — быстрее видео, звонки и QUIC на дальних маршрутах. «По умолчанию» для загрузки — 1 МиБ, для отправки — значение агента."),12,false,palette.muted),spaced(8))
    }

    private fun plural(n:Int,one:String,few:String,many:String)=when{AppLocalization.language==AppLanguage.ENGLISH->if(n==1)one else few;n%10==1&&n%100!=11->one;n%10 in 2..4&&n%100 !in 12..14->few;else->many}
    private fun udpWindowLabel(window: UdpWindow?): String = window?.let {
        (if(it.kib%1024==0)tr("{0} МиБ", it.kib/1024) else tr("{0} КиБ", it.kib))+" · ${it.slots} "+plural(it.slots,tr("слот"),tr("слота"),tr("слотов"))
    } ?: tr("По умолчанию")

    private fun chooseUdpWindow(title: String, current: UdpWindow?, save: (UdpWindow?) -> Unit) {
        val presets = listOf(null, UdpWindow(256, 192), UdpWindow(1024, 512))
        val options = (presets.map(::udpWindowLabel) + tr("Вручную")).toTypedArray()
        val selected = presets.indexOf(current).let { if (it < 0) presets.size else it }
        AlertDialog.Builder(uiContext).setTitle(title)
            .setSingleChoiceItems(options, selected) { dialog, index ->
                dialog.dismiss()
                if (index == presets.size) editUdpWindow(title, current, save)
                else save(presets[index])
            }
            .setNegativeButton(tr("Отмена"), null).show()
    }

    private fun editUdpWindow(title: String, current: UdpWindow?, save: (UdpWindow?) -> Unit) {
        val form = column().apply { setPadding(dp(24), dp(8), dp(24), dp(8)) }
        fun number(caption: String, value: Int): EditText {
            form.addView(label(caption, 14, true), spaced(10))
            return EditText(uiContext).apply {
                setSingleLine(true)
                inputType = InputType.TYPE_CLASS_NUMBER
                contentDescription = caption
                setText(value.toString())
                form.addView(this, LinearLayout.LayoutParams(-1, -2))
            }
        }
        val kib = number(tr("Окно, КиБ (64–1024)"), current?.kib ?: 512)
        val slots = number(tr("Слоты (4–512)"), current?.slots ?: 256)
        val dialog = AlertDialog.Builder(uiContext).setTitle(title)
            .setView(form).setNegativeButton(tr("Отмена"), null)
            .setPositiveButton(tr("Применить"), null).create()
        dialog.setOnShowListener {
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
                val bytesValue = kib.text.toString().toIntOrNull()
                val slotsValue = slots.text.toString().toIntOrNull()
                when {
                    bytesValue == null || bytesValue !in 64..1024 -> kib.error = tr("Введите от 64 до 1024 КиБ")
                    slotsValue == null || slotsValue !in 4..512 -> slots.error = tr("Введите от 4 до 512 слотов")
                    else -> {
                        save(UdpWindow(bytesValue, slotsValue))
                        dialog.dismiss()
                    }
                }
            }
        }
        dialog.show()
    }

    private fun morePage(){
        heading(tr("Ещё"),tr("Подключения, настройки и журнал"))
        fun item(id:String,title:String,detail:String){
            val card=card();card.addView(label("$title  ›",17,true,palette.accent));card.addView(label(detail,13,false,palette.muted),spaced(6))
            card.setOnClickListener{go(id)};tvCardFocus(card);card.contentDescription=title;body.addView(card,spaced(12))
        }
        item("servers",tr("Серверы"),tr("SSH-серверы, ключи доступа, цепочки и проверка агента · {0}", store.servers().size))
        item("stats",tr("Статистика"),tr("Трафик текущего сеанса"))
        item("proxies",tr("Прокси"),tr("Профили подключения и локальный порт SOCKS5 · {0}", store.proxies().size))
        item("settings",tr("Настройки"),tr("Тема, подключение и параметры SSH"))
        item("logs",tr("Журнал"),tr("Диагностика подключения и ошибок"))
        item("about",tr("О программе"),tr("Версия {0} · обновления, агент и справка", MobileUpdates.versionName(this)))
    }

    private fun settingsToggle(parent:LinearLayout,title:String,hint:String,value:Boolean,change:(Boolean)->Unit){
            val line=row().apply{gravity=Gravity.CENTER_VERTICAL}
            val texts=column();texts.addView(label(title,15,true));texts.addView(label(hint,12,false,palette.muted),spaced(4))
            line.addView(texts,LinearLayout.LayoutParams(0,-2,1f))
            val switch=tunnelSwitch().apply{isChecked=value;contentDescription=title;tag=title
                setOnCheckedChangeListener{_,on->change(on)}}
            line.addView(switch)
            if(!isTv)line.setOnClickListener{switch.isChecked=!switch.isChecked}
            parent.addView(line,spaced(14))
    }

    private fun settingsPage(){
        heading(tr("Настройки"),tr("Общие для всех подключений, как в приложении для компьютера"))
        val appearance = card()
        appearance.addView(label(tr("Оформление"), 16, true))
        appearance.addView(button(tr("Язык") + " · " + AppLocalization.language.label) {
            val languages = AppLanguage.entries
            AlertDialog.Builder(uiContext).setTitle(tr("Язык"))
                .setSingleChoiceItems(languages.map { it.label }.toTypedArray(), languages.indexOf(AppLocalization.language)) { dialog, index ->
                    dialog.dismiss()
                    if (languages[index] != AppLocalization.language) {
                        AppLocalization.select(this, languages[index])
                        recreate()
                    }
                }.setNegativeButton(tr("Отмена"), null).show()
        }, spaced(10))
        val themes = Appearance.entries
        val current = store.appearance
        appearance.addView(button(tr("Тема · {0}", current.label)) {
            AlertDialog.Builder(uiContext).setTitle(tr("Тема"))
                .setSingleChoiceItems(themes.map { it.label }.toTypedArray(), themes.indexOf(current)) { dialog, index ->
                    dialog.dismiss()
                    val chosen = themes[index]
                    if (chosen == current) return@setSingleChoiceItems
                    store.appearance = chosen
                    applyAppearance(chosen.isDark(resources.configuration))
                    render(keepTvPosition = true)
                }.setNegativeButton(tr("Отмена"), null).show()
        }.apply { tag = "appearance" }, spaced(8))
        body.addView(appearance, spaced(16))

        val connection=card();connection.addView(label(tr("Подключение"),16,true))
        settingsToggle(connection,tr("Проверять прокси после подключения"),tr("Проверяет TCP, UDP, DNS и IPv6 через локальный SOCKS и запрашивает внешний IP и страну."),store.verifyProxy){store.verifyProxy=it}
        connection.addView(label(tr("При смене сети или обрыве SSH приложение переподключается автоматически, пока VPN включён."),12,false,palette.muted),spaced(14))
        body.addView(connection,spaced(16))
        val privacy=card();privacy.addView(label(tr("Конфиденциальность"),16,true))
        settingsToggle(privacy,tr("Скрывать IP-адреса"),tr("Адреса закрыты точками, пока не нажать на них. Удобно для скриншотов."),store.hideSensitive){store.hideSensitive=it;revealIp=false}
        body.addView(privacy,spaced(12))
        val updates = card()
        updates.addView(label(tr("Обновления"),16,true))
        startupUpdateToggle(updates)
        updates.addView(button(tr("О программе")) { go("about") }, spaced(12))
        body.addView(updates, spaced(12))
        body.addView(tuningCard(),spaced(12))
    }

    private fun startupUpdateToggle(parent: LinearLayout) =
        settingsToggle(parent, tr("Проверять обновления при запуске"),
            tr("При запуске проверяется версия на GitHub. APK скачивается по кнопке."),
            store.checkUpdates) { store.checkUpdates = it }

    private fun newerRelease() =
        MobileUpdates.status.release?.takeIf { compareReleaseVersions(it.version, installedVersion) > 0 }

    private fun aboutPage() {
        heading(tr("О программе"), tr("Secret Tunnel · версия {0}", MobileUpdates.versionName(this)))
        val updates = card()
        updates.addView(label(tr("Обновления"), 18, true))
        val updateStatus = label("", 14, false, palette.muted).apply {
            accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE
        }
        updates.addView(updateStatus, spaced(8))
        val updateActions = column()
        updates.addView(updateActions)
        val check = button(tr("Проверить"), true) {
            MobileUpdates.check()
            refreshUpdateCard()
        }
        updates.addView(check, spaced(12))
        startupUpdateToggle(updates)
        aboutUpdate = AboutUpdateViews(updateStatus, check, updateActions)
        body.addView(updates, spaced(16))
        refreshUpdateCard()

        val agent = card()
        agent.addView(label(tr("Агент на сервере"), 18, true))
        agent.addView(label(tr("Приложение скачивает агент своей версии и передаёт его на сервер по SSH."), 13, false, palette.muted), spaced(8))
        val servers = store.servers().filter { it.host.isNotBlank() && it.user.isNotBlank() }
        val server = servers.firstOrNull { it.id == agentServerId }
            ?: servers.firstOrNull { it.id == store.activeProxy?.serverId } ?: servers.firstOrNull()
        agentServerId = server?.id
        if (server == null) {
            agent.addView(button(tr("Добавить сервер")) { go("servers") }, spaced(12))
        } else {
            val chooseServer = button(tr("Сервер · {0}", server.name)) {
                val selectedIndex = servers.indexOf(server)
                AlertDialog.Builder(uiContext).setTitle(tr("Сервер"))
                    .setSingleChoiceItems(servers.map { it.name }.toTypedArray(), selectedIndex) { dialog, index ->
                        dialog.dismiss()
                        agentServerId = servers[index].id
                        agentResult = ""
                        render(keepTvPosition = true)
                    }.setNegativeButton(tr("Отмена"), null).show()
            }.apply { tag = "agent-server" }
            agent.addView(chooseServer, spaced(12))
            val checkAgent = button(tr("Проверить версию")) { manageServerAgent(server, false) }
            agent.addView(checkAgent, spaced(8))
            val status = label("", 13, false, palette.muted).apply {
                accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE
            }
            agent.addView(status, spaced(8))
            val installAgent = button(tr("Установить агент {0}", installedVersion)) {
                AlertDialog.Builder(uiContext).setTitle(tr("Установить агент {0}?", installedVersion))
                    .setMessage(tr("Сервер «{0}». Новые подключения будут использовать установленную версию.", server.name))
                    .setNegativeButton(tr("Отмена"), null)
                    .setPositiveButton(tr("Установить")) { _, _ -> attempt { manageServerAgent(server, true) } }.show()
            }
            agent.addView(installAgent, spaced(8))
            agentViews = AgentViews(chooseServer, checkAgent, installAgent, status)
            refreshAgentCard()
        }
        body.addView(agent, spaced(12))

        val help = card()
        help.addView(label(tr("Справка"), 18, true))
        for ((section, title) in listOf("setup" to tr("Установка"), "knowledge" to tr("База знаний"))) {
            help.addView(button(title) { helpSection = section; helpQuery = ""; go("help") }, spaced(8))
        }
        body.addView(help, spaced(12))

        val links = card()
        links.addView(label(tr("Проект"), 18, true))
        for ((title, url) in listOf(tr("Сайт") to PROJECT_SITE, "GitHub" to PROJECT_URL,
            tr("Все релизы") to "$PROJECT_URL/releases", tr("Сообщить о проблеме") to "$PROJECT_URL/issues")) {
            links.addView(button(title) { openProjectLink(url) }, spaced(8))
        }
        body.addView(links, spaced(12))
    }

    private fun refreshUpdateCard() {
        val views = aboutUpdate ?: return
        val status = MobileUpdates.status
        if (status == shownUpdate) return
        val previous = shownUpdate
        shownUpdate = status
        val release = status.release
        val newer = newerRelease() != null
        val total = release?.apk?.size ?: 0L
        val message = when {
            status.checking -> tr("Проверяем обновления…")
            status.downloading -> tr("Скачиваем {0} из {1}", TrafficMeter.bytes(status.received.toDouble()), TrafficMeter.bytes(total.toDouble()))
            status.error.isNotEmpty() -> status.error
            status.ready -> tr("APK скачан. Контрольная сумма и подпись совпали.")
            newer && release != null -> listOfNotNull(tr("Доступна версия {0}", release.version),
                release.publishedAt.takeIf { it.isNotEmpty() },
                release.apk?.let { TrafficMeter.bytes(it.size.toDouble()) }).joinToString(" · ")
            release != null && compareReleaseVersions(release.version, installedVersion) < 0 ->
                tr("Установлена версия {0}. Последний публичный релиз — {1}.", installedVersion, release.version)
            release != null -> tr("Установлена актуальная версия {0}.", installedVersion)
            else -> tr("Проверьте, есть ли новая версия.")
        }
        update(views.status, message)
        views.status.setTextColor(if (status.error.isNotEmpty()) palette.error else palette.muted)
        views.check.setActionAvailable(!status.checking && !status.downloading)
        if (status.downloading && previous?.downloading == true) {
            (views.actions.getChildAt(0) as? ProgressBar)?.progress = updatePercent(status.received, total)
            return
        }
        views.actions.removeAllViews()
        if (status.downloading) {
            views.actions.addView(ProgressBar(uiContext, null, android.R.attr.progressBarStyleHorizontal).apply {
                max = 100
                progress = updatePercent(status.received, total)
                progressTintList = ColorStateList.valueOf(palette.accent)
                contentDescription = tr("Загрузка обновления")
            }, spaced(8))
        } else if (newer && release != null) {
            if (release.notes.isNotBlank()) views.actions.addView(button(tr("Что нового")) {
                AlertDialog.Builder(uiContext).setTitle(tr("Версия {0}", release.version)).setMessage(release.notes)
                    .setPositiveButton(tr("Закрыть"), null).show()
            }, spaced(8))
            val action = when {
                status.ready -> button(tr("Установить обновление"), true) {
                    if (!packageManager.canRequestPackageInstalls()) {
                        toast(tr("Разрешите установку для Secret Tunnel, затем нажмите кнопку снова."))
                    }
                    MobileUpdates.install(this, UPDATE_INSTALL_REQUEST)
                    refreshUpdateCard()
                }
                release.apk != null && release.checksums != null -> button(tr("Скачать обновление"), true) {
                    MobileUpdates.download(this)
                    refreshUpdateCard()
                }
                else -> button(tr("Открыть страницу релиза")) { openProjectLink(release.page) }
            }
            views.actions.addView(action, spaced(8))
        }
    }

    private fun updatePercent(received: Long, total: Long) =
        if (total > 0) (received * 100 / total).toInt().coerceIn(0, 100) else 0

    private fun refreshAgentCard(revealResult: Boolean = false) {
        val views = agentViews ?: return
        for (button in listOf(views.server, views.check, views.install)) {
            button.setActionAvailable(!agentBusy)
        }
        views.status.visibility = if (agentResult.isEmpty()) View.GONE else View.VISIBLE
        update(views.status, agentResult)
        views.status.setTextColor(when {
            agentError -> palette.error
            agentBusy -> palette.muted
            else -> palette.text
        })
        if (revealResult) revealTvResult(views.status, views.check, views.install)
    }

    private fun manageServerAgent(server: ServerProfile, install: Boolean) {
        if (agentBusy) return
        val route = store.route(server.id)
        require(route.all { isTrustedSshPin(it.fingerprint) }) { tr("Подтвердите отпечатки в разделе «Серверы» → «Проверить SSH и агент».") }
        agentBusy = true
        agentError = false
        agentResult = if (install) tr("Скачиваем и проверяем архив агента…") else tr("Подключаемся к серверу…")
        refreshAgentCard(revealResult = true)
        val proxy = MobileUpdates.proxy()
        Thread {
            val result = runCatching {
                var archive: java.io.File? = null
                try {
                    if (install) {
                        val release = ReleaseUpdates.exact(installedVersion, proxy)
                        val asset = release.agent ?: error(tr("В релизе нет агента для Linux x86_64"))
                        archive = java.io.File.createTempFile("agent-update-", ".tar.gz", cacheDir)
                        ReleaseUpdates.download(release, asset, archive, proxy)
                        runOnUiThread {
                            agentResult = tr("Передаём проверенный архив на сервер по SSH…")
                            if (!isDestroyed && !isFinishing) refreshAgentCard(revealResult = true)
                        }
                    }
                    store.withSshConfig(route) { config ->
                        NativeBridge.requireAvailable()
                        val response = JSONObject(NativeBridge.manageAgent(config.toString(), installedVersion, archive?.path.orEmpty()))
                        require(!response.has("error")) { friendlyError(response.optString("error")) }
                        when {
                            response.isNull("version") -> tr("Агент на сервере не найден.")
                            install -> tr("Установлен агент {0}. Новые подключения используют его.", response.getString("version"))
                            response.getString("version") == installedVersion -> tr("Агент {0} — та же версия, что у приложения.", installedVersion)
                            else -> tr("На сервере агент {0}, приложение {1}.", response.getString("version"), installedVersion)
                        }
                    }
                } finally {
                    archive?.delete()
                }
            }
            runOnUiThread {
                agentBusy = false
                agentError = result.isFailure
                agentResult = result.getOrElse { it.message ?: tr("Не удалось выполнить операцию с агентом") }
                if (!isDestroyed && !isFinishing) refreshAgentCard(revealResult = true)
            }
        }.start()
    }

    private fun helpPage() {
        val title = if (helpSection == "setup") tr("Установка") else tr("База знаний")
        heading(title, if (helpOnline) tr("Свежая версия с GitHub") else tr("Копия, встроенная в приложение"))
        val bundledHelp = assets.open(if (AppLocalization.language == AppLanguage.ENGLISH) "knowledge.en.md" else "knowledge.md").bufferedReader().use { it.readText() }
        if (!helpLoading && !helpOnline) {
            helpLoading = true
            val port = MobileUpdates.proxy()
            Thread {
                val refreshed = runCatching { ReleaseUpdates.help(port) }.getOrNull()
                runOnUiThread {
                    helpLoading = false
                    if (!refreshed.isNullOrBlank()) {
                        helpOnline = true
                        helpText = refreshed
                        if (page == "help" && !isDestroyed && !isFinishing) render(keepTvPosition = true)
                    }
                }
            }.start()
        }
        val search = EditText(uiContext).apply {
            isSingleLine = true
            hint = tr("Поиск по справке")
            contentDescription = hint
            setTextColor(palette.text)
            setHintTextColor(palette.muted)
            setText(helpQuery)
            tag = "help-search"
            tvOptionFocus(this)
        }
        body.addView(search, spaced(12))
        body.addView(button(tr("Открыть на сайте")) {
            openProjectLink(PROJECT_SITE + "knowledge.html" + if (helpSection == "setup") "#android-start" else "")
        }, spaced(8))
        val articles = column()
        body.addView(articles)
        fun sections(markdown: String) = markdown.split(Regex("(?m)(?=^## )")).drop(1)
        val articlesByTitle = sections(bundledHelp).associateBy { it.substringBefore('\n') }.toMutableMap()
        sections(helpText.orEmpty()).forEach { articlesByTitle[it.substringBefore('\n')] = it }
        fun showArticles() {
            articles.removeAllViews()
            val words = helpQuery.trim().split(Regex("\\s+")).filter { it.isNotEmpty() }
            val filtered = articlesByTitle.values.filter { article ->
                val heading = article.lineSequence().first()
                (helpSection != "setup" || heading.removePrefix("## ") in
                    listOf(tr("Установка и подключение"), tr("Выбор приложений"), tr("Настройки на телефоне"),
                        tr("Обновления на телефоне"), tr("Цепочка на телефоне"), tr("Частые проблемы"))) &&
                    words.all { article.contains(it, ignoreCase = true) }
            }
            if (filtered.isEmpty()) articles.addView(label(tr("По этому запросу ничего не найдено."), 14, false, palette.muted), spaced(16))
            for (article in filtered) {
                val name = article.substringBefore('\n').removePrefix("## ")
                articles.addView(button(name) {
                    val content = article.substringAfter('\n').lineSequence()
                        .filterNot { it.startsWith("<") || it.startsWith("![") || it.startsWith("```") }.joinToString("\n")
                        .replace(Regex("""\[([^\]]+)]\([^)]+\)"""), "$1").replace("**", "").replace("`", "").trim()
                    AlertDialog.Builder(uiContext).setTitle(name).setMessage(content)
                        .setPositiveButton(tr("Закрыть"), null).show()
                }, spaced(8))
            }
        }
        search.addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) = Unit
            override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) {
                helpQuery = s.toString()
                showArticles()
            }
            override fun afterTextChanged(s: Editable?) = Unit
        })
        showArticles()
    }

    private fun openProjectLink(url: String) {
        startActivity(Intent(Intent.ACTION_VIEW, android.net.Uri.parse(url)))
    }

    // Exit address and country: the same facts the desktop shows on «Подключение».
    private fun flag(code:String)=if(code.length==2&&code.all{it in 'A'..'Z'||it in 'a'..'z'})code.uppercase().map{String(Character.toChars(0x1F1E6+(it-'A')))}.joinToString("") else ""
    private fun countryName(code:String)=if(code.length==2)java.util.Locale("",code).getDisplayCountry(AppLocalization.language.locale).ifBlank{code} else code
    private fun country(code:String)=if(code.isBlank())"" else "${flag(code)} ${countryName(code)}".trim()
    private fun shownIp(ip:String)=if(store.hideSensitive&&!revealIp)ip.replace(Regex("[0-9A-Fa-f]"),"•") else ip
    private fun exitLine():String=when{
        !TunnelVpnService.active->tr("Подключите туннель")
        TunnelVpnService.exitIp.isNotBlank()->"${shownIp(TunnelVpnService.exitIp)}  ·  ${country(TunnelVpnService.exitCountry)}"
        !store.verifyProxy->tr("Проверка выключена в настройках")
        TunnelVpnService.phase=="Работает"->tr("Определяем…")
        else->"—"
    }

    private fun formatMbps(v:Double?)=if(v==null)"—" else if(v>=100)String.format(java.util.Locale.US,"%.0f",v) else String.format(java.util.Locale.US,"%.1f",v)
    private fun formatMs(v:Double?)=if(v==null)"—" else tr("{0} мс", Math.round(v))
    private fun formatDuration(seconds:Long):String{val h=seconds/3600;val m=seconds%3600/60
        return when{h>0->tr("{0} ч {1} мин", h, m.toString().padStart(2,'0'));m>0->tr("{0} мин", m);else->tr("{0} с", seconds)}}
    private fun formatWhen(epoch:Long):String=java.text.SimpleDateFormat("d MMM, HH:mm",AppLocalization.language.locale).format(java.util.Date(epoch*1000))

    private fun segmented(options:List<Pair<String,String>>,selected:String,enabled:Boolean=true,pick:(String)->Unit):LinearLayout{
        val line=row()
        options.forEachIndexed{index,(id,title)->
            line.addView(button(title,primary=id==selected){pick(id)}.apply{isEnabled=enabled;alpha=if(enabled)1f else .5f;textSize=13f
                setAutoSizeTextTypeUniformWithConfiguration(10,13,1,android.util.TypedValue.COMPLEX_UNIT_SP)},
                LinearLayout.LayoutParams(0,dp(48),1f).apply{if(index>0)marginStart=dp(6)})
        }
        return line
    }

    private fun statsPage(){
        heading(tr("Статистика"),tr("Сеансы туннеля и трафик приложений"))
        val periods=listOf("today" to tr("Сегодня"),"week" to tr("7 дней"),"month" to tr("30 дней"),"all" to tr("Всё время"))
        body.addView(segmented(periods,statsPeriod){statsPeriod=it;render(keepTvPosition = true)},spaced(16))
        val now=System.currentTimeMillis()/1000
        val from=when(statsPeriod){
            "today"->java.util.Calendar.getInstance().apply{set(java.util.Calendar.HOUR_OF_DAY,0);set(java.util.Calendar.MINUTE,0);set(java.util.Calendar.SECOND,0)}.timeInMillis/1000
            "week"->now-7*86400;"month"->now-30*86400;else->0L
        }
        val shown=SessionHistory.load(this,TunnelVpnService.sessionId).filter{it.endedAt>=from||it.active}.sortedByDescending{it.startedAt}
        val totals=card()
        val grid=row()
        fun cell(title:String,value:String,color:Int=palette.text)=column().apply{addView(label(title,12,true,palette.muted));addView(label(value,18,true,color).apply{setSingleLine(true);setAutoSizeTextTypeUniformWithConfiguration(12,18,1,android.util.TypedValue.COMPLEX_UNIT_SP)},spaced(6))}
        grid.addView(cell(tr("Скачано"),TrafficMeter.bytes(shown.sumOf{it.rx}.toDouble()),palette.accent),LinearLayout.LayoutParams(0,-2,1f))
        grid.addView(cell(tr("Отправлено"),TrafficMeter.bytes(shown.sumOf{it.tx}.toDouble()),palette.upload),LinearLayout.LayoutParams(0,-2,1f))
        totals.addView(grid)
        val grid2=row()
        grid2.addView(cell(tr("Сеансов"),shown.size.toString()),LinearLayout.LayoutParams(0,-2,1f))
        grid2.addView(cell(tr("Время в туннеле"),formatDuration(shown.sumOf{(it.endedAt-it.startedAt).coerceAtLeast(0)})),LinearLayout.LayoutParams(0,-2,1f))
        totals.addView(grid2,spaced(14))
        body.addView(totals,spaced(14))

        data class AppTotal(val name:String,var rx:Long=0,var tx:Long=0,var sessions:Int=0)
        val apps=linkedMapOf<String,AppTotal>()
        for(session in shown){
            for(usage in session.apps){apps.getOrPut(usage.name){AppTotal(usage.name)}.apply{rx+=usage.rx;tx+=usage.tx;sessions++}}
            // Traffic of the session no app accounts for went through the proxy itself,
            // like the desktop's «Без перехвата»: speed tests, checks, apps set to use the proxy.
            val rx=(session.rx-session.apps.sumOf{it.rx}).coerceAtLeast(0);val tx=(session.tx-session.apps.sumOf{it.tx}).coerceAtLeast(0)
            if(rx+tx>=65536)apps.getOrPut(VIA_PROXY){AppTotal(VIA_PROXY)}.apply{this.rx+=rx;this.tx+=tx;sessions++}
        }
        val appCard=card();appCard.addView(label(tr("Приложения"),16,true))
        val sorted=apps.values.sortedByDescending{it.rx+it.tx}
        sorted.firstOrNull{it.name!=UNATTRIBUTED&&it.name!=VIA_PROXY}?.let{appCard.addView(label(tr("Больше всего: {0}", it.name),12,false,palette.accent),spaced(6))}
        if(sorted.isEmpty())appCard.addView(label(tr("За этот период трафика приложений не было."),13,false,palette.muted),spaced(10))
        val peak=(sorted.maxOfOrNull{it.rx+it.tx}?:1L).coerceAtLeast(1L)
        for(app in sorted){
            val line=row().apply{gravity=Gravity.CENTER_VERTICAL}
            val icon=ImageView(this);catalog.firstOrNull{it.label==app.name}?.let{icon.setImageDrawable(applicationIcon(it))}
            line.addView(icon,LinearLayout.LayoutParams(dp(32),dp(32)).apply{marginEnd=dp(12)})
            val texts=column();texts.addView(label(if(app.name==UNATTRIBUTED||app.name==VIA_PROXY)tr(app.name) else app.name,14,true))
            if(app.name==VIA_PROXY)texts.addView(label(tr("тесты скорости, проверки и приложения с прокси 127.0.0.1 в своих настройках"),11,false,palette.muted),spaced(2))
            texts.addView(label(tr("↓ {0}  ·  ↑ {1}  ·  сеансов: {2}", TrafficMeter.bytes(app.rx.toDouble()), TrafficMeter.bytes(app.tx.toDouble()), app.sessions),12,false,palette.muted),spaced(4))
            texts.addView(View(this).apply{background=palette.panel(palette.accent,dp(2).toFloat())},LinearLayout.LayoutParams((dp(220)*(app.rx+app.tx)/peak).toInt().coerceAtLeast(dp(4)),dp(4)).apply{topMargin=dp(6)})
            line.addView(texts,LinearLayout.LayoutParams(0,-2,1f))
            appCard.addView(line,spaced(14))
        }
        body.addView(appCard,spaced(12))

        val sessionsCard=card();sessionsCard.addView(label(tr("Сеансы"),16,true))
        if(shown.isEmpty())sessionsCard.addView(label(tr("За этот период сеансов не было."),13,false,palette.muted),spaced(10))
        for(session in shown.take(50)){
            val route=if(session.route.size>1)session.route.joinToString(" → ") else session.server
            val top=session.apps.sortedByDescending{it.rx+it.tx}.take(3).joinToString(", "){if(it.name==UNATTRIBUTED)tr("без привязки") else it.name}
            val item=column()
            item.addView(label(formatWhen(session.startedAt)+"  ·  "+(if(session.active)tr("идёт сейчас") else formatDuration((session.endedAt-session.startedAt).coerceAtLeast(0))),13,true,if(session.active)palette.accent else palette.text))
            item.addView(label("$route  ${country(session.country)}".trim(),12,false,palette.muted),spaced(4))
            item.addView(label("↓ ${TrafficMeter.bytes(session.rx.toDouble())}  ·  ↑ ${TrafficMeter.bytes(session.tx.toDouble())}"+if(top.isNotBlank())"  ·  $top" else "",12,false,palette.muted),spaced(4))
            sessionsCard.addView(item,spaced(14))
        }
        body.addView(sessionsCard,spaced(12))
        if(shown.isNotEmpty()||SessionHistory.load(this,TunnelVpnService.sessionId).isNotEmpty())
            body.addView(button(tr("Очистить историю"),danger=true){
                AlertDialog.Builder(uiContext).setTitle(tr("Удалить историю?")).setMessage(tr("Сеансы и трафик приложений за всё время удалятся с этого телефона."))
                    .setNegativeButton(tr("Отмена"),null).setPositiveButton(tr("Удалить")){_,_->SessionHistory.clear(this,TunnelVpnService.sessionId);render(keepTvPosition = true)}.show()
            },spaced(12))
        body.addView(label(tr("История хранится только на этом телефоне: названия приложений, время и объём трафика, без адресов."),11,false,palette.muted),spaced(12))
    }

    private val UNATTRIBUTED="Без привязки к приложению"
    private val VIA_PROXY="Без перехвата"

    /** Through the running tunnel's local proxy, or straight from the phone. */
    private fun speedTarget():SpeedTest.Target?{
        if(speedDirect)return SpeedTest.Target(null,speedProvider,"",emptyList(),"")
        if(!TunnelVpnService.active||TunnelVpnService.phase!="Работает")return null
        val port=TunnelVpnService.connectedSocksPort?:return null
        val proxy=store.activeProxy?:return null
        val names=runCatching{store.route(proxy.serverId).map{it.profile.name}}.getOrDefault(emptyList())
        return SpeedTest.Target(port,speedProvider,names.lastOrNull().orEmpty(),if(names.size>1)names else emptyList(),TunnelVpnService.exitCountry)
    }

    private fun speedPage(){
        val live=SpeedTest.live
        heading(tr("Скорость"),tr("Тесты через туннель и без него"))
        body.addView(segmented(listOf("tunnel" to tr("Через туннель"),"direct" to tr("Без туннеля")),if(speedDirect)"direct" else "tunnel",!live.running){speedDirect=it=="direct";render(keepTvPosition = true)},spaced(16))
        if(speedKind!="mlab")body.addView(segmented(listOf("cloudflare" to "Cloudflare","librespeed" to "LibreSpeed"),speedProvider,!live.running){speedProvider=it;render(keepTvPosition = true)},spaced(8))
        body.addView(label(when{
            speedKind=="mlab"->tr("M-Lab сам выбирает ближайший сервер на этом пути.")
            speedProvider=="librespeed"->tr("Открытые серверы LibreSpeed: выбирается тот, что быстрее отвечает на этом пути.")
            else->tr("speed.cloudflare.com: узел Cloudflare рядом с точкой выхода.")
        },12,false,palette.muted),spaced(8))

        val panel=card(palette.raised)
        val status=label("",14,true).also{panel.addView(it)}
        val numbers=row()
        fun number(title:String,color:Int):TextView{
            val cell=column();cell.addView(label(title,12,true,color))
            val value=label("—",22,true).apply{setSingleLine(true);setAutoSizeTextTypeUniformWithConfiguration(14,22,1,android.util.TypedValue.COMPLEX_UNIT_SP)}
            cell.addView(value,spaced(6));numbers.addView(cell,LinearLayout.LayoutParams(0,-2,1f));return value
        }
        val down=number(tr("↓ Мбит/с"),palette.accent);val up=number(tr("↑ Мбит/с"),palette.upload);val ping=number(tr("Пинг"),palette.text)
        panel.addView(numbers,spaced(14))
        val steps=label("",12,false,palette.muted).also{panel.addView(it,spaced(12))}
        val start=button(tr("Запустить"),true){
            if(SpeedTest.live.running){SpeedTest.cancel();return@button}
            val target=speedTarget()?:error(tr("Подключите туннель или выберите «Без туннеля»"))
            SpeedTest.start(applicationContext,speedKind,target){}
            speedWasRunning=true;refreshSpeed()
        }
        panel.addView(start,spaced(14))
        body.addView(panel,spaced(14))
        speedViews=SpeedViews(status,steps,down,up,ping,start)

        body.addView(label(tr("Тест"),16,true),spaced(18))
        for(kind in SpeedKinds.all){
            val chosen=kind.id==speedKind
            val item=card(if(chosen)palette.raised else palette.surface)
            val title=row().apply{gravity=Gravity.CENTER_VERTICAL}
            title.addView(RadioButton(uiContext).apply{isChecked=chosen;isClickable=false;isFocusable=false;buttonTintList=ColorStateList.valueOf(palette.accent);importantForAccessibility=View.IMPORTANT_FOR_ACCESSIBILITY_NO})
            title.addView(label(kind.title,15,true,if(chosen)palette.accent else palette.text),LinearLayout.LayoutParams(0,-2,1f))
            title.addView(label("${kind.time} · ${kind.traffic}",11,false,palette.muted))
            item.addView(title);item.addView(label(kind.text,12,false,palette.muted),spaced(6))
            item.contentDescription=kind.title
            item.tag = kind.id
            item.setOnClickListener{if(!SpeedTest.live.running){speedKind=kind.id;render(keepTvPosition = true)}};tvCardFocus(item)
            body.addView(item,spaced(8))
        }

        val history=SpeedHistory.load(this).sortedByDescending{it.startedAt}
        body.addView(label(tr("История замеров"),16,true),spaced(20))
        if(history.isEmpty())body.addView(label(tr("Замеров пока нет. Запустите «Быстрый» через туннель и без него, чтобы сравнить."),13,false,palette.muted),spaced(8))
        val tunnel=history.firstOrNull{!it.direct&&it.results.downloadMbps!=null};val direct=history.firstOrNull{it.direct&&it.results.downloadMbps!=null}
        if(tunnel!=null&&direct!=null){
            val compare=card();compare.addView(label(tr("Туннель и прямое соединение"),14,true))
            fun line(title:String,a:String,b:String)=compare.addView(label(tr("{0}:  {1}  ·  без туннеля {2}", title, a, b),12,false,palette.muted),spaced(6))
            line(tr("Скачивание"),tr("{0} Мбит/с", formatMbps(tunnel.results.downloadMbps)),tr("{0} Мбит/с", formatMbps(direct.results.downloadMbps)))
            line(tr("Отдача"),tr("{0} Мбит/с", formatMbps(tunnel.results.uploadMbps)),tr("{0} Мбит/с", formatMbps(direct.results.uploadMbps)))
            line(tr("Пинг"),formatMs(tunnel.results.pingMs),formatMs(direct.results.pingMs))
            body.addView(compare,spaced(8))
        }
        for(run in history.take(30)){
            val r=run.results
            val item=card()
            item.addView(label("${formatWhen(run.startedAt)}  ·  ${SpeedKinds.title(run.kind)}",13,true))
            val path=if(run.direct)tr("Без туннеля") else if(run.route.size>1)run.route.joinToString(" → ") else run.server.ifBlank{tr("Туннель")}
            item.addView(label(tr("{0}  ·  выход {1}", path, country(run.exitCountry).ifBlank{"—"})+if(run.exitIp.isNotBlank())" · ${shownIp(run.exitIp)}" else "",12,false,palette.muted),spaced(4))
            val values=listOfNotNull(
                (r.downloadMbps?:r.mixedDownloadMbps)?.let{"↓ ${formatMbps(it)}"},r.uploadMbps?.let{"↑ ${formatMbps(it)}"},
                r.pingMs?.let{tr("пинг {0}", formatMs(it))},r.loadedPingMs?.let{tr("под нагрузкой {0}", formatMs(it))},
                r.dnsMs?.let{"UDP ${formatMs(it)}"},r.requestMs?.let{tr("запрос {0}", formatMs(it))},r.serverRttMs?.let{tr("RTT сервера {0}", formatMs(it))})
            item.addView(label(values.joinToString("  ·  ").ifBlank{"—"}+if(values.any{it.startsWith("↓")||it.startsWith("↑")})tr("  Мбит/с") else "",12,false,palette.text),spaced(6))
            item.addView(label(run.provider,11,false,palette.muted),spaced(4))
            listOfNotNull(run.error.ifBlank{null},if(run.cancelled)"Остановлен" else null,r.downloadSource?.let{tr("Загрузка: {0}", it)}).forEach{item.addView(label(it,11,false,palette.warning),spaced(4))}
            body.addView(item,spaced(8))
        }
        if(history.isNotEmpty())body.addView(button(tr("Очистить историю"),danger=true){
            AlertDialog.Builder(uiContext).setTitle(tr("Удалить историю замеров?")).setNegativeButton(tr("Отмена"),null)
                .setPositiveButton(tr("Удалить")){_,_->SpeedHistory.clear(this);render(keepTvPosition = true)}.show()
        },spaced(12))
        refreshSpeed();main.removeCallbacks(speedTick);main.post(speedTick)
    }

    private fun refreshSpeed(){
        val views=speedViews?:return
        val live=SpeedTest.live
        if(speedWasRunning&&!live.running){speedWasRunning=false;render(keepTvPosition = true);return}
        speedWasRunning=live.running
        val shown=if(live.running)live.results else live.last?.results
        update(views.status,when{
            live.running->"${SpeedKinds.title(live.kind)} · ${if(live.mode=="direct")tr("без туннеля") else tr("через туннель")}"+
                (if(live.provider.isNotBlank())"\n${live.provider}" else "")+(if(live.exitIp.isNotBlank())tr("\nВыход: {0} {1}", shownIp(live.exitIp), country(live.exitCountry)) else tr("\nОпределяем выход…"))
            live.error.isNotBlank()->tr("Тест прерван: {0}", live.error)
            live.last!=null->tr("Последний: {0}, {1}", SpeedKinds.title(live.last!!.kind), formatWhen(live.last!!.startedAt))
            !speedDirect&&speedTarget()==null->tr("Подключите туннель на главной или выберите «Без туннеля».")
            else->tr("Выберите тест ниже и нажмите «Запустить».")
        })
        update(views.down,if(live.running&&live.downMbps>0)formatMbps(live.downMbps) else formatMbps(shown?.downloadMbps?:shown?.mixedDownloadMbps))
        update(views.up,if(live.running&&live.upMbps>0)formatMbps(live.upMbps) else formatMbps(shown?.uploadMbps))
        update(views.ping,if(live.running&&live.pingMs!=null&&shown?.pingMs==null)formatMs(live.pingMs) else formatMs(shown?.pingMs?:shown?.serverRttMs))
        update(views.steps,if(live.running)live.steps.mapIndexed{i,step->when{i<live.step->"✓ $step";i==live.step->"▸ $step · ${Math.round(live.progress*100)}%";else->"  $step"}}.joinToString("\n") else "")
        views.steps.visibility=if(live.running)View.VISIBLE else View.GONE
        val label=if(live.running)tr("Остановить") else if(speedDirect)tr("Запустить без туннеля") else tr("Запустить")
        if(views.start.text.toString()!=label)views.start.text=label
        val can=live.running||speedDirect||speedTarget()!=null
        views.start.isEnabled=can;views.start.alpha=if(can)1f else .45f
    }

    private fun appsPage(){
        val layout=column().apply{setPadding(dp(pageInset),dp(8),dp(pageInset),0)};content.addView(layout)
        layout.addView(label(tr("Приложения"),26,true))
        layout.addView(label(tr("Защита действует, пока Android держит VPN."),13,false,palette.muted),spaced(8))
        val search=EditText(uiContext).apply{setSingleLine(true);hint=tr("Поиск по названию или пакету");setTextColor(palette.text);setHintTextColor(palette.muted);textSize=if(isTv)16f else 14f;inputType=InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;background=focusPanel(palette.surface,14,palette.border);setPadding(dp(14),0,dp(14),0);setText(searchQuery)}
        layout.addView(search,LinearLayout.LayoutParams(-1,dp(50)).apply{topMargin=dp(16)})
        appCount=label("",11,false,palette.muted).also{layout.addView(it,spaced(12))}
        val actions=row();actions.addView(button(tr("Выбранные")){searchQuery="";search.setText("");appAdapter?.showSelected=true;refreshApps()},LinearLayout.LayoutParams(0,dp(48),1f).apply{marginEnd=dp(5)})
        actions.addView(button(tr("Все приложения")){appAdapter?.showSelected=false;refreshApps()},LinearLayout.LayoutParams(0,dp(48),1f).apply{marginStart=dp(5)});layout.addView(actions,spaced(8))
        if(TunnelVpnService.active)layout.addView(label(tr("Изменения вступят в силу при следующем ручном запуске туннеля."),11,false,palette.warning),spaced(8))
        val priorityAvailable = Build.VERSION.SDK_INT >= 29 &&
            store.tuning.ssh.let { it.qos == "voice" && it.priorityUdpSsh }
        if (priorityAvailable) layout.addView(label(tr("Для приложений со звонками отметьте «UDP через второй SSH»."), 12, false, palette.muted), spaced(8))
        val list=ListView(this).apply{
            divider=null;cacheColorHint=palette.background;setPadding(0,dp(6),0,dp(12));clipToPadding=false
            if(isTv){itemsCanFocus=true;descendantFocusability=ViewGroup.FOCUS_AFTER_DESCENDANTS}
        }
        appAdapter=ApplicationAdapter(priorityAvailable);list.adapter=appAdapter
        val listArea=FrameLayout(this);listArea.addView(list,FrameLayout.LayoutParams(-1,-1))
        appEmpty=label(tr("Ищем приложения…"),14,false,palette.muted).apply{gravity=Gravity.CENTER;setPadding(dp(12),dp(24),dp(12),dp(24))}.also{listArea.addView(it,FrameLayout.LayoutParams(-1,-1));list.emptyView=it}
        layout.addView(listArea,LinearLayout.LayoutParams(-1,0,1f));refreshApps()
        search.addTextChangedListener(object:TextWatcher{
            override fun beforeTextChanged(s:CharSequence?,start:Int,count:Int,after:Int){}
            override fun onTextChanged(s:CharSequence?,start:Int,before:Int,count:Int){searchQuery=s.toString();refreshApps()}
            override fun afterTextChanged(s:Editable?){}
        })
    }
    private fun refreshApps(){if(page!="apps")return;appAdapter?.refresh();update(appEmpty,catalogError ?: if(!catalogLoaded)tr("Ищем приложения…") else tr("Ничего не найдено.\nИзмените поиск или откройте все приложения."));updateAppCount()}
    private fun updateAppCount() {
        update(appCount, if (catalogLoaded) tr("Через туннель: {0}  ·  С защитой: {1}", selected.size, selected.count(protectedApps::contains))
            else tr("Ищем установленные приложения…"))
    }
    private fun scanApps() {
        if (!watchingPackages) return
        if (scanningApps) {
            rescanApps = true
            return
        }
        scanningApps = true
        rescanApps = false
        Thread {
            var scanError: String? = null
            val found = try {
                val categories = if (isTv) listOf(Intent.CATEGORY_LEANBACK_LAUNCHER, Intent.CATEGORY_LAUNCHER)
                    else listOf(Intent.CATEGORY_LAUNCHER)
                categories.flatMap { category ->
                    packageManager.queryIntentActivities(Intent(Intent.ACTION_MAIN).addCategory(category), 0)
                }.distinctBy { it.activityInfo.packageName }
                    .filter {
                        val pkg = it.activityInfo.packageName
                        pkg != packageName && packageManager.checkPermission("android.permission.INTERNET", pkg) == PackageManager.PERMISSION_GRANTED
                    }.map {
                        AppEntry(it.activityInfo.packageName, it.loadLabel(packageManager).toString(), it.activityInfo.applicationInfo.sourceDir)
                    }.sortedWith(compareBy({ it.label.lowercase() }, { it.pkg }))
            } catch (error: Exception) {
                android.util.Log.e("SecretTunnelApps", tr("Не удалось прочитать список приложений"), error)
                scanError = tr("Не удалось прочитать список приложений.\n{0}", error.javaClass.simpleName)
                null
            }
            runOnUiThread {
                scanningApps = false
                if (!watchingPackages) return@runOnUiThread
                if (!catalogLoaded || catalogError != scanError || (found != null && catalog != found)) {
                    if (found != null) catalog = found
                    catalogError = scanError
                    catalogLoaded = true
                    refreshApps()
                }
                if (rescanApps) scanApps()
            }
        }.start()
    }

    private fun applicationIcon(entry: AppEntry): Drawable? = iconCache.get(entry) ?: runCatching {
        packageManager.getApplicationIcon(entry.pkg).also { iconCache.put(entry, it) }
    }.getOrNull()

    private class AppHolder(val icon:ImageView,val name:TextView,val pkg:TextView,val toggle:Switch,val guard:CheckBox,val priority:CheckBox)
    private inner class ApplicationAdapter(private val priorityAvailable: Boolean):BaseAdapter(){
        private val priorityPackages = store.priorityUdpPackages.toMutableSet()
        private var visible=listOf<AppEntry>();var showSelected=false
        fun refresh(){visible=catalog.filter{(!showSelected||it.pkg in selected)&&(it.label.contains(searchQuery,true)||it.pkg.contains(searchQuery,true))};notifyDataSetChanged()}
        override fun getCount()=visible.size
        override fun getItem(position:Int)=visible[position]
        override fun getItemId(position:Int)=visible[position].pkg.hashCode().toLong()
        override fun hasStableIds() = true
        override fun getView(position:Int,convertView:View?,parent:ViewGroup):View{
            val container = convertView as? LinearLayout ?: column().apply {
                setPadding(0, dp(6), 0, dp(6))
                val header = row().apply { gravity = Gravity.CENTER_VERTICAL; minimumHeight = dp(64) }
                val icon = ImageView(this@MainActivity)
                header.addView(icon, LinearLayout.LayoutParams(dp(40), dp(40)).apply { marginEnd = dp(12) })
                val texts=column();val name=label("",14,true);val pkg=label("",10,false,palette.muted).apply{maxLines=1;ellipsize=android.text.TextUtils.TruncateAt.END};texts.addView(name);texts.addView(pkg,spaced(4));header.addView(texts,LinearLayout.LayoutParams(0,-2,1f))
                val toggle=tunnelSwitch().apply{minimumWidth=dp(if(isTv)64 else 52);minimumHeight=dp(48)}
                header.addView(toggle)
                addView(header)
                if(!isTv)header.setOnClickListener { toggle.isChecked = !toggle.isChecked }
                fun option(title: String) = CheckBox(uiContext).apply {
                    text = title
                    textSize = 13f
                    minimumHeight = dp(48)
                    setTextColor(palette.text)
                    buttonTintList = toggle.thumbTintList
                    tvOptionFocus(this)
                }
                val guard = option(tr("Блокировать при обрыве туннеля"))
                val priority = option(tr("UDP через второй SSH"))
                for (checkBox in listOf(guard, priority)) {
                    addView(checkBox, LinearLayout.LayoutParams(-1, -2).apply { marginStart = dp(48) })
                }
                tag = AppHolder(icon, name, pkg, toggle, guard, priority)
            }
            val item=visible[position];val holder=container.tag as AppHolder;holder.name.text=item.label;holder.pkg.text=item.pkg
            holder.icon.setImageDrawable(applicationIcon(item))
            holder.toggle.setOnCheckedChangeListener(null);holder.toggle.isChecked=item.pkg in selected;holder.toggle.contentDescription=tr("Через туннель: {0}", item.label)
            holder.priority.setOnCheckedChangeListener(null)
            holder.priority.visibility = if (priorityAvailable && item.pkg in selected) View.VISIBLE else View.GONE
            holder.priority.contentDescription = tr("UDP через второй SSH: {0}", item.label)
            holder.priority.isChecked = item.pkg in priorityPackages
            holder.priority.setOnCheckedChangeListener { _, enabled ->
                if (enabled) priorityPackages.add(item.pkg) else priorityPackages.remove(item.pkg)
                store.priorityUdpPackages = priorityPackages
            }
            holder.guard.setOnCheckedChangeListener(null)
            holder.guard.isChecked = item.pkg in protectedApps
            holder.guard.visibility = if (holder.toggle.isChecked) View.VISIBLE else View.GONE
            holder.guard.contentDescription = tr("Блокировать при обрыве туннеля: {0}", item.label)
            holder.guard.setOnCheckedChangeListener { _, enabled ->
                if (enabled) protectedApps.add(item.pkg) else protectedApps.remove(item.pkg)
                store.protectedPackages = protectedApps
                updateAppCount()
            }
            holder.toggle.setOnCheckedChangeListener { _, enabled ->
                if (enabled) selected.add(item.pkg) else selected.remove(item.pkg)
                if (!enabled) holder.priority.isChecked = false
                store.packages = selected
                holder.priority.visibility = if (priorityAvailable && enabled) View.VISIBLE else View.GONE
                holder.guard.visibility = if (enabled) View.VISIBLE else View.GONE
                updateAppCount()
                if (showSelected) refresh()
            }
            return container
        }
    }

    private fun logsPage(){
        heading(tr("Журнал"),tr("Диагностика подключения и ошибок"))
        val copy = button(tr("Копировать журнал")) {
            val clipboard = getSystemService(CLIPBOARD_SERVICE) as ClipboardManager
            clipboard.setPrimaryClip(ClipData.newPlainText("Secret Tunnel", diagnostics()))
            toast(tr("Журнал скопирован"))
        }
        body.addView(copy, spaced(16))
        val events = label(diagnostics(),11,false,palette.muted).apply {
            typeface = Typeface.MONOSPACE
            if (!isTv) setTextIsSelectable(true)
        }
        logView = events
        if (!isTv) {
            body.addView(events, spaced(16))
            return
        }
        val scroll = ScrollView(this).apply {
            id = View.generateViewId()
            tag = "log-scroll"
            isFocusable = true
            isScrollbarFadingEnabled = false
            scrollBarStyle = View.SCROLLBARS_INSIDE_INSET
            background = focusPanel(palette.surface, 12, palette.border)
            setPadding(dp(12),dp(12),dp(12),dp(12))
            addView(events)
            setOnKeyListener { _, key, event ->
                if (event.action != KeyEvent.ACTION_DOWN) return@setOnKeyListener false
                val step = maxOf(dp(48), height / 4)
                when (key) {
                    KeyEvent.KEYCODE_DPAD_UP -> {
                        if (canScrollVertically(-1)) scrollBy(0, -step) else copy.requestFocus()
                        true
                    }
                    KeyEvent.KEYCODE_DPAD_DOWN -> {
                        scrollBy(0, step)
                        true
                    }
                    else -> false
                }
            }
        }
        copy.nextFocusDownId = scroll.id
        body.addView(scroll, LinearLayout.LayoutParams(-1,0,1f).apply{topMargin=dp(16)})
        pageScroll = scroll
    }
    private fun activeTunnelDetails(): String {
        val status = TunnelVpnService.detail
        val windows = lastStatus.optJSONObject("transport")?.let(TransportDiagnostics::windows)
        val heartbeat = lastStatus.optJSONObject("transport")?.optJSONObject("diagnostics")?.optJSONObject("heartbeat")
        val measuredAt = heartbeat?.optLong("last_ack_at_ms")?:0
        val freshness = (heartbeat?.optLong("interval_ms",5000)?:5000).times(3).coerceAtLeast(15_000)
        val latency = if(measuredAt>0&&System.currentTimeMillis()-measuredAt<=freshness&&!heartbeat!!.isNull("last_rtt_ms"))
            tr("Задержка туннеля: {0} мс", heartbeat.optLong("last_rtt_ms")) else null
        return listOfNotNull(status,windows,latency).joinToString("\n")
    }
    private fun diagnostics():String=buildString{
        append("Secret Tunnel ${BuildConfig.VERSION_NAME} · Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})\n")
        append(tr("Устройство: {0} {1}\n\n", Build.MANUFACTURER, Build.MODEL))
        append(TunnelVpnService.phase+"\n"+TunnelVpnService.detail+"\n\n")
        lastStatus.optJSONObject("ipv6")?.let {
            append(tr("IPv6 через SSH: TCP {0} · UDP {1}\n", if(it.optBoolean("tcp"))tr("доступен") else tr("недоступен"), if(it.optBoolean("udp"))tr("доступен") else tr("недоступен")))
            if(TunnelVpnService.interceptionActive) {
                append(if(lastStatus.optBoolean("ipv6_supported")) tr("IPv6 выбранных приложений идёт через VPN.\n") else tr("IPv6 выбранных приложений заблокирован внутри VPN.\n"))
            }
            append("\n")
        }
        previousConnectionFailure?.let { append(tr("История завершений Android\n{0}\n\n", it.message)) }
        lastStatus.optString("agent").takeIf{it.isNotBlank()&&it!="null"}?.let{append(it+"\n\n")}
        lastStatus.optJSONObject("transport")?.let { t->
            append(tr("Транспорт v3 · SSH-каналов: {0}\n", t.optInt("ssh_channels")))
            TransportDiagnostics.windows(t)?.let { append("$it\n") }
            t.optJSONObject("ssh")?.let { append(TransportDiagnostics.describeSsh(it)+"\n") }
            append(tr("Соединения: {0} / {1} · пик: {2}\n", t.optInt("active"), t.optInt("limit"), t.optInt("peak")))
            append(tr("Очереди: приём {0} / {1} B; отправка {2} / {3} B\n", t.optLong("receive_queue_bytes"), t.optLong("receive_budget"), t.optLong("send_queue_bytes"), t.optLong("send_budget")))
            append(tr("UDP отброшено: {0} · отказов по лимиту: {1} · ошибок протокола: {2}\n\n", t.optLong("udp_drops"), t.optLong("rejected"), t.optLong("protocol_errors")))
            t.optJSONObject("diagnostics")?.let { append(tr("Клиент\n") + TransportDiagnostics.describe(it) + "\n") }
            t.optJSONObject("server")?.let { server->
                append(tr("Отчёт сервера: соединений {0} / {1} · пик {2}\n", server.optInt("active"), server.optInt("limit"), server.optInt("peak")))
                append(tr("Сервер: UDP отброшено {0} · отказов по лимиту {1}\n", server.optLong("udp_drops"), server.optLong("rejected")))
                if(server.optString("last_udp_error").isNotBlank())append(tr("UDP сервера: {0}\n", server.optString("last_udp_error")))
                server.optJSONObject("diagnostics")?.let { append(TransportDiagnostics.describe(it)) }
                append("\n")
            }
        }
        val logs=lastStatus.optJSONArray("logs");if(logs!=null)for(i in 0 until logs.length())append(TransportDiagnostics.formatLog(logs.optString(i))+"\n")
        val file=java.io.File(filesDir,"tun.log")
        if(file.isFile)java.io.RandomAccessFile(file,"r").use{f->val length=f.length();val size=length.coerceAtMost(8192).toInt();f.seek(length-size);val bytes=ByteArray(size);f.readFully(bytes);append(bytes.toString(Charsets.UTF_8))}
        if(logs==null||logs.length()==0)append(tr("Событий пока нет"))
    }
    private fun friendlyError(raw:String)=ConnectionMessages.describe(raw)
    private fun readinessAction():String {
        val proxy=store.activeProxy?:return tr("Добавить прокси")
        val server=store.server(proxy.serverId)?:return tr("Добавить сервер")
        if(server.host.isBlank()||server.user.isBlank())return tr("Настроить сервер")
        if(!SettingsStore(this,server.id).hasKey())return tr("Добавить SSH-ключ")
        if(store.packages.isEmpty())return tr("Выбрать приложения")
        return tr("Подключить")
    }
    private fun connectOrStop(){
        if (TunnelVpnService.active) {
            fun stop() { startService(Intent(this, TunnelVpnService::class.java).setAction("STOP")) }
            if (TunnelVpnService.protectedCount == 0) stop()
            else AlertDialog.Builder(uiContext).setTitle(tr("Остановить туннель?"))
                .setMessage(tr("После остановки приложения смогут выходить в интернет напрямую."))
                .setNegativeButton(tr("Отмена"), null).setPositiveButton(tr("Остановить")) { _, _ -> stop() }.show()
            return
        }
        val proxy=store.activeProxy
        if(proxy==null){navigate("proxies");return}
        val server=store.server(proxy.serverId)
        if(server==null){navigate("servers");return}
        if(server.host.isBlank()||server.user.isBlank()||!SettingsStore(this,server.id).hasKey()){
            editingId=server.id;navigate("serverEditor");toast(tr("Заполните сервер и добавьте SSH-ключ"));return
        }
        if(store.packages.isEmpty()){navigate("apps");toast(tr("Выберите приложения для туннеля"));return}
        val session = store.snapshot()
        pendingProxy = store.activeProxyId
        fun prepareVpn() {
            ConnectionAttempt.mark(this, tr("Разрешение Android VPN"))
            try {
                val permission = VpnService.prepare(this)
                if (permission != null) startActivityForResult(permission, 103) else startTunnel()
            } catch (error: Exception) {
                ConnectionAttempt.clear(this)
                message(error.message ?: tr("Не удалось запросить разрешение Android VPN"))
            }
        }
        if (session.route.all { isTrustedSshPin(it.fingerprint) }) prepareVpn()
        else {
            ConnectionAttempt.mark(this, tr("Проверка отпечатка SSH"))
            verifyServer(server.id, onFailure = { error ->
                ConnectionAttempt.clear(this)
                message(error)
            }, onCancel = { ConnectionAttempt.clear(this) }) { prepareVpn() }
        }
    }
    private fun startTunnel() {
        require(pendingProxy == store.activeProxyId) { tr("Профиль изменился. Подключитесь заново.") }
        ConnectionAttempt.mark(this, tr("Запуск службы VPN"))
        try {
            startForegroundService(Intent(this, TunnelVpnService::class.java))
        } catch (error: Exception) {
            ConnectionAttempt.clear(this)
            throw error
        }
    }
    private fun refreshServerCheck() {
        val status=serverCheck
        val running=status?.state==CheckState.RUNNING
        val visible=status?.takeIf{it.id==editingId}
        val enabled=!running&&!checking&&!pendingTrust
        sshCheckButton?.setActionAvailable(enabled)
        fingerprintCheckButton?.setActionAvailable(enabled)
        serverCheckView?.apply {
            visibility=if(visible==null)View.GONE else View.VISIBLE
            val result = visible?.text.orEmpty()
            val changed = text.toString() != result
            update(this, result)
            setTextColor(when(visible?.state) {
                CheckState.PASSED -> palette.accent
                CheckState.FAILED -> palette.error
                else -> palette.muted
            })
            if(changed && visible!=null) revealTvResult(this, sshCheckButton, fingerprintCheckButton)
        }
    }

    // A diagnostic can use an active VPN's saved profile without editing it.
    private fun startServerCheck(checkAgent:Boolean?) {
        val id=editingId
        val readOnly=TunnelVpnService.active
        fun complete(state:CheckState,text:String) {
            serverCheck=ServerCheck(id,state,text)
            if(page=="serverEditor"&&editingId==id) {
                val pin=SettingsStore(this,id).fingerprint
                update(fingerprintView,if(isTrustedSshPin(pin))pin else tr("Отпечаток будет получен автоматически"))
            }
            refreshServerCheck()
        }
        if(checking||pendingTrust||serverCheck?.state==CheckState.RUNNING)return
        try {
            if(!readOnly)saveServerForm()
            val route=store.route(id)
            val fail:(String)->Unit={complete(CheckState.FAILED,it)}
            val progress=when(checkAgent) {
                true -> tr("Проверяем SSH, ключ и серверный агент…")
                false -> tr("Проверяем SSH и ключ…")
                null -> tr("Проверяем отпечаток SSH-сервера…")
            }
            complete(CheckState.RUNNING,progress)
            if(checkAgent!=null)require(SettingsStore(this,id).hasKey()){tr("Сначала импортируйте SSH-ключ")}
            fun verifyLogin(agent:Boolean) {
                checkServerAgent(store.route(id),agent){ok,result->
                    complete(if(ok)CheckState.PASSED else CheckState.FAILED,result)
                }
            }
            if(checkAgent!=null&&route.all{isTrustedSshPin(it.fingerprint)}) {
                verifyLogin(checkAgent)
            } else {
                verifyServer(id,readOnly=readOnly,onFailure=fail,onCancel={fail(tr("Проверка отменена."))}) {
                    if(checkAgent==null)complete(CheckState.PASSED,tr("Отпечаток сервера подтверждён."))
                    else verifyLogin(checkAgent)
                }
            }
        }catch(error:Exception){
            complete(CheckState.FAILED,error.message?:tr("Не удалось проверить SSH"))
        }
    }

    private fun verifyServer(
        id:String,
        readOnly:Boolean=false,
        onFailure:(String)->Unit=::message,
        onCancel:()->Unit={},
        after:()->Unit,
    ){
        if(!readOnly)unlocked()
        require(!pendingTrust){tr("Проверка уже выполняется")}
        fun verifyNext(index: Int) {
            val route = store.route(id)
            if (index == route.size) { after(); return }
            if (index < route.lastIndex && isTrustedSshPin(route[index].fingerprint)) verifyNext(index + 1)
            else verifyHost(route.take(index + 1),readOnly,onFailure,onCancel) { verifyNext(index + 1) }
        }
        verifyNext(0)
    }
    private fun verifyHost(route:List<SshEndpoint>,readOnly:Boolean,onFailure:(String)->Unit,onCancel:()->Unit,after:()->Unit) {
        val endpoint = route.last()
        val server = endpoint.profile
        val scoped=SettingsStore(this,server.id);val saved=endpoint.fingerprint
        if(!readOnly){checkingDetail=tr("Получаем ключ SSH-сервера «{0}»…", server.name);checking=true}
        Thread{
            val result=try{
                NativeBridge.requireAvailable()
                store.withSshConfig(route, includeTargetKey=false) { JSONObject(NativeBridge.probe(it.toString())) }
            }
                catch(e:Exception){JSONObject().put("error",e.message)}
            runOnUiThread{
                if(!readOnly)checking=false
                if(isFinishing||isDestroyed)return@runOnUiThread
                if(store.server(server.id)!=server){onFailure(tr("Профиль сервера изменился. Повторите проверку."));return@runOnUiThread}
                val pin=result.optString("fingerprint")
                if(!isTrustedSshPin(pin)){onFailure(tr("Не удалось проверить сервер «{0}». ", server.name)+friendlyError(result.optString("error")));return@runOnUiThread}
                fun proceed(){
                    try{if(!readOnly&&pin!=saved)scoped.fingerprint=pin;after()}
                    catch(error:Exception){onFailure(error.message?:tr("Не удалось проверить SSH"))}
                }
                if(saved==pin)proceed()
                else if(readOnly)onFailure(tr("Сверьте и подтвердите отпечаток сервера «{0}» после остановки VPN.", server.name))
                else{
                    pendingTrust=true
                    var accepted=false
                    AlertDialog.Builder(uiContext).setTitle(if(isTrustedSshPin(saved))tr("Ключ сервера изменился") else tr("Доверять серверу?"))
                        .setMessage("${server.host}:${server.port}\n\n$pin\n\n"+if(isTrustedSshPin(saved))tr("Сохранён другой ключ. Сверьте замену у администратора перед подтверждением.") else tr("Сверьте отпечаток у администратора. После подтверждения приложение запомнит этот сервер."))
                        .setNegativeButton(tr("Отмена"),null).setPositiveButton(tr("Доверять")){dialog,_->
                            accepted=true;dialog.dismiss();pendingTrust=false;proceed()
                        }.setOnDismissListener{
                            pendingTrust=false
                            if(!accepted)onCancel()
                        }.show()
                }
            }
        }.start()
    }
    private fun checkServerAgent(route:List<SshEndpoint>,checkAgent:Boolean,complete:(Boolean,String)->Unit){
        Thread{
            val result=try{
                NativeBridge.requireAvailable()
                store.withSshConfig(route) { config ->
                    config.put("check_agent",checkAgent)
                    val response=JSONObject(NativeBridge.checkServer(config.toString()))
                    if(response.optBoolean("ok"))true to response.getString("summary")
                    else false to friendlyError(response.optString("error",tr("Не удалось проверить SSH")))
                }
            }catch(error:Exception){false to (error.message?:tr("Не удалось проверить SSH"))}
            runOnUiThread{if(!isFinishing&&!isDestroyed)complete(result.first,result.second)}
        }.start()
    }
    private fun enrollDevice(id:String){
        val scoped=SettingsStore(this,id);val route=store.route(id);checking=true
        Thread{
            var device:java.io.File?=null;var bytes:ByteArray?=null
            val error=try{
                NativeBridge.requireAvailable()
                val pair=JSONObject(NativeBridge.generateKey());require(!pair.has("error")){pair.optString("error")}
                val file=java.io.File.createTempFile("ssh-device-",".runtime",cacheDir);device=file
                val privateKey=pair.getString("private").toByteArray(Charsets.UTF_8);bytes=privateKey
                file.writeBytes(privateKey)
                store.withSshConfig(route) { config ->
                    config.put("device_key_path",file.absolutePath)
                    val result=NativeBridge.registerKey(config.toString(),pair.getString("public"));require(result.isEmpty()){result}
                }
                scoped.importKey(privateKey);""
            }catch(e:Exception){e.message?:tr("Не удалось зарегистрировать ключ")}finally{bytes?.fill(0);device?.delete()}
            runOnUiThread{checking=false;if(!isFinishing&&!isDestroyed){if(error.isEmpty()){render(keepTvPosition = true);message(tr("Ключ телефона добавлен и проверен. Можно подключаться."))}else message(error)}}
        }.start()
    }
    @Deprecated("Platform activity result")
    override fun onActivityResult(requestCode:Int,resultCode:Int,data:Intent?){
        super.onActivityResult(requestCode,resultCode,data)
        if(requestCode==105){if(resultCode==RESULT_OK)navigate("home");return}
        if(requestCode==107){if(resultCode==RESULT_OK)navigate("servers");return}
        if (requestCode == UPDATE_INSTALL_REQUEST) {
            if (resultCode == RESULT_FIRST_USER) message(tr("Android не смог установить обновление. Можно повторить установку из «О программе»."))
            return
        }
        if(resultCode!=RESULT_OK){
            if(requestCode==103)ConnectionAttempt.clear(this)
            return
        }
        if(requestCode==103){attempt{startTunnel()};return}
        if(requestCode==102){val id=importId;val uri=data?.data?:return;checking=true
            Thread{
                val error=try{
                    require(store.server(id)!=null){tr("Сервер удалён")}
                    val bytes=contentResolver.openInputStream(uri)!!.use{input->val out=java.io.ByteArrayOutputStream();val buffer=ByteArray(8192);while(out.size()<=1048576){val count=input.read(buffer);if(count<0)break;out.write(buffer,0,count)};out.toByteArray()}
                    try{SettingsStore(this,id).importKey(bytes)}finally{bytes.fill(0)};""
                }catch(e:Exception){e.message?:tr("Не удалось импортировать ключ")}
                runOnUiThread{checking=false;if(!isFinishing&&!isDestroyed){if(error.isEmpty()){render(keepTvPosition = true);toast(tr("SSH-ключ добавлен"))}else message(error)}}
            }.start()
        }
    }
    private fun unlocked(){require(!TunnelVpnService.active){tr("Сначала остановите туннель, чтобы изменить подключение")};require(!checking){tr("Дождитесь завершения операции")}}
    private fun heading(title:String,subtitle:String){body.addView(label(title,26,true));body.addView(label(subtitle,13,false,palette.muted),spaced(8))}
    private fun emptyState(title:String,detail:String){val card=card();card.addView(label(title,18,true));card.addView(label(detail,14,false,palette.muted),spaced(10));body.addView(card,spaced(20))}
    private fun field(title:String,value:String,hint:String,idValue:Int,numeric:Boolean=false):EditText{
        body.addView(label(title,12,true,palette.muted),spaced(16))
        return EditText(uiContext).apply{id=idValue;setSingleLine(true);inputType=if(numeric)InputType.TYPE_CLASS_NUMBER else InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
            importantForAutofill=View.IMPORTANT_FOR_AUTOFILL_NO;setText(value);this.hint=hint;textSize=16f;setTextColor(palette.text);setHintTextColor(palette.muted)
            background=focusPanel(palette.surface,13,palette.border);setPadding(dp(14),0,dp(14),0);body.addView(this,LinearLayout.LayoutParams(-1,dp(54)).apply{topMargin=dp(7)})}
    }
    private fun card(color:Int=palette.surface)=column().apply{setPadding(dp(18),dp(18),dp(18),dp(18));background=palette.panel(color,dp(22).toFloat())}
    private fun column()=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL}
    private fun row()=LinearLayout(this).apply{orientation=LinearLayout.HORIZONTAL}
    private fun label(value:String,size:Int,bold:Boolean=false,color:Int=palette.text)=TextView(this).apply{text=value;textSize=(size+if(isTv)2 else 0).toFloat();setTextColor(color);if(bold)typeface=Typeface.create("sans-serif-medium",Typeface.NORMAL);includeFontPadding=false}
    private fun button(value:String,primary:Boolean=false,danger:Boolean=false,action:()->Unit)=Button(uiContext).apply{
        text=value;isAllCaps=false;textSize=14f;minimumHeight=dp(50);setPadding(dp(12),dp(9),dp(12),dp(9))
        background=focusPanel(if(primary)palette.accent else palette.raised,14,if(primary)0 else palette.border,if(primary)palette.onAccent else palette.accent)
        setTextColor(if(danger)palette.error else if(primary)palette.onAccent else palette.accent);setOnClickListener{attempt(action)}
        if(isTv){isFocusable=true;textSize=16f;tag=value}
    }
    private fun Button.setActionAvailable(available: Boolean) {
        // The focused action keeps its place while its result arrives.
        if (isTv) isClickable = available else isEnabled = available
        alpha = if (available) 1f else if (isTv) .7f else .45f
    }
    private fun revealTvResult(status: TextView, vararg actions: View?) {
        if (!isTv || actions.none { it?.isFocused == true }) return
        status.whenLaidOut {
            if (status.isShown && actions.any { it?.isFocused == true }) {
                status.requestRectangleOnScreen(Rect(0,0,status.width,minOf(status.height,status.lineHeight * 2)),true)
            }
        }
    }
    private fun View.whenLaidOut(action: () -> Unit) {
        if (isLaidOut && !isLayoutRequested) {
            action()
            return
        }
        addOnLayoutChangeListener(object : View.OnLayoutChangeListener {
            override fun onLayoutChange(
                view: View, left: Int, top: Int, right: Int, bottom: Int,
                oldLeft: Int, oldTop: Int, oldRight: Int, oldBottom: Int,
            ) {
                view.removeOnLayoutChangeListener(this)
                action()
            }
        })
    }
    private fun tunnelSwitch() = Switch(uiContext).apply {
        thumbTintList = ColorStateList(arrayOf(intArrayOf(android.R.attr.state_checked),intArrayOf()),intArrayOf(palette.accent,palette.muted))
        trackTintList = ColorStateList(arrayOf(intArrayOf(android.R.attr.state_checked),intArrayOf()),intArrayOf(palette.raised,palette.border))
        tvOptionFocus(this)
        if (isTv) {
            minimumWidth = dp(64)
            setPaddingRelative(dp(8),0,dp(8),0)
        }
    }
    private fun tvOptionFocus(view: View) {
        if (isTv) {
            view.isFocusable = true
            view.minimumHeight = dp(48)
            view.foreground = focusPanel(Color.TRANSPARENT, 10)
        }
    }
    private fun focusPanel(fill:Int,radius:Int,stroke:Int=0,focusColor:Int=palette.accent):Drawable {
        val idle=palette.panel(fill,dp(radius).toFloat(),stroke)
        if(!isTv)return idle
        val focused=palette.panel(fill,dp(radius).toFloat()).apply{setStroke(dp(3),focusColor)}
        return android.graphics.drawable.StateListDrawable().apply {
            addState(intArrayOf(android.R.attr.state_focused),focused)
            addState(intArrayOf(),idle)
        }
    }
    private fun tvCardFocus(view:View){
        if(isTv){view.isFocusable=true;view.background=focusPanel(palette.surface,22)}
    }
    private fun spaced(top:Int)=LinearLayout.LayoutParams(-1,-2).apply{topMargin=dp(top)}
    private fun update(view:TextView?,value:String){val localized=tr(value);if(view!=null&&view.text.toString()!=localized)view.text=localized}
    private fun confirmDelete(title:String,action:()->Unit){AlertDialog.Builder(uiContext).setTitle(title).setNegativeButton(tr("Отмена"),null).setPositiveButton(tr("Удалить")){_,_->attempt(action)}.show()}
    private fun attempt(action:()->Unit){try{action()}catch(e:Exception){message(e.message.orEmpty())}}
    private fun toast(value:String){Toast.makeText(this,tr(value),Toast.LENGTH_SHORT).show()}
    private fun message(value:String){AlertDialog.Builder(uiContext).setMessage(tr(value)).setPositiveButton(tr("Понятно"),null).show()}
    private fun dp(value:Int)=(resources.displayMetrics.density*value).toInt()
}
