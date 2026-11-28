package dev.secrettunnel.android

import android.app.Activity
import android.app.Application
import android.content.Context
import android.content.res.Configuration
import org.json.JSONObject
import java.util.Locale

internal enum class AppLanguage(val code: String, val label: String) {
    RUSSIAN("ru", "Русский"), ENGLISH("en", "English");
    val locale: Locale get() = if (this == ENGLISH) Locale.US else Locale("ru", "RU")
}

internal object AppLocalization {
    @Volatile var language = AppLanguage.RUSSIAN
        private set
    @Volatile private var catalog = MessageCatalog(emptyMap())

    fun initialize(context: Context) {
        language = AppLanguage.entries.firstOrNull {
            it.code == context.getSharedPreferences("tunnel", Context.MODE_PRIVATE).getString("language", "ru")
        } ?: AppLanguage.RUSSIAN
        val values = runCatching {
            val document = JSONObject(context.assets.open("en.json").bufferedReader().use { it.readText() })
            document.keys().asSequence().associateWith { document.getString(it) }
        }.getOrDefault(emptyMap())
        catalog = MessageCatalog(values)
    }

    fun select(context: Context, value: AppLanguage) {
        language = value
        context.getSharedPreferences("tunnel", Context.MODE_PRIVATE).edit().putString("language", value.code).apply()
    }

    fun text(source: String, args: Array<out Any?>): String =
        catalog.text(source, language == AppLanguage.ENGLISH, args)

}

internal fun tr(source: String, vararg args: Any?): String = AppLocalization.text(source, args)

class TunnelApplication : Application() {
    override fun onCreate() { super.onCreate(); AppLocalization.initialize(this) }
}

/** Per-app locale also covers Android resources and native dialogs. */
open class LocalizedActivity : Activity() {
    override fun attachBaseContext(base: Context) {
        val configuration = Configuration(base.resources.configuration)
        configuration.setLocale(AppLocalization.language.locale)
        super.attachBaseContext(base.createConfigurationContext(configuration))
    }
}
