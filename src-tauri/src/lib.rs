use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{Manager, State, WindowEvent};

#[cfg(feature = "embedded-ssh")]
mod embedded_ssh;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ServerProfile {
    id: String,
    name: String,
    host: String,
    port: u16,
    user: String,
    key_path: String,
    #[serde(default)]
    compression: bool,
    #[serde(default = "default_strict_host_key_checking")]
    strict_host_key_checking: String,
    #[serde(default = "default_connect_timeout")]
    connect_timeout: u16,
    #[serde(default = "default_server_alive_interval")]
    server_alive_interval: u16,
    #[serde(default = "default_server_alive_count_max")]
    server_alive_count_max: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ProxyProfile {
    id: String,
    name: String,
    host: String,
    port: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AppSettings {
    ipinfo_token: String,
    #[serde(default = "default_ssh_engine")]
    ssh_engine: String,
    #[serde(default = "default_true")]
    hide_sensitive: bool,
    #[serde(default = "default_true")]
    verify_proxy_on_start: bool,
    #[serde(default)]
    auto_reconnect: bool,
    #[serde(default = "default_reconnect_delay_seconds")]
    reconnect_delay_seconds: u16,
    #[serde(default)]
    start_tunnel_on_launch: bool,
    #[serde(default)]
    clear_logs_on_start: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            ipinfo_token: String::new(),
            ssh_engine: default_ssh_engine(),
            hide_sensitive: true,
            verify_proxy_on_start: true,
            auto_reconnect: false,
            reconnect_delay_seconds: default_reconnect_delay_seconds(),
            start_tunnel_on_launch: false,
            clear_logs_on_start: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AppConfig {
    selected_server_id: String,
    selected_proxy_id: String,
    servers: Vec<ServerProfile>,
    proxies: Vec<ProxyProfile>,
    settings: AppSettings,
}

#[derive(Clone, Debug, Default, Serialize)]
struct TunnelStatus {
    state: String,
    state_emoji: String,
    app_version: String,
    ssh_engine: String,
    server_id: String,
    server_name: String,
    proxy_id: String,
    proxy_name: String,
    proxy_host: String,
    proxy_port: u16,
    pid: Option<u32>,
    started_at: Option<u64>,
    ip: String,
    country_code: String,
    country_name: String,
    country_flag: String,
    rx_total: u64,
    tx_total: u64,
    last_error: String,
}

#[derive(Clone, Debug, Default, Serialize)]
struct TrafficPoint {
    ts: u64,
    rx_bps: u64,
    tx_bps: u64,
    rx_total: u64,
    tx_total: u64,
    latency_ms: Option<u64>,
}

#[derive(Clone, Debug, Default)]
struct TrafficSample {
    ts: u64,
    rx_total: u64,
    tx_total: u64,
}

#[derive(Clone, Debug, Default)]
struct IpInfo {
    ip: String,
    country_code: String,
    country_name: String,
}

enum TunnelHandle {
    System(Child),
    #[cfg(feature = "embedded-ssh")]
    Embedded(embedded_ssh::EmbeddedTunnelHandle),
}

struct TunnelRuntime {
    handle: TunnelHandle,
    server: ServerProfile,
    proxy: ProxyProfile,
    started_at: u64,
    last_error: String,
    last_traffic: Option<TrafficSample>,
}

impl TunnelRuntime {
    fn ssh_engine(&self) -> String {
        match &self.handle {
            TunnelHandle::System(_) => "system".to_string(),
            #[cfg(feature = "embedded-ssh")]
            TunnelHandle::Embedded(_) => "embedded".to_string(),
        }
    }

    fn pid(&self) -> Option<u32> {
        match &self.handle {
            TunnelHandle::System(child) => Some(child.id()),
            #[cfg(feature = "embedded-ssh")]
            TunnelHandle::Embedded(_) => None,
        }
    }

    fn try_wait(&mut self) -> Result<Option<String>, String> {
        match &mut self.handle {
            TunnelHandle::System(child) => child
                .try_wait()
                .map(|status| status.map(|status| format!("ssh exited with {status}")))
                .map_err(|error| error.to_string()),
            #[cfg(feature = "embedded-ssh")]
            TunnelHandle::Embedded(_) => Ok(None),
        }
    }

    fn traffic_totals(&self) -> Result<Option<(u64, u64)>, String> {
        match &self.handle {
            TunnelHandle::System(child) => sample_process_traffic(child.id()),
            #[cfg(feature = "embedded-ssh")]
            TunnelHandle::Embedded(handle) => Ok(Some(handle.traffic_totals())),
        }
    }

    fn stop(self, logs: &Arc<Mutex<Vec<String>>>) {
        match self.handle {
            TunnelHandle::System(mut child) => {
                log_line(
                    logs,
                    format!("Stopping tunnel '{}', pid={}", self.server.name, child.id()),
                );
                let _ = child.kill();
                let _ = child.wait();
            }
            #[cfg(feature = "embedded-ssh")]
            TunnelHandle::Embedded(handle) => {
                log_line(
                    logs,
                    format!("Stopping embedded tunnel '{}'", self.server.name),
                );
                handle.stop();
            }
        }
        log_line(logs, "Tunnel stopped");
    }
}

fn default_true() -> bool {
    true
}

fn default_ssh_engine() -> String {
    "system".to_string()
}

fn default_strict_host_key_checking() -> String {
    "yes".to_string()
}

fn default_connect_timeout() -> u16 {
    8
}

fn default_server_alive_interval() -> u16 {
    15
}

fn default_server_alive_count_max() -> u16 {
    3
}

fn default_reconnect_delay_seconds() -> u16 {
    3
}

#[derive(Default)]
struct AppState {
    runtime: Mutex<Option<TunnelRuntime>>,
    logs: Arc<Mutex<Vec<String>>>,
    traffic: Mutex<Vec<TrafficPoint>>,
    ip_cache: Mutex<Option<(u64, String, IpInfo)>>,
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn log_line(logs: &Arc<Mutex<Vec<String>>>, message: impl Into<String>) {
    let line = format!("[{}] {}", now_epoch(), message.into());
    if let Ok(mut items) = logs.lock() {
        items.push(line);
        if items.len() > 1000 {
            let overflow = items.len() - 1000;
            items.drain(0..overflow);
        }
    }
}

fn config_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    Ok(Path::new(&home)
        .join(".config")
        .join("secret-tunnel")
        .join("config.json"))
}

fn legacy_config_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    Ok(Path::new(&home)
        .join(".config")
        .join("fr-tunnel-desktop")
        .join("config.json"))
}

fn default_config() -> AppConfig {
    AppConfig {
        selected_server_id: "example".to_string(),
        selected_proxy_id: "local-1080".to_string(),
        servers: vec![ServerProfile {
            id: "example".to_string(),
            name: "Example server".to_string(),
            host: "example.com".to_string(),
            port: 22,
            user: "user".to_string(),
            key_path: "~/.ssh/id_ed25519".to_string(),
            compression: false,
            strict_host_key_checking: default_strict_host_key_checking(),
            connect_timeout: default_connect_timeout(),
            server_alive_interval: default_server_alive_interval(),
            server_alive_count_max: default_server_alive_count_max(),
        }],
        proxies: vec![ProxyProfile {
            id: "local-1080".to_string(),
            name: "Local 1080".to_string(),
            host: "127.0.0.1".to_string(),
            port: 1080,
        }],
        settings: AppSettings::default(),
    }
}

fn parse_config(raw: &str) -> Result<AppConfig, String> {
    let value: Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    if value.get("proxies").is_some() {
        return serde_json::from_value(value).map_err(|error| error.to_string());
    }

    let servers: Vec<ServerProfile> = serde_json::from_value(
        value
            .get("servers")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    )
    .map_err(|error| error.to_string())?;

    let proxy_value = value
        .get("proxy")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"host":"127.0.0.1","port":1080}));
    let host = proxy_value
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or("127.0.0.1")
        .to_string();
    let port = proxy_value
        .get("port")
        .and_then(Value::as_u64)
        .unwrap_or(1080) as u16;

    Ok(AppConfig {
        selected_server_id: value
            .get("selected_server_id")
            .and_then(Value::as_str)
            .unwrap_or("fr")
            .to_string(),
        selected_proxy_id: format!("local-{port}"),
        servers,
        proxies: vec![ProxyProfile {
            id: format!("local-{port}"),
            name: format!("Local {port}"),
            host,
            port,
        }],
        settings: AppSettings::default(),
    })
}

