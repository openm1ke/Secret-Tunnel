package dev.secrettunnel.android

import android.content.Context
import android.net.ConnectivityManager
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.Closeable
import java.io.File
import java.io.InputStream
import java.io.OutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.net.SocketException
import java.net.SocketTimeoutException
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference
import javax.net.ssl.HttpsURLConnection
import javax.net.ssl.SSLSocket
import javax.net.ssl.SSLSocketFactory

/**
 * Speed tests, the same as on the desktop: through the tunnel's local SOCKS
 * proxy or straight from the phone. Steps, results and history use the
 * desktop's names and JSON fields, so both apps read the same way.
 */
object SpeedKinds {
    data class Kind(val id: String, val title: String, val text: String, val time: String, val traffic: String)
    val all = listOf(
        Kind("quick", "Быстрый", "Пинг, загрузка и отдача в один поток.", "≈ 15 с", "до 150 МБ"),
        Kind("full", "Полный", "Все проверки по очереди: пинг, UDP, один и шесть потоков, отдача, короткие запросы.", "≈ 50 с", "до 400 МБ"),
        Kind("bandwidth", "Ширина канала", "8 потоков вниз и 4 вверх, как торрент или облачная синхронизация. Пинг под нагрузкой.", "≈ 25 с", "до 400 МБ"),
        Kind("browsing", "Как браузер", "60 коротких запросов, каждый по новому соединению, и DNS. Отвечает, насколько быстро открываются страницы.", "≈ 15 с", "≈ 5 МБ"),
        Kind("mixed", "Смешанный", "Две длинные загрузки, поток коротких запросов и пинг одновременно: несколько программ сразу.", "≈ 20 с", "до 200 МБ"),
        Kind("latency", "Задержка и UDP", "30 пингов по открытому соединению и 30 DNS-запросов по UDP: звонки и игры.", "≈ 15 с", "< 1 МБ"),
        Kind("mlab", "M-Lab NDT7", "Методика M-Lab: по 10 секунд загрузки и отдачи одним соединением до ближайшего сервера. M-Lab публикует результаты вместе с IP-адресом в открытом наборе данных.", "≈ 25 с", "до 300 МБ"),
    )
    fun title(id: String) = all.firstOrNull { it.id == id }?.title ?: id
}

class SpeedResults {
    var pingMs: Double? = null; var pingMinMs: Double? = null; var jitterMs: Double? = null
    var loadedPingMs: Double? = null; var dnsMs: Double? = null; var dnsLoss: Double? = null
    var downloadSingleMbps: Double? = null; var downloadMbps: Double? = null; var uploadMbps: Double? = null
    var requestsPerSecond: Double? = null; var requestMs: Double? = null
    var mixedDownloadMbps: Double? = null; var mixedRequestMs: Double? = null
    var downloadSource: String? = null; var serverRttMs: Double? = null
    var bytesDown = 0L; var bytesUp = 0L

    private fun fields() = listOf(
        "ping_ms" to pingMs, "ping_min_ms" to pingMinMs, "jitter_ms" to jitterMs, "loaded_ping_ms" to loadedPingMs,
        "dns_ms" to dnsMs, "dns_loss" to dnsLoss, "download_single_mbps" to downloadSingleMbps, "download_mbps" to downloadMbps,
        "upload_mbps" to uploadMbps, "requests_per_second" to requestsPerSecond, "request_ms" to requestMs,
        "mixed_download_mbps" to mixedDownloadMbps, "mixed_request_ms" to mixedRequestMs, "server_rtt_ms" to serverRttMs,
    )
    fun isEmpty() = fields().all { it.second == null } && bytesDown == 0L && bytesUp == 0L
    fun copy(): SpeedResults = fromJson(toJson())
    fun toJson(): JSONObject = JSONObject().apply {
        fields().forEach { (key, value) -> put(key, value ?: JSONObject.NULL) }
        put("download_source", downloadSource ?: JSONObject.NULL); put("bytes_down", bytesDown); put("bytes_up", bytesUp)
    }
    companion object {
        fun fromJson(o: JSONObject) = SpeedResults().apply {
            fun d(key: String) = if (o.isNull(key)) null else o.optDouble(key).takeIf { !it.isNaN() }
            pingMs = d("ping_ms"); pingMinMs = d("ping_min_ms"); jitterMs = d("jitter_ms"); loadedPingMs = d("loaded_ping_ms")
            dnsMs = d("dns_ms"); dnsLoss = d("dns_loss"); downloadSingleMbps = d("download_single_mbps"); downloadMbps = d("download_mbps")
            uploadMbps = d("upload_mbps"); requestsPerSecond = d("requests_per_second"); requestMs = d("request_ms")
            mixedDownloadMbps = d("mixed_download_mbps"); mixedRequestMs = d("mixed_request_ms"); serverRttMs = d("server_rtt_ms")
            downloadSource = if (o.isNull("download_source")) null else o.optString("download_source").ifBlank { null }
            bytesDown = o.optLong("bytes_down"); bytesUp = o.optLong("bytes_up")
        }
    }
}

data class SpeedRun(
    val id: String, val kind: String, val mode: String, val exitIp: String, val exitCountry: String, val edge: String,
    val provider: String, val startedAt: Long, val finishedAt: Long, val server: String, val route: List<String>,
    val country: String, val results: SpeedResults, val error: String, val cancelled: Boolean,
) {
    val direct get() = mode == "direct"
    fun toJson(): JSONObject = JSONObject().put("id", id).put("kind", kind).put("mode", mode).put("exit_ip", exitIp)
        .put("exit_country", exitCountry).put("edge", edge).put("provider", provider).put("started_at", startedAt)
        .put("finished_at", finishedAt).put("server", server).put("route", JSONArray(route)).put("country", country)
        .put("results", results.toJson()).put("error", error).put("cancelled", cancelled)
    companion object {
        fun fromJson(o: JSONObject): SpeedRun {
            val route = o.optJSONArray("route")
            return SpeedRun(o.optString("id"), o.optString("kind"), o.optString("mode", "tunnel"), o.optString("exit_ip"),
                o.optString("exit_country"), o.optString("edge"), o.optString("provider"), o.optLong("started_at"),
                o.optLong("finished_at"), o.optString("server"), (0 until (route?.length() ?: 0)).map { route!!.optString(it) },
                o.optString("country"), SpeedResults.fromJson(o.optJSONObject("results") ?: JSONObject()),
                o.optString("error"), o.optBoolean("cancelled"))
        }
    }
}

