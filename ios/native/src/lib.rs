//! C API for the iOS app and its Packet Tunnel extension.
//!
//! The extension starts the shared Rust SSH core as a loopback SOCKS5 server and
//! hev-socks5-tunnel forwards utun packets into it, the same chain the Android
//! VpnService uses. Sockets opened by a Packet Tunnel provider are not routed
//! through its own tunnel, so no socket protection callback is needed here.
//!
//! Every function returning `*mut c_char` hands ownership to the caller, which
//! must release it with `st_ios_free`.

use secret_tunnel_core::embedded_ssh::{
    self, EmbeddedJump, EmbeddedOptions, EmbeddedProxy, EmbeddedServer, EmbeddedTunnelHandle,
    FlowWindow,
};
use serde::Deserialize;
use serde_json::json;
use std::ffi::{c_char, CStr, CString};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};

#[derive(Deserialize)]
struct Config {
    #[serde(default = "ios_name")]
    name: String,
    host: String,
    port: u16,
    user: String,
    key_path: String,
    #[serde(default)]
    host_key_sha256: String,
    #[serde(default)]
    socks_port: u16,
    #[serde(default)]
    ssh_jumps: Vec<JumpConfig>,
    check_agent: Option<bool>,
    #[serde(default)]
    new_session: bool,
    #[serde(default)]
    max_connections: usize,
    #[serde(default)]
    ssh_tuning: embedded_ssh::SshTuning,
}

fn ios_name() -> String {
    "iOS".into()
}

#[derive(Deserialize)]
struct JumpConfig {
    name: String,
    host: String,
    port: u16,
    user: String,
    key_path: String,
    host_key_sha256: String,
}

#[derive(Deserialize)]
struct ProbeConfig {
    host: String,
    port: u16,
    #[serde(default)]
    ssh_jumps: Vec<JumpConfig>,
}

fn ssh_jumps(jumps: Vec<JumpConfig>) -> Result<Vec<EmbeddedJump>, String> {
    if jumps.len() > embedded_ssh::MAX_SSH_JUMPS {
        return Err("Слишком много промежуточных SSH-серверов".into());
    }
    jumps
        .into_iter()
        .map(|jump| {
            if jump.host.trim().is_empty()
                || jump.user.trim().is_empty()
                || jump.port == 0
                || jump.key_path.is_empty()
                || jump.host_key_sha256.is_empty()
            {
                return Err(format!(
                    "Настройте адрес, ключ и отпечаток сервера «{}»",
                    jump.name
                ));
            }
            Ok(EmbeddedJump {
                server: EmbeddedServer {
                    name: jump.name,
                    host: jump.host,
                    port: jump.port,
                    user: jump.user,
                    key_path: jump.key_path,
                    strict_host_key_checking: "yes".into(),
                },
                host_key_sha256: Some(jump.host_key_sha256),
            })
        })
        .collect()
}

fn server(cfg_name: String, host: String, port: u16, user: String, key_path: String) -> EmbeddedServer {
    EmbeddedServer {
        name: cfg_name,
        host,
        port,
        user,
        key_path,
        strict_host_key_checking: "yes".into(),
    }
}

#[derive(Default)]
struct State {
    tunnel: Option<EmbeddedTunnelHandle>,
    starting: bool,
    error: String,
    totals: (u64, u64),
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static LOGS: OnceLock<Arc<Mutex<Vec<String>>>> = OnceLock::new();
static OBSERVED: OnceLock<Arc<Mutex<Option<String>>>> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn state() -> &'static Mutex<State> {
    STATE.get_or_init(Default::default)
}
fn logs() -> Arc<Mutex<Vec<String>>> {
    LOGS.get_or_init(Default::default).clone()
}
fn observed() -> Arc<Mutex<Option<String>>> {
    OBSERVED.get_or_init(Default::default).clone()
}

fn input(ptr: *const c_char) -> Result<String, String> {
    if ptr.is_null() {
        return Err("Пустая конфигурация".into());
    }
    // SAFETY: the caller passes a valid NUL-terminated string that outlives this call.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map(str::to_owned)
        .map_err(|e| e.to_string())
}

