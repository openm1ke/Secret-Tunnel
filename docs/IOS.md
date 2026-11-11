# Secret Tunnel для iOS/iPadOS (экспериментально)

Нативный клиент в `ios/`: приложение SwiftUI и расширение **Packet Tunnel** (`NEPacketTunnelProvider`), iOS/iPadOS 15+.

Цепочка такая же, как на Android: `utun → hev-socks5-tunnel → Rust SOCKS5 ([::1]:10808) → SSH → secret-tunnel-agent`. Rust-ядро общее (`tunnel-core`, C API в `ios/native`). Сокеты самого расширения (SSH-соединение) не идут через его туннель, поэтому `protect()` из Android не нужен.

Проверено 10 ноября 2026 на iPad Air 2 (iOS 15.8.8): VPN для всего устройства, speedtest.net 27 Мбит/с скачивание и 18 Мбит/с загрузка через SSH-сервер, память расширения до 8,4 МБ.

Особенности, найденные на устройстве:

- SOCKS слушает `::1`, а не `127.0.0.1`. hev всегда открывает IPv6-сокеты, и v4-mapped `::ffff:127.0.0.1` внутри расширения не подключался: hev отбрасывал все UDP (DNS) и TCP-сессии.
- Лимит мультиплексора 1024 потока, окно UDP равно окну TCP (64 КБ). При лимите 256 новые соединения отклонялись (`Receive buffer budget reached`): бюджет приёма был равен «лимит × 64 КБ», а каждый UDP-поток резервировал на агенте 128 КБ. С 11 ноября 2026 бюджет рассчитан на наибольшее окно, а DNS-ассоциации закрываются через 5 секунд; для полной ёмкости на стороне сервера нужен пересобранный агент.
- С агентом, который объявляет `elastic_tcp`, окно TCP равно 256 КБ и растёт от 64 КБ, пока канал быстрый. Поток с остановившимся получателем может держать в очереди до 256 КБ, поэтому при многих зависших загрузках следите за строкой `memory` в журнале расширения.
- В расширении включена диагностика в системный журнал: самопроверка SOCKS, счётчики hev, память. Читать: `idevicesyslog -u <UDID> | grep "PacketTunnel\["`.

## Ограничения iOS

- **Туннель для всего устройства.** Выбрать отдельные приложения нельзя: per-app VPN на iOS работает только на устройствах под MDM и только для управляемых приложений.
- **Симулятор.** Network Extension в iOS Simulator не работает. Там можно проверить только интерфейс и подписанные действия приложения, VPN не запустится.
- **Подпись.** Нужен платный аккаунт Apple Developer: право Network Extension для Personal Team недоступно.
- **Память расширения** на iOS ограничена (десятки МБ), поэтому буферы UDP и лимиты сессий уменьшены по сравнению с Android.
- **Только IPv4.** У сервера нет IPv6-выхода, а объявленный в туннеле IPv6 заставлял приложения сначала пробовать IPv6 и получать отказ. Если Wi-Fi сама раздаёт IPv6, этот трафик идёт мимо туннеля.

## Сборка

Требуются Xcode, XcodeGen, Rust (`aarch64-apple-ios`, `aarch64-apple-ios-sim`).

```bash
scripts/build-ios.sh sim                      # симулятор, без подписи
ST_DEVELOPMENT_TEAM=<TeamID> ST_BUNDLE_PREFIX=<ваш.префикс> scripts/build-ios.sh device
```

Без `ST_DEVELOPMENT_TEAM` сборка для устройства не подписывается (только проверка компиляции и линковки). Xcode с `-allowProvisioningUpdates` сам создаёт App ID, App Group `group.<префикс>.ios` и профили.

### Установка на iOS 15

Xcode 27 не предлагает устройства с iOS 15 как цель запуска, поэтому подписанное приложение ставится через libimobiledevice:

```bash
brew install ideviceinstaller
mkdir -p /tmp/st-ipa/Payload && cp -R ios/build/DerivedData/Build/Products/Release-iphoneos/SecretTunnel.app /tmp/st-ipa/Payload/
(cd /tmp/st-ipa && zip -qry SecretTunnel.ipa Payload)
ideviceinstaller -u <UDID> install /tmp/st-ipa/SecretTunnel.ipa
```

UDID устройства должен быть в профиле разработки (Certificates, Identifiers & Profiles → Devices). Журнал устройства: `idevicesyslog -u <UDID> | grep PacketTunnel`.

## Проверка

1. Введите адрес, порт и пользователя SSH, импортируйте приватный ключ (OpenSSH, без passphrase) через «Файлы».
2. «Получить отпечаток сервера», сверьте с `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` на сервере и подтвердите.
3. «Проверить вход и агент»: должен пройти вход по ключу и найтись `secret-tunnel-agent`.
4. «Подключить весь трафик», разрешите добавление VPN-конфигурации. Откройте сайт проверки IP в Safari: должен показываться IP сервера, а счётчики в приложении расти.