/** What the screen shows while a test runs. */
class SpeedLive {
    @Volatile var running = false
    @Volatile var kind = ""; @Volatile var mode = ""; @Volatile var provider = ""
    @Volatile var exitIp = ""; @Volatile var exitCountry = ""; @Volatile var edge = ""
    @Volatile var steps = listOf<String>(); @Volatile var step = 0; @Volatile var progress = 0.0
    @Volatile var downMbps = 0.0; @Volatile var upMbps = 0.0; @Volatile var pingMs: Double? = null
    @Volatile var results = SpeedResults(); @Volatile var error = ""; @Volatile var last: SpeedRun? = null
}

object SpeedHistory {
    private const val MAX_RUNS = 200
    private fun file(context: Context) = File(context.filesDir, "speedtests.json")
    @Synchronized fun load(context: Context): List<SpeedRun> = runCatching {
        val array = JSONArray(file(context).readText())
        (0 until array.length()).map { SpeedRun.fromJson(array.getJSONObject(it)) }
    }.getOrDefault(emptyList())
    @Synchronized fun add(context: Context, run: SpeedRun) {
        val runs = (load(context) + run).takeLast(MAX_RUNS)
        write(context, runs)
    }
    @Synchronized fun clear(context: Context) = write(context, emptyList())
    private fun write(context: Context, runs: List<SpeedRun>) {
        val target = file(context)
        val temporary = File(target.parentFile, target.name + ".tmp")
        temporary.writeText(JSONArray(runs.map { it.toJson() }).toString())
        temporary.renameTo(target)
    }
}

/** The address, country and Cloudflare location a path exits through. */
data class ExitTrace(val ip: String, val country: String, val edge: String)

object SpeedTest {
    private const val CLOUDFLARE = "speed.cloudflare.com"
    /** Download requests stay well under the endpoint's 100 MB limit; it also
     *  rate-limits an address that asks for many large ones (HTTP 429). */
    private const val CHUNK = 25_000_000L
    private const val FALLBACK_HOST = "nbg1-speed.hetzner.com"
    private const val FALLBACK_PATH = "/1GB.bin"
    private const val WARMUP_MS = 1500L
    val live = SpeedLive()
    private var current: RunControl? = null

    fun cancel() = synchronized(live) { current?.abort("Тест остановлен пользователем.") }

    fun tunnelUnavailable(reason: String) = synchronized(live) {
        current?.takeIf { it.throughTunnel }?.abort(reason)
    }

    /** Where traffic through this proxy (or none) comes out. */
    fun trace(proxyPort: Int?): ExitTrace? = trace(proxyPort, null)

    private fun trace(proxyPort: Int?, sockets: SpeedSockets?): ExitTrace? = runCatching {
        Http.fetchText(proxyPort, CLOUDFLARE, "/cdn-cgi/trace", 8000, sockets)
    }.getOrNull()?.let { text ->
        fun field(name: String) = text.lineSequence().firstOrNull { it.startsWith("$name=") }?.substringAfter('=')?.trim().orEmpty()
        field("ip").takeIf { it.isNotBlank() }?.let { ExitTrace(it, field("loc"), field("colo")) }
    }

    data class Target(val proxyPort: Int?, val provider: String, val server: String, val route: List<String>, val country: String)

    fun start(context: Context, kind: String, target: Target, done: (SpeedRun) -> Unit) {
        val steps = plan(kind) ?: error("Неизвестный тест")
        val control = RunControl(target.proxyPort != null)
        val connectivity = context.getSystemService(ConnectivityManager::class.java)
        val initialNetwork = if (control.throughTunnel) null else connectivity.activeNetwork
        synchronized(live) {
            require(!live.running) { "Тест уже идёт" }
            if (control.throughTunnel) require(TunnelVpnService.connectedSocksPort == target.proxyPort) {
                "Дождитесь восстановления туннеля."
            }
            current = control
            val previous = live.last
            live.apply {
                running = true; this.kind = kind; mode = if (target.proxyPort != null) "tunnel" else "direct"
                provider = ""; exitIp = ""; exitCountry = ""; edge = ""; this.steps = steps.map { it.title() }; step = 0
                progress = 0.0; downMbps = 0.0; upMbps = 0.0; pingMs = null; results = SpeedResults(); error = ""; last = previous
            }
        }
        Thread {
            val started = System.currentTimeMillis() / 1000
            val mode = if (target.proxyPort != null) "tunnel" else "direct"
            var exit = ExitTrace("", "", "")
            var error = ""
            var providerLabel = ""
            val watcher = if (control.throughTunnel) null else Executors.newSingleThreadScheduledExecutor()
            watcher?.scheduleWithFixedDelay({
                if (connectivity.activeNetwork != initialNetwork) synchronized(live) {
                    if (current === control) control.abort("Сеть изменилась. Запустите тест заново.")
                }
            }, 0, 500, TimeUnit.MILLISECONDS)
            try {
                val refused = AtomicBoolean(false)
                val chosen = control.withSockets { sockets ->
                    trace(target.proxyPort, sockets)?.let {
                        exit = it; live.exitIp = it.ip; live.exitCountry = it.country; live.edge = it.edge
                    }
                    if (control.stopped()) return@withSockets Provider.Cloudflare to ""
                    val tester = Tester(target.proxyPort, control, sockets, refused)
                    when {
                        kind == "mlab" -> tester.locateNdt7()
                        target.provider == "librespeed" -> tester.nearestLibreSpeed()
                        else -> Provider.Cloudflare to "Cloudflare" + if (exit.edge.isBlank()) "" else " · ${exit.edge}"
                    }
                }
                providerLabel = chosen.second; live.provider = providerLabel
                for ((index, step) in steps.withIndex()) {
                    if (control.stopped()) break
                    live.step = index; live.progress = 0.0
                    control.withSockets { sockets ->
                        Tester(target.proxyPort, control, sockets, refused, chosen.first).run(step)
                    }
                }
            } catch (e: Exception) {
                val stage = if (providerLabel.isBlank()) "Выбор сервера" else steps[live.step].title()
                error = "$stage: ${e.message ?: e.javaClass.simpleName}"
            } finally {
                watcher?.shutdownNow()
            }
            val run = synchronized(live) {
                val interruption = control.interruption.get()
                val result = SpeedRun("$started-$mode-$kind", kind, mode, exit.ip, exit.country, exit.edge, providerLabel, started,
                    System.currentTimeMillis() / 1000, target.server, target.route, target.country, live.results.copy(),
                    interruption ?: error, interruption != null)
                if (!result.cancelled && !result.results.isEmpty()) runCatching { SpeedHistory.add(context, result) }
                current = null
                live.apply { downMbps = 0.0; upMbps = 0.0; this.error = result.error; last = result; running = false }
                result
            }
            done(run)
        }.apply { name = "speed-test"; isDaemon = true }.start()
    }

