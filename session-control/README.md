# Session control

Small dependency-free desktop desired-state controller and atomic file replacement.

The controller stores the immutable profile snapshot for one session generation.
Stop invalidates that generation immediately; a worker must publish a ready runtime
through `publish` so a stale startup cannot install itself after cancellation.
Retry deadlines are monotonic and polled by the supervisor, independently of UI IPC.
Network operations and teardown never run inside the controller's install closure.

`storage::replace` writes and syncs a same-directory temporary file, then replaces
the destination atomically. Windows sharing violations are retried for at most
500 ms while retaining the previous file. Configuration read-modify-write locking
and backup selection belong to the desktop configuration repository.

```sh
cargo test --manifest-path session-control/Cargo.toml --locked
```

Tests cover Stop during backoff, stale completion, repeated retries, terminal
failures, and simultaneous readers during replacement on Windows/Linux.
