package dev.secrettunnel.android

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.core.content.FileProvider
import java.io.File
import java.util.concurrent.Executors

internal object MobileUpdates {
    data class Status(
        val release: AppRelease? = null,
        val checking: Boolean = false,
        val downloading: Boolean = false,
        val received: Long = 0,
        val ready: Boolean = false,
        val error: String = "",
    )

    private val worker = Executors.newSingleThreadExecutor()
    private var checkedOnStartup = false
    @Volatile var status = Status()
        private set

    fun proxy(): Int? = TunnelVpnService.connectedSocksPort

    fun check(automatic: Boolean = false) {
        if (automatic && checkedOnStartup) return
        if (status.checking || status.downloading) return
        checkedOnStartup = true
        val port = proxy()
        val previous = status
        status = previous.copy(checking = true, error = "")
        worker.execute {
            status = try {
                val release = ReleaseUpdates.latest(port)
                if (previous.release == release) previous.copy(error = "")
                else Status(release = release)
            } catch (error: Exception) {
                previous.copy(error = error.message ?: "Не удалось проверить обновления")
            }
        }
    }

    fun download(context: Context) {
        if (status.checking || status.downloading) return
        val release = status.release ?: return
        val asset = release.apk ?: return
        val app = context.applicationContext
        val port = proxy()
        status = Status(release, downloading = true)
        worker.execute {
            val apk = apkFile(app)
            val temporary = File(apk.parentFile, "download.part.apk")
            status = try {
                require(compareReleaseVersions(release.version, installedVersion(app)) > 0) { "Новая версия не найдена" }
                check(apk.parentFile?.let { it.isDirectory || it.mkdirs() } == true) {
                    "Не удалось создать каталог для обновления"
                }
                apk.delete()
                ReleaseUpdates.download(release, asset, temporary, port) { received ->
                    status = status.copy(received = received)
                }
                stageVerifiedApk(app, temporary, release.version)
                Status(release, ready = true)
            } catch (error: Exception) {
                Status(release, error = error.message ?: "Не удалось скачать обновление")
            } finally {
                temporary.delete()
            }
        }
    }

    fun install(activity: Activity, requestCode: Int) {
        val release = status.release ?: return
        check(status.ready) { "Сначала скачайте обновление" }
        val apk = apkFile(activity)
        if (!apk.isFile) {
            status = Status(release, error = "Файл обновления удалён. Скачайте его заново.")
            return
        }
        if (!activity.packageManager.canRequestPackageInstalls()) {
            activity.startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                Uri.parse("package:${activity.packageName}")))
            return
        }
        openInstaller(activity, requestCode)
    }

    fun stageVerifiedApk(context: Context, source: File, version: String) {
        verifyApk(context, source, version)
        val destination = apkFile(context)
        check(destination.parentFile?.let { it.isDirectory || it.mkdirs() } == true &&
            source.renameTo(destination)) { "Не удалось сохранить обновление" }
    }

    fun openInstaller(activity: Activity, requestCode: Int) {
        val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.updates", apkFile(activity))
        @Suppress("DEPRECATION")
        activity.startActivityForResult(Intent(Intent.ACTION_INSTALL_PACKAGE, uri)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            .putExtra(Intent.EXTRA_RETURN_RESULT, true), requestCode)
    }

    private fun apkFile(context: Context) = File(context.cacheDir, "app-update/update.apk")

    @Suppress("DEPRECATION")
    fun versionName(context: Context): String =
        context.packageManager.getPackageInfo(context.packageName, 0).versionName.orEmpty()

    fun installedVersion(context: Context): String = versionName(context).substringBefore('-')

    @Suppress("DEPRECATION")
    private fun verifyApk(context: Context, apk: File, version: String) {
        val manager = context.packageManager
        val flag = if (Build.VERSION.SDK_INT >= 28) PackageManager.GET_SIGNING_CERTIFICATES else PackageManager.GET_SIGNATURES
        val current = manager.getPackageInfo(context.packageName, flag)
        val candidate = manager.getPackageArchiveInfo(apk.absolutePath, flag)
            ?: error("Android не распознал скачанный APK")
        require(candidate.packageName == context.packageName && candidate.versionName == version) {
            "Скачанный APK предназначен для другой сборки или версии Secret Tunnel"
        }
        fun versionCode(info: PackageInfo) =
            if (Build.VERSION.SDK_INT >= 28) info.longVersionCode else info.versionCode.toLong()
        require(versionCode(candidate) > versionCode(current)) { "Код скачанной версии не новее установленного" }
        fun signers(info: PackageInfo) = if (Build.VERSION.SDK_INT >= 28)
            info.signingInfo?.apkContentsSigners.orEmpty().toSet() else info.signatures.orEmpty().toSet()
        val currentSigners = signers(current)
        val candidateSigners = signers(candidate)
        require(currentSigners.isNotEmpty() && (currentSigners == candidateSigners ||
            (Build.VERSION.SDK_INT >= 28 && currentSigners.size == 1 && candidateSigners.size == 1 &&
                candidate.signingInfo?.signingCertificateHistory?.contains(currentSigners.single()) == true))) {
            "Подпись APK отличается от подписи установленного приложения"
        }
    }
}