fn load_config() -> Result<AppConfig, String> {
    let path = config_path()?;
    if !path.exists() {
        if let Ok(legacy_path) = legacy_config_path() {
            if legacy_path.exists() {
                let raw = fs::read_to_string(legacy_path).map_err(|error| error.to_string())?;
                let config = parse_config(&raw)?;
                save_config(config.clone())?;
                return Ok(config);
            }
        }

        let config = default_config();
        save_config(config.clone())?;
        return Ok(config);
    }

    let raw = fs::read_to_string(path).map_err(|error| error.to_string())?;
    parse_config(&raw)
}

fn save_config(config: AppConfig) -> Result<AppConfig, String> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let raw = serde_json::to_string_pretty(&config).map_err(|error| error.to_string())?;
    fs::write(path, raw).map_err(|error| error.to_string())?;
    Ok(config)
}

fn expand_home(path: &str) -> String {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{stripped}");
        }
    }
    path.to_string()
}

fn validate_server(server: &ServerProfile) -> Result<(), String> {
    if server.id.trim().is_empty() || server.name.trim().is_empty() {
        return Err("Server id and name are required".to_string());
    }
    if server.host.trim().is_empty() || server.user.trim().is_empty() {
        return Err("Server host and SSH user are required".to_string());
    }
    if server.port == 0 {
        return Err("SSH port must be in range 1..65535".to_string());
    }
    if server.key_path.trim().is_empty() {
        return Err("SSH key path is required".to_string());
    }
    if server.connect_timeout == 0 || server.connect_timeout > 120 {
        return Err("Connect timeout must be in range 1..120 seconds".to_string());
    }
    if server.server_alive_interval == 0 || server.server_alive_interval > 300 {
        return Err("ServerAliveInterval must be in range 1..300 seconds".to_string());
    }
    if server.server_alive_count_max == 0 || server.server_alive_count_max > 20 {
        return Err("ServerAliveCountMax must be in range 1..20".to_string());
    }
    match server.strict_host_key_checking.as_str() {
        "yes" | "accept-new" | "no" => {}
        _ => return Err("StrictHostKeyChecking must be yes, accept-new, or no".to_string()),
    }
    Ok(())
}