fn output(value: String) -> *mut c_char {
    CString::new(value.replace('\0', " "))
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

fn guarded(f: impl FnOnce() -> String) -> *mut c_char {
    let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .unwrap_or_else(|_| json!({"error": "Внутренняя ошибка Rust"}).to_string());
    output(value)
}

fn start(raw: String) -> Result<(), String> {
    let cfg: Config = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    if cfg.host.trim().is_empty() || cfg.user.trim().is_empty() || cfg.port == 0 || cfg.socks_port == 0 {
        return Err("Укажите сервер, пользователя и порты 1–65535".into());
    }
    if cfg.host_key_sha256.is_empty() {
        return Err("Сначала подтвердите отпечаток ключа сервера".into());
    }
    let jumps = ssh_jumps(cfg.ssh_jumps)?;
    {
        let mut s = state().lock().map_err(|e| e.to_string())?;
        if s.starting || s.tunnel.is_some() {
            return Err("Туннель уже запущен".into());
        }
        s.starting = true;
        s.error.clear();
        if cfg.new_session {
            s.totals = (0, 0);
        }
    }
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    if cfg.new_session {
        if let Ok(mut l) = logs().lock() {
            l.clear();
        }
    }
    if let Ok(mut o) = observed().lock() {
        *o = None;
    }
    // Network Extension memory is tight on iOS: keep UDP buffers and session caps modest.
    // Listen on IPv6 loopback: hev-socks5-tunnel always uses AF_INET6 sockets, and inside
    // the extension (with ::/0 routed into utun) a v4-mapped ::ffff:127.0.0.1 connect fails.
    let result = embedded_ssh::start_with_options_cancellable(
        server(cfg.name, cfg.host, cfg.port, cfg.user, cfg.key_path),
        EmbeddedProxy {
            host: "::1".into(),
            port: cfg.socks_port,
        },
        logs(),
        EmbeddedOptions {
            host_key_sha256: Some(cfg.host_key_sha256),
            observed_host_key: Some(observed()),
            protect_socket: None,
            jumps,
            // The extension's memory limit is tens of MB: keep the 2 MiB SSH
            // window here even though desktop and Android default to 4 MiB.
            ssh_tuning: embedded_ssh::SshTuning {
                channel_receive_window_bytes: cfg
                    .ssh_tuning
                    .channel_receive_window_bytes
                    .min(2 * 1024 * 1024),
                // A stalled consumer may hold one window per flow inside the
                // extension's ~50 MB memory limit.
                tcp_receive_window_bytes: cfg.ssh_tuning.tcp_receive_window_bytes.min(256 * 1024),
                ..cfg.ssh_tuning
            },
            probe_ipv6: true,
            // Extension memory is tight, so UDP uses the TCP window size. Agents built
            // before the receive budget was sized for the largest window still reserve
            // 128 KiB per UDP flow out of `limit × 64 KiB`; see max_connections below.
            udp_receive_window: Some(FlowWindow {
                bytes: 64 * 1024,
                slots: 64,
            }),
            cap_default_udp_receive_slots: true,
            agent_udp_receive_window: None,
                bind_physical_interface: false,
                route_note: None,
            // The receive budget is accounting, not preallocated memory. With an agent
            // older than the budget fix, half of this limit is usable for UDP flows, so
            // a small cap made it reject new flows during page loads on the device.
            max_connections: if cfg.max_connections == 0 {
                1024
            } else {
                cfg.max_connections.clamp(32, 1024)
            },
            ..Default::default()
        },
        &|| GENERATION.load(Ordering::SeqCst) != generation,
    );
    let mut s = state().lock().map_err(|e| e.to_string())?;
    s.starting = false;
    match result {
        Ok(tunnel) if GENERATION.load(Ordering::SeqCst) == generation => {
            s.tunnel = Some(tunnel);
            Ok(())
        }
        Ok(tunnel) => {
            drop(s);
            tunnel.stop();
            Err("Запуск отменён".into())
        }
        Err(error) => {
            s.error = error.clone();
            secret_tunnel_core::log_line(&logs(), format!("SSH: {error}"));
            Err(error)
        }
    }
}

/// Starts SSH and the loopback SOCKS5 server. Returns "" on success or an error message.
#[no_mangle]
pub extern "C" fn st_ios_start(config_json: *const c_char) -> *mut c_char {
    let raw = input(config_json);
    let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| raw.and_then(start)))
        .unwrap_or_else(|_| Err("Внутренняя ошибка Rust SSH".into()));
    output(value.err().unwrap_or_default())
}

