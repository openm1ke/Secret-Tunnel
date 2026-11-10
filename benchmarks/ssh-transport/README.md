# SSH transport comparison

`matrix.py` compares the actual shared embedded SSH engine with the same TCP
and UDP workload, directly and through configured jump hosts. It changes only
the SSH queue size (8/32 messages); the cipher is Auto, receive window 2 MiB,
and UDP windows remain those of the input configuration.

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
`voice_during_tcp` separately reports packets sent before the last TCP body
finished, so an unloaded tail cannot hide latency during bulk transfer.
`--tcp-streams 16` splits the same total 4 MiB over sixteen 256 KiB downloads;
this exercises more concurrent flow windows without increasing total bytes.

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
