package dev.secrettunnel.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LocalizationDeviceTest {
    @Test fun bundledTranslationsWorkWithAndroidIcuAndPersistWithoutChangingProfiles() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val store = SettingsStore(context)
        val servers = store.servers()
        val selection = store.packages
        val previous = AppLocalization.language
        try {
            AppLocalization.select(context, AppLanguage.RUSSIAN)
            assertEquals("Быстрый", SpeedKinds.title("quick"))
            assertEquals("Светлая", Appearance.LIGHT.label)
            assertEquals("1 мин", ChartRange.MINUTE.label)
            AppLocalization.select(context, AppLanguage.ENGLISH)
            assertEquals("Connect", tr("Подключить"))
            assertEquals("Quick", SpeedKinds.title("quick"))
            assertEquals("Light", Appearance.LIGHT.label)
            assertEquals("1 min", ChartRange.MINUTE.label)
            assertEquals("Server “Рабочий {1} 🔒” saved", tr("Сервер «{0}» сохранён", "Рабочий {1} 🔒"))
            assertEquals("The speed test server returned HTTP 429.", tr("Сервер теста скорости вернул HTTP 429."))
            AppLocalization.initialize(context)
            assertEquals(AppLanguage.ENGLISH, AppLocalization.language)
            AppLocalization.select(context, AppLanguage.RUSSIAN)
            assertEquals("Подключить", tr("Подключить"))
            assertEquals(servers, store.servers())
            assertEquals(selection, store.packages)
        } finally { AppLocalization.select(context, previous) }
    }
}