    private class RunControl(val throughTunnel: Boolean) {
        val interruption = AtomicReference<String?>(null)
        @Volatile private var activeSockets: SpeedSockets? = null
        fun stopped() = interruption.get() != null

        fun abort(reason: String) {
            if (interruption.compareAndSet(null, reason)) activeSockets?.close()
        }

        fun <T> withSockets(run: (SpeedSockets) -> T): T = SpeedSockets().use { sockets ->
            activeSockets = sockets
            try {
                check(!stopped())
                run(sockets)
            } finally {
                activeSockets = null
            }
        }
    }

    private sealed class Step {
        data class Ping(val count: Int) : Step()
        data class Dns(val count: Int) : Step()
        data class Download(val streams: Int, val seconds: Int, val loadedPing: Boolean) : Step()
        data class Upload(val streams: Int, val seconds: Int) : Step()
        data class Requests(val count: Int, val parallel: Int, val bytes: Long) : Step()
        data class Mixed(val seconds: Int) : Step()
        object Ndt7Download : Step()
        object Ndt7Upload : Step()
        fun title(): String = when (this) {
            is Ping -> "Пинг"
            is Dns -> "UDP: DNS-запросы"
            is Download -> if (streams == 1) "Загрузка, 1 поток" else "Загрузка, $streams потоков"
            is Upload -> if (streams == 1) "Отдача, 1 поток" else "Отдача, $streams потока"
            is Requests -> "Короткие запросы, $count"
            is Mixed -> "Смешанная нагрузка"
            Ndt7Download -> "NDT7: загрузка"
            Ndt7Upload -> "NDT7: отдача"
        }
    }

    private fun plan(kind: String): List<Step>? = when (kind) {
        "quick" -> listOf(Step.Ping(10), Step.Download(1, 6, false), Step.Upload(1, 5))
        "full" -> listOf(Step.Ping(20), Step.Dns(20), Step.Download(1, 8, false), Step.Download(6, 8, true),
            Step.Upload(3, 8), Step.Requests(40, 6, 50_000))
        "bandwidth" -> listOf(Step.Ping(10), Step.Download(8, 10, true), Step.Upload(4, 8))
        "browsing" -> listOf(Step.Ping(10), Step.Dns(20), Step.Requests(60, 6, 50_000))
        "mixed" -> listOf(Step.Ping(10), Step.Mixed(12))
        "latency" -> listOf(Step.Ping(30), Step.Dns(30))
        "mlab" -> listOf(Step.Ndt7Download, Step.Ndt7Upload)
        else -> null
    }

    private sealed class Provider {
        object Cloudflare : Provider()
        /** A LibreSpeed backend: host and path prefix without a trailing slash. */
        data class LibreSpeed(val host: String, val base: String) : Provider()
        data class Ndt7(val down: String, val up: String) : Provider()
    }

    private fun median(values: List<Double>): Double? {
        if (values.isEmpty()) return null
        val sorted = values.sorted(); val middle = sorted.size / 2
        return if (sorted.size % 2 == 0) (sorted[middle - 1] + sorted[middle]) / 2 else sorted[middle]
    }
    private fun jitter(samples: List<Double>) = if (samples.size > 1) samples.zipWithNext { a, b -> kotlin.math.abs(b - a) }.average() else null
    private fun mbps(bytes: Long, millis: Long) = if (millis <= 0) 0.0 else bytes * 8.0 / (millis / 1000.0) / 1_000_000.0

    /** Incompressible bytes, so no layer on the way can shrink them. */
    private val noise: ByteArray by lazy {
        var state = -0x61c8864680b583ebL
        ByteArray(64 * 1024) { state = state xor (state shl 13); state = state xor (state ushr 7); state = state xor (state shl 17); state.toByte() }
    }

