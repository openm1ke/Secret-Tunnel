package dev.secrettunnel.android

import android.util.Log
import org.json.JSONObject
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

internal object TransportDiagnostics {
    private val timestampPrefix = Regex("^\\[(\\d{13})] ")
    private val timeFormat = DateTimeFormatter.ofPattern("HH:mm:ss.SSS", Locale.ROOT)
        .withZone(ZoneId.systemDefault())

    fun timestamp(epochMs: Long): String =
        timeFormat.format(Instant.ofEpochMilli(epochMs))

    fun formatLog(line: String): String {
        val match = timestampPrefix.find(line) ?: return line
        return line.replaceRange(match.range, "[${timestamp(match.groupValues[1].toLong())}] ")
    }

    fun describe(metrics: JSONObject): String = buildString {
        append("Снимок ${timestamp(metrics.optLong("at_ms"))} · ↓ ${metrics.optLong("bytes_rx")} Б · ↑ ${metrics.optLong("bytes_tx")} Б\n")
        append("UDP: окно потока ${metrics.optLong("udp_flow_credit")} · бюджет байтов ${metrics.optLong("udp_byte_budget")} · слоты ${metrics.optLong("udp_slots")}\n")
        val attempts = metrics.optLong("udp_tx_attempts")
        val window = metrics.optLong("udp_window_bytes")
        if (window > 0) {
            append("Размер окна UDP: ${window / 1024} КиБ · ${metrics.optLong("udp_window_slots")} слотов\n")
        }
        if (attempts > 0 || metrics.optLong("udp_flow_credit") == 0L) {
            append("UDP: попыток отправки $attempts · принято ${metrics.optLong("udp_received")}\n")
            append("В окне UDP не хватило: байтов ${metrics.optLong("udp_credit_bytes")} · слотов ${metrics.optLong("udp_credit_slots")} · обоих ${metrics.optLong("udp_credit_both")}\n")
        }
        append("UDP: ошибки DNS ${metrics.optLong("udp_dns")} · отправка ${metrics.optLong("udp_send")}\n")
        for ((key, label) in listOf(
            "tx_queue" to "Ожидание отправки",
            "rx_hold" to "Приём до освобождения буфера",
            "write_batch" to "Запись в транспорт",
            "udp_credit_return" to "До следующего кредита UDP",
        )) {
            val delay = metrics.optJSONObject(key) ?: continue
            append("$label: макс. ${delay.optLong("max_ms")} мс · ≥100 мс: ${delay.optLong("over_100_ms")}\n")
        }
        append("TCP: ошибок ${metrics.optLong("tcp_errors")} · медленных открытий / этапов ${metrics.optLong("tcp_slow")}\n")
        append("TCP: отмен ${metrics.optLong("tcp_cancelled")} · проверок IPv6 ${metrics.optLong("tcp_probes")} · недоступно ${metrics.optLong("tcp_probe_errors")}\n")
        metrics.optJSONObject("last_tcp")?.let { event ->
            val stage = when (event.optString("stage")) {
                "open" -> "открытие потока"
                "dns" -> "DNS"
                "connect" -> "подключение к цели"
                "relay" -> "передача"
                else -> event.optString("stage")
            }
            val outcome = event.optString("outcome").let { if (it == "slow") "медленно" else it }
            append("${timestamp(event.optLong("at_ms"))} · поток #${event.optLong("flow")} · $stage · ${event.optLong("elapsed_ms")} мс · $outcome\n")
        }
    }

    class Sampler {
        private var nextSample = 0L
        private var client = ""
        private var server = ""

        fun sample(now: Long, transport: JSONObject) {
            if (now < nextSample) return
            nextSample = now + 5000
            client = emit("client", transport, client)
            transport.optJSONObject("server")?.let { server = emit("server", it, server) }
        }

        private fun emit(side: String, transport: JSONObject, previous: String): String {
            val metrics = transport.optJSONObject("diagnostics") ?: return ""
            val signature = JSONObject(metrics.toString()).apply { remove("at_ms") }.toString()
            if (signature != previous) {
                Log.i("SecretTunnelDiag", "$side send_q=${transport.optLong("send_queue_bytes")} receive_q=${transport.optLong("receive_queue_bytes")} $metrics")
            }
            return signature
        }
    }
}