fn validate_proxy(proxy: &ProxyProfile) -> Result<(), String> {
    if proxy.id.trim().is_empty() || proxy.name.trim().is_empty() {
        return Err("Proxy id and name are required".to_string());
    }
    if proxy.host.trim().is_empty() {
        return Err("Proxy host is required".to_string());
    }
    if proxy.port == 0 {
        return Err("Proxy port must be in range 1..65535".to_string());
    }
    Ok(())
}

fn validate_settings(settings: &AppSettings) -> Result<(), String> {
    match settings.ssh_engine.as_str() {
        "system" | "embedded" => {}
        _ => return Err("SSH engine must be system or embedded".to_string()),
    }
    if settings.reconnect_delay_seconds == 0 || settings.reconnect_delay_seconds > 120 {
        return Err("Reconnect delay must be in range 1..120 seconds".to_string());
    }
    Ok(())
}

fn selected_server(config: &AppConfig) -> Result<ServerProfile, String> {
    config
        .servers
        .iter()
        .find(|server| server.id == config.selected_server_id)
        .cloned()
        .or_else(|| config.servers.first().cloned())
        .ok_or_else(|| "Add at least one server profile".to_string())
}

fn selected_proxy(config: &AppConfig) -> Result<ProxyProfile, String> {
    config
        .proxies
        .iter()
        .find(|proxy| proxy.id == config.selected_proxy_id)
        .cloned()
        .or_else(|| config.proxies.first().cloned())
        .ok_or_else(|| "Add at least one local proxy profile".to_string())
}

fn tcp_connects(host: &str, port: u16, timeout: Duration) -> bool {
    let Ok(addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    addrs
        .into_iter()
        .any(|addr| TcpStream::connect_timeout(&addr, timeout).is_ok())
}

fn tcp_latency_ms(host: &str, port: u16, timeout: Duration) -> Option<u64> {
    let Ok(addrs) = (host, port).to_socket_addrs() else {
        return None;
    };

    for addr in addrs {
        let started = Instant::now();
        if TcpStream::connect_timeout(&addr, timeout).is_ok() {
            return Some(started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
        }
    }
    None
}

fn attach_log_reader(
    label: &'static str,
    reader: impl std::io::Read + Send + 'static,
    logs: Arc<Mutex<Vec<String>>>,
) {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if !line.trim().is_empty() {
                log_line(&logs, format!("{label}: {line}"));
            }
        }
    });
}

