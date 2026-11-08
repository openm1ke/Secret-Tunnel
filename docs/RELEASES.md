# Публикация релизов

Workflow **Publish releases** (`.github/workflows/release.yml`) создаёт:

- подписанный **release** APK (ARM64/x86_64), не debug APK;
- Windows x64 NSIS setup с offline WebView2 и полным комплектом перехвата;
- Windows x64 portable ZIP: EXE, ProxyBridge CLI/Core, WinDivert DLL/подписанный SYS, лицензии, SHA256-манифест и серверный агент;
- статический Linux x86_64 UDP-agent с пользовательским установочным скриптом;
- общий `SHA256SUMS.txt`.

Сначала проходят сборки и проверки всех платформ. Публикация происходит только после их успеха. На существующем удалённом SSH-сервере workflow ничего не запускает.

## Запуск

При отправке тега `v0.2.0` workflow стартует автоматически. Номер версии в `package.json`, Tauri и Cargo должен соответствовать тегу.

Также можно запустить Actions → **Publish releases** → **Run workflow**, указать `tag`, например `v0.2.0`, и выбрать нужный commit через ветку запуска. При ручном запуске тег создаётся только после успешных сборок; неуспешная сборка не публикует неполный релиз. Уже существующий тег должен указывать на проверяемый commit.

Через GitHub CLI: `gh workflow run release.yml -f tag=v0.2.0 --ref main`.

## Подпись APK

Настроены GitHub Actions secrets:

- `ANDROID_KEYSTORE_BASE64`
- `ANDROID_KEYSTORE_PASSWORD`
- `ANDROID_KEY_ALIAS`
- `ANDROID_KEY_PASSWORD`

Постоянный PKCS12-ключ создан для проекта и хранится вне репозитория, локально в `%USERPROFILE%\.config\secret-tunnel-signing\android-release.p12`. Пароли находятся рядом в закрытом `android-signing.json`. Каталог доступен текущему пользователю, его нужно сохранить в защищённой резервной копии. Эти файлы **не являются кэшем и не подлежат очистке**. В репозиторий, артефакты или release assets они не включаются. Это отдельный ключ подписи Android, не приватный SSH-ключ.

CI расшифровывает keystore во временный каталог runner, подписывает APK и удаляет временный файл. При отсутствии секретов release-сборка завершается ошибкой; подмена debug-подписью или публикация unsigned APK исключены. Счётчик versionCode увеличивается по run number release-workflow.

Windows EXE и setup пока без Authenticode-подписи приложения; upstream-драйвер WinDivert сохраняет проверенную подпись. Контрольные суммы драйвера, DLL, import library и заголовка закреплены в `scripts/prepare-windows-component.ps1`; компилируемый ProxyBridge закреплён по commit. Скрипты не отключают Defender, SmartScreen или проверку драйверов.

Android остаётся первой версией без физической проверки на телефоне; это явно отражено в release notes. Доменные regex-правила есть в Windows; Android пока маршрутизирует по приложениям.
