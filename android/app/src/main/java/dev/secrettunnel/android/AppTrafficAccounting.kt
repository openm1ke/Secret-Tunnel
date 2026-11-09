package dev.secrettunnel.android

import android.content.Context
import android.net.ConnectivityManager
import android.os.Build
import java.net.InetAddress
import java.net.InetSocketAddress
import java.nio.ByteBuffer
import java.nio.ByteOrder

internal class AppTrafficAccounting(context: Context, packages: Set<String>) {
    data class Entry(val uid: Int, val name: String, val down: Long, val up: Long, val rate: TrafficMeter.Rate)
    private data class FlowKey(val a: Long, val b: Long, val c: Long, val d: Long, val e: Long)
    private data class Owner(val uid: Int, val expires: Long)
    private data class Totals(var down: Long = 0, var up: Long = 0, var oldDown: Long = 0, var oldUp: Long = 0)

    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private val names = packages.mapNotNull { pkg ->
        runCatching {
            val app = context.packageManager.getApplicationInfo(pkg, 0)
            app.uid to app.loadLabel(context.packageManager).toString()
        }.getOrNull()
    }.groupBy({ it.first }, { it.second }).mapValues { (_, values) -> values.sorted().joinToString(" / ") }
    private val owners = object : LinkedHashMap<FlowKey, Owner>(128, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<FlowKey, Owner>?): Boolean = size > 4096
    }
    private val totals = mutableMapOf<Int, Totals>()
    private var lastRateAt = 0L
    @Volatile var entries: List<Entry> = emptyList()
        private set

    fun sample(now: Long, rows: ByteArray) {
        val data = ByteBuffer.wrap(rows).order(ByteOrder.nativeOrder())
        var lookupsLeft = 64
        while (data.remaining() >= ROW_SIZE) {
            val start = data.position()
            val protocol = data.get(start).toInt()
            val key = FlowKey(data.long, data.long, data.long, data.long, data.long)
            val up = data.long
            val down = data.long
            val syn = data.get().toInt() != 0
            data.position(start + ROW_SIZE)
            var uid = UNKNOWN
            if (protocol == 6 || protocol == 17) {
                if (syn) owners.remove(key)
                val cached = owners[key]
                uid = if (cached != null && cached.expires > now) {
                    cached.uid
                } else if (lookupsLeft > 0) {
                    lookupsLeft--
                    val found = owner(data, start, protocol)
                    val timeout = if (found == UNKNOWN) 250 else if (protocol == 17) 5_000 else 30_000
                    owners[key] = Owner(found, now + timeout)
                    found
                } else UNKNOWN
            }
            totals.getOrPut(uid) { Totals() }.apply {
                this.down += down
                this.up += up
            }
        }
        if (lastRateAt == 0L) lastRateAt = now - 250
        val elapsed = now - lastRateAt
        if (elapsed < 1000) return
        entries = totals.map { (uid, value) ->
            val rate = TrafficMeter.Rate(
                (value.down - value.oldDown) * 1000.0 / elapsed,
                (value.up - value.oldUp) * 1000.0 / elapsed,
            )
            value.oldDown = value.down
            value.oldUp = value.up
            Entry(uid, names[uid] ?: "Без привязки к приложению", value.down, value.up, rate)
        }.sortedWith(compareBy<Entry> { it.uid == UNKNOWN }.thenBy { it.name })
        lastRateAt = now
    }

    private fun owner(data: ByteBuffer, start: Int, protocol: Int): Int {
        if (Build.VERSION.SDK_INT < 29) return UNKNOWN
        val size = if (data.get(start + 1).toInt() == 4) 4 else 16
        fun address(offset: Int, portOffset: Int): InetSocketAddress {
            val raw = ByteArray(size) { data.get(start + offset + it) }
            val port = (data.get(start + portOffset).toInt() and 255) * 256 +
                (data.get(start + portOffset + 1).toInt() and 255)
            return InetSocketAddress(InetAddress.getByAddress(raw), port)
        }
        val uid = runCatching {
            connectivity.getConnectionOwnerUid(protocol, address(8, 4), address(24, 6))
        }.getOrDefault(UNKNOWN)
        return if (uid in names) uid else UNKNOWN
    }

    companion object {
        private const val ROW_SIZE = 64
        private const val UNKNOWN = -1
    }
}
