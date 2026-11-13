# SSH transport comparison

`matrix.py` compares the actual shared embedded SSH engine with the same TCP
and UDP workload, directly and through configured jump hosts. It changes only
the requested SSH queue size and optional TCP budget. The cipher is Auto,
the receive window is 2 MiB, and UDP windows come from the input configuration.

The local bridge adds a pipelined propagation delay and a rate limit to the
physical entry byte stream. It never drops encrypted bytes. It is **not** an
IP packet-loss emulator or a replacement for real mobile testing. Real network
conditions beyond this bridge can still vary. The explicit bridge queue is
bounded at 64 KiB per direction, in addition to stream/OS buffers.

Defaults:

| Scenario | Added delay, each direction | Rate limit, each direction |
|---|---:|---:|
| unshaped | 0 | none |
| mobile | 50 ms | 5 Mbit/s |
| long | 150 ms | 2 Mbit/s |

Each measurement concurrently downloads two deterministic, hash-checked 2 MiB
TCP bodies and sends 160-byte UDP echoes at 50 packets/second for 12 seconds.
The report separates all received echoes from those received within 700 ms.
It records full-route heartbeats and both client/server mux diagnostics.
Quantiles over received responses do not include missing responses.
`voice_during_tcp` separately reports packets sent between TCP start and the
last completed body. This excludes both the warmup and the unloaded tail.
`--tcp-streams 16` splits the same total 4 MiB over sixteen 256 KiB downloads;
this exercises more concurrent flow windows without increasing total bytes.
`--tcp-budget-kib 64` negotiates the per-session budget with a QoS-capable
agent. The default `0` leaves the TCP budget disabled. `--voice-lead-seconds 3`
starts the UDP stream three seconds before the TCP downloads. Keep both flags
fixed across repeats and use a separate output directory per combination.

Queue order reverses between repeats, and adjacent pairs use the same route
and impairment. The reports are experiments, not a guarantee that a profile is
optimal on every network. Build activity should finish before measuring.

## Run

1. Build `cargo build --manifest-path tunnel-core/Cargo.toml --release --example check_route --locked`.
2. Start `echo-server.py --seconds 1800` as your SSH user on the destination.
   It binds only loopback, prints TCP/UDP ports and PID, creates no files or
   services, and exits at its deadline. Keep its SSH session open during the
   measurement and stop that helper afterwards. Do not kill unrelated agents.
3. Create an **untracked** JSON file from this shape, using the printed ports:

```json
{
  "server": {
    "name": "Exit", "host": "exit.example", "port": 2222,
    "user": "my-user", "key_path": "/private/path/device-key",
    "host_key_sha256": "SHA256:verified-exit-fingerprint"
  },
  "jumps": [{
    "name": "Entry", "host": "entry.example", "port": 2222,
    "user": "my-user", "key_path": "/private/path/entry-key",
    "host_key_sha256": "SHA256:verified-entry-fingerprint"
  }],
  "udp_receive_window": {"bytes": 262144, "slots": 192},
  "targets": {"host": "127.0.0.1", "tcp_port": 12345, "udp_port": 23456}
}
```

The proxy changes the entry address to loopback but retains its explicit SSH
fingerprint. Both server fingerprints must be independently verified.

4. Run, with no other listener on SOCKS 1082:

```sh
python benchmarks/ssh-transport/matrix.py \
  --probe tunnel-core/target/release/examples/check_route \
  --config artifacts/my-matrix.json --output artifacts/matrix --repeats 2
```

On Windows the probe path ends in `.exe`. Use `--routes chain` if the exit
cannot be reached directly. `--scenarios mobile,long`, `--duration 12`,
`--queues 32,8` and `--first-repeat 2` can select/resume a bounded series.
Output includes local configurations containing key **paths**, traces and JSON
metrics; keep it outside tracked source directories. Private key contents are
never read by the Python benchmark.

The benchmark closes the probe's stdin to request normal SSH shutdown; a hung
shutdown fails the run rather than silently leaving it successful.

The shared example uses ring. The production Windows GUI uses aws-lc, so a
separate GUI smoke test and Android per-app VPN test are still required.

## One versus two SSH connections on Linux

`session-load.py` starts the real embedded client and server agent on two
private network namespaces joined by a veth pair. Both SSH connections share
one `netem` qdisc per direction. The direct topology uses one SSH hop. The
chain adds an SSH forwarding hop on server loopback, with the impairment
applied to the outer client-to-entry TCP link. A disposable sshd and temporary
keys are created for each invocation. All payload endpoints bind to loopback.

Run in a disposable Linux VM with Python 3.9+, OpenSSH server and client,
`iproute2`, `ethtool`, `unshare`, and `nsenter`. Use Linux builds of the
client example and agent. OpenSSH needs its usual runtime directory
(`/run/sshd` on Ubuntu). The benchmark needs root for namespace and qdisc
setup. The output directory must be new.

```sh
cargo build --manifest-path tunnel-core/Cargo.toml --release --example check_route --locked
cargo build --manifest-path udp-agent/Cargo.toml --release --locked
sudo unshare --net -- python3 benchmarks/ssh-transport/session-load.py \
  --probe tunnel-core/target/release/examples/check_route \
  --agent udp-agent/target/release/secret-tunnel-agent \
  --output /tmp/secret-tunnel-ssh-load \
  --scenarios lossy --routes chain --loads idle download upload udp udp-shared \
  --bulk-udp-pps 600 --repeats 2
```

The `lossy` profile adds 50 ms one way, 10 ms of jitter, 0.3% random packet
loss and a 5 Mbit/s limit per direction. `clean` removes jitter and loss.
`narrow` uses 100 ms one way, 20 ms of jitter, 0.3% loss and 2 Mbit/s.
The voice model is 160-byte UDP echoes at 50 packets/s. The benchmark counts
missing echoes and echoes above the deadline (200 ms RTT, 350 ms for
`narrow`) in addition to quantiles over received packets. For TCP it includes
only packets sent before the last transfer completes. Four downloads or
uploads verify their payloads by SHA-256. `udp` offers a separate 1200-byte
echo workload at the specified packet rate. `udp-shared` deliberately puts both
UDP streams on the second SSH connection. At 600 packets/s, the offered UDP
payload alone is 5.76 Mbit/s in each direction if all packets are echoed.

The output records mux queue peaks, Linux TCP counters, and sampled qdisc
backlog. Netem's backlog includes packets waiting for the configured
propagation delay. The test measures transport under controlled load. Actual
voice codec behavior, mobile radio and Android VPN routing require device tests.

## Agent DNS isolation

On Linux or WSL, run the production agent locally without SSH:

```sh
python3 benchmarks/ssh-transport/agent-dns-isolation.py \
  udp-agent/target/x86_64-unknown-linux-musl/release/secret-tunnel-agent
```

The command needs `unshare`, `ip`, `mount` and working user namespaces. It
starts loopback-only DNS and UDP echo in private network and mount namespaces.
It supplies the resolver configuration only inside that namespace. The fixture
sends IP-addressed packets in two associations while one waits on a deliberately
slow domain lookup. It also checks whether a delayed positive DNS answer leads
to UDP egress after the agent processed `Abort`. The JSON is a measurement and
reports the observed behavior, including `udp_sent_after_abort`.
