# macOS: SSH и трафик выбранных приложений

## Как работает

`Выбранное приложение → ProxyBridge Network Extension → локальный SOCKS5 → Embedded Rust SSH → SSH-сервер → secret-tunnel-agent (UDP) / direct-tcpip (TCP)`.

Secret Tunnel управляет подписанным ProxyBridge через CLI, используя **формат профиля macOS** и передачу JSON в stdin. Требуется ProxyBridge **4.1.2 или новее**, macOS 13+ и разрешённое системное расширение. Windows-комплект с WinDivert на Mac не используется. Системный SOCKS/HTTP proxy и маршруты остальных приложений Secret Tunnel не меняет.

- **Embedded Rust SSH:** TCP и UDP внутри существующей SSH-сессии. Для UDP серверу нужен исполняемый `~/.local/bin/secret-tunnel-agent`; см. [установку агента](SERVER-UDP-SETUP.md).
- **System OpenSSH:** TCP через `ssh -D`. UDP выбранных приложений блокируется, чтобы не подменять туннелирование незаметным прямым выходом.
- Системный DNS может идти напрямую. В этом адаптере DNS proxy ProxyBridge выключен, DNS и доменные regex-правила Windows не переносятся. DNS-конфигурации с fake IP, защищённый DNS и другие VPN требуют отдельного совместного тестирования.
- Перехват применяется к новым соединениям. После включения или изменения выбора переподключите приложения.
- Независимого kill switch нет: при остановке/аварии новые соединения могут идти напрямую. UDP внутри SSH наследует задержки TCP при потере пакетов; синтетический тест не доказывает качество звонка.

Основа интеграции проверена по исходникам ProxyBridge `cb897b017a88a142ec803c3af138b0ad75c2ae12`, `MacOS/ProxyBridge/ProxyBridge/CLIProfile.swift`, `CLIRunner.swift` и `extension/AppProxyProvider.swift`. macOS-релиз имеет собственную нумерацию: 4.1.2, Windows — 4.3.0.

## Настройка

