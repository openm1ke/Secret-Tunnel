# Публикация релизов

Workflow **Publish releases** (`.github/workflows/release.yml`) создаёт:

- подписанный **release** APK (ARM64/x86_64), не debug APK;
- Windows x64 NSIS setup с offline WebView2 и полным комплектом перехвата;
- Windows x64 portable ZIP: EXE, ProxyBridge CLI/Core, WinDivert DLL/подписанный SYS, лицензии, SHA256-манифест и серверный агент;
- macOS 13+ universal DMG и ZIP с `.app` для Apple Silicon и Intel, Embedded SSH, лицензиями и серверным агентом;
- статический Linux x86_64 UDP-agent с пользовательским установочным скриптом;
- общий `SHA256SUMS.txt`.

Сначала проходят сборки и проверки всех платформ. Публикация происходит только после их успеха. На существующем удалённом SSH-сервере workflow ничего не запускает.

## Запуск

При отправке тега `v0.2.0` workflow стартует автоматически. Номер версии в `package.json`, Tauri и Cargo должен соответствовать тегу.

Также можно запустить Actions → **Publish releases** → **Run workflow**, указать `tag`, например `v0.2.0`, и выбрать нужный commit через ветку запуска. При ручном запуске тег создаётся только после успешных сборок; неуспешная сборка не публикует неполный релиз. Уже существующий тег должен указывать на проверяемый commit.

Через GitHub CLI: `gh workflow run release.yml -f tag=v0.2.0 --ref main`.

Для проверки кандидата до публикации выключите input `publish`: все платформы и подписанный APK соберутся в Actions artifacts, тег и GitHub Release не создаются. Например: `gh workflow run release.yml --ref <branch> -f tag=v0.2.3 -F publish=false`. Проверенные файлы можно затем опубликовать для точного commit кандидата вместе с SHA256SUMS.txt.

## Windows: инструменты упаковки

Tauri CLI закреплён на 2.11.2. Перед упаковкой `scripts/prepare-windows-bundler.py` получает NSIS 3.11 и `nsis_tauri_utils` 0.5.3 через официальный GitHub Release API с повторными попытками и проверкой закреплённых SHA-256. Проверенный архив заново распаковывается в `src-tauri/target/.tauri/NSIS`; `useLocalToolsDir` направляет Tauri в этот каталог. Контрольные суммы соответствуют также SHA-1, используемым самим Tauri. Это устраняет зависимость упаковки от разовой загрузки NSIS через browser download URL, ранее возвращавший HTTP 504.

Компиляция приложения выполняется отдельно от `tauri bundle`. Упаковка повторяется до трёх раз при временных ошибках загрузки WebView2 или других ресурсов; ошибка компиляции не маскируется повторным запуском. При обновлении Tauri CLI нужно проверить соответствие версий NSIS и обновить pins в скрипте.

## macOS

Отдельный macOS runner собирает `universal-apple-darwin` с обеими Rust-архитектурами. Перед публикацией проверяются архитектуры `arm64`/`x86_64`, ad-hoc подпись, состав ресурсов, работа guardian из упакованного приложения, целостность ZIP и DMG. macOS-артефакты обязательны для общего шага публикации.

До компиляции выполняется `python3 scripts/check-macos-packaging.py`: временное подписанное universal-приложение проходит настоящий `package-macos.sh`, включая создание и проверку DMG. Проверяется также отказ для корректно подписанного приложения только с ARM64. Каждая архитектура проверяется отдельным вызовом `xcrun lipo <binary> -verify_arch <architecture>`: путь к файлу передаётся до флага, чтобы он не попал в список архитектур.

Сборка приложения имеет ad-hoc подпись, без Developer ID и нотариализации Apple. Для загруженного приложения может потребоваться разрешение запуска в системных настройках. Подписанное сетевое расширение ProxyBridge устанавливается отдельно из официального macOS-релиза; оно не заменяется Windows-драйвером и не включается в этот DMG. Настройка: [macOS](MACOS.md).

## Подпись APK

Настроены GitHub Actions secrets:

- `ANDROID_KEYSTORE_BASE64`
- `ANDROID_KEYSTORE_PASSWORD`
- `ANDROID_KEY_ALIAS`
- `ANDROID_KEY_PASSWORD`

Постоянный PKCS12-ключ создан для проекта и хранится вне репозитория, локально в `%USERPROFILE%\.config\secret-tunnel-signing\android-release.p12`. Пароли находятся рядом в закрытом `android-signing.json`. Каталог доступен текущему пользователю, его нужно сохранить в защищённой резервной копии. Эти файлы **не являются кэшем и не подлежат очистке**. В репозиторий, артефакты или release assets они не включаются. Это отдельный ключ подписи Android, не приватный SSH-ключ.

CI расшифровывает keystore во временный каталог runner, подписывает APK и удаляет временный файл. При отсутствии секретов release-сборка завершается ошибкой; подмена debug-подписью или публикация unsigned APK исключены. Счётчик versionCode увеличивается по run number release-workflow.

Windows EXE и setup пока без Authenticode-подписи приложения; upstream-драйвер WinDivert сохраняет проверенную подпись. Контрольные суммы драйвера, DLL, import library и заголовка закреплены в `scripts/prepare-windows-component.ps1`; компилируемый ProxyBridge закреплён по commit. Скрипты не отключают Defender, SmartScreen или проверку драйверов.

Android проверен на физическом телефоне через VPN выбранных приложений. Доменные regex-правила есть в Windows, Android маршрутизирует по приложениям.
