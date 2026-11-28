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
        return tr("UDP · ↓ {0} КиБ / {1} сл. · ↑ {2} КиБ / {3} сл.", down, client.optInt("udp_window_slots"), up, agent.optInt("slots"))
    }

    fun describe(metrics: JSONObject): String = buildString {
        append(tr("Снимок {0} · ↓ {1} Б · ↑ {2} Б\n", timestamp(metrics.optLong("at_ms")), metrics.optLong("bytes_rx"), metrics.optLong("bytes_tx")))
        if(metrics.optLong("process_id")>0)append(tr("Процесс #{0}\n", metrics.optLong("process_id")))
        metrics.optJSONObject("heartbeat")?.let { h ->
            fun value(key:String)=if(h.has(key)&&!h.isNull(key))tr("{0} мс", h.optLong(key)) else tr("н/д")
            append(tr("Весь туннель: RTT {0} · сглаженный {1} · джиттер {2}\n", value("last_rtt_ms"), value("smoothed_rtt_ms"), value("jitter_ms")))
            append(tr("Heartbeat: {0} / {1} · p95 {2} ({3} замеров) · тайм-аутов {4}\n", h.optLong("acknowledged"), h.optLong("probes"), value("p95_rtt_ms"), h.optInt("recent_samples"), h.optLong("timeouts")))
            if(!h.isNull("pending_ms"))append(tr("Ожидаем подтверждение {0} мс / {1} мс\n", h.optLong("pending_ms"), h.optLong("timeout_ms")))
        }
        append(tr("UDP: окно потока {0} · бюджет байтов {1} · слоты {2}\n", metrics.optLong("udp_flow_credit"), metrics.optLong("udp_byte_budget"), metrics.optLong("udp_slots")))
        val attempts = metrics.optLong("udp_tx_attempts")
        val window = metrics.optLong("udp_window_bytes")
        if (window > 0) {
            append(tr("Размер окна UDP: {0} КиБ · {1} слотов\n", window / 1024, metrics.optLong("udp_window_slots")))
        }
        if (attempts > 0 || metrics.optLong("udp_flow_credit") == 0L) {
            append(tr("UDP: попыток отправки {0} · принято {1}\n", attempts, metrics.optLong("udp_received")))
            append(tr("В окне UDP не хватило: байтов {0} · слотов {1} · обоих {2}\n", metrics.optLong("udp_credit_bytes"), metrics.optLong("udp_credit_slots"), metrics.optLong("udp_credit_both")))
        }
        append(tr("UDP: ошибки DNS {0} · отправка {1} · приём {2}\n", metrics.optLong("udp_dns"), metrics.optLong("udp_send"), metrics.optLong("udp_receive")))
        for ((key, label) in listOf(
            "tx_queue" to tr("Ожидание отправки"),
            "rx_hold" to tr("Приём до освобождения буфера"),
            "write_batch" to tr("Запись в транспорт"),
            "udp_credit_return" to tr("До следующего кредита UDP"),
            "udp_credit_stall" to tr("От нехватки окна до следующего кредита UDP"),
        )) {
            val delay = metrics.optJSONObject(key) ?: continue
            append(tr("{0}: макс. {1} мс · ≥100 мс: {2}\n", label, delay.optLong("max_ms"), delay.optLong("over_100_ms")))
        }
        append(tr("UDP: эпизодов нехватки окна {0}\n", metrics.optLong("udp_credit_stall_events")))
        if(metrics.has("short_udp_queue")){
            append(tr("Планировщик: коротких UDP {0} · остальных DATA {1}\n", metrics.optLong("short_udp_packets_written"), metrics.optLong("bulk_data_packets_written")))
            append(tr("Очередь до SSH: короткий UDP макс. {0} мс · остальные DATA {1} мс\n", metrics.optJSONObject("short_udp_queue")?.optLong("max_ms"), metrics.optJSONObject("bulk_data_queue")?.optLong("max_ms")))
            append(tr("Пик пакета записи: {0} Б\n", metrics.optLong("write_batch_peak_bytes")))
        }
        metrics.optJSONObject("tcp_flight")?.let { flight ->
            if(!flight.isNull("configured_bytes")){
                append(tr("Приоритет UDP: {0}\n", if(flight.optBoolean("interactive"))tr("активен") else tr("ожидание")))
                append(tr("TCP: учтено {0} Б · бюджет {1} Б · при UDP {2} Б", flight.optLong("held_bytes"), flight.optLong("effective_bytes"), flight.optLong("configured_bytes")))
                if (flight.has("path_delay_ms") && !flight.isNull("path_delay_ms")) append(tr(" · путь {0} мс", flight.optLong("path_delay_ms")))
                append("\n")
            }else append(tr("Приоритет UDP: без дополнительного бюджета TCP\n"))
        }
        metrics.optJSONObject("last_udp_credit_stall")?.let {
            append(tr("{0} · поток #{1} · следующий кредит через {2} мс · {3} Б / {4} сл.\n", timestamp(it.optLong("at_ms")), it.optLong("flow"), it.optLong("elapsed_ms"), it.optLong("credit_bytes"), it.optLong("credit_slots")))
        }
        append(tr("TCP: ошибок {0} · медленных открытий / этапов {1}\n", metrics.optLong("tcp_errors"), metrics.optLong("tcp_slow")))
        append(tr("TCP: отмен {0} · проверок IPv6 {1} · недоступно {2}\n", metrics.optLong("tcp_cancelled"), metrics.optLong("tcp_probes"), metrics.optLong("tcp_probe_errors")))
        metrics.optJSONObject("last_tcp")?.let { event ->
            val stage = when (event.optString("stage")) {
                "open" -> tr("открытие потока")
                "dns" -> "DNS"
                "connect" -> tr("подключение к цели")
                "relay" -> tr("передача")
                else -> event.optString("stage")
            }
            val outcome = event.optString("outcome").let { if (it == "slow") tr("медленно") else it }
            append(tr("{0} · поток #{1} · {2} · {3} мс · {4}\n", timestamp(event.optLong("at_ms")), event.optLong("flow"), stage, event.optLong("elapsed_ms"), outcome))
        }
    }

    fun describeSsh(ssh: JSONObject): String = buildString {
        val tuning = ssh.optJSONObject("tuning")
        val cipher = when(tuning?.optString("cipher")) {
            "aes128_gcm" -> "AES-128-GCM"
            "chacha20_poly1305" -> "ChaCha20-Poly1305"
            else -> tr("Авто")
        }
        append(tr("SSH: выбор шифра {0} · очередь {1} сообщений · окно {2} КиБ\n", cipher, tuning?.optInt("channel_buffer_messages"), (tuning?.optLong("channel_receive_window_bytes")?:0)/1024))
        val tcp = ssh.optJSONObject("outer_tcp") ?: return@buildString
        append(tr("TCP до первого сервера · {0}\n", timestamp(tcp.optLong("at_ms"))))
        if(!tcp.optBoolean("connected")) { append(tr("Соединение закрыто\n")); return@buildString }
        fun metric(key:String, scale:Double=1.0):String = if(tcp.has(key)&&!tcp.isNull(key))
            String.format(Locale.ROOT,"%.1f",tcp.optDouble(key)/scale) else tr("н/д")
        append(tr("RTT {0} мс · RTO {1} мс\n", metric("rtt_us",1000.0), metric("rto_us",1000.0)))
        append(tr("Не отправлено {0} Б · очередь сокета {1} Б\n", metric("not_sent_bytes"), metric("send_queue_bytes")))
        append(tr("Повторно отправлено {0} Б · сегментов {1}\n", metric("bytes_retransmitted"), metric("total_retransmits")))
        append(tr("Без подтверждения: {0} сегм. / {1} Б · источник {2}\n", metric("unacked_packets"), metric("bytes_in_flight"), tcp.optString("source")))
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
