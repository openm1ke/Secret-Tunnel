package dev.secrettunnel.android

import android.app.Activity
import android.content.Intent
import android.net.ConnectivityManager
import android.net.Network
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.View
import android.widget.*
import androidx.core.view.WindowCompat
import org.json.JSONArray
import org.json.JSONObject
import java.util.Locale

/** Public offers are deliberately stored outside SettingsStore and never become SSH profiles. */
class HostingCatalogActivity : LocalizedActivity() {
    private lateinit var palette: Palette
    private lateinit var body: LinearLayout
    private val main = Handler(Looper.getMainLooper())
    private var offers = emptyList<JSONObject>()
    private var results = emptyMap<String, JSONObject>()
    private var region = tr("Все")
    private val expanded = mutableSetOf<String>()
    private var explanation = false
    private var catalogScroll: ScrollView? = null
    private var sorted = false
    private var busy = ""
    private var message = ""
    private var measuredAt = ""
    private var generation = 0
    private var job = ""
    private val probeButtonID = View.generateViewId()
    private var network: Network? = null
    private val connectivity by lazy { getSystemService(ConnectivityManager::class.java) }
    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(next: Network) { main.post {
            if (network != next) { network = next; networkChanged() }
        } }
        override fun onLost(lost: Network) { main.post { if (network == lost) { network = null; networkChanged() } } }
    }
    private fun dp(value: Int) = (value * resources.displayMetrics.density).toInt()
    override fun onCreate(state: Bundle?) {
        val dark = SettingsStore(this).appearance.isDark(resources.configuration)
        setTheme(if (dark) R.style.Theme_SecretTunnel_Dark else R.style.Theme_SecretTunnel_Light)
        super.onCreate(state)
        palette = if (dark) Palette.dark else Palette.light
        window.statusBarColor = palette.background; window.navigationBarColor = palette.background
        WindowCompat.getInsetsController(window, window.decorView).apply {
            isAppearanceLightStatusBars = !dark; isAppearanceLightNavigationBars = !dark
        }
        network = connectivity.activeNetwork
        connectivity.registerDefaultNetworkCallback(callback)
        render(); run("load")
    }
    override fun onPause() { if (busy == "probe") cancel(tr("Приложение свёрнуто. Повторите замер.")); super.onPause() }
    override fun onDestroy() {
        cancel(""); runCatching { connectivity.unregisterNetworkCallback(callback) }; super.onDestroy()
    }
    private fun networkChanged() {
        cancel(tr("Сеть изменилась. Повторите замер.")); results = emptyMap(); measuredAt = ""; render()
    }
    private fun cancel(reason: String) {
        generation++
        if (busy == "probe") message = reason
        busy = ""
        runCatching { NativeBridge.hosting(JSONObject().put("action", "cancel").put("job", job).toString(), cacheDir.path, false) }
        if (!isDestroyed && !isFinishing && ::body.isInitialized) render()
    }
    private fun visible() = offers.filter { region == tr("Все") || it.getString("country") == region }
        .let { rows -> if (sorted) rows.sortedBy { results[it.getString("id")]?.let { r -> if (r.isNull("median_ms")) Double.POSITIVE_INFINITY else r.getDouble("median_ms") } ?: Double.POSITIVE_INFINITY } else rows }
    private fun run(action: String) {
        if (busy.isNotEmpty()) return
        val ids = JSONArray(visible().map { it.getString("id") })
        val token = ++generation
        if (action == "probe") { job = java.util.UUID.randomUUID().toString(); results = emptyMap(); measuredAt = "" }
        val currentJob = job
        busy = action; message = ""; render()
        val cache = cacheDir.resolve("hosting").path
        val active = TunnelVpnService.active
        Thread {
            val response = try {
                NativeBridge.requireAvailable()
                JSONObject(NativeBridge.hosting(JSONObject().put("action", action).put("ids", ids).put("job", currentJob).toString(), cache, active))
            } catch (e: Exception) { JSONObject().put("error", e.message ?: tr("Не удалось загрузить каталог")) }
            main.post {
                if (token != generation || isDestroyed || isFinishing) return@post
                response.optJSONObject("catalog")?.getJSONArray("offers")?.let { rows -> offers = (0 until rows.length()).map { rows.getJSONObject(it) }; results = emptyMap(); measuredAt = "" }
                response.optJSONArray("results")?.let { rows ->
                    results = (0 until rows.length()).map { rows.getJSONObject(it) }.associateBy { it.getString("id") }
                    measuredAt = java.text.DateFormat.getTimeInstance(java.text.DateFormat.SHORT).format(java.util.Date())
                }
                message = when {
                    response.has("error") -> response.getString("error")
                    response.has("warning") -> response.getString("warning")
                    response.optString("source") == "bundled" -> tr("Встроенная копия. Дата проверки указана у каждого тарифа.")
                    response.optString("source") == "cache" -> tr("Сохранённая копия каталога.")
                    response.optString("source") == "network" -> tr("Каталог обновлён из сети.")
                    else -> ""
                }
                busy = ""; render()
                if (action == "load") run("refresh")
            }
        }.start()
    }
    private fun text(parent: LinearLayout, value: String, size: Int = 14, bold: Boolean = false, color: Int = palette.text) {
        parent.addView(TextView(this).apply {
            text = tr(value); textSize = size.toFloat(); setTextColor(color)
            if (bold) setTypeface(typeface, android.graphics.Typeface.BOLD)
            setPadding(0, dp(6), 0, dp(6))
        })
    }
    private fun button(parent: LinearLayout, value: String, primary: Boolean = false, enabled: Boolean = true, action: () -> Unit): Button {
        val control = Button(this).apply {
            text = value; isAllCaps = false; textSize = 14f; minHeight = dp(48)
            stateListAnimator = null; elevation = 0f
            setTextColor(if (primary) palette.onAccent else palette.accent)
            background = palette.panel(if (primary) palette.accent else palette.raised, dp(14).toFloat())
            isEnabled = enabled; alpha = if (enabled) 1f else .45f
            setOnFocusChangeListener { _, focused ->
                background = palette.panel(if (primary) palette.accent else palette.raised, dp(14).toFloat()).apply {
                    if (focused) setStroke(dp(3), palette.text)
                }
            }
            setOnClickListener { action() }
        }
        parent.addView(control, LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(8) })
        return control
    }
    private fun link(parent: LinearLayout, title: String, url: String) = button(parent, title) {
        runCatching { startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url))) }
            .onFailure { Toast.makeText(this, tr("Не удалось открыть браузер"), Toast.LENGTH_SHORT).show() }
    }
    private fun flag(code: String): String = if (code.length == 2 && code.all { it in 'A'..'Z' }) code.map { String(Character.toChars(0x1f1e6 + it.code - 'A'.code)) }.joinToString("") else "🌐"
    private fun country(code: String): String = Locale("", code).getDisplayCountry(AppLocalization.language.locale).ifBlank { code }
    private fun column() = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
    private fun line() = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL; gravity = android.view.Gravity.CENTER_VERTICAL }
    private fun note(o: JSONObject) = if (o.isNull("price_note")) tr("Условия цены — у хостера") else o.getString("price_note")
    private fun purchase() {
        if (intent.getBooleanExtra("from_wizard", false)) { setResult(RESULT_OK); finish() }
        else startActivity(Intent(this, SetupActivity::class.java).putExtra("server_purchased", true))
    }
    private fun render() {
        val focusedID = currentFocus?.id
        val oldOffset = catalogScroll?.scrollY ?: 0
        val root = column().apply { setBackgroundColor(palette.background) }
        val top = line().apply { setPadding(dp(20), dp(4), dp(20), dp(4)) }
        button(top, "‹") { finish() }.apply {
            contentDescription = tr("Назад"); textSize = 26f
            layoutParams = LinearLayout.LayoutParams(dp(48), dp(48))
            background = palette.panel(palette.background)
        }
        text(top, "Secret Tunnel", 14, color = palette.muted)
        root.addView(top)
        val scroll = ScrollView(this).apply { isFillViewport = true }
        catalogScroll = scroll
        body = column().apply { setPadding(dp(20), dp(16), dp(20), dp(24)) }
        scroll.addView(body); root.addView(scroll, LinearLayout.LayoutParams(-1, 0, 1f)); setContentView(root)
        text(body, tr("ВЫБОР СВОЕГО VPS"), 12, true, palette.accent)
        text(body, tr("Каталог серверов"), 28, true)
        text(body, tr("Сравните локации и задержку. Выберите хостера и настройте SSH после покупки."), 14, color = palette.muted)
        text(body, tr("Провайдеры: {0} · страны: {1}", offers.size, offers.map { it.getString("country") }.distinct().size), 12, color = palette.muted)
        val filters = line()
        val filterScroll = HorizontalScrollView(this).apply { isHorizontalScrollBarEnabled = false; addView(filters) }
        body.addView(filterScroll, LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(12); bottomMargin = dp(8) })
        val codes = listOf(tr("Все")) + offers.map { it.getString("country") }.distinct().sorted()
        for (code in codes) {
            val control = button(filters, if (code == tr("Все")) tr("Все страны") else "${flag(code)} ${country(code)}", code == region, busy != "probe") {
                region = code; catalogScroll?.scrollTo(0, 0); render()
            }
            control.layoutParams = LinearLayout.LayoutParams(-2, dp(48)).apply { marginEnd = dp(8) }
            control.setPadding(dp(16), 0, dp(16), 0)
            control.isSelected = code == region
        }
        val actions = line()
        val probe = button(actions, if (busy == "probe") tr("Отменить замер") else tr("Проверить задержку"), true, offers.isNotEmpty() && (busy.isEmpty() || busy == "probe")) {
            if (busy == "probe") cancel(tr("Замер отменён.")) else run("probe")
        }
        probe.layoutParams = LinearLayout.LayoutParams(0, dp(48), 1f).apply { marginEnd = dp(8) }; probe.id = probeButtonID
        button(actions, "↻", enabled = busy.isEmpty()) { run("refresh") }.apply {
            contentDescription = tr("Обновить каталог"); layoutParams = LinearLayout.LayoutParams(dp(48), dp(48)); textSize = 22f
        }
        body.addView(actions)
        if (busy.isNotEmpty()) text(body, if (busy == "probe") tr("Измеряем задержку…") else tr("Обновляем данные…"), 12, color = palette.muted)
        else if (message.isNotEmpty()) text(body, message, 12, color = palette.muted)
        if (results.isNotEmpty()) {
            body.addView(Switch(this).apply {
                text = tr("Сначала с меньшей задержкой"); textSize = 14f; setTextColor(palette.text); isChecked = sorted; minHeight = dp(48)
                setOnCheckedChangeListener { _, value -> sorted = value; render() }
            }, LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(16) })
            text(body, tr("Замер в {0} · для другой сети повторите проверку.", measuredAt), 12, color = palette.muted)
        }
        val rows = visible()
        val wide = resources.configuration.screenWidthDp >= 720 && resources.configuration.fontScale < 1.3f
        for (chunk in rows.chunked(if (wide) 2 else 1)) {
            val row = line().apply { gravity = android.view.Gravity.TOP }
            for ((index, offer) in chunk.withIndex()) row.addView(offerCard(offer), LinearLayout.LayoutParams(0, -2, 1f).apply { if (index > 0) marginStart = dp(16) })
            if (wide && chunk.size == 1) row.addView(Space(this), LinearLayout.LayoutParams(0, 0, 1f).apply { marginStart = dp(16) })
            body.addView(row, LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(16) })
        }
        button(body, if (explanation) tr("Как сравнивать серверы  ⌃") else tr("Как сравнивать серверы  ⌄")) { explanation = !explanation; render() }
        if (explanation) text(body, tr("Флаг — страна сервера. Задержка — медиана пяти TCP-подключений из текущей сети, не ICMP-пинг. Другой VPN влияет на результат. Скорость порта заявлена хостером и не гарантирует скорость SSH. Тестовый узел и будущий VPS могут различаться. Цены в исходной валюте; итоговая сумма — у хостера."), 13, color = palette.muted)
        button(body, tr("Сервер уже куплен — настроить SSH")) { purchase() }
        scroll.post { scroll.scrollTo(0, oldOffset) }
        if (focusedID == probeButtonID) probe.requestFocus()
    }
    private fun offerCard(o: JSONObject): LinearLayout {
        val id = o.getString("id")
        val card = column().apply { background = palette.panel(radius = dp(24).toFloat(), stroke = palette.border); setPadding(dp(24), dp(20), dp(24), dp(16)) }
        val identity = line()
        identity.addView(TextView(this).apply { text = flag(o.getString("country")); textSize = 32f; importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO }, LinearLayout.LayoutParams(dp(48), -2).apply { marginEnd = dp(12) })
        val title = column(); text(title, o.getString("provider"), 20, true); text(title, "${country(o.getString("country"))} · ${o.getString("city")}", 12, color = palette.muted)
        identity.addView(title, LinearLayout.LayoutParams(0, -2, 1f)); card.addView(identity)
        val r = results[id]
        val numeric = java.text.NumberFormat.getNumberInstance(AppLocalization.language.locale).apply { maximumFractionDigits = 2 }.format(o.getLong("price_minor") / 100.0)
        val symbol = when (o.getString("currency")) { "RUB" -> "₽"; "EUR" -> "€"; "USD" -> "$"; else -> o.getString("currency") }
        val metrics = line().apply { gravity = android.view.Gravity.TOP }
        val money = column(); text(money, "$numeric $symbol", 30, true); text(money, tr("в месяц"), 12, color = palette.muted)
        val latency = column()
        val measured = r != null && !r.isNull("median_ms")
        val hasTestNode = !o.isNull("test_host")
        val latencyValue = if (!hasTestNode) tr("Нет узла") else if (measured) tr("{0} мс", r!!.getDouble("median_ms").toInt()) else if (r == null) "—" else tr("Нет замера")
        text(latency, latencyValue, if (r != null && !measured) 20 else 30, true, if (r != null && !measured) palette.muted else palette.accent)
        text(latency, if (hasTestNode) tr("задержка TCP") else tr("замер недоступен"), 12, color = palette.muted)
        metrics.addView(money, LinearLayout.LayoutParams(0, -2, 1f)); metrics.addView(latency, LinearLayout.LayoutParams(0, -2, 1f))
        card.addView(metrics, LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(16) })
        text(card, note(o), 12, color = palette.muted)
        val port = line().apply { setPadding(dp(12), dp(4), dp(12), dp(4)); background = palette.panel(palette.raised, dp(12).toFloat()) }
        val left = TextView(this).apply { text = tr("Порт"); textSize = 13f; setTextColor(palette.muted) }; port.addView(left, LinearLayout.LayoutParams(0, -2, 1f))
        val speed = if (o.isNull("port_mbps")) tr("Не указана") else o.getInt("port_mbps").let { if (it >= 1000 && it % 1000 == 0) tr("до {0} Гбит/с", it/1000) else tr("до {0} Мбит/с", it) }
        text(port, speed, 15, true, palette.accent)
        card.addView(port, LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(12); bottomMargin = dp(8) })
        val ram = o.getInt("ram_mb").let { if (it % 1024 == 0) tr("{0} ГБ", it/1024) else tr("{0} МБ", it) }
        text(card, tr("{0} vCPU · {1} RAM · {2} ГБ", o.getInt("cpu"), ram, o.getInt("disk_gb")), 13, color = palette.muted)
        link(card, tr("Сайт хостера  ↗"), o.getString("order_url"))
        button(card, if (id in expanded) tr("Подробнее  ⌃") else tr("Подробнее  ⌄")) {
            if (!expanded.add(id)) expanded.remove(id); render()
        }.apply { background = palette.panel(palette.surface, dp(12).toFloat()); contentDescription = tr("Подробнее о {0}", o.getString("provider")) }
        if (id in expanded) {
            text(card, o.getString("plan"), 14, true)
            text(card, o.getString("conditions"), 13, color = palette.muted)
            if (r != null) text(card, if (measured) String.format(Locale.getDefault(), tr("TCP · %.0f–%.0f мс · %d/%d ответов · %s"), r.getDouble("min_ms"), r.getDouble("max_ms"), r.getInt("successes"), r.getInt("attempts"), r.getString("family")) else r.optString("error", tr("Нет ответа TCP")), 13, color = palette.muted)
            text(card, o.getString("test_note"), 12, color = palette.muted)
            text(card, tr("Проверено: {0}", o.getString("verified_at").take(10)), 12, color = palette.muted)
            if (o.optString("company_country") != tr("Не уточнено")) text(card, tr("Страна компании: {0}", tr(o.optString("company_country"))), 12, color = palette.muted)
            link(card, tr("Источник цены  ↗"), o.getString("source_url")); link(card, tr("Тестовый узел  ↗"), o.getString("test_source_url"))
        }
        if (!o.isNull("update_error")) text(card, tr("Цена требует обновления"), 12, color = palette.warning)
        return card
    }
}