#[cfg(target_os = "macos")]
fn sample_process_traffic(pid: u32) -> Result<Option<(u64, u64)>, String> {
    let output = Command::new("nettop")
        .arg("-P")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-L")
        .arg("1")
        .arg("-x")
        .output()
        .map_err(|error| error.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("nettop exited with {}", output.status)
        } else {
            stderr
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut bytes_in_idx = None;
    let mut bytes_out_idx = None;

    for line in stdout.lines() {
        let cols: Vec<&str> = line.split(',').collect();
        if cols.iter().any(|col| *col == "bytes_in") {
            bytes_in_idx = cols.iter().position(|col| *col == "bytes_in");
            bytes_out_idx = cols.iter().position(|col| *col == "bytes_out");
            continue;
        }

        let (Some(in_idx), Some(out_idx)) = (bytes_in_idx, bytes_out_idx) else {
            continue;
        };
        if cols.len() <= in_idx || cols.len() <= out_idx {
            continue;
        }

        let rx = cols[in_idx].trim().parse::<u64>().ok();
        let tx = cols[out_idx].trim().parse::<u64>().ok();
        if let (Some(rx), Some(tx)) = (rx, tx) {
            return Ok(Some((rx, tx)));
        }
    }
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
fn sample_process_traffic(_pid: u32) -> Result<Option<(u64, u64)>, String> {
    Ok(None)
}

fn update_traffic(state: &State<'_, AppState>, runtime: &mut TunnelRuntime) {
    let now = now_epoch();
    match runtime.traffic_totals() {
        Ok(Some((rx_total, tx_total))) => {
            if let Some(previous) = &runtime.last_traffic {
                let elapsed = now.saturating_sub(previous.ts).max(1);
                let rx_bps = rx_total.saturating_sub(previous.rx_total) / elapsed;
                let tx_bps = tx_total.saturating_sub(previous.tx_total) / elapsed;
                let latency_ms = tcp_latency_ms(
                    &runtime.server.host,
                    runtime.server.port,
                    Duration::from_secs(2),
                );
                if let Ok(mut traffic) = state.traffic.lock() {
                    traffic.push(TrafficPoint {
                        ts: now,
                        rx_bps,
                        tx_bps,
                        rx_total,
                        tx_total,
                        latency_ms,
                    });
                    if traffic.len() > 120 {
                        let overflow = traffic.len() - 120;
                        traffic.drain(0..overflow);
                    }
                }
            }
            runtime.last_traffic = Some(TrafficSample {
                ts: now,
                rx_total,
                tx_total,
            });
        }
        Ok(None) => {}
        Err(error) => {
            if runtime.last_error != error {
                runtime.last_error = error.clone();
                log_line(&state.logs, format!("Traffic sampler: {error}"));
            }
        }
    }
}

fn country_code_to_flag(cc: &str) -> String {
    let cc = cc.trim().to_uppercase();
    if cc.len() != 2 || !cc.chars().all(|ch| ch.is_ascii_uppercase()) {
        return String::new();
    }
    cc.chars()
        .filter_map(|ch| char::from_u32(127397 + ch as u32))
        .collect()
}

fn fetch_ipinfo(token: &str, proxy: Option<&ProxyProfile>) -> Result<Option<IpInfo>, String> {
    let token = token.trim();
    if token.is_empty() {
        return Ok(None);
    }

    let url = format!("https://api.ipinfo.io/lite/me?token={token}");
    let mut command = Command::new("curl");
    command.arg("-sS").arg("--max-time").arg("6");
    if let Some(proxy) = proxy {
        command
            .arg("--socks5-hostname")
            .arg(format!("{}:{}", proxy.host, proxy.port));
    }
    command.arg(url);

    let output = command.output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("curl exited with {}", output.status)
        } else {
            stderr
        });
    }

    let value: Value = serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    Ok(Some(IpInfo {
        ip: value
            .get("ip")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        country_code: value
            .get("country_code")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        country_name: value
            .get("country")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }))
}

fn verify_proxy(proxy: &ProxyProfile) -> Result<String, String> {
    let output = Command::new("curl")
        .arg("-sS")
        .arg("--max-time")
        .arg("8")
        .arg("--socks5-hostname")
        .arg(format!("{}:{}", proxy.host, proxy.port))
        .arg("https://api.ipify.org")
        .output()
        .map_err(|error| error.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("curl exited with {}", output.status)
        } else {
            stderr
        });
    }

    let ip = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if ip.is_empty() {
        Err("Proxy verification returned empty response".to_string())
    } else {
        Ok(ip)
    }
}

fn status_emoji(state: &str) -> String {
    match state {
        "running" => "🟢",
        "unknown" => "🟡",
        _ => "🔴",
    }
    .to_string()
}

fn totals_from_traffic(state: &State<'_, AppState>) -> (u64, u64) {
    state
        .traffic
        .lock()
        .ok()
        .and_then(|items| items.last().cloned())
        .map(|point| (point.rx_total, point.tx_total))
        .unwrap_or((0, 0))
}

