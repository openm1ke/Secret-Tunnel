# Окно потока на полном пути: SOCKS → stream-mux → russh → sshd → агент

Скрипты для [независимого аудита](../../CLAUDE-INDEPENDENT-AUDIT.md). Используются только loopback, отдельный экземпляр `sshd` на непривилегированном порту и одноразовые ключи. Пользовательские ключи, `~/.ssh` и боевые серверы не затрагиваются. Агент запускается из временного каталога через `SetEnv HOME=...`.

## Фикстура

```bash
F=/tmp/st-audit-fixture && mkdir -p $F/home/.local/bin
cargo build --release --locked --manifest-path udp-agent/Cargo.toml
cargo build --release --locked --manifest-path tunnel-core/Cargo.toml --example check_route
cp udp-agent/target/release/secret-tunnel-agent $F/home/.local/bin/
ssh-keygen -q -t ed25519 -N '' -f $F/host_key
ssh-keygen -q -t ed25519 -N '' -f $F/client_key
cp $F/client_key.pub $F/authorized_keys
cat > $F/sshd_config <<EOF
Port 22990
ListenAddress 127.0.0.1
HostKey $F/host_key
PidFile $F/sshd.pid
AuthorizedKeysFile $F/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
SetEnv HOME=$F/home
EOF
/usr/sbin/sshd -D -e -f $F/sshd_config &
```

`sshd` без root принимает вход только текущего пользователя. После замеров остановите его: `kill $(cat $F/sshd.pid)`.

## Замеры

```bash
FP=$(ssh-keygen -lf $F/host_key.pub -E sha256 | awk '{print $2}')
# Задержка без ограничения скорости: проверка предела «окно / RTT».
python3 docs/audit-repros/e2e-window/e2e_window.py \
  --probe tunnel-core/target/release/examples/check_route \
  --ssh-port 22990 --user "$USER" --key $F/client_key --fingerprint "$FP" \
  --one-way-ms 25,50,100 --flows 1,4
# Без задержки: потолок по процессору и цена в CPU-секундах.
python3 docs/audit-repros/e2e-window/e2e_window.py --direct \
  --probe tunnel-core/target/release/examples/check_route \
  --ssh-port 22990 --user "$USER" --key $F/client_key --fingerprint "$FP" \
  --flows 1,4,16 --seconds 10 --warmup 2 --cpu-pids $(cat $F/sshd.pid)
# Задержка второй из двух мелких записей (проверка влияния Nagle на локальном SOCKS).
python3 docs/audit-repros/e2e-window/small_write_latency.py SOCKS_PORT
# Время жизни UDP-ассоциации: DNS закрывается через ~5 с после ответа, остальные нет.
python3 docs/audit-repros/e2e-window/dns_association_lifetime.py SOCKS_PORT dns
python3 docs/audit-repros/e2e-window/dns_association_lifetime.py SOCKS_PORT echo UDP_PORT
```

Режим `dns` отправляет один настоящий DNS-запрос на 1.1.1.1 через агент фикстуры. Режим `echo` использует UDP-порт `benchmarks/ssh-transport/echo-server.py`.

С параметром `--dns-lookups N` во время каждого замера каждые 2 секунды выполняются N настоящих DNS-запросов к 1.1.1.1, каждый через свою UDP-ассоциацию, как за TUN-адаптером.

С параметром `--tcp-ping` во время замера отдельное TCP-соединение каждые 100 мс отправляет 16 байт на локальный эхо-сервер, как замер «пинга под нагрузкой» в Speedtest. В строку результата добавляются его медиана и p95, доля кадров, ждавших в мультиплексоре загружающей стороны больше 100 мс, бюджет TCP и задержка пути, по которой он регулируется.

С параметром `--mbit N` прокси дополнительно ограничивает скорость канала в каждую сторону и сообщает наибольшее время ожидания в своей очереди. Очередь не ограничена, как у канала с глубоким буфером, а фактическая скорость составляет около 80% номинала.

`e2e_window.py` сам запускает `check_route`, прокси задержки и локальные источник и приёмник TCP. Прокси добавляет одинаковую задержку в обе стороны к зашифрованному потоку SSH, не ограничивает скорость и не теряет байты. Это не эмулятор потерь пакетов. Для `small_write_latency.py` нужен уже запущенный `check_route` с SOCKS на указанном порту.

Тот же `sshd` годится для стенда команды `benchmarks/ssh-transport/matrix.py`: в его конфигурации укажите `127.0.0.1:22990`, одноразовый ключ и порты `echo-server.py`.

Результаты прогона 10 ноября 2026 сохранены в [JSON](../../benchmarks/claude-audit-2026-11-10.json), разделы `e2e_window`, `e2e_direct` и `team_matrix_on_loopback_fixture`.