    private class Tester(
        val proxyPort: Int?, private val control: RunControl, private val sockets: SpeedSockets,
        private val refused: AtomicBoolean, private val provider: Provider = Provider.Cloudflare,
    ) {

        fun run(step: Step) = when (step) {
            is Step.Ping -> ping(step.count)
            is Step.Dns -> dns(step.count)
            is Step.Download -> download(step.streams, step.seconds, step.loadedPing)
            is Step.Upload -> upload(step.streams, step.seconds)
            is Step.Requests -> requests(step.count, step.parallel, step.bytes)
            is Step.Mixed -> mixed(step.seconds)
            Step.Ndt7Download -> ndt7(false)
            Step.Ndt7Upload -> ndt7(true)
        }

        private fun pingTarget(): Pair<String, String> = when (val p = provider) {
            is Provider.LibreSpeed -> p.host to "${p.base}/empty.php"
            else -> CLOUDFLARE to "/__down?bytes=0"
        }
        private fun smallTarget(bytes: Long): Pair<String, String> = when (val p = provider) {
            is Provider.LibreSpeed -> p.host to "${p.base}/garbage.php?ckSize=1"
            else -> CLOUDFLARE to "/__down?bytes=$bytes"
        }

        /** Round trips of empty requests on one connection; the first (TLS setup) is skipped. */
        private fun pingLoop(count: Int, samples: MutableList<Double>, stop: AtomicBoolean, deadline: Long) {
            val (host, path) = pingTarget()
            Http.Link(proxyPort, host, sockets = sockets).use { link ->
                link.get(path).drain()
                repeat(count) {
                    if (stop.get() || control.stopped() || System.currentTimeMillis() > deadline) return
                    val sent = System.nanoTime()
                    val response = link.get(path)
                    val ms = (System.nanoTime() - sent) / 1e6
                    response.drain()
                    synchronized(samples) { samples.add(ms) }
                    live.pingMs = ms
                }
            }
        }

        fun ping(count: Int) {
            val samples = mutableListOf<Double>()
            val progress = Thread { while (!Thread.interrupted()) { live.progress = synchronized(samples) { samples.size } / count.toDouble(); try { Thread.sleep(100) } catch (_: InterruptedException) { return@Thread } } }
            progress.start()
            try {
                pingLoop(count, samples, AtomicBoolean(false), System.currentTimeMillis() + 10_000 + count * 1000L)
            } finally {
                progress.interrupt()
                progress.join()
            }
            require(samples.isNotEmpty()) { "нет ответа через туннель" }
            live.results.pingMs = median(samples); live.results.pingMinMs = samples.min(); live.results.jitterMs = jitter(samples)
            live.progress = 1.0
        }

        /** DNS questions over UDP: through the tunnel's relay (SOCKS5 UDP ASSOCIATE), or straight to 1.1.1.1. */
        fun dns(count: Int) {
            val samples = mutableListOf<Double>(); var lost = 0
            fun ask(socket: DatagramSocket, to: InetSocketAddress, socks: Boolean) {
                socket.soTimeout = 1500
                for (index in 0 until count) {
                    if (control.stopped()) break
                    val id = (0x5100 + index) and 0xffff
                    val packet = java.io.ByteArrayOutputStream()
                    if (socks) packet.write(byteArrayOf(0, 0, 0, 1, 1, 1, 1, 1, 0, 53))
                    val header = packet.size()
                    packet.write(byteArrayOf((id shr 8).toByte(), id.toByte(), 1, 0, 0, 1, 0, 0, 0, 0, 0, 0))
                    for (label in listOf("cloudflare", "com")) { packet.write(label.length); packet.write(label.toByteArray()) }
                    packet.write(byteArrayOf(0, 0, 1, 0, 1))
                    val bytes = packet.toByteArray()
                    val sent = System.nanoTime()
                    socket.send(DatagramPacket(bytes, bytes.size, to))
                    val buffer = ByteArray(1500)
                    var answered = false
                    while (true) {
                        val reply = DatagramPacket(buffer, buffer.size)
                        try { socket.receive(reply) } catch (_: SocketTimeoutException) { break }
                        if (reply.length > header + 2 && (buffer[header].toInt() and 255) == id shr 8 && (buffer[header + 1].toInt() and 255) == id and 255) { answered = true; break }
                    }
                    if (answered) { val ms = (System.nanoTime() - sent) / 1e6; samples.add(ms); live.pingMs = ms } else lost++
                    live.progress = (index + 1) / count.toDouble()
                    Thread.sleep(100)
                }
            }
            if (proxyPort == null) {
                sockets.track(DatagramSocket()).use { ask(it, InetSocketAddress("1.1.1.1", 53), false) }
            } else {
                sockets.track(Socket()).use { control ->
                    control.connect(InetSocketAddress("127.0.0.1", proxyPort), 5000); control.soTimeout = 5000
                    val out = control.getOutputStream(); val input = control.getInputStream()
                    out.write(byteArrayOf(5, 1, 0)); Http.readFully(input, 2)
                    out.write(byteArrayOf(5, 3, 0, 1, 0, 0, 0, 0, 0, 0))
                    val head = Http.readFully(input, 4)
                    require(head[1].toInt() == 0) { "туннель не открыл UDP (код ${head[1]})" }
                    val address = when (head[3].toInt()) { 1 -> Http.readFully(input, 4); 4 -> Http.readFully(input, 16); else -> error("неожиданный ответ SOCKS") }
                    val port = Http.readFully(input, 2).let { (it[0].toInt() and 255) * 256 + (it[1].toInt() and 255) }
                    val relayIp = InetAddress.getByAddress(address).let { if (it.isAnyLocalAddress) InetAddress.getByName("127.0.0.1") else it }
                    sockets.track(DatagramSocket(0, InetAddress.getByName("127.0.0.1"))).use { ask(it, InetSocketAddress(relayIp, port), true) }
                }
            }
            val asked = samples.size + lost
            live.results.dnsMs = median(samples)
            live.results.dnsLoss = if (asked > 0) lost / asked.toDouble() else null
            require(live.results.dnsMs != null || asked == 0) { "UDP-запросы не получили ответа" }
        }

        /** A chain of the largest allowed downloads on one connection, counted as bytes arrive. */
        private fun downloadStream(counter: AtomicLong, stop: AtomicBoolean) {
            while (!stop.get() && !control.stopped()) {
                val p = provider
                val (host, path) = when {
                    p is Provider.LibreSpeed -> p.host to "${p.base}/garbage.php?ckSize=25"
                    refused.get() -> FALLBACK_HOST to FALLBACK_PATH
                    else -> CLOUDFLARE to "/__down?bytes=$CHUNK"
                }
                Http.Link(proxyPort, host, sockets = sockets).use { link ->
                    while (!stop.get() && !control.stopped()) {
                        val response = link.get(path)
                        if (response.status == 429 || response.status == 403) { response.close(); if (p is Provider.Cloudflare) { refused.set(true); return@use } else error("HTTP ${response.status}") }
                        require(response.status == 200) { "HTTP ${response.status}" }
                        response.body { counter.addAndGet(it.toLong()); !stop.get() && !control.stopped() }
                        if (!response.keepAlive) return@use
                    }
                }
            }
        }

        /** An upload that streams for as long as the step runs. */
        private fun uploadStream(counter: AtomicLong, stop: AtomicBoolean) {
            val p = provider
            if (p is Provider.LibreSpeed) {
                // LibreSpeed takes uploads as repeated 4 MiB POSTs, like its own web test.
                Http.Link(proxyPort, p.host, sockets = sockets).use { link ->
                    while (!stop.get() && !control.stopped()) {
                        val response = link.post("${p.base}/empty.php", 4L shl 20) { out, sent -> out.write(noise, 0, minOf(noise.size.toLong(), (4L shl 20) - sent).toInt()) }
                        response.drain(); counter.addAndGet(4L shl 20)
                        if (!response.keepAlive) break
                    }
                }
                return
            }
            Http.Link(proxyPort, CLOUDFLARE, sockets = sockets).use { link ->
                link.postChunked("/__up") { out ->
                    while (!stop.get() && !control.stopped()) { out.write(noise); counter.addAndGet(noise.size.toLong()) }
                }.close()
            }
        }

        /** Run streams for a while, restarting any that end; returns bytes after warm-up, their time and all bytes. */
        private fun sustain(seconds: Int, streams: Int, upload: Boolean): Triple<Long, Long, Long> {
            val counter = AtomicLong(); val stop = AtomicBoolean(false)
            val pool = Executors.newFixedThreadPool(streams)
            val firstError = AtomicReference<String?>(null)
            repeat(streams) {
                pool.execute {
                    while (!stop.get() && !control.stopped()) {
                        try {
                            if (upload) uploadStream(counter, stop) else downloadStream(counter, stop)
                        } catch (e: Exception) {
                            if (stop.get() || control.stopped()) return@execute
                            firstError.compareAndSet(null, e.message ?: e.javaClass.simpleName)
                            try {
                                Thread.sleep(300)
                            } catch (_: InterruptedException) {
                                return@execute
                            }
                        }
                    }
                }
            }
            val start = System.currentTimeMillis(); val total = seconds * 1000L
            val window = ArrayDeque<Pair<Long, Long>>(); var afterWarmup: Pair<Long, Long>? = null
            try {
                while (System.currentTimeMillis() - start < total && !control.stopped()) {
                    Thread.sleep(250)
                    val now = System.currentTimeMillis(); val bytes = counter.get()
                    window.addLast(now to bytes)
                    while (window.isNotEmpty() && now - window.first().first > 1000) window.removeFirst()
                    val rate = window.firstOrNull()?.let { mbps(bytes - it.second, now - it.first) } ?: 0.0
                    if (afterWarmup == null && now - start >= WARMUP_MS) afterWarmup = now to bytes
                    if (upload) live.upMbps = rate else live.downMbps = rate
                    live.progress = ((now - start) / total.toDouble()).coerceAtMost(1.0)
                }
            } finally {
                stop.set(true)
                pool.shutdownNow()
                sockets.close()
                pool.awaitTermination(3, TimeUnit.SECONDS)
            }
            val bytes = counter.get(); val (from, atWarmup) = afterWarmup ?: (start to 0L)
            if (upload) live.upMbps = 0.0 else live.downMbps = 0.0
            if (bytes == 0L) firstError.get()?.let { error(it) }
            return Triple(bytes - atWarmup, System.currentTimeMillis() - from, bytes)
        }

        private fun backgroundPing(samples: MutableList<Double>, stop: AtomicBoolean): Thread =
            Thread { runCatching { pingLoop(200, samples, stop, Long.MAX_VALUE) } }.apply { isDaemon = true; start() }

        fun download(streams: Int, seconds: Int, loadedPing: Boolean) {
            val samples = mutableListOf<Double>(); val stopPing = AtomicBoolean(false)
            val pinger = if (loadedPing) backgroundPing(samples, stopPing) else null
            val (bytes, millis, total) = try { sustain(seconds, streams, false) } finally { stopPing.set(true) }
            pinger?.join(2000)
            require(total >= 1_000_000) { if (refused.get()) "источники загрузки отказали" else "ничего не загрузилось" }
            val rate = mbps(bytes, millis)
            if (provider is Provider.Cloudflare && refused.get()) live.results.downloadSource = "Hetzner, Нюрнберг: Cloudflare временно ограничил загрузки"
            if (streams == 1) live.results.downloadSingleMbps = rate else live.results.downloadMbps = rate
            if (live.results.downloadMbps == null) live.results.downloadMbps = rate
            if (loadedPing) live.results.loadedPingMs = median(synchronized(samples) { samples.toList() })
            live.results.bytesDown += total
        }

        fun upload(streams: Int, seconds: Int) {
            val (bytes, millis, total) = sustain(seconds, streams, true)
            require(total > 0) { "ничего не отправилось" }
            live.results.uploadMbps = mbps(bytes, millis); live.results.bytesUp += total
        }

        /** One short request on a new connection: connect, TLS and answer, like a browser opening a resource. */
        private fun shortRequest(bytes: Long): Pair<Double, Long> {
            val (host, path) = smallTarget(bytes)
            val start = System.nanoTime()
            var received = 0L
            Http.Link(proxyPort, host, sockets = sockets).use { link -> link.get(path, close = true).body { received += it; true } }
            return (System.nanoTime() - start) / 1e6 to received
        }

        fun requests(count: Int, parallel: Int, bytes: Long) {
            val start = System.currentTimeMillis()
            val times = java.util.Collections.synchronizedList(mutableListOf<Double>()); val received = AtomicLong()
            val pool = Executors.newFixedThreadPool(parallel)
            repeat(count) {
                pool.execute {
                    if (control.stopped() || sockets.isClosed()) return@execute
                    runCatching { shortRequest(bytes) }.getOrNull()?.takeIf { it.second > 0 }?.let { (ms, size) ->
                        if (control.stopped() || sockets.isClosed()) return@execute
                        times.add(ms); val total = received.addAndGet(size)
                        live.progress = times.size / count.toDouble(); live.pingMs = ms
                        live.downMbps = mbps(total, System.currentTimeMillis() - start)
                    }
                }
            }
            pool.shutdown()
            try {
                if (!pool.awaitTermination(120, TimeUnit.SECONDS)) error("запросы не завершились за 120 секунд")
            } finally {
                sockets.close()
                pool.shutdownNow()
                pool.awaitTermination(3, TimeUnit.SECONDS)
            }
            val elapsed = System.currentTimeMillis() - start
            require(times.isNotEmpty()) { "запросы не прошли" }
            live.results.requestsPerSecond = times.size / (elapsed / 1000.0); live.results.requestMs = median(times.toList())
            live.results.bytesDown += received.get(); live.downMbps = 0.0
        }

        fun mixed(seconds: Int) {
            val samples = mutableListOf<Double>(); val stop = AtomicBoolean(false)
            val pinger = backgroundPing(samples, stop)
            val times = java.util.Collections.synchronizedList(mutableListOf<Double>())
            val burst = Executors.newFixedThreadPool(4)
            repeat(4) { burst.execute { while (!stop.get() && !control.stopped()) runCatching { shortRequest(50_000) }.getOrNull()?.let { times.add(it.first) } } }
            val (bytes, millis, total) = try {
                sustain(seconds, 2, false)
            } finally {
                stop.set(true)
                burst.shutdownNow()
                burst.awaitTermination(3, TimeUnit.SECONDS)
                pinger.join(2000)
            }
            require(total >= 1_000_000) { "ничего не загрузилось" }
            live.results.mixedDownloadMbps = mbps(bytes, millis); live.results.mixedRequestMs = median(times.toList())
            live.results.loadedPingMs = median(synchronized(samples) { samples.toList() }); live.results.bytesDown += total
        }

        /** The public LibreSpeed server that answers fastest on this path. */
        fun nearestLibreSpeed(): Pair<Provider, String> {
            val list = JSONArray(Http.fetchText(proxyPort, "librespeed.org", "/backend-servers/servers.php", 10_000, sockets))
            val candidates = (0 until list.length()).mapNotNull { index ->
                val o = list.optJSONObject(index) ?: return@mapNotNull null
                val url = o.optString("server").trimEnd('/')
                if (!url.startsWith("https://")) return@mapNotNull null
                val rest = url.removePrefix("https://")
                Triple(rest.substringBefore('/'), if ('/' in rest) "/" + rest.substringAfter('/') else "", o.optString("name"))
            }.take(40)
            val pool = Executors.newFixedThreadPool(12)
            val timed = candidates.map { (host, base, name) ->
                pool.submit<Pair<Double, Triple<String, String, String>>?> {
                    runCatching {
                        val start = System.nanoTime()
                        Http.Link(proxyPort, host, timeout = 4000, sockets = sockets).use { link -> link.get("$base/empty.php").also { require(it.status == 200) }.drain() }
                        (System.nanoTime() - start) / 1e6 to Triple(host, base, name)
                    }.getOrNull()
                }
            }
            val best = try {
                timed.mapNotNull {
                    if (control.stopped()) return@mapNotNull null
                    runCatching { it.get(6, TimeUnit.SECONDS) }.getOrNull()
                }.minByOrNull { it.first }
            } finally {
                sockets.close()
                pool.shutdownNow()
            }
            val (host, base, name) = best?.second ?: error("ни один сервер LibreSpeed не ответил")
            return Provider.LibreSpeed(host, base) to "LibreSpeed · $name"
        }

        /** M-Lab's nearest NDT7 server on this path. */
        fun locateNdt7(): Pair<Provider, String> {
            val answer = JSONObject(Http.fetchText(proxyPort, "locate.measurementlab.net", "/v2/nearest/ndt/ndt7", 10_000, sockets))
            val result = answer.optJSONArray("results")?.optJSONObject(0) ?: error("M-Lab не нашёл сервер")
            val urls = result.optJSONObject("urls") ?: error("M-Lab не выдал адреса теста")
            val down = urls.optString("ws:///ndt/v7/download"); val up = urls.optString("ws:///ndt/v7/upload")
            require(down.isNotBlank() && up.isNotBlank()) { "M-Lab не выдал адреса теста" }
            val place = result.optJSONObject("location")?.let { "${it.optString("city")}, ${it.optString("country")}" }.orEmpty()
            return Provider.Ndt7(down, up) to "M-Lab NDT7 · $place"
        }

        /** M-Lab's method: ten seconds on one connection, download or upload. */
        fun ndt7(upload: Boolean) {
            val p = provider as? Provider.Ndt7 ?: error("сервер M-Lab не выбран")
            val url = (if (upload) p.up else p.down).removePrefix("ws://")
            val hostPort = url.substringBefore('/'); val path = "/" + url.substringAfter('/', "")
            val host = hostPort.substringBefore(':'); val port = hostPort.substringAfter(':', "80").toIntOrNull() ?: 80
            val socket = Http.connect(proxyPort, host, port, 15_000, sockets)
            socket.use {
                val out = socket.getOutputStream(); val input = BufferedInputStream(socket.getInputStream(), 65536)
                out.write(("GET $path HTTP/1.1\r\nHost: $host\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
                    "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n" +
                    "Sec-WebSocket-Protocol: net.measurementlab.ndt.v7\r\nUser-Agent: secret-tunnel-android\r\n\r\n").toByteArray())
                val status = Http.readLine(input)
                require(status.contains(" 101 ")) { "M-Lab отказал: ${status.trim()}" }
                while (Http.readLine(input).isNotEmpty()) Unit
                val counter = AtomicLong(); val finished = AtomicBoolean(false)
                val receiver = Thread {
                    runCatching {
                        while (true) {
                            val head = Http.readFully(input, 2)
                            val opcode = head[0].toInt() and 0x0f
                            var length = (head[1].toInt() and 0x7f).toLong()
                            if (length == 126L) length = Http.readFully(input, 2).let { ((it[0].toInt() and 255) shl 8 or (it[1].toInt() and 255)).toLong() }
                            else if (length == 127L) length = Http.readFully(input, 8).fold(0L) { acc, b -> (acc shl 8) or (b.toLong() and 255) }
                            if (head[1].toInt() and 0x80 != 0) Http.readFully(input, 4)
                            if (opcode == 8) break
                            if (opcode == 1 && length < 65536) {
                                val text = String(Http.readFully(input, length.toInt()))
                                runCatching { JSONObject(text).getJSONObject("TCPInfo").getDouble("MinRTT") / 1000.0 }.getOrNull()?.let {
                                    if (!control.stopped() && !sockets.isClosed()) {
                                        live.results.serverRttMs = it; live.pingMs = it
                                    }
                                }
                                continue
                            }
                            Http.skip(input, length)
                            if (!upload) counter.addAndGet(length)
                        }
                    }
                    finished.set(true)
                }.apply { isDaemon = true; start() }
                val sender = if (upload) Thread {
                    runCatching {
                        val payload = noise.copyOf(60_000)
                        val frame = java.io.ByteArrayOutputStream().apply {
                            write(0x82); write(0x80 or 126); write(payload.size shr 8); write(payload.size and 255)
                            write(byteArrayOf(0x5e, 0xc2.toByte(), 0x7a, 0x11)); write(payload)
                        }.toByteArray()
                        val start = System.currentTimeMillis()
                        while (System.currentTimeMillis() - start < 10_000 && !finished.get() && !control.stopped()) { out.write(frame); counter.addAndGet(payload.size.toLong()) }
                        out.write(byteArrayOf(0x88.toByte(), 0x80.toByte(), 0, 0, 0, 0))
                    }
                }.apply { isDaemon = true; start() } else null
                val start = System.currentTimeMillis(); val window = ArrayDeque<Pair<Long, Long>>(); var afterWarmup: Pair<Long, Long>? = null
                while (System.currentTimeMillis() - start < 13_000 && !control.stopped()) {
                    Thread.sleep(250)
                    val now = System.currentTimeMillis(); val bytes = counter.get()
                    window.addLast(now to bytes); while (window.isNotEmpty() && now - window.first().first > 1000) window.removeFirst()
                    val rate = window.firstOrNull()?.let { mbps(bytes - it.second, now - it.first) } ?: 0.0
                    if (afterWarmup == null && now - start >= WARMUP_MS) afterWarmup = now to bytes
                    if (upload) live.upMbps = rate else live.downMbps = rate
                    live.progress = ((now - start) / 10_000.0).coerceAtMost(1.0)
                    if (finished.get() || (sender != null && !sender.isAlive)) break
                }
                val bytes = counter.get(); val (from, atWarmup) = afterWarmup ?: (start to 0L)
                val rate = mbps(bytes - atWarmup, System.currentTimeMillis() - from)
                if (upload) { live.upMbps = 0.0; live.results.uploadMbps = rate; live.results.bytesUp += bytes }
                else { live.downMbps = 0.0; live.results.downloadMbps = rate; live.results.bytesDown += bytes }
                runCatching { socket.shutdownOutput() }
                sockets.close()
                receiver.join(1000)
                sender?.join(1000)
                require(bytes > 0) { if (upload) "ничего не отправилось" else "ничего не загрузилось" }
            }
        }
    }
}