fn build_status(
    state_name: &str,
    runtime: Option<&TunnelRuntime>,
    config: &AppConfig,
    app_state: &State<'_, AppState>,
) -> TunnelStatus {
    let server = runtime
        .map(|runtime| runtime.server.clone())
        .or_else(|| selected_server(config).ok())
        .unwrap_or(ServerProfile {
            id: String::new(),
            name: "No server".to_string(),
            host: String::new(),
            port: 22,
            user: String::new(),
            key_path: String::new(),
            compression: false,
            strict_host_key_checking: default_strict_host_key_checking(),
            connect_timeout: default_connect_timeout(),
            server_alive_interval: default_server_alive_interval(),
            server_alive_count_max: default_server_alive_count_max(),
        });
    let proxy = runtime
        .map(|runtime| runtime.proxy.clone())
        .or_else(|| selected_proxy(config).ok())
        .unwrap_or(ProxyProfile {
            id: String::new(),
            name: "No proxy".to_string(),
            host: String::new(),
            port: 0,
        });

    let (rx_total, tx_total) = totals_from_traffic(app_state);
    let mut ip_info = IpInfo::default();
    if state_name == "running" {
        let cache_key = format!("{}:{}", proxy.host, proxy.port);
        let now = now_epoch();
        if let Ok(mut cache) = app_state.ip_cache.lock() {
            let cached = cache
                .as_ref()
                .filter(|(ts, key, _)| *key == cache_key && now.saturating_sub(*ts) <= 60)
                .map(|(_, _, info)| info.clone());
            if let Some(info) = cached {
                ip_info = info;
            } else {
                match fetch_ipinfo(&config.settings.ipinfo_token, Some(&proxy)) {
                    Ok(Some(info)) => {
                        ip_info = info.clone();
                        *cache = Some((now, cache_key, info));
                    }
                    Ok(None) => {}
                    Err(error) => log_line(&app_state.logs, format!("IPinfo: {error}")),
                }
            }
        }
    }

    TunnelStatus {
        state: state_name.to_string(),
        state_emoji: status_emoji(state_name),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        ssh_engine: runtime
            .map(TunnelRuntime::ssh_engine)
            .unwrap_or_else(|| config.settings.ssh_engine.clone()),
        server_id: server.id,
        server_name: server.name,
        proxy_id: proxy.id,
        proxy_name: proxy.name,
        proxy_host: proxy.host,
        proxy_port: proxy.port,
        pid: runtime.and_then(|runtime| runtime.pid()),
        started_at: runtime.map(|runtime| runtime.started_at),
        ip: ip_info.ip,
        country_flag: country_code_to_flag(&ip_info.country_code),
        country_code: ip_info.country_code,
        country_name: ip_info.country_name,
        rx_total,
        tx_total,
        last_error: runtime
            .map(|runtime| runtime.last_error.clone())
            .unwrap_or_default(),
    }
}

#[tauri::command]
fn get_config() -> Result<AppConfig, String> {
    load_config()
}

#[tauri::command]
fn save_server(server: ServerProfile) -> Result<AppConfig, String> {
    validate_server(&server)?;
    let mut config = load_config()?;
    if let Some(existing) = config.servers.iter_mut().find(|item| item.id == server.id) {
        *existing = server.clone();
    } else {
        config.servers.push(server.clone());
    }
    if config.selected_server_id.is_empty() {
        config.selected_server_id = server.id;
    }
    save_config(config)
}

#[tauri::command]
fn delete_server(id: String) -> Result<AppConfig, String> {
    let mut config = load_config()?;
    config.servers.retain(|server| server.id != id);
    if config.selected_server_id == id {
        config.selected_server_id = config
            .servers
            .first()
            .map(|server| server.id.clone())
            .unwrap_or_default();
    }
    save_config(config)
}

#[tauri::command]
fn select_server(id: String) -> Result<AppConfig, String> {
    let mut config = load_config()?;
    if !config.servers.iter().any(|server| server.id == id) {
        return Err("Server profile not found".to_string());
    }
    config.selected_server_id = id;
    save_config(config)
}

#[tauri::command]
fn save_proxy(proxy: ProxyProfile) -> Result<AppConfig, String> {
    validate_proxy(&proxy)?;
    let mut config = load_config()?;
    if let Some(existing) = config.proxies.iter_mut().find(|item| item.id == proxy.id) {
        *existing = proxy.clone();
    } else {
        config.proxies.push(proxy.clone());
    }
    if config.selected_proxy_id.is_empty() {
        config.selected_proxy_id = proxy.id;
    }
    save_config(config)
}

#[tauri::command]
fn delete_proxy(id: String) -> Result<AppConfig, String> {
    let mut config = load_config()?;
    config.proxies.retain(|proxy| proxy.id != id);
    if config.selected_proxy_id == id {
        config.selected_proxy_id = config
            .proxies
            .first()
            .map(|proxy| proxy.id.clone())
            .unwrap_or_default();
    }
    save_config(config)
}

