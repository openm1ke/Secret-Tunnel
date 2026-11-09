use secret_tunnel_mux::Snapshot;

#[derive(Default)]
pub struct DiagnosticLog {
    #[cfg(unix)]
    socket: Option<std::os::unix::net::UnixDatagram>,
}

impl DiagnosticLog {
    pub fn new(enabled: bool) -> Self {
        let mut log = Self::default();
        #[cfg(unix)]
        if enabled {
            log.socket = std::os::unix::net::UnixDatagram::unbound()
                .and_then(|socket| {
                    socket.set_nonblocking(true)?;
                    socket.connect("/dev/log")?;
                    Ok(socket)
                })
                .ok();
        }
        log
    }

    pub fn report(&self, status: &Snapshot) {
        let payload = serde_json::json!({
            "active": status.active,
            "udp_drops": status.udp_drops,
            "send_queue_bytes": status.send_queue_bytes,
            "receive_queue_bytes": status.receive_queue_bytes,
            "metrics": status.diagnostics,
        });
        #[cfg(unix)]
        if let Some(socket) = &self.socket {
            let line = format!("<14>secret-tunnel-agent[{}]: {payload}", std::process::id());
            let _ = socket.send(line.as_bytes());
            return;
        }
        eprintln!("secret-tunnel-agent: {payload}");
    }
}
