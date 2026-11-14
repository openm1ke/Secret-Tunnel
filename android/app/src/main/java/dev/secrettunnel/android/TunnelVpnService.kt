package dev.secrettunnel.android

import android.app.*
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
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
        @Volatile var protectedCount = 0
            private set
        @Volatile var connectedSocksPort: Int? = null
            private set
        val traffic=TrafficHistory()
        @Volatile internal var appTraffic: AppTrafficAccounting? = null
            private set
        /** Where the tunnel comes out, asked through it after connecting. */
        @Volatile var exitIp = ""
        @Volatile var exitCountry = ""
        /** The session being written to the statistics, while connected. */
        @Volatile var sessionId: String? = null
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
    private val underlying = UnderlyingNetworkTracker<Network>()
    @Volatile private var transportLink: UnderlyingNetworkTracker.Link<Network>? = null
    private var recoveryLink: UnderlyingNetworkTracker.Link<Network>? = null
    @Volatile private var prepared = false
    private var polling = false // Main-handler owned.
    private val connectivity by lazy { getSystemService(ConnectivityManager::class.java) }
    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) { main.post(networkWatch) }
        override fun onLost(network: Network) { main.post(networkWatch) }
        override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
            main.post(networkWatch)
        }
    }
    private val networkWatch = object : Runnable {
        override fun run() {
            if (!prepared || stopping.get()) return
            // Reconcile outside callbacks and independently of a blocking SSH attempt.
            val network = connectivity.activeNetwork
            val capabilities = network?.let(connectivity::getNetworkCapabilities)
            if (network != null && capabilities != null &&
                !capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN) &&
                capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)) {
                if (underlying.available(network, SystemClock.elapsedRealtime())) {
                    android.util.Log.i("SecretTunnelNetwork", "default_available handle=${network.networkHandle}")
                    wakeRecovery()
                }
            } else {
                underlying.currentLink()?.let {
                    if (underlying.lost(it.network)) {
                        android.util.Log.i("SecretTunnelNetwork", "default_lost handle=${it.network.networkHandle}")
                        wakeRecovery()
                    }
                }
            }
            main.removeCallbacks(this)
            main.postDelayed(this, 1000)
        }
    }
    private val diagnostics = TransportDiagnostics.Sampler()
    private data class Routes(val packages: Set<String>, val ipv6: Boolean)
    private var routes: Routes? = null
    private var session:TunnelSession?=null
    private var newSession = true
    private var verifyExit = false
    // Flows refused by either multiplexer end in the current SSH session.
    private var reportedRejections=0L
    private var sessionStarted=0L
    private var sessionBaseline=longArrayOf(0,0)
    private var sessionSavedAt=0L
    /** Saves this session's totals and per-app traffic to the statistics. */
    private fun recordSession(active:Boolean){
        val id=sessionId?:return
        val current=session?:return
        val totals=runCatching{NativeBridge.trafficTotals()}.getOrNull()?:return
        val apps=appTraffic?.entries.orEmpty().filter{it.down>0||it.up>0}.map{AppUsage(it.name,it.down,it.up)}
        val names=current.route.map{it.profile.name}
        runCatching{SessionHistory.save(this,SessionRecord(id,sessionStarted,System.currentTimeMillis()/1000,names.last(),
            if(names.size>1)names else emptyList(),exitCountry,(totals[0]-sessionBaseline[0]).coerceAtLeast(0),
            (totals[1]-sessionBaseline[1]).coerceAtLeast(0),apps,active))}
        sessionSavedAt=SystemClock.elapsedRealtime()
        if(!active)sessionId=null
    }
    private fun readyDetail()="TCP и UDP · ${if(routes?.ipv6==true)"IPv4/IPv6" else "IPv4"} · приложений: ${routes?.packages?.size?:0}"+
        if(reportedRejections>0)" · отказов по лимиту: $reportedRejections" else ""
    private fun wakeRecovery() {
        NativeBridge.cancel()
        main.removeCallbacks(monitor)
        main.post(monitor)
    }
    private val monitor = object : Runnable {
        override fun run() {
            if (!prepared || stopping.get() || worker.isShutdown || polling) return
            polling = true
            worker.execute {
                val link = underlying.currentLink()
                val now = SystemClock.elapsedRealtime()
                try {
                    if (!stopping.get()) {
                        recoverTunnel(link, now)
                    }
                } catch (error: Exception) {
                    if (!stopping.get() && link === underlying.currentLink()) {
                        val reason = error.message.orEmpty()
                        recovery.failed(reason, SystemClock.elapsedRealtime())
                        if (recovery.needsUser && tun == null) {
                            failStart(reason)
                        } else {
                            val next = if (recovery.needsUser) "Проверьте настройки SSH или VPN." else "Повторим подключение автоматически."
                            setState("Ошибка соединения", "${ConnectionMessages.describe(reason)}\n$next", blocked = true)
                        }
                    }
                } finally {
                    val delay = recovery.delay(SystemClock.elapsedRealtime())
                    main.post {
                        polling = false
                        if (prepared && !stopping.get()) {
                            main.removeCallbacks(this)
                            main.postDelayed(this, if (link !== underlying.currentLink()) 0 else delay)
                        }
                    }
                }
            }
        }
    }
    private fun recoverTunnel(link: UnderlyingNetworkTracker.Link<Network>?, now: Long) {
        if (link !== recoveryLink) {
            if (!recovery.needsUser) recovery.reset()
            recoveryLink = link
        }
        val networkAction = underlying.action(now)
        if (link == null || networkAction == UnderlyingNetworkTracker.Action.WAIT ||
            networkAction == UnderlyingNetworkTracker.Action.OFFLINE) {
            pauseForwarding("Сеть изменилась. Запустите тест после восстановления туннеля.")
            stopSsh()
            if (recovery.needsUser) return
            if (link == null) setState("Ожидание сети", "Ждём доступную сеть для SSH.", blocked = true)
            else setState("Смена сети", "Ждём, пока Android переключит сеть.", blocked = true)
            return
        }
        if (recovery.needsUser) return
        val status = JSONObject(NativeBridge.status())
        val sshReady = transportLink === link && status.optBoolean("running")
        val engineReady = TProxyIsRunning()
        if (!sshReady || !engineReady) {
            val reason = if (networkAction == UnderlyingNetworkTracker.Action.RECONNECT)
                "Сеть изменилась. Запустите тест после восстановления туннеля."
            else "Туннель временно недоступен. Запустите тест после восстановления."
            pauseForwarding(reason)
            if (!sshReady) stopSsh()
        }
        when (recovery.action(now, sshReady, engineReady)) {
            RecoveryPolicy.Action.RECONNECT_SSH -> {
                val state = if (networkAction == UnderlyingNetworkTracker.Action.RECONNECT) "Смена сети"
                    else if (sessionId == null) "Подключение" else "Переподключение"
                setState(state, "Подключаем SSH и серверный агент.", blocked = true)
                if (!isCurrent(link)) return
                connectSsh(link)
                if (!isCurrent(link)) return
                startPacketEngine()
                markReady(link)
            }
            RecoveryPolicy.Action.RESTART_ENGINE -> {
                setState("Восстановление", "Перезапускаем обработчик TCP/UDP.", blocked = true)
                startPacketEngine()
                markReady(link)
            }
            RecoveryPolicy.Action.HEALTHY -> {
                val nativeLog = File(filesDir, "tun.log")
                if (nativeLog.length() > 1_048_576) java.io.RandomAccessFile(nativeLog, "rw").use { it.setLength(0) }
                val transport = status.optJSONObject("transport")
                transport?.let { diagnostics.sample(now, it) }
                val rejected = (transport?.optLong("rejected") ?: 0L) +
                    (transport?.optJSONObject("server")?.optLong("rejected") ?: 0L)
                if (rejected != reportedRejections) {
                    reportedRejections = rejected
                    setState("Работает", readyDetail())
                }
            }
            RecoveryPolicy.Action.WAIT, RecoveryPolicy.Action.NEEDS_USER -> Unit
        }
    }
    private fun isCurrent(link: UnderlyingNetworkTracker.Link<Network>) =
        !stopping.get() && link === underlying.currentLink()

    private fun pauseForwarding(reason: String) {
        connectedSocksPort = null
        SpeedTest.tunnelUnavailable(reason)
        TProxyStopService()
        val guarded = session?.protectedPackages.orEmpty()
        if (guarded.isNotEmpty()) establishRoutes(Routes(guarded, false))
    }

    private fun stopSsh() {
        if (transportLink != null) {
            NativeBridge.stop()
            transportLink = null
            if (tun != null) setUnderlyingNetworks(underlyingNetworks())
        }
    }

    private fun setState(state: String, message: String, blocked: Boolean = false) {
        if (stopping.get()) return
        val description = if (blocked && protectedCount > 0) "$message\nТрафик приложений с защитой заблокирован." else message
        if (phase == state && detail == description) return
        phase = state
        detail = description
        notifyState()
    }
    override fun onCreate() {
        super.onCreate()
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
        exitIp = ""; exitCountry = ""; sessionId = null
        protectedCount = 0
        active = true; phase = "Подключение"; detail = "Запускаем VPN."
        val notification = notification()
        if (Build.VERSION.SDK_INT >= 34) startForeground(NOTICE, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(NOTICE, notification)
        worker.execute {
            try {
                val settings = SettingsStore(this)
                val snapshot = settings.snapshot()
                require(packageName !in snapshot.packages) { "Нельзя перехватывать собственный SSH-транспорт" }
                val installed = snapshot.packages.filterTo(mutableSetOf()) {
                    try { packageManager.getApplicationInfo(it, 0); true }
                    catch (_: PackageManager.NameNotFoundException) { false }
                }
                require(installed.isNotEmpty()) { "Выберите хотя бы одно установленное приложение" }
                val guarded = snapshot.protectedPackages.intersect(installed)
                session = snapshot.copy(packages = installed, protectedPackages = guarded)
                if (stopping.get()) return@execute
                if (guarded.isNotEmpty()) {
                    establishRoutes(Routes(guarded, false))
                    protectedCount = guarded.size
                }
                verifyExit = settings.verifyProxy
                prepared = true
                main.post(networkWatch)
                main.post(monitor)
            } catch (error: Exception) {
                if (!stopping.get()) failStart(error.message.orEmpty())
            }
        }
        return START_NOT_STICKY
    }
    private fun failStart(reason: String) {
        stopping.set(true)
        finishService("Ошибка", reason)
    }
    private fun markReady(link: UnderlyingNetworkTracker.Link<Network>) {
        if (!isCurrent(link)) return
        underlying.connectedOn(link)
        val current = checkNotNull(session)
        connectedSocksPort = current.socksPort
        if (sessionId == null) {
            val accounting = if (Build.VERSION.SDK_INT >= 29) AppTrafficAccounting(this, current.packages) else null
            appTraffic = accounting
            TProxySetAccounting(accounting != null)
            sessionStarted = System.currentTimeMillis() / 1000
            sessionBaseline = NativeBridge.trafficTotals()
            sessionSavedAt = SystemClock.elapsedRealtime()
            sessionId = sessionStarted.toString()
            sampling.scheduleWithFixedDelay({
                if (!stopping.get()) {
                    val now = SystemClock.elapsedRealtime()
                    val (rx, tx) = NativeBridge.trafficTotals()
                    traffic.sample(now, rx, tx)
                    if (accounting != null) accounting.sample(now, TProxyDrainTraffic() ?: ByteArray(0))
                    if (now - sessionSavedAt >= 30_000) recordSession(true)
                }
            }, 0, 250, TimeUnit.MILLISECONDS)
            if (verifyExit) {
                Thread {
                    SpeedTest.trace(current.socksPort)?.let {
                        if (!stopping.get()) {
                            exitIp = it.ip; exitCountry = it.country
                            recordSession(true)
                        }
                    }
                }.apply { isDaemon = true }.start()
            }
        }
        reportedRejections = 0
        setState("Работает", readyDetail())
    }
    private fun establishRoutes(next: Routes) {
        if (next == routes) return
        val builder = Builder().setSession("Secret Tunnel SSH").setMtu(1500)
            .addAddress("198.18.0.1", 30).addRoute("0.0.0.0", 0).addDnsServer("1.1.1.1")
            .setUnderlyingNetworks(underlyingNetworks())
        // Android blocks IPv6 when the VPN does not configure the family.
        if (next.ipv6) builder.addAddress("fd00:198:18::1", 126).addRoute("::", 0)
        if (Build.VERSION.SDK_INT >= 29) builder.setMetered(false)
        for (pkg in next.packages) builder.addAllowedApplication(pkg)
        // Establish first so Android replaces the UID routes without a direct-traffic window.
        val replacement = builder.establish() ?: error("Android не разрешил VPN-интерфейс")
        val previous = tun
        tun = replacement
        routes = next
        previous?.close()
    }
    private fun startPacketEngine() {
        val status = JSONObject(NativeBridge.status())
        val ipv6 = status.optBoolean("ipv6_supported")
        val capacity = status.optJSONObject("transport")?.optInt("limit", 256) ?: 256
        check(status.optBoolean("running")) { "SSH не готов для обработки пакетов" }
        val current = checkNotNull(session)
        val config = File(filesDir, "tun.yml")
        config.writeText("""
            tunnel:
              mtu: 1500
              ipv4: 198.18.0.1
              ${if (ipv6) "ipv6: 'fd00:198:18::1'" else "# IPv6 blocked: server has no verified IPv6 egress"}
            socks5:
              address: 127.0.0.1
              port: ${current.socksPort}
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
        if (stopping.get()) return
        TProxyStopService() // join the native reader before replacing its borrowed descriptor
        establishRoutes(Routes(current.packages, ipv6))
        check(TProxyStartService(config.absolutePath, checkNotNull(tun).fd)) { "Не удалось запустить обработчик TCP/UDP" }
        check(TProxyIsRunning()) { "Ошибка запуска обработчика TCP/UDP; смотрите журнал" }
    }
    private fun connectSsh(link: UnderlyingNetworkTracker.Link<Network>) {
        val current = checkNotNull(session)
        require(current.route.all { isTrustedSshPin(it.fingerprint) }) { "Ключ сервера не подтверждён" }
        SettingsStore(this).withSshConfig(current.route) { config ->
            if (!isCurrent(link)) return@withSshConfig
            config.put("socks_port", current.socksPort)
                .put("new_session", newSession)
                .put("max_connections", connectionBudget())
                .put("udp_receive_window", current.udpReceive?.toJson())
                .put("agent_udp_receive_window", current.udpAgentReceive?.toJson())
                .put("ssh_tuning",current.ssh.toNativeJson())
            transportLink = link
            if (tun != null) setUnderlyingNetworks(underlyingNetworks())
            val error = NativeBridge.start(config.toString(), this)
            check(error.isEmpty()) { error }
            newSession = false
        }
    }
    // Called from Rust before connect() on the first SSH hop.
    fun bindSshSocket(fd: Int): Boolean {
        val link = transportLink ?: return false
        if (!isCurrent(link) || !protect(fd)) return false
        return runCatching {
            ParcelFileDescriptor.fromFd(fd).use { link.network.bindSocket(it.fileDescriptor) }
            isCurrent(link)
        }.getOrDefault(false)
    }
    private fun underlyingNetworks(): Array<Network> =
        transportLink?.let { arrayOf(it.network) } ?: emptyArray()
    private fun notification(): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val stop = PendingIntent.getService(this, 1, Intent(this, TunnelVpnService::class.java).setAction("STOP"), PendingIntent.FLAG_IMMUTABLE)
        val builder = Notification.Builder(this, CHANNEL).setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentTitle("Secret Tunnel: $phase").setContentText(detail).setContentIntent(open)
            .setStyle(Notification.BigTextStyle().bigText(detail)).setOngoing(true)
        if (protectedCount == 0) builder.addAction(Notification.Action.Builder(null, "Остановить", stop).build())
        return builder.build()
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
        prepared = false
        connectedSocksPort = null
        SpeedTest.tunnelUnavailable("Туннель остановлен. Подключите его и запустите тест заново.")
        main.removeCallbacks(monitor)
        main.removeCallbacks(networkWatch)
        sampling.shutdownNow()
        sampling.awaitTermination(1, TimeUnit.SECONDS)
        recordSession(false)
        exitIp = ""; exitCountry = ""
        TProxyStopService() // join native reader BEFORE closing its borrowed fd
        TProxySetAccounting(false)
        tun?.close(); tun = null; routes = null; protectedCount = 0
        NativeBridge.stop()
        transportLink = null
        File(filesDir, "ssh-key.runtime").delete()
    }
    private fun stopAsync() {
        if (!stopping.compareAndSet(false, true)) return
        phase = "Остановка"; NativeBridge.cancel(); main.removeCallbacks(monitor)
        worker.execute { finishService("Остановлен", "") }
    }
    private fun finishService(state: String, reason: String) {
        cleanup()
        phase = state
        detail = reason
        active = false
        main.post { stopForeground(STOP_FOREGROUND_REMOVE); stopSelf() }
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
