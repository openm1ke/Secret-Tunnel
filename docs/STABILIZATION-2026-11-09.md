# Стабилизация Secret Tunnel — 9 ноября 2026

Рабочая ветка `codex/macos-app-routing`, изменения поверх `da9ab22` / v0.2.3.
Основание — [аудит](AUDIT-2026-11-09.md) и первый этап [плана](ROADMAP-2026-11-09.md).
Это локальные изменения; новый релиз и обновление серверных служб не выполнялись.

## Изменения

| Находка | Реализация |
| --- | --- |
| A1/A2: зависшие relay и пропущенная отмена | Терминальное watch-состояние сохраняется без подписчиков; cancellation прерывает локальные read/write/shutdown. При Abort слот удерживается до завершения владельца Flow, чтобы учитывать ещё живые ресурсы. TCP half-close сохранён. |
| A3: reconnect внутри UI poll | Отдельный supervisor владеет запуском/остановкой и повторными попытками. Команды задают желаемое состояние; generation блокирует публикацию устаревшего runtime. Повторяется снимок исходного профиля, с backoff до 30 секунд и небольшим jitter. Stop отменяет и ожидание, и SSH startup. |
| A4: Android engine | Monitor отдельно различает потерю SSH и остановку packet engine. При живом SSH перезапускается только engine; при reconnect capabilities IPv6/лимит проверяются заново. При изменении параметров TUN заменяется после остановки native reader. Пока замена не готова, старый дескриптор сохраняется. |
| A5: конфигурация | Чтение/изменение/сохранение сериализованы. Temp + sync + atomic replace, последняя валидная `.json.bak`, восстановление без перезаписи backup повреждённым JSON. Восстановление отмечается в журнале. Windows краткие sharing/access-denied ошибки повторяются ограниченное время. |
| A6: медленный status | `tunnel_status` читает состояние из памяти. SSH TCP latency и IPinfo выполняются отдельно от runtime mutex, history обновляется без UI poll. IP сбрасывается при новом профиле и не показывается при остановленном туннеле. |

Android recovery использует монотонные дедлайны и не удерживает worker во время backoff.
Ошибка ключа/доверия/несовместимого агента требует действия пользователя; серия сбоев ограничена.
Кратковременный старт engine не сбрасывает бюджет: требуется 10 секунд здорового состояния.
Stop инвалидирует JNI startup, а не только отбрасывает его результат после полного таймаута.

Desktop UI различает подключение, повторное подключение, остановку и ошибку.
Во время startup/reconnect доступна остановка и заблокирован повторный Start.
После восстановления SSH desktop-перехват включается явно пользователем: автоматическое восстановление
перехвата и системная политика block/direct пока не реализованы. Их нельзя считать kill switch.

## Проверки

- Windows `cargo test --manifest-path src-tauri/Cargo.toml --features embedded-ssh,tauri/custom-protocol --lib --locked`: **20 passed**. Включены восстановление config (повреждённый и неполный JSON, backup старого формата), сохранение terminal auth-диагностики OpenSSH и отмена настоящего SSH handshake на локальном неотвечающем TCP-сервере; после отмены наблюдается закрытие сокета.
- `stream-mux`: **12 passed**, включая 1000 отмен с заполненным ограниченным duplex-буфером и 200 циклов с настоящими loopback TCP-сокетами в обеих ролях. Проверяются завершение relay, освобождение admission и receive queue; также shutdown транспорта.
- `session-control`: **4 passed на Windows и Linux**, включая параллельное чтение во время атомарной замены. Зависимость добавлена в desktop CI и release gate.
- Два первоначально падавших [воспроизведения](audit-repros/mux-cancellation/README.md) теперь проходят.
- `tunnel-core`: исходные 11 тестов прошли; добавленный тест отмены SSH handshake отдельно прошёл на Linux и в Windows desktop suite.
- `udp-agent`: **8 unit + 2 integration passed**; `routing-engine`: **2 passed** в исходном прогоне аудита. Серверный wire format не менялся.
- TypeScript/Vite production build и mock Tauri UI-проверки доменных правил/macOS прошли. В UI-проверку добавлены состояния startup/reconnect/stopping и доставка Stop в IPC.
- Android native ARM64/x86_64 собраны штатным build script. Финальный Gradle-прогон `testDebugUnitTest assembleDebug lintDebug assembleDebugAndroidTest` успешен: **6 unit passed**, lint **0 ошибок / 12 предупреждений**. Instrumentation APK с fault injection скомпилирован, но на устройстве не запускался. APK проходит проверку двух ABI, 16 KiB ELF alignment, лицензий и подписи.
- `git diff --check` и actionlint для изменённых workflows проходят.

Отдельная дополнительная Windows-сборка без feature `embedded-ssh` остановлена из-за нехватки места;
успешная проверка Windows выше относится к варианту с встроенным SSH.

## Локальные сборки

Эти результаты и SHA256 относятся к первой сборке стабилизации (`db6986f`). Рабочие пути APK/EXE пересобираются; актуальная следующая итерация описана в [отчёте preflight](AGENT-PREFLIGHT-2026-11-09.md).

- `artifacts/android/SecretTunnel-debug.apk` — тестовый APK с debug-подписью, ARM64/x86_64. SHA256: `086c9bd502f9a51e171b1423cbaf17563f819aa74e754404e1546f3500e6bf74`.
- `src-tauri/target/debug/secret-tunnel.exe` — Windows debug-клиент со встроенными web-ресурсами (`tauri/custom-protocol`), без зависимости от запущенного Vite. Проверена сборка; окно и реальный перехват в этой итерации не запускались. Это не новый подписанный установщик.
- Логи финальных Windows проверок: `artifacts/stabilization-2026-11-09/`. Android unit/lint: `android/app/build/test-results/testDebugUnitTest/`, `android/app/build/reports/lint-results-debug.html`.

## Границы проверки и следующий шаг

Unit/loopback тесты не подтверждают качество звонка, расход батареи или работу recovery на телефоне.
В этой итерации не запускались реальные Windows interception, macOS Network Extension и Android VPN на устройстве.
В Android instrumentation fixture добавлена опция `injectEngineCrash=true` для следующего запуска
`VpnFixtureTest.runVpnTrafficSession` с `runVpnIntegration=true` и изолированным SSH fixture.
Она проверяет перезапуск engine с сохранением активной VPN-сессии; независимые TCP/UDP probes остаются обязательными.

До объявления первого этапа завершённым нужны preflight старого/отсутствующего агента с понятным следующим
действием, реальные Stop/reconnect/engine-crash сценарии и длительный замер RSS/PSS/FD.
Следующий функциональный шаг — диагностика возможностей агента и безопасный мастер установки в пользовательский
каталог. Затем — явная политика block/direct и восстановление перехвата. Старые SSH-службы, Xray и wstunnel не менялись.
