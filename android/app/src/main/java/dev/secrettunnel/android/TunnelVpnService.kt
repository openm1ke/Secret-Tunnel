package dev.secrettunnel.android

import android.app.*
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.VpnService
import android.os.*
import org.json.JSONObject
import java.io.File
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

class TunnelVpnService : VpnService() {
    companion object {
        init { System.loadLibrary("hev-socks5-tunnel") }
        @Volatile var phase = "Остановлен"
        @Volatile var detail = ""
        @Volatile var active = false
        private const val CHANNEL = "ssh-tunnel"
        private const val NOTICE = 1080
    }
    external fun TProxyStartService(configPath: String, fd: Int): Boolean
    external fun TProxyStopService(): Boolean
    external fun TProxyIsRunning(): Boolean
    external fun TProxyGetStats(): LongArray
    private val worker = Executors.newSingleThreadExecutor()
    private val stopping = AtomicBoolean(false)
    private val started = AtomicBoolean(false)
    private var tun: ParcelFileDescriptor? = null
    private val main = Handler(Looper.getMainLooper())
    private var retries = 0
    private val monitor = object : Runnable {
        override fun run() {
            if (stopping.get() || !active) return
            worker.execute {
                if (!stopping.get()) {
                    try {
                        val nativeLog = File(filesDir, "tun.log")
                        if (nativeLog.length() > 1_048_576) java.io.RandomAccessFile(nativeLog, "rw").use { it.setLength(0) }
                        val status = JSONObject(NativeBridge.status())
                        if (!TProxyIsRunning()) throw IllegalStateException("Обработчик TCP/UDP остановился")
                        if (!status.optBoolean("running")) {
                            phase = "Переподключение"
                            detail = "Трафик остаётся в VPN. ${status.optString("error")}"; notifyState()
                            NativeBridge.stop()
                            Thread.sleep((1000L shl retries.coerceAtMost(4)).coerceAtMost(15000L))
                            if (!stopping.get()) { connectSsh(); phase = "Работает"; detail = "TCP и UDP через SSH"; retries = 0; notifyState() }
                        }
                    } catch (error: Exception) {
                        retries++
                        phase = "Ошибка соединения"; detail = error.message.orEmpty(); notifyState()
                    }
                    if (!stopping.get()) main.postDelayed(this, 1500)
                }
            }
        }
    }
    override fun onCreate() {
        super.onCreate()
        File(filesDir, "ssh-key.runtime").delete()
        getSystemService(NotificationManager::class.java).createNotificationChannel(NotificationChannel(CHANNEL, "SSH-туннель", NotificationManager.IMPORTANCE_LOW))
    }
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == "STOP") { stopAsync(); return START_NOT_STICKY }
        if (!started.compareAndSet(false, true)) return START_NOT_STICKY
        stopping.set(false); active = true; phase = "Подключение"; detail = "Проверка SSH-сервера"
        val notification = notification()
        if (Build.VERSION.SDK_INT >= 34) startForeground(NOTICE, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(NOTICE, notification)
        worker.execute {
            try {
                val settings = SettingsStore(this)
                val packages = settings.packages
                require(packages.isNotEmpty()) { "Выберите хотя бы одно приложение" }
                require(packageName !in packages) { "Нельзя перехватывать собственный SSH-транспорт" }
                // SSH authenticates before claiming routes. Socket protection also
                // applies on reconnection while the VPN interface already exists.
                connectSsh()
                if (stopping.get()) return@execute
                val builder = Builder().setSession("Secret Tunnel SSH").setMtu(1500)
                    .addAddress("198.18.0.1", 30).addAddress("fd00:198:18::1", 126)
                    .addRoute("0.0.0.0", 0).addRoute("::", 0).addDnsServer("1.1.1.1")
                for (pkg in packages) builder.addAllowedApplication(pkg)
                tun = builder.establish() ?: error("Android не разрешил VPN-интерфейс")
                val config = File(filesDir, "tun.yml")
                config.writeText("""
                    tunnel:
                      mtu: 1500
                      ipv4: 198.18.0.1
                      ipv6: 'fd00:198:18::1'
                    socks5:
                      address: 127.0.0.1
                      port: 1080
                      udp: 'udp'
                    misc:
                      task-stack-size: 65536
                      tcp-buffer-size: 32768
                      max-session-count: 256
                      connect-timeout: 10000
                      read-write-timeout: 300000
                      log-file: '${File(filesDir, "tun.log").absolutePath}'
                      log-level: warn
                """.trimIndent())
                File(filesDir, "tun.log").writeText("")
                check(TProxyStartService(config.absolutePath, tun!!.fd)) { "Не удалось запустить обработчик TCP/UDP" }
                Thread.sleep(150)
                check(TProxyIsRunning()) { "Ошибка запуска обработчика TCP/UDP; смотрите журнал" }
                phase = "Работает"; detail = "TCP и UDP · приложений: ${packages.size}"; notifyState()
                main.postDelayed(monitor, 1500)
            } catch (error: Exception) {
                phase = "Ошибка"; detail = error.message.orEmpty()
                cleanup(); active = false; started.set(false)
                main.post { stopForeground(STOP_FOREGROUND_REMOVE); stopSelf() }
            }
        }
        return START_NOT_STICKY
    }
    private fun connectSsh() {
        if (stopping.get()) return
        val settings = SettingsStore(this)
        val key = File(filesDir, "ssh-key.runtime")
        try {
            val bytes = settings.decryptKey()
            try {
                openFileOutput(key.name, MODE_PRIVATE).use { it.write(bytes) }
            } finally { bytes.fill(0) }
            val config = JSONObject().put("host", settings.host).put("port", settings.port).put("user", settings.user)
                .put("key_path", key.absolutePath).put("host_key_sha256", settings.fingerprint).put("socks_port", 1080)
                .put("new_session", tun == null)
            val error = NativeBridge.start(config.toString(), this)
            check(error.isEmpty()) { error }
        } finally { key.delete() }
    }
    private fun notification(): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val stop = PendingIntent.getService(this, 1, Intent(this, TunnelVpnService::class.java).setAction("STOP"), PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, CHANNEL).setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentTitle("Secret Tunnel: $phase").setContentText(detail).setContentIntent(open)
            .setOngoing(true).addAction(Notification.Action.Builder(null, "Остановить", stop).build()).build()
    }
    private fun notifyState() { if (active) getSystemService(NotificationManager::class.java).notify(NOTICE, notification()) }
    private fun cleanup() {
        main.removeCallbacks(monitor)
        TProxyStopService() // join native reader BEFORE closing its borrowed fd
        tun?.close(); tun = null
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
        if (active && !stopping.get()) stopAsync()
        worker.shutdown()
        super.onDestroy()
    }
}
