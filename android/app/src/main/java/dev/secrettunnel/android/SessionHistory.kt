package dev.secrettunnel.android

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * Tunnel sessions and the traffic each application sent through them, kept in
 * `sessions.json` with the desktop's fields. Only names, times and byte
 * counts; no addresses or contents.
 */
data class AppUsage(val name: String, val rx: Long, val tx: Long)

data class SessionRecord(
    val id: String, val startedAt: Long, val endedAt: Long, val server: String, val route: List<String>,
    val country: String, val rx: Long, val tx: Long, val apps: List<AppUsage>, val active: Boolean,
) {
    fun toJson(): JSONObject = JSONObject().put("id", id).put("started_at", startedAt).put("ended_at", endedAt)
        .put("server", server).put("route", JSONArray(route)).put("country", country).put("rx", rx).put("tx", tx)
        .put("apps", JSONArray(apps.map { JSONObject().put("name", it.name).put("rx", it.rx).put("tx", it.tx).put("flows", 0) }))
        .put("active", active)
    companion object {
        fun fromJson(o: JSONObject): SessionRecord {
            val route = o.optJSONArray("route"); val apps = o.optJSONArray("apps")
            return SessionRecord(o.optString("id"), o.optLong("started_at"), o.optLong("ended_at"), o.optString("server"),
                (0 until (route?.length() ?: 0)).map { route!!.optString(it) }, o.optString("country"), o.optLong("rx"), o.optLong("tx"),
                (0 until (apps?.length() ?: 0)).map { apps!!.getJSONObject(it).let { a -> AppUsage(a.optString("name"), a.optLong("rx"), a.optLong("tx")) } },
                o.optBoolean("active"))
        }
    }
}

object SessionHistory {
    private const val MAX_SESSIONS = 1000
    private fun file(context: Context) = File(context.filesDir, "sessions.json")

    /** Sessions, oldest first. One left active by a crash ends when it was last saved. */
    @Synchronized fun load(context: Context, runningId: String? = null): List<SessionRecord> = runCatching {
        val array = JSONArray(file(context).readText())
        (0 until array.length()).map { SessionRecord.fromJson(array.getJSONObject(it)) }
            .map { if (it.active && it.id != runningId) it.copy(active = false) else it }
    }.getOrDefault(emptyList())

    @Synchronized fun save(context: Context, record: SessionRecord) {
        val sessions = load(context, record.id).filterNot { it.id == record.id } + record
        write(context, sessions.sortedBy { it.startedAt }.takeLast(MAX_SESSIONS))
    }

    /** Clears finished sessions; a running one stays and keeps counting. */
    @Synchronized fun clear(context: Context, runningId: String?) =
        write(context, load(context, runningId).filter { it.active && it.id == runningId })

    private fun write(context: Context, sessions: List<SessionRecord>) {
        val target = file(context)
        val temporary = File(target.parentFile, target.name + ".tmp")
        temporary.writeText(JSONArray(sessions.map { it.toJson() }).toString())
        temporary.renameTo(target)
    }
}
