package dev.secrettunnel.android

import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.OutputStream
import java.net.URI
import java.security.DigestOutputStream
import java.security.MessageDigest

internal const val PROJECT_URL = "https://github.com/openm1ke/Secret-Tunnel"
internal const val PROJECT_SITE = "https://openm1ke.github.io/Secret-Tunnel/"
private const val RELEASES_API = "https://api.github.com/repos/openm1ke/Secret-Tunnel/releases"
private const val MAX_METADATA = 2L * 1024 * 1024
private const val MAX_APK = 128L * 1024 * 1024
private const val MAX_AGENT = 32L * 1024 * 1024
private val versionPattern = Regex("""\d+\.\d+\.\d+""")
private val checksumLine = Regex("""^([a-fA-F0-9]{64})[ \t]+\*?(?:\./)?([^/\\]+)$""")

internal data class ReleaseAsset(val name: String, val url: String, val size: Long)

internal data class AppRelease(
    val version: String,
    val apk: ReleaseAsset?,
    val agent: ReleaseAsset?,
    val checksums: ReleaseAsset?,
    val notes: String,
    val publishedAt: String,
) {
    val page: String get() = "$PROJECT_URL/releases/tag/v$version"
}

internal fun releaseVersion(name: String): List<Long> {
    require(versionPattern.matches(name)) { "Некорректный номер версии" }
    return name.split('.').map { it.toLongOrNull() ?: error("Некорректный номер версии") }
}

internal fun compareReleaseVersions(first: String, second: String): Int {
    val a = releaseVersion(first)
    val b = releaseVersion(second)
    return a.indices.firstNotNullOfOrNull { index -> a[index].compareTo(b[index]).takeIf { it != 0 } } ?: 0
}

internal fun parseAppRelease(json: JSONObject, expected: String? = null): AppRelease {
    val tag = json.optString("tag_name")
    val version = tag.removePrefix("v")
    require(tag.startsWith("v") && !json.optBoolean("draft") && !json.optBoolean("prerelease") &&
        (expected == null || expected == version)) { "GitHub вернул неподходящий релиз" }
    releaseVersion(version)
    fun asset(name: String, limit: Long): ReleaseAsset? {
        val url = "$PROJECT_URL/releases/download/$tag/$name"
        val assets = json.optJSONArray("assets") ?: return null
        return (0 until assets.length()).asSequence().mapNotNull(assets::optJSONObject)
            .firstOrNull { it.optString("name") == name && it.optString("browser_download_url") == url }
            ?.optLong("size")?.takeIf { it in 1..limit }?.let { ReleaseAsset(name, url, it) }
    }
    return AppRelease(version, asset("SecretTunnel-$version-android.apk", MAX_APK),
        asset("SecretTunnel-$version-server-linux-x86_64.tar.gz", MAX_AGENT),
        asset("SHA256SUMS.txt", MAX_METADATA), json.optString("body").take(24_000),
        json.optString("published_at").take(10))
}

internal fun releaseChecksum(contents: String, name: String): String {
    val entries = contents.lineSequence().mapNotNull { checksumLine.matchEntire(it.trimEnd()) }
        .filter { it.groupValues[2] == name }.map { it.groupValues[1].lowercase() }.toList()
    require(entries.size == 1) { "В релизе отсутствует однозначная контрольная сумма для $name" }
    return entries.single()
}

/** Uses the same TLS/SOCKS implementation as the tunnel checks. Each retry writes a fresh file. */
internal object ReleaseUpdates {
    fun latest(proxy: Int?): AppRelease = release("latest", null, proxy)

    fun exact(version: String, proxy: Int?): AppRelease {
        releaseVersion(version)
        return release("tags/v$version", version, proxy)
    }

    private fun release(path: String, expected: String?, proxy: Int?): AppRelease =
        routes(proxy) { route -> parseAppRelease(JSONObject(text("$RELEASES_API/$path", route, MAX_METADATA)), expected) }

    fun download(release: AppRelease, asset: ReleaseAsset, file: File, proxy: Int?, progress: (Long) -> Unit = {}) {
        val sums = release.checksums ?: error("В релизе нет файла SHA256SUMS.txt")
        val expected = routes(proxy) { route -> releaseChecksum(text(sums.url, route, sums.size), asset.name) }
        try {
            routes(proxy) { route ->
                progress(0)
                val hash = MessageDigest.getInstance("SHA-256")
                val size = file.outputStream().buffered().use { stream ->
                    receive(asset.url, route, asset.size, DigestOutputStream(stream, hash), progress)
                }
                require(size == asset.size) { "Файл скачан не полностью" }
                require(hash.digest().joinToString("") { "%02x".format(it) } == expected) {
                    "Контрольная сумма не совпала. Скачайте обновление заново."
                }
            }
        } catch (error: Exception) {
            file.delete()
            throw error
        }
    }

    fun help(proxy: Int?): String = routes(proxy) { route ->
        text("https://raw.githubusercontent.com/openm1ke/Secret-Tunnel/main/docs/help/knowledge.md", route, MAX_METADATA)
    }

    private fun <T> routes(proxy: Int?, action: (Int?) -> T): T =
        try { action(null) } catch (error: Exception) {
            if (proxy == null) throw error
            action(proxy)
        }

    private fun text(url: String, route: Int?, limit: Long): String =
        ByteArrayOutputStream().use { out ->
            receive(url, route, limit, out) {}
            out.toString(Charsets.UTF_8.name())
        }

    private fun receive(url: String, route: Int?, limit: Long, output: OutputStream, progress: (Long) -> Unit): Long {
        var uri = URI(url)
        val deadline = System.nanoTime() + java.util.concurrent.TimeUnit.SECONDS.toNanos(120)
        repeat(5) {
            val host = uri.host?.lowercase() ?: error("Недопустимая ссылка GitHub")
            require(uri.scheme == "https" && uri.userInfo == null && uri.port == -1 &&
                (host in setOf("github.com", "api.github.com", "raw.githubusercontent.com") ||
                    host.endsWith(".githubusercontent.com"))) { "Недопустимая ссылка GitHub" }
            Http.Link(route, host).use { link ->
                val path = uri.rawPath.ifEmpty { "/" } + (uri.rawQuery?.let { "?$it" } ?: "")
                val response = link.get(path, close = true)
                if (response.status in listOf(301, 302, 303, 307, 308)) {
                    uri = uri.resolve(response.header("location") ?: error("GitHub не указал адрес перенаправления"))
                } else {
                    require(response.status == 200) {
                        if (response.status in listOf(403, 429)) "GitHub ограничил число запросов. Попробуйте позже."
                        else "GitHub ответил HTTP ${response.status}"
                    }
                    var total = 0L
                    response.body(object : OutputStream() {
                        override fun write(value: Int) = write(byteArrayOf(value.toByte()), 0, 1)
                        override fun write(bytes: ByteArray, offset: Int, length: Int) {
                            total += length
                            require(total <= limit) { "Файл больше заявленного размера" }
                            check(System.nanoTime() < deadline) { "Истекло время загрузки" }
                            output.write(bytes, offset, length)
                        }
                    }) { progress(total); true }
                    return total
                }
            }
        }
        error("Слишком много перенаправлений GitHub")
    }
}
