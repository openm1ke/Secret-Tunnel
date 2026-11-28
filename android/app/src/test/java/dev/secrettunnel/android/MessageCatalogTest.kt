package dev.secrettunnel.android

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.io.File

class MessageCatalogTest {
    private fun catalog(): MessageCatalog {
        val file = listOf(File("../locales/en.json"), File("../../locales/en.json"))
            .first { it.isFile }
        val document = JSONObject(file.readText())
        return MessageCatalog(document.keys().asSequence().associateWith { document.getString(it) })
    }

    @Test fun sharedEnglishAndRussianFallbackPreserveOpaqueArguments() {
        val catalog = catalog()
        assertEquals("Connect", catalog.text("Подключить", true))
        assertEquals("Подключить", catalog.text("Подключить", false))
        assertEquals("Server “Рабочий {1} 🔒” saved", catalog.text("Сервер «{0}» сохранён", true, arrayOf("Рабочий {1} 🔒")))
        assertEquals("21 h 8 min", catalog.text("{0} ч {1} мин", true, arrayOf(21, 8)))
        assertEquals("21 ч 8 мин", catalog.text("{0} ч {1} мин", false, arrayOf(21, 8)))
        assertEquals("E_CUSTOM 203.0.113.1", catalog.text("E_CUSTOM 203.0.113.1", true))
    }

    @Test fun formattedNativeErrorsUseTheMostSpecificMessage() {
        val catalog = catalog()
        assertEquals("The speed test server returned HTTP 429.", catalog.text("Сервер теста скорости вернул HTTP 429.", true))
        assertEquals("UDP: DNS errors 2 · send 3 · receive 4\n", catalog.text("UDP: ошибки DNS 2 · отправка 3 · приём 4\n", true))
        assertEquals("Could not verify server “Рабочий”. ", catalog.text("Не удалось проверить сервер «Рабочий». ", true))
    }
}
