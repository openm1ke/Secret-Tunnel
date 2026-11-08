# Android build — 2026-11-08

Готов подписанный debug APK первой Android-версии в том же репозитории.

- Файл: `artifacts/android/SecretTunnel-debug.apk`
- Package: `dev.secrettunnel.android`, version `0.1.0` / code `1`
- Android 8.0+ (API 26), target/compile API 34
- ABI: `arm64-v8a`, `x86_64`
- Размер: 12 600 937 байт
- SHA256: `d2bfeb0dfda5824873096c0ad8624f3407f322b856d8fc00d25dc9988551453e`

## Что проверено

| Проверка | Результат |
|---|---|
| Общий `tunnel-core` | 6 unit tests passed: UDP framing/parsing и строгий host-key pinning |
| Windows `src-tauri`, embedded SSH | 12 unit tests passed после изменения общего SSH-кода |
| JNI adapter | Компилируется для ARM64 и x86_64; в `.so` найдены требуемые JNI entry points |
| Общий клиент → существующий SSH-сервер | HTTPS через SOCKS; 8/8 UDP DNS-ответов; закрытие UDP-ассоциации при закрытии управляющего TCP |
| Rust + hev/lwIP | Android release-библиотеки собраны для обеих ABI |
| Kotlin / resources / manifest / APK | `assembleDebug` успешно |
| Android lint | 0 ошибок, 2 предупреждения только о значке: избыточный resource qualifier v26 и отсутствие themed monochrome icon |
| Gradle unit tests | `NO-SOURCE`; отдельных Kotlin unit tests в первой версии нет |
| APK signature | `apksigner verify`: действительная подпись APK Signature Scheme v2, один debug signer |
| APK native packaging | Обе ABI, четыре `.so`; uncompressed ZIP и ELF PT_LOAD выровнены на 16 KiB |
| Runtime dependencies | У ARM64 библиотек только стандартные Android `libc.so`, `libm.so`, `libdl.so` |
| Dependency notices | MIT notices tun2socks/lwIP/task-system/yaml/core и список/лицензии 155 Rust dependencies включены в assets |

Ключей, паролей и параметров конкретного SSH-сервера в APK нет. Отдельный проверочный SOCKS использовал порт 1082 и сам завершился; работающий Windows-перехват не останавливался. Серверные сервисы, Xray/wstunnel и UDP-agent не изменялись.

## Что ещё требует телефона

По просьбе пользователя APK подготовлен **без подключения телефона**. Реальные Android VPN consent, маршрутизация выбранных package UID, `VpnService.protect`, DNS/IPv6 на устройстве, фон/экран, переключение Wi-Fi ↔ мобильная сеть и Discord голос/видео пока не проверены. Тест общего транспорта не заменяет эти проверки. Утверждать, что перехват уже проверен на Android, нельзя.

Android-редактор доменных regex/DIRECT-исключений пока не реализован; эта версия покрывает SSH-туннель и маршрутизацию TCP/UDP по приложениям. Подробности и настройка: [ANDROID.md](ANDROID.md).

## Инструменты и повторная сборка

Локально использованы WSL, Rust stable 1.99.1, Corretto JDK 17, NDK 27.0.12077973, platform/build-tools 34, Gradle 8.7, Android Gradle Plugin 8.5.2, Kotlin 1.9.24. Для старого SDK восстановлены символические ссылки NDK и доступ владельца к инструментам на NTFS. Подготовлен изолированный SDK view, чистая platform 34 загружена из Google с проверкой опубликованной контрольной суммы. Эмулятор не устанавливался, защита Windows не менялась.

Повторная сборка с этого Windows-компьютера: `./scripts/build-android.ps1`. Linux/CI: `bash scripts/build-android.sh` с `ANDROID_HOME` и `JAVA_HOME`. Результат и статические APK-проверки формируются скриптом автоматически.
