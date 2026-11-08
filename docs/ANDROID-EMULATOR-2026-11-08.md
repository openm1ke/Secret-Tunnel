# Проверка Android на эмуляторе — 8 ноября 2026

## APK

- `artifacts/android/SecretTunnel-debug.apk`, **13 377 559 байт**.
- Package `dev.secrettunnel.android`, версия `0.2.2`, versionCode `20200`.
- ARM64 и x86_64, Android 8.0+ / minSdk 26, targetSdk 34.
- SHA-256: `b72ef9fb70ed3894f8a6b8de56c524ef9836ba804907a2937e4d3c0ce0fb5b4b`.
- Debug-подпись APK v2 проверена. Это локальная тестовая сборка, не магазинный релиз.
- В APK нет параметров конкретного SSH-сервера, приватных ключей и тестовых подключений. Тестовый runner и два probe APK устанавливаются отдельно.

В итоговой сборке исправлены подписи нижней навигации и кнопки выбора приложений. Повторно прошли сборка, unit tests, lint и device-тест навигации; полные подписи визуально проверены на ширине 411 и 320 dp. Сетевая проверка ниже выполнена до этих изменений интерфейса, на сборке SHA-256 `fe555696ea9d49204b9c5084eb4ee501340bc71b60126c7b17f65cf0da63da22`; сетевой код после неё не менялся.

## Среда

Сборка выполнена на Apple Silicon macOS: Corretto JDK 17.0.18, Android SDK/build-tools 34, NDK 27.0.12077973, Gradle 8.7, AGP 8.5.2. `scripts/build-android.sh` теперь выбирает NDK `darwin-x86_64` на Mac и `linux-x86_64` на Linux/WSL; сохранён один скрипт для обеих платформ. Сборка приложения не требует сборки тестовых probe-модулей.

Создан отдельный AVD **SecretTunnel_QA**, Android 17 / API 37, ARM64 Google APIs, **PAGE_SIZE 16384**. Существующие AVD сохранены. Физический телефон не использовался.

## Что проверено

| Проверка | Результат |
| --- | --- |
| Rust `tunnel-core` | 10 тестов passed |
| JNI host check | `cargo check --manifest-path android/native/Cargo.toml --locked` прошёл |
| Обе нативные ABI | SSH/Rust и hev собраны; 4 `.so`, ZIP/ELF PT_LOAD alignment 16 KiB |
| Лицензии и upstream pin | 155 Rust notices + hev/lwIP/yaml/task-system; закреплённый hev revision проверен |
| Gradle unit tests | 2 теста TrafficMeter/History passed |
| Android lint | 0 ошибок, 13 предупреждений: target API, доступные версии test-зависимостей, ресурсы/иконка и выделения памяти при рисовании |
| Device instrumentation | 4 passed: неверный порт, Keystore round-trip, отдельность профилей, навигация и повторное использование переключателей; 2 теста старого сценария регистрации пропущены без специальных аргументов |
| Реальный SSH fixture | Ключ сервера независимо подтверждён свежим OpenSSH с `StrictHostKeyChecking=yes`, затем совпал с ключом, полученным JNI внутри эмулятора |
| APK install / JNI load | Прошли на 16-КБ эмуляторе, без AndroidRuntime crash |
| Системное VPN-согласие | Настоящий Android Connection request подтверждён через UI |
| Выбранный отдельный UID | Android `TRANSPORT_VPN=true`; HTTPS 200, системный DNS, 8/8 сырых UDP DNS-ответов; внешний IP соответствует выходу SSH-сервера |
| Невыбранный отдельный UID | Android `TRANSPORT_VPN=false`; HTTPS/DNS/8 UDP-ответов; исходный внешний IP сохранён |
| Полезные байты в Rust SSH | RX вырос с 0 до 19 202, TX с 0 до 3 242 во время проверки |
| Штатная остановка | `active=false`, SSH `running=false`; временный `ssh-key.runtime` отсутствует |
| Выбранное приложение после остановки | VPN=false; HTTPS/DNS/8 UDP-ответов продолжают работать с обычной маршрутизацией |