/// Stops the tunnel (or cancels a start in progress).
#[no_mangle]
pub extern "C" fn st_ios_stop() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    let tunnel = {
        let Ok(mut s) = state().lock() else { return };
        let t = s.tunnel.take();
        if let Some(t) = &t {
            let (rx, tx) = t.traffic_totals();
            s.totals.0 = s.totals.0.saturating_add(rx);
            s.totals.1 = s.totals.1.saturating_add(tx);
        }
        t
    };
    if let Some(tunnel) = tunnel {
        tunnel.stop();
    }
}

/// JSON status: running, error, rx/tx byte totals, recent log lines.
#[no_mangle]
pub extern "C" fn st_ios_status() -> *mut c_char {
    guarded(|| {
        let Ok(mut s) = state().lock() else {
            return json!({"error": "state lock"}).to_string();
        };
        if let Some(error) = s.tunnel.as_ref().and_then(|t| t.try_wait()) {
            s.error = error;
        }
        let current = s.tunnel.as_ref().map(|t| t.traffic_totals()).unwrap_or_default();
        let logs = logs().lock().map(|l| l.clone()).unwrap_or_default();
        let tail: Vec<String> = logs.iter().rev().take(80).rev().cloned().collect();
        json!({
            "running": s.tunnel.is_some() && s.error.is_empty(),
            "starting": s.starting,
            "error": s.error,
            "rx": s.totals.0.saturating_add(current.0),
            "tx": s.totals.1.saturating_add(current.1),
            "agent": s.tunnel.as_ref().map(|t| t.agent_summary()),
            "ipv6": s.tunnel.as_ref().and_then(|t| t.ipv6_status()),
            "observed_host_key": observed().lock().map(|o| o.clone()).unwrap_or_default(),
            "logs": tail,
        })
        .to_string()
    })
}

/// Fetches the server host key fingerprint: {"fingerprint": "..."} or {"error": "..."}.
#[no_mangle]
pub extern "C" fn st_ios_probe(config_json: *const c_char) -> *mut c_char {
    let raw = input(config_json);
    guarded(|| {
        let result = (|| -> Result<String, String> {
            let cfg: ProbeConfig = serde_json::from_str(&raw?).map_err(|e| e.to_string())?;
            if cfg.host.trim().is_empty() || cfg.port == 0 {
                return Err("Укажите адрес и порт сервера".into());
            }
            embedded_ssh::probe_server(&cfg.host, cfg.port, ssh_jumps(cfg.ssh_jumps)?)
        })();
        match result {
            Ok(pin) => json!({"fingerprint": pin}).to_string(),
            Err(e) => json!({"error": embedded_ssh::setup::friendly_error(&e)}).to_string(),
        }
    })
}

/// Logs in with the key and checks the server agent: {"ok": bool, "summary"|"error": "..."}.
#[no_mangle]
pub extern "C" fn st_ios_check(config_json: *const c_char) -> *mut c_char {
    let raw = input(config_json);
    guarded(|| {
        let result = (|| -> Result<String, String> {
            let cfg: Config = serde_json::from_str(&raw?).map_err(|e| e.to_string())?;
            if cfg.host.trim().is_empty() || cfg.user.trim().is_empty() || cfg.port == 0 || cfg.host_key_sha256.is_empty() {
                return Err("Укажите сервер, ключ и подтверждённый отпечаток".into());
            }
            embedded_ssh::diagnose_server_with_options(
                server(cfg.name, cfg.host, cfg.port, cfg.user, cfg.key_path),
                EmbeddedOptions {
                    host_key_sha256: Some(cfg.host_key_sha256),
                    jumps: ssh_jumps(cfg.ssh_jumps)?,
                    ssh_tuning: cfg.ssh_tuning,
                    ..Default::default()
                },
                cfg.check_agent.unwrap_or(true),
            )
        })();
        match result {
            Ok(summary) => json!({"ok": true, "summary": summary}).to_string(),
            Err(error) => json!({"ok": false, "error": embedded_ssh::setup::friendly_error(&error)}).to_string(),
        }
    })
}