1. Установите [официальный macOS-релиз ProxyBridge](https://github.com/InterceptSuite/ProxyBridge/releases/tag/v4.1.2_MacOS) в `/Applications`. Secret Tunnel запускает `/Applications/ProxyBridge.app/Contents/MacOS/ProxyBridge`; отдельный launcher в PATH не требуется. Закройте GUI ProxyBridge: CLI и GUI не могут одновременно управлять расширением.
2. Разрешите расширение в системных настройках. На проверенном Mac: **Основные → Объекты входа и расширения → ProxyBridge → сетевое расширение**. В более ранних macOS запрос также может появляться в разделе «Конфиденциальность и безопасность». При необходимости подтвердите добавление сетевой конфигурации.
3. Добавьте SSH-сервер с ключом и локальный прокси `127.0.0.1:1080` или `[::1]:1080`. SOCKS без пароля разрешён только на loopback.
4. Выберите **Embedded Rust SSH** в настройках, сохраните и запустите туннель.
5. Во вкладке **Приложения** отметьте Discord или другую программу. Можно добавить `.app` из нестандартного каталога через файловый диалог. Нажмите **Включить перехват**. Статус станет активным только после подтверждения готовности от расширения.
6. Перезапустите выбранную программу для новых соединений. Для Discord проверьте вход, голос и демонстрацию экрана отдельно.

Список читается из `/Applications`, `/System/Applications`, `/System/Library/CoreServices/Applications` и `~/Applications`, включая вложенные `.app` и `.xpc`. Сканирование не запускает приложения и не требует Spotlight. Правила содержат точные идентификаторы, а не wildcard по префиксу.

### Семейство Discord

На установленном Discord проверены `Info.plist` и `codesign -dv` для всех пяти исполняемых файлов:

| Процесс | Идентификатор в правиле |
| --- | --- |
| Discord | `com.hnc.Discord` |
| Discord Helper | `com.hnc.Discord.helper` |
| Discord Helper (Renderer) | `com.hnc.Discord.helper.Renderer` |
| Discord Helper (GPU) | `com.hnc.Discord.helper.GPU` |
| Discord Helper (Plugin) | `com.hnc.Discord.helper.Plugin` |

Выбор Discord включает все пять. Не требуется вручную отмечать каждый renderer или отслеживать PID после перезапуска.

Внутри Discord также найдены `Squirrel.framework/.../Resources/ShipIt` и `chrome_crashpad_handler`. Это самостоятельные утилиты с общими идентификаторами подписи `ShipIt` и `chrome_crashpad_handler`. ProxyBridge macOS сопоставляет идентификатор подписи/имя процесса; точный путь утилиты в правилах не поддерживается. Поэтому они **не включаются автоматически** в семейство Discord: общее правило захватило бы такие утилиты других Electron-программ. Если это нужно, добавьте эти точные идентификаторы в дополнительные настройки; интерфейс покажет предупреждение об общем действии. Для строгого ограничения таких утилит одним Discord нужна поддержка пути/требования подписи в сетевом расширении.

## Завершение процессов

На Unix `Child::kill()` отправляет SIGKILL, который не даёт CLI очистить правила расширения. Поэтому отдельный процесс Secret Tunnel (`--macos-redirect-guardian`) владеет CLI и контролирует stdin родителя. Закрытие канала, в том числе при аварии GUI, вызывает SIGTERM для CLI и ожидание очистки. Временной предел остановки сопровождается ошибкой: успешная очистка не объявляется без подтверждения завершения. Остановка туннеля, сброс настроек и закрытие окна также останавливают перехват.

## Сборка и проверки

```bash
npm ci
npm run tauri -- build --bundles app --features embedded-ssh
cargo build --manifest-path src-tauri/Cargo.toml --features embedded-ssh --examples --locked
python3 scripts/check-macos-redirect.py src-tauri/target/release/secret-tunnel
```

Для реальной проверки остановите перехват в GUI (сам туннель на 1080 может продолжать работать). Диагностика использует отдельный порт 1082 и сохранённый выбранный SSH-профиль:

```bash
python3 scripts/check-macos-network.py
```

Проверка собирает два ad-hoc подписанных `.app`, создаёт профиль **тем же Rust-генератором, что GUI**, запускает тот же SSH-код и production guardian. Она проверяет HTTPS/SOCKS, UDP DNS, обычные TCP/UDP-сокеты выбранного приложения, прямой UDP другого приложения, очистку при EOF родителя и прямой выход после остановки SSH/перехвата. Конфигурация пользователя не переписывается; ключ и реквизиты остаются локально. Результат: `artifacts/macos-integration/result.json`.

8 ноября 2026 реальная проверка на Apple Silicon macOS прошла с установленным ProxyBridge 4.1.2 и существующим серверным агентом. В журнале расширения зафиксировано:

```text
UDP TunnelProbe -> 1.1.1.1:53 via SOCKS5
TCP TunnelProbe -> 1.1.1.1:80 via SOCKS5 127.0.0.1:1082
UDP DirectProbe -> unknown:unknown via Direct
Rules cleared, tunnel stopped.
```

Из нативного GUI проверены запуск Embedded SSH, получение IP через туннель, выбор семейства Discord, включение/остановка перехвата. Реальный голосовой звонок и демонстрация экрана должны подтверждаться отдельно; сетевые probe-тесты этого не заменяют.

Проверенный установщик: `ProxyBridge-v4.1.2-Universal-Installer.pkg`, SHA-256 `485fc224e9a671bc997de5353e3e524c4d6c7f0454951a23fe99dbe35864bb1c`. Подпись `Developer ID Installer: Sourav Kalal (L4HJT32Z59)`, нотариализация Apple и `spctl` успешно проверены. Proxifier и его настройки при проверке не менялись.

Источники: [ProxyBridge macOS](https://github.com/InterceptSuite/ProxyBridge/tree/main/MacOS), [развёртывание Network Extension — Apple](https://developer.apple.com/documentation/technotes/tn3134-network-extension-provider-deployment).
