package dev.secrettunnel.android

import android.app.*
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.VpnService
import android.os.*
import org.json.JSONObject
import java.io.File
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

class TunnelVpnService : VpnService() {
    companion object {
        init { System.loadLibrary("hev-socks5-tunnel") }
        @Volatile var phase = "Остановлен"
        @Volatile var detail = ""
        @Volatile var active = false
        val traffic=TrafficHistory()
        @Volatile internal var appTraffic: AppTrafficAccounting? = null
            private set
        private const val CHANNEL = "ssh-tunnel"
        private const val NOTICE = 1080
    }
    external fun TProxyStartService(configPath: String, fd: Int): Boolean
    external fun TProxyStopService(): Boolean
    external fun TProxyIsRunning(): Boolean
    external fun TProxyGetStats(): LongArray
    external fun TProxySetAccounting(enabled: Boolean)
    external fun TProxyDrainTraffic(): ByteArray?
    private val worker = Executors.newSingleThreadExecutor()
    private val sampling = Executors.newSingleThreadScheduledExecutor()
    private val stopping = AtomicBoolean(false)
    private val started = AtomicBoolean(false)
    private var tun: ParcelFileDescriptor? = null
    private val main = Handler(Looper.getMainLooper())
    private val recovery=RecoveryPolicy()
    private val underlying=UnderlyingNetworkTracker()
    private var recoveryNetworkHandle:Long?=null
    private val connectivity by lazy {getSystemService(android.net.ConnectivityManager::class.java)}
    private val networkCallback=object:android.net.ConnectivityManager.NetworkCallback(){
        override fun onAvailable(network:android.net.Network){
            connectivity.getNetworkCapabilities(network)?.let{onCapabilitiesChanged(network,it)}
        }
        override fun onCapabilitiesChanged(network:android.net.Network,capabilities:android.net.NetworkCapabilities){
            if(connectivity.activeNetwork!=network)return
            if(!capabilities.hasTransport(android.net.NetworkCapabilities.TRANSPORT_VPN)&&capabilities.hasCapability(android.net.NetworkCapabilities.NET_CAPABILITY_INTERNET)){
                if(underlying.currentHandle()!=network.networkHandle)android.util.Log.i("SecretTunnelNetwork","default_available handle=${network.networkHandle}")
                underlying.available(network.networkHandle,SystemClock.elapsedRealtime())
            }
        }
        override fun onLost(network:android.net.Network){
            if(underlying.currentHandle()==network.networkHandle)android.util.Log.i("SecretTunnelNetwork","default_lost handle=${network.networkHandle}")
            underlying.lost(network.networkHandle,SystemClock.elapsedRealtime())
        }
    }
    private val diagnostics = TransportDiagnostics.Sampler()
    private var engineIpv6:Boolean?=null
    private var engineCapacity=0
    private var session:TunnelSession?=null
    private val monitor = object:Runnable {
        override fun run(){
            if(stopping.get()||!active||worker.isShutdown)return
            worker.execute {
                if(stopping.get())return@execute
                var recoveryAttempted=false
                try {
                    val networkAction=underlying.action(SystemClock.elapsedRealtime())
                    if(networkAction==UnderlyingNetworkTracker.Action.OFFLINE||networkAction==UnderlyingNetworkTracker.Action.WAIT){
                        val waiting=if(networkAction==UnderlyingNetworkTracker.Action.OFFLINE)"Ожидание сети" else "Смена сети"
                        if(phase!=waiting){phase=waiting;detail="VPN включён. Ждём доступную сеть для SSH.";notifyState()}
                        main.postDelayed(this,1500);return@execute
                    }
                    val moved=networkAction==UnderlyingNetworkTracker.Action.RECONNECT
                    val nativeLog=File(filesDir,"tun.log")
                    if(nativeLog.length()>1_048_576)java.io.RandomAccessFile(nativeLog,"rw").use{it.setLength(0)}
                    val status=JSONObject(NativeBridge.status())
                    if (status.optBoolean("running")) {
                        status.optJSONObject("transport")?.let { diagnostics.sample(SystemClock.elapsedRealtime(), it) }
                    }
                    val newNetwork=moved&&recoveryNetworkHandle!=underlying.currentHandle()
                    when(if(newNetwork)RecoveryPolicy.Action.RECONNECT_SSH else recovery.action(SystemClock.elapsedRealtime(),status.optBoolean("running")&&!moved,TProxyIsRunning())){
                        RecoveryPolicy.Action.RECONNECT_SSH->{
                            recoveryAttempted=!newNetwork
                            if(newNetwork)recovery.reset()
                            else if(!beginRecovery(status.optString("error").ifBlank{"SSH connection stopped"})){main.postDelayed(this,1500);return@execute}
                            phase=if(newNetwork)"Смена сети" else "Переподключение";detail="Восстанавливаем SSH. Трафик остаётся в VPN.";notifyState()
                            NativeBridge.stop()
                            if(!stopping.get())connectSsh()
                            if(!stopping.get()){startPacketEngine();markReady()}
                        }
                        RecoveryPolicy.Action.RESTART_ENGINE->{
                            recoveryAttempted=true
                            if(!beginRecovery("Обработчик TCP/UDP завершился")){main.postDelayed(this,1500);return@execute}
                            phase="Восстановление";detail="Перезапускаем обработчик TCP/UDP. VPN остаётся включён.";notifyState()
                            startPacketEngine();if(!stopping.get())markReady()
                        }
                        else->{}
                    }
                }catch(error:Exception){
                    if(!stopping.get()){
                        recovery.failed(error.message.orEmpty(),SystemClock.elapsedRealtime(),countAttempt=!recoveryAttempted)
                        phase="Ошибка соединения"
                        detail=if(recovery.needsUser)"${error.message}. Остановите туннель и проверьте настройки." else "${error.message}. Повторяем восстановление…"
                        notifyState()
                    }
                }
                if(!stopping.get())main.postDelayed(this,recovery.delay(SystemClock.elapsedRealtime()).coerceAtLeast(500))
            }
        }
    }
    override fun onCreate() {
        super.onCreate()
        connectivity.activeNetwork?.let {network->connectivity.getNetworkCapabilities(network)?.let{networkCallback.onCapabilitiesChanged(network,it)}}
        connectivity.registerDefaultNetworkCallback(networkCallback,main)
        File(filesDir, "ssh-key.runtime").delete()
        getSystemService(NotificationManager::class.java).createNotificationChannel(NotificationChannel(CHANNEL, "SSH-туннель", NotificationManager.IMPORTANCE_LOW))
    }
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == "STOP") { stopAsync(); return START_NOT_STICKY }
        if (!started.compareAndSet(false, true)) return START_NOT_STICKY
        recovery.reset()
        traffic.clear()
        appTraffic = null
        stopping.set(false); active = true; phase = "Подключение"; detail = "Проверка SSH-сервера"
        val notification = notification()
        if (Build.VERSION.SDK_INT >= 34) startForeground(NOTICE, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(NOTICE, notification)
        worker.execute {
            try {
                val settings = SettingsStore(this)
                session = settings.snapshot()
                val packages = session!!.packages
                require(packages.isNotEmpty()) { "Выберите хотя бы одно приложение" }
                require(packageName !in packages) { "Нельзя перехватывать собственный SSH-транспорт" }
                // SSH authenticates before claiming routes. Socket protection also
                // applies on reconnection while the VPN interface already exists.
                connectSsh()
                if (stopping.get()) return@execute
                startPacketEngine()
                if(!stopping.get()){
                    val accounting = if (Build.VERSION.SDK_INT >= 29) AppTrafficAccounting(this, packages) else null
                    appTraffic = accounting
                    TProxySetAccounting(accounting != null)
                    sampling.scheduleWithFixedDelay({
                        if (!stopping.get()) {
                            val now = SystemClock.elapsedRealtime()
                            val (rx, tx) = NativeBridge.trafficTotals()
                            traffic.sample(now, rx, tx)
                            if (accounting != null) accounting.sample(now, TProxyDrainTraffic() ?: ByteArray(0))
                        }
                    }, 0, 250, TimeUnit.MILLISECONDS)
                    markReady()
                }
                main.postDelayed(monitor, 1500)
            } catch (error: Exception) {
                if(!stopping.get()){phase = "Ошибка"; detail = error.message.orEmpty()}
                cleanup(); active = false; started.set(false)
                main.post { stopForeground(STOP_FOREGROUND_REMOVE); stopSelf() }
            }
        }
        return START_NOT_STICKY
    }
    private fun beginRecovery(error:String):Boolean {
        recovery.failed(error,SystemClock.elapsedRealtime())
        if(!recovery.needsUser)return true
        phase="Ошибка соединения";detail="$error. Автовосстановление остановлено; проверьте настройки и переподключитесь."
        notifyState();return false
    }
    private fun markReady(){
        if(stopping.get())return
        when(underlying.action(SystemClock.elapsedRealtime())){
            UnderlyingNetworkTracker.Action.OFFLINE->{phase="Ожидание сети";detail="VPN включён. Ждём доступную сеть для SSH."}
            UnderlyingNetworkTracker.Action.WAIT,UnderlyingNetworkTracker.Action.RECONNECT->{phase="Смена сети";detail="VPN включён. Переподключаем SSH."}
            else->{phase="Работает";detail="TCP и UDP · ${if(engineIpv6==true)"IPv4/IPv6" else "IPv4"} · приложений: ${session?.packages?.size?:0}"}
        }
        notifyState()
    }
    private fun startPacketEngine(){
        val status = JSONObject(NativeBridge.status())
        val ipv6 = status.optBoolean("ipv6_supported")
        val capacity = status.optJSONObject("transport")?.optInt("limit",256) ?: 256
        check(status.optBoolean("running")){"SSH не готов для обработки пакетов"}
        val current=session?:error("Нет сеанса VPN")
        val packages=current.packages
        TProxyStopService()
        if(stopping.get())return
        if(tun==null||engineIpv6!=ipv6||engineCapacity!=capacity){
        val builder = Builder().setSession("Secret Tunnel SSH").setMtu(1500)
            .addAddress("198.18.0.1", 30).addRoute("0.0.0.0", 0).addDnsServer("1.1.1.1")
        if (ipv6) builder.addAddress("fd00:198:18::1", 126).addRoute("::", 0)
        // Android 10+ otherwise marks every VPN as metered, including Wi-Fi,
        // which can make media apps suppress large/background downloads.
        if (Build.VERSION.SDK_INT >= 29) builder.setMetered(false)
        for (pkg in packages) builder.addAllowedApplication(pkg)
        val replacement=builder.establish()?:error("Android не разрешил VPN-интерфейс")
        val previous=tun
        tun=replacement
        previous?.close()
        engineIpv6=ipv6;engineCapacity=capacity
        }
        val config = File(filesDir, "tun.yml")
        config.writeText("""
            tunnel:
              mtu: 1500
              ipv4: 198.18.0.1
              ${if (ipv6) "ipv6: 'fd00:198:18::1'" else "# IPv6 blocked: server has no verified IPv6 egress"}
            socks5:
              address: 127.0.0.1
              port: ${session!!.socksPort}
              udp: 'udp'
            misc:
              task-stack-size: 65536
              tcp-buffer-size: 65536
              max-session-count: $capacity
              connect-timeout: 14000
              tcp-read-write-timeout: 300000
              udp-read-write-timeout: 60000
              log-file: '${File(filesDir, "tun.log").absolutePath}'
              log-level: warn
        """.trimIndent())
        File(filesDir, "tun.log").writeText("")
        if(stopping.get())return
        check(TProxyStartService(config.absolutePath, tun!!.fd)) { "Не удалось запустить обработчик TCP/UDP" }
        check(TProxyIsRunning()) { "Ошибка запуска обработчика TCP/UDP; смотрите журнал" }
    }
    private fun connectSsh() {
        if (stopping.get()) return
        val networkWhenConnecting=underlying.currentHandle()
        recoveryNetworkHandle=networkWhenConnecting
        val current = session ?: error("Нет параметров сеанса")
        SettingsStore(this).withSshConfig(current.route) { config ->
            config.put("socks_port", current.socksPort)
                .put("new_session", tun == null)
                .put("max_connections", connectionBudget())
                .put("udp_receive_window", current.udpReceive?.toJson())
                .put("agent_udp_receive_window", current.udpAgentReceive?.toJson())
                .put("ssh_tuning",current.ssh.toNativeJson())
            val error = NativeBridge.start(config.toString(), this)
            check(error.isEmpty()) { error }
            underlying.connectedOn(networkWhenConnecting)
        }
    }
    private fun notification(): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val stop = PendingIntent.getService(this, 1, Intent(this, TunnelVpnService::class.java).setAction("STOP"), PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, CHANNEL).setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentTitle("Secret Tunnel: $phase").setContentText(detail).setContentIntent(open)
            .setOngoing(true).addAction(Notification.Action.Builder(null, "Остановить", stop).build()).build()
    }
    private fun connectionBudget():Int {
        val manager=getSystemService(ActivityManager::class.java)
        val memory=ActivityManager.MemoryInfo();manager.getMemoryInfo(memory)
        if(manager.isLowRamDevice)return 256
        val gib=1024L*1024*1024
        return when {memory.totalMem>=6*gib->2048;memory.totalMem>=5*gib->1024;memory.totalMem>=3*gib->512;else->256}
    }
    private fun notifyState() { if (active) getSystemService(NotificationManager::class.java).notify(NOTICE, notification()) }
    private fun cleanup() {
        main.removeCallbacks(monitor)
        sampling.shutdownNow()
        TProxyStopService() // join native reader BEFORE closing its borrowed fd
        TProxySetAccounting(false)
        tun?.close(); tun = null;engineIpv6=null;engineCapacity=0
        NativeBridge.stop()
        File(filesDir, "ssh-key.runtime").delete()
    }
    private fun stopAsync() {
        if (!stopping.compareAndSet(false, true)) return
        phase = "Остановка"; NativeBridge.cancel(); main.removeCallbacks(monitor)
        worker.execute {
            cleanup(); active = false; started.set(false); phase = "Остановлен"; detail = ""
            main.post { stopForeground(STOP_FOREGROUND_REMOVE); stopSelf() }
        }
    }
    override fun onRevoke() { stopAsync() }
    override fun onDestroy() {
        runCatching{connectivity.unregisterNetworkCallback(networkCallback)}
        if (active && !stopping.get()) stopAsync()
        worker.shutdown()
        sampling.shutdownNow()
        super.onDestroy()
    }
}
