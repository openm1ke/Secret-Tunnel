package dev.secrettunnel.android

/** Keep the successful SSH stage visible when a later agent check fails. */
object ConnectionMessages {
    fun describe(raw: String): String {
        val text = raw.lowercase()
        return when {
            text.startsWith("agent_") -> raw.substringAfter(':').trim().take(400)
            "authentication failed" in text -> "Сервер не принял SSH-ключ. Проверьте пользователя и ключ на экране «Серверы»."
            "address already in use" in text || "os error 98" in text -> "Локальный порт занят. Выберите другой на экране «Прокси»."
            "unknown server key" in text || "rejected the server host key" in text -> "Проверьте и подтвердите ключ сервера в его настройках."
            "timed out" in text || "timeout" in text -> "Сервер не ответил. Проверьте адрес, порт и подключение к интернету."
            else -> raw.take(240)
        }
    }
}