/** Closing the raw sockets also releases threads blocked in TLS or SOCKS I/O. */
class SpeedSockets : Closeable {
    private val sockets = mutableSetOf<Closeable>()
    private var closed = false

    @Synchronized fun <T : Closeable> track(socket: T): T {
        if (closed) {
            runCatching { socket.close() }
            throw SocketException("Тест остановлен")
        }
        sockets.add(socket)
        return socket
    }

    @Synchronized fun forget(socket: Closeable) { sockets.remove(socket) }
    @Synchronized fun isClosed() = closed

    override fun close() {
        val pending = synchronized(this) {
            closed = true
            sockets.toList().also { sockets.clear() }
        }
        pending.forEach { runCatching { it.close() } }
    }
}

/**
 * A small HTTP/1.1 client over our own SOCKS5 connection, so names resolve on
 * the server's side and timings are exactly the round trips.
 */
object Http {
    fun readFully(input: InputStream, size: Int): ByteArray {
        val bytes = ByteArray(size); var read = 0
        while (read < size) { val n = input.read(bytes, read, size - read); if (n < 0) error("соединение закрыто"); read += n }
        return bytes
    }
    fun skip(input: InputStream, size: Long) {
        var left = size; val buffer = ByteArray(65536)
        while (left > 0) { val n = input.read(buffer, 0, minOf(left, buffer.size.toLong()).toInt()); if (n < 0) error("соединение закрыто"); left -= n }
    }
    fun readLine(input: InputStream): String {
        val line = StringBuilder()
        while (true) { val c = input.read(); if (c < 0) error("соединение закрыто"); if (c == '\n'.code) break; if (c != '\r'.code) line.append(c.toChar()) }
        return line.toString()
    }

