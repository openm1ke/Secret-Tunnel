package dev.secrettunnel.android

import android.app.ActivityManager
import android.app.ApplicationExitInfo
import android.content.Context
import android.os.Build
import android.os.Process
import android.util.Log
import androidx.annotation.RequiresApi

internal object ConnectionAttempt {
    private val processStartedAt = System.currentTimeMillis()

    private fun preferences(context: Context) =
        context.getSharedPreferences("connection-attempt", Context.MODE_PRIVATE)

    fun mark(context: Context, stage: String) {
        Log.i("SecretTunnelConnection", stage)
        // Synchronous persistence keeps the preceding stage if native code terminates the process.
        val prefs = preferences(context)
        if (prefs.getInt("pid", 0) == Process.myPid() && prefs.getString("stage", null) == stage) return
        prefs.edit().putInt("pid", Process.myPid()).putLong("time", System.currentTimeMillis())
            .putString("stage", stage).commit()
    }

    fun clear(context: Context) {
        val prefs = preferences(context)
        if (prefs.getInt("pid", 0) == Process.myPid()) prefs.edit().clear().apply()
    }

    fun previousFailure(context: Context): String? {
        val prefs = preferences(context)
        val pid = prefs.getInt("pid", 0)
        val time = prefs.getLong("time", 0)
        val stage = prefs.getString("stage", null)?.takeIf { pid != Process.myPid() }
        val failure = if (Build.VERSION.SDK_INT >= 30) systemExit(context, pid, time, stage) else null
        if (stage == null && failure == null) return null
        val abi = if (Process.is64Bit()) Build.SUPPORTED_64_BIT_ABIS else Build.SUPPORTED_32_BIT_ABIS
        return listOfNotNull(
            stage?.let { "Последний этап · $it" },
            failure ?: "Android не сохранил причину завершения процесса.",
            "Android ${Build.VERSION.RELEASE} · ${abi.joinToString()}",
        ).joinToString("\n")
    }

    @RequiresApi(30)
    private fun systemExit(context: Context, pid: Int, time: Long, stage: String?): String? = try {
        val manager = context.getSystemService(ActivityManager::class.java)
        val exits = manager.getHistoricalProcessExitReasons(context.packageName, if (stage == null) 0 else pid, 8)
        val exit = exits.firstOrNull { info ->
            info.processName == context.packageName && info.timestamp < processStartedAt &&
                if (stage != null) info.timestamp >= time else {
                    processStartedAt - info.timestamp <= 3 * 60 * 60 * 1000L &&
                        info.reason in setOf(ApplicationExitInfo.REASON_CRASH_NATIVE, ApplicationExitInfo.REASON_CRASH,
                            ApplicationExitInfo.REASON_ANR, ApplicationExitInfo.REASON_LOW_MEMORY)
                }
        }
        exit?.let {
            val reason = when (it.reason) {
                ApplicationExitInfo.REASON_CRASH_NATIVE -> "Сбой в нативной библиотеке"
                ApplicationExitInfo.REASON_CRASH -> "Ошибка в коде Android"
                ApplicationExitInfo.REASON_ANR -> "Приложение не ответило Android"
                ApplicationExitInfo.REASON_LOW_MEMORY -> "Android завершил процесс из-за нехватки памяти"
                ApplicationExitInfo.REASON_SIGNALED -> "Процесс завершён сигналом ${it.status}"
                ApplicationExitInfo.REASON_USER_REQUESTED -> "Android сообщает об остановке приложения"
                else -> "Причина завершения Android · ${it.reason}"
            }
            listOfNotNull(reason, it.description?.takeIf(String::isNotBlank)?.take(400)).joinToString("\n")
        }
    } catch (error: Exception) {
        Log.w("SecretTunnelConnection", "История завершений Android недоступна", error)
        null
    }
}
