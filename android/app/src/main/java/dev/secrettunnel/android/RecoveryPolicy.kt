package dev.secrettunnel.android

/** Monotonic recovery deadlines. Waiting never occupies the service worker. */
class RecoveryPolicy(private val maxFailures: Int? = null) {
    enum class Action { HEALTHY, WAIT, RECONNECT_SSH, RESTART_ENGINE, NEEDS_USER }
    var failures = 0
        private set
    var nextAttemptAt = 0L
        private set
    var needsUser = false
        private set
    private var healthySince = -1L

    fun reset() {
        failures = 0
        nextAttemptAt = 0
        needsUser = false
        healthySince = -1
    }
    // A process which starts and crashes repeatedly is not a recovered session.
    fun healthy(now: Long) {
        if (healthySince < 0) healthySince = now
        if (now - healthySince >= 10_000) reset()
    }
    fun action(now: Long, ssh: Boolean, engine: Boolean): Action {
        if (ssh && engine) {
            healthy(now)
            return Action.HEALTHY
        }
        healthySince = -1
        if (needsUser) return Action.NEEDS_USER
        if (now < nextAttemptAt) return Action.WAIT
        return if (!ssh) Action.RECONNECT_SSH else Action.RESTART_ENGINE
    }
    fun failed(error: String, now: Long) {
        if (failures < Int.MAX_VALUE) failures++
        healthySince = -1
        val message = error.lowercase()
        needsUser = needsUser || (maxFailures != null && failures >= maxFailures) || listOf(
            "authentication failed", "public key authentication", "permission denied",
            "unknown server key", "rejected the server host key", "host key verification failed",
            "server host key rejected", "ключ сервера не подтверждён",
            "stmux003 server agent required", "cannot load ssh key", "encrypted private key",
            "not permitted", "не разрешил vpn",
            "agent_missing:", "agent_not_executable:", "agent_incompatible:",
            "agent_invalid_response:", "agent_resource_limit:", "agent_check_failed:"
        ).any { it in message }
        nextAttemptAt = now + (1000L shl (failures - 1).coerceIn(0, 4)).coerceAtMost(15_000L)
    }
    fun delay(now: Long): Long {
        val remaining = nextAttemptAt - now
        return if (!needsUser && remaining in 1..999) remaining else 1000
    }
}