    /** A TCP connection to host:port, through the tunnel's SOCKS5 proxy when there is one. */
    fun connect(proxyPort: Int?, host: String, port: Int, timeout: Int, sockets: SpeedSockets? = null): Socket {
        val socket = Socket()
        sockets?.track(socket)
        try {
            if (proxyPort == null) {
                socket.connect(InetSocketAddress(host, port), timeout)
                socket.soTimeout = 15_000
                return socket
            }
            socket.connect(InetSocketAddress("127.0.0.1", proxyPort), timeout); socket.soTimeout = 15_000
            val out = socket.getOutputStream(); val input = socket.getInputStream()
            out.write(byteArrayOf(5, 1, 0)); readFully(input, 2)
            val name = host.toByteArray()
            out.write(byteArrayOf(5, 1, 0, 3, name.size.toByte()) + name + byteArrayOf((port shr 8).toByte(), port.toByte()))
            val head = readFully(input, 4)
            require(head[1].toInt() == 0) { "туннель не соединился с $host (код ${head[1]})" }
            readFully(input, when (head[3].toInt()) { 1 -> 6; 4 -> 18; 3 -> (readFully(input, 1)[0].toInt() and 255) + 2; else -> error("неожиданный ответ SOCKS") })
            return socket
        } catch (e: Exception) {
            runCatching { socket.close() }
            sockets?.forget(socket)
            throw e
        }
    }