#[tauri::command]
fn select_proxy(id: String) -> Result<AppConfig, String> {
    let mut config = load_config()?;
    if !config.proxies.iter().any(|proxy| proxy.id == id) {
        return Err("Proxy profile not found".to_string());
    }
    config.selected_proxy_id = id;
    save_config(config)
}

#[tauri::command]
fn save_settings(settings: AppSettings) -> Result<AppConfig, String> {
    validate_settings(&settings)?;
    let mut config = load_config()?;
    config.settings = settings;
    save_config(config)
}

#[tauri::command]
fn reset_config(state: State<'_, AppState>) -> Result<AppConfig, String> {
    {
        let mut runtime_guard = state.runtime.lock().map_err(|error| error.to_string())?;
        if let Some(runtime) = runtime_guard.take() {
            runtime.stop(&state.logs);
        }
    }
    if let Ok(mut traffic) = state.traffic.lock() {
        traffic.clear();
    }
    if let Ok(mut cache) = state.ip_cache.lock() {
        *cache = None;
    }
    let config = default_config();
    log_line(&state.logs, "Config reset to defaults");
    save_config(config)
}

#[tauri::command]
fn test_server(server: ServerProfile, state: State<'_, AppState>) -> Result<String, String> {
    validate_server(&server)?;
    log_line(
        &state.logs,
        format!(
            "Testing TCP reachability for '{}:{}'...",
            server.name, server.port
        ),
    );
    if !tcp_connects(&server.host, server.port, Duration::from_secs(3)) {
        let message = format!("Cannot connect to {}:{}", server.host, server.port);
        log_line(&state.logs, &message);
        return Err(message);
    }
    log_line(&state.logs, "TCP reachability OK");

    let key_path = expand_home(&server.key_path);
    let destination = format!("{}@{}", server.user, server.host);
    log_line(&state.logs, "Testing SSH key authentication...");
    let output = Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("PasswordAuthentication=no")
        .arg("-o")
        .arg("IdentitiesOnly=yes")
        .arg("-o")
        .arg(format!("ConnectTimeout={}", server.connect_timeout))
        .arg("-o")
        .arg(format!(
            "StrictHostKeyChecking={}",
            server.strict_host_key_checking
        ))
        .arg("-p")
        .arg(server.port.to_string())
        .arg("-i")
        .arg(key_path)
        .arg(destination)
        .arg("exit")
        .output()
        .map_err(|error| error.to_string())?;

    if output.status.success() {
        log_line(&state.logs, "SSH key authentication OK");
        Ok("TCP and SSH key authentication OK".to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            format!("SSH test exited with {}", output.status)
        } else {
            stderr
        };
        log_line(&state.logs, format!("SSH test failed: {message}"));
        Err(message)
    }
}

#[tauri::command]
fn start_tunnel(state: State<'_, AppState>) -> Result<String, String> {
    let config = load_config()?;
    let server = selected_server(&config)?;
    let proxy = selected_proxy(&config)?;
    validate_server(&server)?;
    validate_proxy(&proxy)?;
    validate_settings(&config.settings)?;

    if config.settings.clear_logs_on_start {
        if let Ok(mut logs) = state.logs.lock() {
            logs.clear();
        }
    }

    match config.settings.ssh_engine.as_str() {
        "system" => start_system_ssh_tunnel(state, config, server, proxy),
        "embedded" => start_embedded_ssh_tunnel(state, config, server, proxy),
        _ => Err("Unknown SSH engine".to_string()),
    }
}

#[cfg(not(feature = "embedded-ssh"))]
fn start_embedded_ssh_tunnel(
    state: State<'_, AppState>,
    _config: AppConfig,
    _server: ServerProfile,
    _proxy: ProxyProfile,
) -> Result<String, String> {
    let message = "Embedded Rust SSH engine is disabled in this build. Build with Cargo feature 'embedded-ssh' or switch SSH engine to System OpenSSH.".to_string();
    log_line(&state.logs, &message);
    Err(message)
}

