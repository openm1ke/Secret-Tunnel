package dev.secrettunnel.android

/** Keep the successful SSH stage visible when a later agent check fails. */
object ConnectionMessages {
    fun describe(raw: String): String {
        val text = raw.lowercase()
        return when {
            text.startsWith("agent_") -> tr(raw.substringAfter(':').trim().take(400))
            "authentication failed" in text -> tr("Сервер не принял SSH-ключ. Проверьте пользователя и ключ на экране «Серверы».")
            "address already in use" in text || "os error 98" in text -> tr("Локальный порт занят. Выберите другой на экране «Прокси».")
            "unknown server key" in text || "rejected the server host key" in text -> tr("Проверьте и подтвердите ключ сервера в его настройках.")
            "connection refused" in text -> tr("Сервер отклонил TCP-соединение. Проверьте порт SSH.")
            "timed out" in text || "timeout" in text -> tr("Сервер не ответил. Проверьте адрес, порт и подключение к интернету.")
            else -> tr(raw.take(240))
        }
    }
}