    fun fetchText(proxyPort: Int?, host: String, path: String, timeout: Int, sockets: SpeedSockets? = null): String =
        Link(proxyPort, host, timeout, sockets).use { link ->
            val response = link.get(path)
            require(response.status == 200) { "HTTP ${response.status}" }
            val out = java.io.ByteArrayOutputStream()
            response.body(capture = out) { out.size() < 4 shl 20 }
            out.toString("UTF-8")
        }

    class Response(private val input: InputStream, val status: Int, private val headers: Map<String, String>) : Closeable {
        val keepAlive get() = !headers["connection"].orEmpty().equals("close", true)
        fun header(name: String): String? = headers[name.lowercase()]
        private var done = false
        /** Reads the body, reporting each piece; `more` returning false stops early. */
        fun body(capture: OutputStream? = null, more: (Int) -> Boolean) {
            if (done) return
            done = true
            val buffer = ByteArray(65536)
            fun take(length: Long): Boolean {
                var left = length
                while (left > 0) {
                    val n = input.read(buffer, 0, minOf(left, buffer.size.toLong()).toInt())
                    if (n < 0) error("соединение закрыто")
                    capture?.write(buffer, 0, n); left -= n
                    if (!more(n)) return false
                }
                return true
            }
            when {
                headers["transfer-encoding"].orEmpty().contains("chunked", true) -> while (true) {
                    val size = readLine(input).substringBefore(';').trim().toLong(16)
                    if (size == 0L) { while (readLine(input).isNotEmpty()) Unit; break }
                    if (!take(size)) return
                    readLine(input)
                }
                headers["content-length"] != null -> take(headers["content-length"]!!.toLong())
                // Until the server closes; some reset instead of closing once they have sent it all.
                else -> { var got = false
                    while (true) {
                        val n = try { input.read(buffer) } catch (e: java.net.SocketException) { if (got) -1 else throw e }
                        if (n < 0) break; got = true; capture?.write(buffer, 0, n); if (!more(n)) return
                    } }
            }
        }
        fun drain() = body { true }
        override fun close() { done = true }
    }