/// Generates a separate device key without persisting it.
#[no_mangle]
pub extern "C" fn st_ios_transfer_export(document_json: *const c_char) -> *mut c_char {
    let raw = input(document_json);
    guarded(|| match raw.and_then(|raw| secret_tunnel_core::profile_exchange::export(&raw)) {
        Ok(value) => value,
        Err(error) => json!({"error":error}).to_string(),
    })
}

#[no_mangle]
pub extern "C" fn st_ios_transfer_unlock(payload: *const c_char, code: *const c_char, preview: i32) -> *mut c_char {
    let payload = input(payload); let code = input(code);
    guarded(|| match payload.and_then(|payload| code.and_then(|code| secret_tunnel_core::profile_exchange::open(&payload, &code, preview != 0))) {
        Ok(value) => value,
        Err(error) => json!({"error":error}).to_string(),
    })
}

/// Generates a separate device key without persisting it.
#[no_mangle]
pub extern "C" fn st_ios_generate_key() -> *mut c_char {
    guarded(|| match embedded_ssh::setup::generate_key() {
        Ok(key) => json!({"private": key}).to_string(),
        Err(error) => json!({"error": error}).to_string(),
    })
}

#[no_mangle]
pub extern "C" fn st_ios_enroll_password(config_json: *const c_char, password: *const c_char) -> *mut c_char {
    let raw = input(config_json);
    let password = input(password);
    guarded(|| {
        let result = (|| -> Result<(), String> {
            let cfg: Config = serde_json::from_str(&raw?).map_err(|e| e.to_string())?;
            if cfg.host.trim().is_empty() || cfg.user.trim().is_empty() || cfg.port == 0 || !cfg.ssh_jumps.is_empty() {
                return Err("Укажите адрес, SSH-порт и пользователя для прямого подключения".into());
            }
            embedded_ssh::setup::enroll_password(
                server(cfg.name, cfg.host, cfg.port, cfg.user, cfg.key_path),
                cfg.host_key_sha256, password?, false,
            )
        })();
        match result {
            Ok(()) => json!({"ok": true}).to_string(),
            Err(error) => json!({"error": embedded_ssh::setup::friendly_error(&error)}).to_string(),
        }
    })
}

#[no_mangle]
pub extern "C" fn st_ios_install_agent(config_json: *const c_char, version: *const c_char, archive_path: *const c_char) -> *mut c_char {
    let raw = input(config_json);
    let version = input(version);
    let archive = input(archive_path);
    guarded(|| {
        let result = (|| -> Result<(), String> {
            use secret_tunnel_core::agent_update;
            use std::io::Read;
            let cfg: Config = serde_json::from_str(&raw?).map_err(|e| e.to_string())?;
            let version = version?;
            if !agent_update::valid_version(&version) || cfg.host_key_sha256.is_empty() {
                return Err("Укажите версию агента и подтверждённый отпечаток сервера".into());
            }
            let mut bytes = Vec::new();
            std::fs::File::open(archive?).map_err(|e| e.to_string())?
                .take(32 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            if bytes.is_empty() || bytes.len() > 32 * 1024 * 1024 {
                return Err("Некорректный размер архива агента".into());
            }
            let (status, stdout, _) = embedded_ssh::run_on_server_blocking(
                server(cfg.name, cfg.host, cfg.port, cfg.user, cfg.key_path),
                EmbeddedOptions { host_key_sha256: Some(cfg.host_key_sha256), jumps: ssh_jumps(cfg.ssh_jumps)?, ..Default::default() },
                &agent_update::agent_install_command(&version), bytes, std::time::Duration::from_secs(90),
            )?;
            if status != 0 || agent_update::agent_version_from_output(&stdout)? != Some(version.as_str()) {
                return Err("Не удалось установить агент. Требуется Linux x86_64 и доступ на запись в домашнюю папку.".into());
            }
            Ok(())
        })();
        match result {
            Ok(()) => json!({"ok": true}).to_string(),
            Err(error) => json!({"error": embedded_ssh::setup::friendly_error(&error)}).to_string(),
        }
    })
}

/// Releases a string returned by this library.
#[no_mangle]
pub extern "C" fn st_ios_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        // SAFETY: ptr was produced by CString::into_raw in this library.
        drop(unsafe { CString::from_raw(ptr) });
    }
}