#[cfg(feature = "embedded-ssh")]
fn start_embedded_ssh_tunnel(
    state: State<'_, AppState>,
    _config: AppConfig,
    server: ServerProfile,
    proxy: ProxyProfile,
) -> Result<String, String> {
    let mut runtime_guard = state.runtime.lock().map_err(|error| error.to_string())?;
    if let Some(runtime) = runtime_guard.as_mut() {
        if runtime.try_wait()?.is_none() {
            return Ok("Tunnel is already running".to_string());
        }
        *runtime_guard = None;
    }

    if tcp_connects(&proxy.host, proxy.port, Duration::from_millis(200)) {
        return Err(format!(
            "Local proxy {}:{} is already occupied",
            proxy.host, proxy.port
        ));
    }

    if let Ok(mut traffic) = state.traffic.lock() {
        traffic.clear();
    }

    log_line(
        &state.logs,
        format!(
            "Starting '{}' with Embedded Rust SSH listener -> {}:{} ({})",
            server.name, proxy.host, proxy.port, proxy.name
        ),
    );

    let handle = embedded_ssh::start(
        embedded_ssh::EmbeddedServer {
            name: server.name.clone(),
            host: server.host.clone(),
            port: server.port,
            user: server.user.clone(),
            key_path: expand_home(&server.key_path),
            strict_host_key_checking: server.strict_host_key_checking.clone(),
        },
        embedded_ssh::EmbeddedProxy {
            host: proxy.host.clone(),
            port: proxy.port,
        },
    )?;

    *runtime_guard = Some(TunnelRuntime {
        handle: TunnelHandle::Embedded(handle),
        server,
        proxy,
        started_at: now_epoch(),
        last_error: "Embedded russh direct-tcpip transport is not implemented yet".to_string(),
        last_traffic: None,
    });

    Ok(
        "Embedded SOCKS listener started; russh direct-tcpip transport is not implemented yet"
            .to_string(),
    )
}