    /** One keep-alive connection to an HTTPS host. */
    class Link(proxyPort: Int?, private val host: String, timeout: Int = 10_000, private val sockets: SpeedSockets? = null) : Closeable {
        private val raw = connect(proxyPort, host, 443, timeout, sockets)
        private val socket: SSLSocket
        private val input: InputStream
        private val output: OutputStream
        init {
            try {
                socket = (SSLSocketFactory.getDefault() as SSLSocketFactory).createSocket(raw, host, 443, true) as SSLSocket
                socket.soTimeout = 15_000
                socket.startHandshake()
                require(HttpsURLConnection.getDefaultHostnameVerifier().verify(host, socket.session)) { "сертификат не подходит для $host" }
                input = BufferedInputStream(socket.inputStream, 65536); output = socket.outputStream
            } catch (e: Exception) {
                runCatching { raw.close() }
                sockets?.forget(raw)
                throw e
            }
        }
        private fun head(method: String, path: String, close: Boolean, extra: String) =
            "$method $path HTTP/1.1\r\nHost: $host\r\nUser-Agent: secret-tunnel-android\r\nAccept: */*\r\n" +
                (if (close) "Connection: close\r\n" else "") + extra + "\r\n"
        private fun response(): Response {
            val status = readLine(input).split(' ').getOrNull(1)?.toIntOrNull() ?: error("неверный ответ сервера")
            val headers = mutableMapOf<String, String>()
            while (true) { val line = readLine(input); if (line.isEmpty()) break; headers[line.substringBefore(':').trim().lowercase()] = line.substringAfter(':').trim() }
            return Response(input, status, headers)
        }
        fun get(path: String, close: Boolean = false): Response {
            require(path.startsWith("/") && path.none { it <= ' ' || it == '\u007f' }) { "Некорректный HTTP-путь" }
            output.write(head("GET", path, close, "").toByteArray())
            output.flush()
            return response()
        }
        fun post(path: String, length: Long, write: (OutputStream, Long) -> Unit): Response {
            output.write(head("POST", path, false, "Content-Type: application/octet-stream\r\nContent-Length: $length\r\n").toByteArray())
            var sent = 0L
            while (sent < length) { val before = sent; write(output, sent); sent = before + minOf(noiseSize, length - before) }
            output.flush(); return response()
        }
        fun postChunked(path: String, write: (OutputStream) -> Unit): Response {
            output.write(head("POST", path, false, "Content-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\n").toByteArray())
            val chunked = object : OutputStream() {
                override fun write(b: Int) = write(byteArrayOf(b.toByte()))
                override fun write(b: ByteArray, off: Int, len: Int) {
                    if (len == 0) return
                    output.write((Integer.toHexString(len) + "\r\n").toByteArray()); output.write(b, off, len); output.write("\r\n".toByteArray())
                }
            }
            runCatching { write(chunked) }
            output.write("0\r\n\r\n".toByteArray()); output.flush()
            return response()
        }
        override fun close() {
            runCatching { socket.close() }
            sockets?.forget(raw)
        }
        private val noiseSize = 64L * 1024
    }
}
