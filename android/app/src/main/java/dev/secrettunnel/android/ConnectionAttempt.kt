package dev.secrettunnel.android

import android.app.ActivityManager
import android.app.ApplicationExitInfo
import android.content.Context
import android.os.Build
import android.os.Process
import android.os.SystemClock
import android.util.Log
import androidx.annotation.RequiresApi
import java.text.DateFormat
import java.util.Date

internal data class ConnectionAttemptMarker(val pid: Int, val time: Long, val stage: String) {
    fun stageForExit(exitPid: Int, exitTime: Long): String? =
        stage.takeIf { pid == exitPid && time <= exitTime }
}

internal data class PreviousConnectionFailure(
    val time: Long,
    val message: String,
    val interruptedStart: Boolean,
) {
    fun consumeStartupWarning(tunnelActive: Boolean, lastHandled: Long, markHandled: (Long) -> Unit): String? {
        if (!interruptedStart || time <= lastHandled) return null
        markHandled(time)
        return message.takeUnless { tunnelActive }
    }
}

internal object ConnectionAttempt {
    private const val HISTORY_AGE_MS = 3 * 60 * 60 * 1000L
    private const val HANDLED_WARNING = "handled_warning_time"
    private val processStartedAt = System.currentTimeMillis() -
        (SystemClock.elapsedRealtime() - Process.getStartElapsedRealtime())

    private fun preferences(context: Context) =
        context.getSharedPreferences("connection-attempt", Context.MODE_PRIVATE)

    fun mark(context: Context, stage: String) {
        Log.i("SecretTunnelConnection", stage)
        // Synchronous persistence keeps the preceding stage if native code terminates the process.
        val prefs = preferences(context)
        if (prefs.getInt("pid", 0) == Process.myPid() && prefs.getLong("time", 0) >= processStartedAt &&
            prefs.getString("stage", null) == stage) return
        prefs.edit().putInt("pid", Process.myPid()).putLong("time", System.currentTimeMillis())
            .putString("stage", stage).commit()
    }

    fun clear(context: Context) {
        val prefs = preferences(context)
        if (prefs.getInt("pid", 0) == Process.myPid() && prefs.getLong("time", 0) >= processStartedAt) {
            prefs.edit().remove("pid").remove("time").remove("stage").apply()
        }
    }

    fun previousFailure(context: Context): PreviousConnectionFailure? {
        val oldest = System.currentTimeMillis() - HISTORY_AGE_MS
        val saved = preferences(context).all
        val pid = saved["pid"] as? Int ?: 0
        val time = saved["time"] as? Long ?: 0L
        val marker = (saved["stage"] as? String)
            ?.takeIf { pid > 0 && time in oldest until processStartedAt }
            ?.let { ConnectionAttemptMarker(pid, time, it) }
        if (Build.VERSION.SDK_INT >= 30) {
            systemExit(context, oldest, marker)?.let { return it }
        }
        return marker?.let {
            report(it.time, it.stage, tr("Android не подтвердил причину завершения."), interruptedStart = false)
        }
    }

    fun consumeStartupWarning(context: Context, failure: PreviousConnectionFailure, tunnelActive: Boolean): String? {
        val prefs = preferences(context)
        return failure.consumeStartupWarning(tunnelActive, prefs.getLong(HANDLED_WARNING, 0)) {
            prefs.edit().putLong(HANDLED_WARNING, it).apply()
        }
    }

    private fun report(time: Long, stage: String?, reason: String, interruptedStart: Boolean): PreviousConnectionFailure {
        val date = DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.MEDIUM).format(Date(time))
        val abi = if (Process.is64Bit()) Build.SUPPORTED_64_BIT_ABIS else Build.SUPPORTED_32_BIT_ABIS
        val message = listOfNotNull(
            date,
            stage?.let { tr("Последний этап · {0}", it) },
            reason,
            "Android ${Build.VERSION.RELEASE} · ${abi.joinToString()}",
        ).joinToString("\n")
        return PreviousConnectionFailure(time, message, interruptedStart)
    }

    @RequiresApi(30)
    private fun systemExit(context: Context, oldest: Long, marker: ConnectionAttemptMarker?): PreviousConnectionFailure? = try {
        val manager = context.getSystemService(ActivityManager::class.java)
        val exits = manager.getHistoricalProcessExitReasons(context.packageName, 0, 8)
        val exit = exits.firstOrNull { info ->
            info.processName == context.packageName && info.timestamp in oldest until processStartedAt &&
                when (info.reason) {
                    ApplicationExitInfo.REASON_CRASH_NATIVE, ApplicationExitInfo.REASON_CRASH,
                    ApplicationExitInfo.REASON_ANR, ApplicationExitInfo.REASON_LOW_MEMORY -> true
                    else -> false
                }
        }
        exit?.let {
            val stage = marker?.stageForExit(it.pid, it.timestamp)
            val reason = when (it.reason) {
                ApplicationExitInfo.REASON_CRASH_NATIVE -> tr("Сбой в нативной библиотеке")
                ApplicationExitInfo.REASON_CRASH -> tr("Ошибка в коде Android")
                ApplicationExitInfo.REASON_ANR -> tr("Приложение не ответило Android")
                ApplicationExitInfo.REASON_LOW_MEMORY -> tr("Android завершил процесс из-за нехватки памяти")
                else -> tr("Причина завершения Android · {0}", it.reason)
            }
            val detail = it.description?.trim()?.takeIf { text -> text.isNotEmpty() && !text.equals("crash", ignoreCase = true) }
            val summary = listOfNotNull(reason, detail?.take(400)).joinToString("\n")
            report(it.timestamp, stage, summary, interruptedStart = stage != null)
        }
    } catch (error: Exception) {
        Log.w("SecretTunnelConnection", tr("История завершений Android недоступна"), error)
        null
    }
}