fn start_system_ssh_tunnel(
    state: State<'_, AppState>,
    config: AppConfig,
    server: ServerProfile,
    proxy: ProxyProfile,
) -> Result<String, String> {
    let mut runtime_guard = state.runtime.lock().map_err(|error| error.to_string())?;
    if let Some(runtime) = runtime_guard.as_mut() {
        if runtime.try_wait()?.is_none() {
            return Ok("Tunnel is already running".to_string());
        }
        *runtime_guard = None;
    }

    if tcp_connects(&proxy.host, proxy.port, Duration::from_millis(200)) {
        return Err(format!(
            "Local proxy {}:{} is already occupied",
            proxy.host, proxy.port
        ));
    }

    if let Ok(mut traffic) = state.traffic.lock() {
        traffic.clear();
    }
    let key_path = expand_home(&server.key_path);
    let proxy_addr = format!("{}:{}", proxy.host, proxy.port);
    let destination = format!("{}@{}", server.user, server.host);

    log_line(
        &state.logs,
        format!(
            "Starting '{}' with System OpenSSH on SSH port {} -> {} ({})",
            server.name, server.port, proxy_addr, proxy.name
        ),
    );

    let mut command = Command::new("ssh");
    command
        .arg("-NT")
        .arg("-o")
        .arg("ExitOnForwardFailure=yes")
        .arg("-o")
        .arg(format!(
            "ServerAliveInterval={}",
            server.server_alive_interval
        ))
        .arg("-o")
        .arg(format!(
            "ServerAliveCountMax={}",
            server.server_alive_count_max
        ))
        .arg("-o")
        .arg(format!("ConnectTimeout={}", server.connect_timeout))
        .arg("-o")
        .arg("TCPKeepAlive=yes")
        .arg("-o")
        .arg("IdentitiesOnly=yes")
        .arg("-o")
        .arg("PasswordAuthentication=no")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg(format!(
            "StrictHostKeyChecking={}",
            server.strict_host_key_checking
        ));
    if server.compression {
        command.arg("-C");
    }
    let mut child = command
        .arg("-p")
        .arg(server.port.to_string())
        .arg("-D")
        .arg(&proxy_addr)
        .arg("-i")
        .arg(&key_path)
        .arg(destination)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;

    if let Some(stdout) = child.stdout.take() {
        attach_log_reader("ssh stdout", stdout, Arc::clone(&state.logs));
    }
    if let Some(stderr) = child.stderr.take() {
        attach_log_reader("ssh stderr", stderr, Arc::clone(&state.logs));
    }

    let pid = child.id();
    for _ in 0..24 {
        thread::sleep(Duration::from_millis(250));
        if let Some(exit_status) = child.try_wait().map_err(|error| error.to_string())? {
            let message = format!("ssh exited during startup with {exit_status}");
            log_line(&state.logs, &message);
            return Err(message);
        }
        if tcp_connects(&proxy.host, proxy.port, Duration::from_millis(200)) {
            log_line(&state.logs, format!("Tunnel is running, ssh pid={pid}"));
            if config.settings.verify_proxy_on_start {
                match verify_proxy(&proxy) {
                    Ok(ip) => {
                        if config.settings.hide_sensitive {
                            log_line(&state.logs, "Proxy verification OK, public IP hidden");
                        } else {
                            log_line(
                                &state.logs,
                                format!("Proxy verification OK, public IP {ip}"),
                            );
                        }
                    }
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        let message = format!("Proxy verification failed: {error}");
                        log_line(&state.logs, &message);
                        return Err(message);
                    }
                }
            }
            *runtime_guard = Some(TunnelRuntime {
                handle: TunnelHandle::System(child),
                server,
                proxy,
                started_at: now_epoch(),
                last_error: String::new(),
                last_traffic: None,
            });
            return Ok("Tunnel started".to_string());
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    let message = "Timed out waiting for local SOCKS listener".to_string();
    log_line(&state.logs, &message);
    Err(message)
}

#[tauri::command]
fn stop_tunnel(state: State<'_, AppState>) -> Result<String, String> {
    let mut runtime_guard = state.runtime.lock().map_err(|error| error.to_string())?;
    let Some(runtime) = runtime_guard.take() else {
        return Ok("Tunnel is already stopped".to_string());
    };
    runtime.stop(&state.logs);
    Ok("Tunnel stopped".to_string())
}

#[tauri::command]
fn restart_tunnel(state: State<'_, AppState>) -> Result<String, String> {
    let _ = stop_tunnel(state.clone());
    thread::sleep(Duration::from_millis(500));
    start_tunnel(state)
}

#[tauri::command]
fn tunnel_status(state: State<'_, AppState>) -> Result<TunnelStatus, String> {
    let config = load_config()?;
    let mut reconnect_status = None;

    {
        let mut runtime_guard = state.runtime.lock().map_err(|error| error.to_string())?;
        if let Some(runtime) = runtime_guard.as_mut() {
            match runtime.try_wait()? {
                Some(exit_message) => {
                    runtime.last_error = exit_message;
                    let status = build_status("stopped", Some(runtime), &config, &state);
                    log_line(&state.logs, status.last_error.clone());
                    *runtime_guard = None;
                    reconnect_status = Some(status);
                }
                None => {
                    update_traffic(&state, runtime);
                    return Ok(build_status("running", Some(runtime), &config, &state));
                }
            }
        }
    }

    if let Some(mut status) = reconnect_status {
        if config.settings.auto_reconnect {
            let delay = config.settings.reconnect_delay_seconds.max(1);
            log_line(
                &state.logs,
                format!("Auto reconnect in {delay}s using selected profiles"),
            );
            thread::sleep(Duration::from_secs(delay.into()));
            match start_tunnel(state.clone()) {
                Ok(message) => {
                    log_line(&state.logs, format!("Auto reconnect: {message}"));
                    return tunnel_status(state);
                }
                Err(error) => {
                    status.last_error = format!("Auto reconnect failed: {error}");
                    log_line(&state.logs, status.last_error.clone());
                }
            }
        }
        return Ok(status);
    }

    Ok(build_status("stopped", None, &config, &state))
}

#[tauri::command]
fn traffic_history(state: State<'_, AppState>) -> Result<Vec<TrafficPoint>, String> {
    state
        .traffic
        .lock()
        .map(|items| items.clone())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_logs(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    state
        .logs
        .lock()
        .map(|items| items.clone())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn clear_logs(state: State<'_, AppState>) -> Result<(), String> {
    state
        .logs
        .lock()
        .map(|mut items| items.clear())
        .map_err(|error| error.to_string())
}

pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .setup(|app| {
            let should_start = load_config()
                .map(|config| config.settings.start_tunnel_on_launch)
                .unwrap_or(false);
            if should_start {
                let app_handle = app.handle().clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_secs(1));
                    let state = app_handle.state::<AppState>();
                    match start_tunnel(state) {
                        Ok(message) => log_line(&app_handle.state::<AppState>().logs, message),
                        Err(error) => log_line(
                            &app_handle.state::<AppState>().logs,
                            format!("Autostart failed: {error}"),
                        ),
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::CloseRequested { .. }) {
                let state = window.app_handle().state::<AppState>();
                let runtime = match state.runtime.lock() {
                    Ok(mut runtime_guard) => runtime_guard.take(),
                    Err(_) => None,
                };
                if let Some(runtime) = runtime {
                    runtime.stop(&state.logs);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_server,
            delete_server,
            select_server,
            save_proxy,
            delete_proxy,
            select_proxy,
            save_settings,
            reset_config,
            test_server,
            start_tunnel,
            stop_tunnel,
            restart_tunnel,
            tunnel_status,
            traffic_history,
            get_logs,
            clear_logs
        ])
        .run(tauri::generate_context!())
        .expect("error while running Secret Tunnel application");
}
