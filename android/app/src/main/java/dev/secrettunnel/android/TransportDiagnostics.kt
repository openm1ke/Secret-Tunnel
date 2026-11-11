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

    fun windows(transport: JSONObject): String? {
        val client = transport.optJSONObject("diagnostics") ?: return null
        val agent = transport.optJSONObject("peer_udp_window") ?: return null
        val down = client.optLong("udp_window_bytes") / 1024
        val up = agent.optLong("bytes") / 1024
        if (down <= 0 || up <= 0) return null
        return "UDP · ↓ $down КиБ / ${client.optInt("udp_window_slots")} сл. · ↑ $up КиБ / ${agent.optInt("slots")} сл."
    }

    fun describe(metrics: JSONObject): String = buildString {
        append("Снимок ${timestamp(metrics.optLong("at_ms"))} · ↓ ${metrics.optLong("bytes_rx")} Б · ↑ ${metrics.optLong("bytes_tx")} Б\n")
        if(metrics.optLong("process_id")>0)append("Процесс #${metrics.optLong("process_id")}\n")
        metrics.optJSONObject("heartbeat")?.let { h ->
            fun value(key:String)=if(h.has(key)&&!h.isNull(key))"${h.optLong(key)} мс" else "н/д"
            append("Весь туннель: RTT ${value("last_rtt_ms")} · сглаженный ${value("smoothed_rtt_ms")} · джиттер ${value("jitter_ms")}\n")
            append("Heartbeat: ${h.optLong("acknowledged")} / ${h.optLong("probes")} · p95 ${value("p95_rtt_ms")} (${h.optInt("recent_samples")} замеров) · тайм-аутов ${h.optLong("timeouts")}\n")
            if(!h.isNull("pending_ms"))append("Ожидаем подтверждение ${h.optLong("pending_ms")} мс / ${h.optLong("timeout_ms")} мс\n")
        }
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
            "udp_credit_stall" to "От нехватки окна до следующего кредита UDP",
        )) {
            val delay = metrics.optJSONObject(key) ?: continue
            append("$label: макс. ${delay.optLong("max_ms")} мс · ≥100 мс: ${delay.optLong("over_100_ms")}\n")
        }
        append("UDP: эпизодов нехватки окна ${metrics.optLong("udp_credit_stall_events")}\n")
        if(metrics.has("short_udp_queue")){
            append("Планировщик: коротких UDP ${metrics.optLong("short_udp_packets_written")} · остальных DATA ${metrics.optLong("bulk_data_packets_written")}\n")
            append("Очередь до SSH: короткий UDP макс. ${metrics.optJSONObject("short_udp_queue")?.optLong("max_ms")} мс · остальные DATA ${metrics.optJSONObject("bulk_data_queue")?.optLong("max_ms")} мс\n")
            append("Пик пакета записи: ${metrics.optLong("write_batch_peak_bytes")} Б\n")
        }
        metrics.optJSONObject("tcp_flight")?.let { flight ->
            if(!flight.isNull("configured_bytes")){
                append("Приоритет UDP: ${if(flight.optBoolean("interactive"))"активен" else "ожидание"}\n")
                append("TCP: учтено ${flight.optLong("held_bytes")} Б · бюджет ${flight.optLong("effective_bytes")} Б · при UDP ${flight.optLong("configured_bytes")} Б")
                if (flight.has("path_delay_ms") && !flight.isNull("path_delay_ms")) append(" · путь ${flight.optLong("path_delay_ms")} мс")
                append("\n")
            }else append("Приоритет UDP: без дополнительного бюджета TCP\n")
        }
        metrics.optJSONObject("last_udp_credit_stall")?.let {
            append("${timestamp(it.optLong("at_ms"))} · поток #${it.optLong("flow")} · следующий кредит через ${it.optLong("elapsed_ms")} мс · ${it.optLong("credit_bytes")} Б / ${it.optLong("credit_slots")} сл.\n")
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

    fun describeSsh(ssh: JSONObject): String = buildString {
        val tuning = ssh.optJSONObject("tuning")
        val cipher = when(tuning?.optString("cipher")) {
            "aes128_gcm" -> "AES-128-GCM"
            "chacha20_poly1305" -> "ChaCha20-Poly1305"
            else -> "Авто"
        }
        append("SSH: выбор шифра $cipher · очередь ${tuning?.optInt("channel_buffer_messages")} сообщений · окно ${(tuning?.optLong("channel_receive_window_bytes")?:0)/1024} КиБ\n")
        val tcp = ssh.optJSONObject("outer_tcp") ?: return@buildString
        append("TCP до первого сервера · ${timestamp(tcp.optLong("at_ms"))}\n")
        if(!tcp.optBoolean("connected")) { append("Соединение закрыто\n"); return@buildString }
        fun metric(key:String, scale:Double=1.0):String = if(tcp.has(key)&&!tcp.isNull(key))
            String.format(Locale.ROOT,"%.1f",tcp.optDouble(key)/scale) else "н/д"
        append("RTT ${metric("rtt_us",1000.0)} мс · RTO ${metric("rto_us",1000.0)} мс\n")
        append("Не отправлено ${metric("not_sent_bytes")} Б · очередь сокета ${metric("send_queue_bytes")} Б\n")
        append("Повторно отправлено ${metric("bytes_retransmitted")} Б · сегментов ${metric("total_retransmits")}\n")
        append("Без подтверждения: ${metric("unacked_packets")} сегм. / ${metric("bytes_in_flight")} Б · источник ${tcp.optString("source")}\n")
    }

    class Sampler {
        private var nextSample = 0L
        private var client = ""
        private var server = ""
        private var ssh = ""

        fun sample(now: Long, transport: JSONObject) {
            if (now < nextSample) return
            nextSample = now + 5000
            client = emit("client", transport, client)
            transport.optJSONObject("server")?.let { server = emit("server", it, server) }
            transport.optJSONObject("ssh")?.let { value ->
                val signature = JSONObject(value.toString()).apply { optJSONObject("outer_tcp")?.remove("at_ms") }.toString()
                if(signature!=ssh) Log.i("SecretTunnelTcp",value.toString())
                ssh=signature
            }
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