Важно: на этом Mac базовый выход эмулятора и SSH-сервер показывали **один внешний IP** из-за уже существующей сетевой конфигурации хоста. Поэтому совпадение IP само по себе не использовалось как доказательство. Дополнительно проверены VPN-capabilities **из процесса каждого probe UID**, разрешённый UID в Android connectivity, рост счётчиков полезной нагрузки **в Rust SSH forwarding** и закрытие VPN/SSH после остановки. Системные proxy/VPN-настройки хоста не менялись.

Путь проверки: `probe UID → Android VpnService / tun0 → hev-socks5-tunnel → JNI Rust SOCKS → SSH → direct-tcpip / UDP-agent → интернет`. Probe не умеет SOCKS и не настраивает proxy: он использует обычные Android сокеты/HTTPS. `adb shell curl` не подменяет эту проверку, поскольку shell имеет другой UID и не включён в VPN.

## Повторение

```bash
export JAVA_HOME=/path/to/jdk17
export ANDROID_HOME=/path/to/android-sdk
bash scripts/build-android.sh
bash android/gradlew -p android --no-daemon \
  :app:assembleDebugAndroidTest \
  :traffic-probe:assembleSelectedDebug :traffic-probe:assembleDirectDebug
```

Установить через adb четыре APK:

```bash
adb -s emulator-5554 install -r artifacts/android/SecretTunnel-debug.apk
adb -s emulator-5554 install -r android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
adb -s emulator-5554 install -r android/traffic-probe/build/outputs/apk/selected/debug/traffic-probe-selected-debug.apk
adb -s emulator-5554 install -r android/traffic-probe/build/outputs/apk/direct/debug/traffic-probe-direct-debug.apk
```

`scripts/prepare-android-emulator.py` использует выбранный локальный desktop SSH-профиль по умолчанию. Он допускает только эмулятор (`ro.kernel.qemu=1`), проверяет OpenSSH host key, передаёт существующий ключ **через stdin в app-private каталог** и запускает отдельный instrumentation-метод импорта. Ключ сохраняется зашифрованным Android Keystore; временные fixture/ключ удаляются. `authorized_keys`, серверный агент и остальные серверные файлы не меняются. Для другого профиля укажите `--config`.

```bash
python3 scripts/prepare-android-emulator.py --serial emulator-5554
adb -s emulator-5554 shell am instrument -w -r \
  -e runVpnIntegration true \
  -e class dev.secrettunnel.android.VpnFixtureTest#runVpnTrafficSession \
  dev.secrettunnel.android.test/androidx.test.runner.AndroidJUnitRunner
```

Во время этого теста подтвердите реальный VPN-диалог. После появления app-private `files/integration-ready.json` запустите `dev.secrettunnel.probe` и `dev.secrettunnel.directprobe` отдельными процессами; их `files/result.json` содержит проверку VPN/HTTPS/DNS/UDP. Маркер `files/integration-finish` завершает сессию, проверяет рост счётчиков, сохраняет `integration-status.json`/`integration-stop.json` и штатно останавливает сервис. Таймаут ожидания внешних probe — 180 секунд. Не запускайте другие instrumentation-сессии параллельно VPN-тесту: Android может перезапустить target process.

Сырые результаты сохранялись только локально в `artifacts/android/qa/*.private.*`; итог без IP/ключей — `artifacts/android/qa/result.json` (`python3 scripts/verify-android-routing.py`). Скриншот отдельного выбранного приложения: `artifacts/android/qa/selected-vpn.png`.

## Границы проверки

Это реальный Android VpnService-тест на ARM64-эмуляторе, а не подтверждение работы на всех телефонах. На физическом устройстве ещё нужны голос/видео Discord, Wi-Fi ↔ cellular, сон, принудительное завершение процесса, VPN revoke/always-on, длительная нагрузка, IPv6 и UDP потери. x86_64-библиотеки собраны и проверены статически, но не запускались на x86_64-эмуляторе. Регистрация нового серверного ключа в этой сессии не выполнялась; использовался уже разрешённый ключ.
