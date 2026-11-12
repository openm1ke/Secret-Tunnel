use secret_tunnel_session_control::{Controller, Phase};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{Manager, State, WindowEvent};

// Console utilities must stay in the background when launched by the GUI.
fn background_command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(feature = "embedded-ssh")]
mod embedded_ssh;
mod stats;

#[cfg_attr(target_os = "macos", allow(dead_code))]
mod app_redirect;
mod installed_apps;
#[cfg(target_os = "macos")]
mod macos_apps;
#[cfg(target_os = "macos")]
mod macos_redirect;
#[cfg(not(target_os = "macos"))]
use app_redirect as platform_redirect;
pub use installed_apps::InstalledApp;
#[cfg(target_os = "macos")]
use macos_redirect as platform_redirect;
#[cfg(target_os = "macos")]
pub use macos_redirect::guardian_if_requested as run_redirect_guardian_if_requested;
#[cfg(target_os = "macos")]
pub use macos_redirect::profile as macos_application_profile;

/// Local application inventory, also used by the diagnostic example.
pub fn application_catalog() -> Result<Vec<InstalledApp>, String> {
    installed_apps::scan()
}
mod traffic_relay;

#[cfg(feature = "embedded-ssh")]
mod udp_protocol;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ServerProfile {
    id: String,
    name: String,
    host: String,
    port: u16,
    user: String,
    key_path: String,
    #[serde(default)]
    jump_server_id: Option<String>,
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
    #[serde(default = "default_redirect_cli")]
    redirect_cli_path: String,
    #[serde(default = "default_redirect_processes")]
    redirect_processes: Vec<String>,
    #[serde(default)]
    domain_rules: Vec<secret_tunnel_routing::DomainRule>,
    ipinfo_token: String,
    #[serde(default = "default_ssh_engine")]
    ssh_engine: String,
    #[serde(default)]
    ssh_tuning: Option<Value>,
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
    /// macOS: SSH to the server bypasses other VPNs' TUN interfaces.
    #[serde(default = "default_true")]
    bypass_vpn_for_ssh: bool,
    /// macOS: programs the selected applications start get a rule while
    /// running, applied without dropping connections.
    #[serde(default = "default_true")]
    follow_child_processes: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            redirect_cli_path: default_redirect_cli(),
            redirect_processes: default_redirect_processes(),
            domain_rules: Vec::new(),
            ipinfo_token: String::new(),
            ssh_engine: default_ssh_engine(),
            ssh_tuning: None,
            hide_sensitive: true,
            verify_proxy_on_start: true,
            auto_reconnect: false,
            reconnect_delay_seconds: default_reconnect_delay_seconds(),
            start_tunnel_on_launch: false,
            clear_logs_on_start: false,
            bypass_vpn_for_ssh: true,
            follow_child_processes: true,
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
    ssh_route: Vec<String>,
    #[cfg(feature = "embedded-ssh")]
    ssh_transport: Option<embedded_ssh::SshSnapshot>,
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
    System(Child, traffic_relay::TrafficRelay),
    #[cfg(feature = "embedded-ssh")]
    Embedded(embedded_ssh::EmbeddedTunnelHandle),
}

struct TunnelRuntime {
    generation: u64,
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
            TunnelHandle::System(..) => "system".to_string(),
            #[cfg(feature = "embedded-ssh")]
            TunnelHandle::Embedded(_) => "embedded".to_string(),
        }
    }

    fn pid(&self) -> Option<u32> {
        match &self.handle {
            TunnelHandle::System(child, _) => Some(child.id()),
            #[cfg(feature = "embedded-ssh")]
            TunnelHandle::Embedded(_) => None,
        }
    }

    fn try_wait(&mut self) -> Result<Option<String>, String> {
        match &mut self.handle {
            TunnelHandle::System(child, _) => child
                .try_wait()
                .map(|status| status.map(|status| format!("ssh exited with {status}")))
                .map_err(|error| error.to_string()),
            #[cfg(feature = "embedded-ssh")]
            TunnelHandle::Embedded(handle) => Ok(handle.try_wait()),
        }
    }

    fn traffic_totals(&self) -> Result<Option<(u64, u64)>, String> {
        match &self.handle {
            TunnelHandle::System(_, relay) => Ok(Some(relay.totals())),
            #[cfg(feature = "embedded-ssh")]
            TunnelHandle::Embedded(handle) => Ok(Some(handle.traffic_totals())),
        }
    }

    fn stop(self, logs: &Arc<Mutex<Vec<String>>>) {
        match self.handle {
            TunnelHandle::System(mut child, relay) => {
                drop(relay);
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

fn default_redirect_processes() -> Vec<String> {
    #[cfg(target_os = "macos")]
    return Vec::new();
    #[cfg(not(target_os = "macos"))]
    vec!["Discord.exe".into()]
}

fn default_redirect_cli() -> String {
    #[cfg(target_os = "macos")]
    return macos_redirect::EXECUTABLE.into();
    #[cfg(not(target_os = "macos"))]
    {
        if let Ok(executable) = std::env::current_exe() {
            if let Some(directory) = executable.parent() {
                let bundled = directory.join("tools/proxybridge/ProxyBridge_CLI.exe");
                if bundled.is_file() {
                    return bundled.to_string_lossy().into_owned();
                }
            }
        }
        app_config_dir("secret-tunnel")
            .map(|dir| {
                dir.join("tools/proxybridge/ProxyBridge_CLI.exe")
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default()
    }
}

fn default_ssh_engine() -> String {
    if cfg!(feature = "embedded-ssh") {
        "embedded"
    } else {
        "system"
    }
    .to_string()
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
    redirect: Mutex<platform_redirect::RedirectState>,
    runtime: Mutex<Option<TunnelRuntime>>,
    control: Controller<AppConfig>,
    cached_status: Mutex<TunnelStatus>,
    closing: AtomicBool,
    latency: Mutex<Option<u64>>,
    logs: Arc<Mutex<Vec<String>>>,
    traffic: Mutex<Vec<TrafficPoint>>,
    ip_cache: Mutex<Option<(u64, String, IpInfo)>>,
    sessions: Mutex<stats::SessionStore>,
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
        if items.len() > 4000 {
            let overflow = items.len() - 4000;
            items.drain(0..overflow);
        }
    }
}

#[cfg(target_os = "windows")]
fn app_config_dir(app_name: &str) -> Result<PathBuf, String> {
    if let Ok(appdata) = std::env::var("APPDATA") {
        return Ok(Path::new(&appdata).join(app_name));
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return Ok(Path::new(&profile)
            .join("AppData")
            .join("Roaming")
            .join(app_name));
    }
    Err("APPDATA and USERPROFILE are not set".to_string())
}

#[cfg(not(target_os = "windows"))]
fn app_config_dir(app_name: &str) -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    Ok(Path::new(&home).join(".config").join(app_name))
}

fn config_path() -> Result<PathBuf, String> {
    Ok(app_config_dir("secret-tunnel")?.join("config.json"))
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
            jump_server_id: None,
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
            .ok_or_else(|| "Missing servers in configuration".to_string())?,
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

static CONFIG_LOCK: Mutex<()> = Mutex::new(());
static CONFIG_RECOVERY_NOTICE: Mutex<bool> = Mutex::new(false);
fn read_config_at(path: &Path) -> Result<AppConfig, String> {
    let primary = fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|raw| parse_config(&raw));
    primary.or_else(|primary_error| {
        let backup = path.with_extension("json.bak");
        let recovered = fs::read_to_string(&backup)
            .map_err(|_| primary_error.clone())
            .and_then(|raw| parse_config(&raw).map_err(|_| primary_error))?;
        *CONFIG_RECOVERY_NOTICE.lock().unwrap() = true;
        Ok(recovered)
    })
}
fn report_config_recovery(state: &AppState) {
    if std::mem::take(&mut *CONFIG_RECOVERY_NOTICE.lock().unwrap()) {
        log_line(&state.logs, "Основной файл настроек повреждён или недоступен. Загружена резервная копия; проверьте профили и сохраните настройки.");
    }
}
fn write_config_at(path: &Path, config: &AppConfig) -> Result<(), String> {
    let raw = serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?;
    if let Ok(previous) = fs::read_to_string(path) {
        if parse_config(&previous).is_ok() {
            secret_tunnel_session_control::storage::replace(
                &path.with_extension("json.bak"),
                previous.as_bytes(),
            )
            .map_err(|e| e.to_string())?;
        }
    }
    secret_tunnel_session_control::storage::replace(path, &raw).map_err(|e| e.to_string())
}
fn load_config_unlocked() -> Result<AppConfig, String> {
    let path = config_path()?;
    if !path.exists() && !path.with_extension("json.bak").exists() {
        let config = default_config();
        write_config_at(&path, &config)?;
        return Ok(config);
    }
    read_config_at(&path)
}
fn load_config() -> Result<AppConfig, String> {
    let _guard = CONFIG_LOCK.lock().map_err(|e| e.to_string())?;
    load_config_unlocked()
}
fn save_config(config: AppConfig) -> Result<AppConfig, String> {
    let _guard = CONFIG_LOCK.lock().map_err(|e| e.to_string())?;
    write_config_at(&config_path()?, &config)?;
    Ok(config)
}
fn mutate_config(
    update: impl FnOnce(&mut AppConfig) -> Result<(), String>,
) -> Result<AppConfig, String> {
    let _guard = CONFIG_LOCK.lock().map_err(|e| e.to_string())?;
    let mut config = load_config_unlocked()?;
    update(&mut config)?;
    write_config_at(&config_path()?, &config)?;
    Ok(config)
}

fn expand_home(path: &str) -> String {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{stripped}");
        }
        if let Ok(profile) = std::env::var("USERPROFILE") {
            return Path::new(&profile)
                .join(stripped)
                .to_string_lossy()
                .to_string();
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
    if [&server.host, &server.user]
        .iter()
        .any(|value| value.starts_with('-') || value.chars().any(char::is_control))
    {
        return Err("SSH host/user cannot start with '-' or contain control characters".into());
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

fn server_jumps<'a>(config: &'a AppConfig, destination: &ServerProfile) -> Result<Vec<&'a ServerProfile>, String> {
    let mut seen = std::collections::HashSet::from([destination.id.clone()]);
    let mut next = destination.jump_server_id.as_deref();
    let mut jumps = Vec::new();
    while let Some(id) = next {
        if !seen.insert(id.to_owned()) {
            return Err("В цепочке SSH обнаружен цикл".into());
        }
        if jumps.len() == 3 {
            return Err("В цепочке допустимо до трёх промежуточных серверов".into());
        }
        let hop = config.servers.iter().find(|server| server.id == id)
            .ok_or_else(|| format!("Промежуточный SSH-сервер не найден: {id}"))?;
        jumps.push(hop);
        next = hop.jump_server_id.as_deref();
    }
    jumps.reverse();
    Ok(jumps)
}

#[cfg(feature = "embedded-ssh")]
fn embedded_server(profile: &ServerProfile) -> embedded_ssh::EmbeddedServer {
    embedded_ssh::EmbeddedServer {
        name: profile.name.clone(),
        host: profile.host.clone(),
        port: profile.port,
        user: profile.user.clone(),
        key_path: expand_home(&profile.key_path),
        strict_host_key_checking: profile.strict_host_key_checking.clone(),
    }
}

#[cfg(feature = "embedded-ssh")]
fn embedded_route(config: &AppConfig, destination: &ServerProfile) -> Result<embedded_ssh::EmbeddedOptions, String> {
    let profiles = server_jumps(config, destination)?;
    if !profiles.is_empty() {
        for profile in profiles.iter().copied().chain(std::iter::once(destination)) {
            if profile.strict_host_key_checking == "no" {
                return Err(format!("Для цепочки SSH включите проверку ключа сервера «{}»", profile.name));
            }
        }
    }
    let jumps = profiles.into_iter().map(|hop| {
        validate_server(hop)?;
        Ok(embedded_ssh::EmbeddedJump { server: embedded_server(hop), host_key_sha256: None })
    }).collect::<Result<Vec<_>, String>>()?;
    let ssh_tuning = config.settings.ssh_tuning.as_ref()
        .map(|value| serde_json::from_value::<embedded_ssh::SshTuning>(value.clone()).map_err(|e| e.to_string()))
        .transpose()?.unwrap_or_default();
    ssh_tuning.validate()?;
    Ok(embedded_ssh::EmbeddedOptions {
        jumps,
        ssh_tuning,
        // QUIC downloads need room for bursts: 128 datagrams per flow capped one
        // at 25 Mbit/s on a 50 ms path. Older agents lower the slots to theirs.
        udp_receive_window: Some(secret_tunnel_mux::FlowWindow { bytes: 1024 * 1024, slots: 512 }),
        cap_default_udp_receive_slots: true,
        bind_physical_interface: config.settings.bypass_vpn_for_ssh,
        route_note: Some(Default::default()),
        ..Default::default()
    })
}

fn validate_proxy(proxy: &ProxyProfile) -> Result<(), String> {
    if proxy.id.trim().is_empty() || proxy.name.trim().is_empty() {
        return Err("Proxy id and name are required".to_string());
    }
    if !proxy
        .host
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
    {
        return Err(
            "SOCKS без пароля разрешён только на loopback: используйте 127.0.0.1 или ::1"
                .to_string(),
        );
    }
    if proxy.port == 0 {
        return Err("Proxy port must be in range 1..65535".to_string());
    }
    Ok(())
}

fn validate_settings(settings: &AppSettings) -> Result<(), String> {
    #[cfg(feature = "embedded-ssh")]
    if let Some(value) = &settings.ssh_tuning {
        let tuning: embedded_ssh::SshTuning = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        tuning.validate()?;
    }
    secret_tunnel_routing::validate(&settings.domain_rules)?;
    if !settings.redirect_processes.is_empty() || !settings.domain_rules.is_empty() {
        #[cfg(target_os = "macos")]
        macos_redirect::validate_ids(&settings.redirect_processes)?;
        #[cfg(not(target_os = "macos"))]
        {
            let current = std::env::current_exe().map_err(|e| e.to_string())?;
            let own_exe = current
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("secret-tunnel.exe");
            app_redirect::domain_profile(
                &settings.redirect_processes,
                &settings.domain_rules,
                "127.0.0.1",
                1080,
                own_exe,
            )?;
        }
    }
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
) -> std::sync::mpsc::Receiver<String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut diagnostic = String::new();
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if !line.trim().is_empty() {
                // Retain a terminal SSH diagnostic even if later lines are generic.
                if diagnostic.is_empty() || !secret_tunnel_session_control::retryable(&line) {
                    diagnostic = line.chars().take(4096).collect();
                }
                log_line(&logs, format!("{label}: {line}"));
            }
        }
        let _ = sender.send(diagnostic);
    });
    receiver
}

fn update_traffic(state: &AppState, runtime: &mut TunnelRuntime) {
    let now = now_epoch();
    match runtime.traffic_totals() {
        Ok(Some((rx_total, tx_total))) => {
            if let Some(previous) = &runtime.last_traffic {
                let elapsed = now.saturating_sub(previous.ts).max(1);
                let rx_bps = rx_total.saturating_sub(previous.rx_total) / elapsed;
                let tx_bps = tx_total.saturating_sub(previous.tx_total) / elapsed;
                let latency_ms = state.latency.lock().ok().and_then(|value| *value);
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
            record_session(state, runtime, rx_total, tx_total, now);
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

/// Keep the running session's totals and per-application traffic in history.
fn record_session(state: &AppState, runtime: &TunnelRuntime, rx: u64, tx: u64, now: u64) {
    #[cfg(feature = "embedded-ssh")]
    let apps = embedded_ssh::app_usage::snapshot()
        .into_iter()
        .map(|(name, rx, tx, flows)| stats::AppUsage { name, rx, tx, flows })
        .collect();
    #[cfg(not(feature = "embedded-ssh"))]
    let apps = Vec::new();
    let (route, country) = state
        .cached_status
        .lock()
        .map(|status| (status.ssh_route.clone(), status.country_code.clone()))
        .unwrap_or_default();
    let id = format!("{}-{}", runtime.started_at, runtime.generation);
    if let Ok(mut sessions) = state.sessions.lock() {
        sessions.observe(
            stats::Observation { id: &id, started_at: runtime.started_at, server: &runtime.server.name, route: &route, country: &country, rx, tx, apps },
            now,
        );
    }
}

fn finish_session(state: &AppState) {
    if let Ok(mut sessions) = state.sessions.lock() {
        sessions.finish(now_epoch());
    }
}

#[tauri::command]
fn session_history(state: State<'_, AppState>) -> Result<Vec<stats::SessionRecord>, String> {
    Ok(state.sessions.lock().map_err(|e| e.to_string())?.list())
}

#[tauri::command]
fn clear_session_history(state: State<'_, AppState>) -> Result<Vec<stats::SessionRecord>, String> {
    let mut sessions = state.sessions.lock().map_err(|e| e.to_string())?;
    sessions.clear(now_epoch());
    Ok(sessions.list())
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
    let mut command = background_command("curl");
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

fn verify_proxy_cancellable(
    proxy: &ProxyProfile,
    cancelled: &dyn Fn() -> bool,
) -> Result<String, String> {
    let mut child = background_command("curl")
        .arg("-sS")
        .arg("--max-time")
        .arg("8")
        .arg("--socks5-hostname")
        .arg(format!("{}:{}", proxy.host, proxy.port))
        .arg("https://api.ipify.org")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;

    let deadline = Instant::now() + Duration::from_secs(9);
    loop {
        if cancelled() || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Proxy verification cancelled or timed out".into());
        }
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;

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
        "unknown" | "starting" | "reconnecting" | "stopping" => "🟡",
        _ => "🔴",
    }
    .to_string()
}

fn totals_from_traffic(state: &AppState) -> (u64, u64) {
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
    app_state: &AppState,
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
            jump_server_id: None,
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
        let key = format!("{}:{}", proxy.host, proxy.port);
        if let Ok(cache) = app_state.ip_cache.lock() {
            if let Some((ts, cache_key, info)) = &*cache {
                if cache_key == &key && now_epoch().saturating_sub(*ts) <= 90 {
                    ip_info = info.clone();
                }
            }
        }
    }

    let ssh_route: Vec<String> = server_jumps(config, &server).ok().filter(|jumps| !jumps.is_empty())
        .map(|jumps| jumps.into_iter().chain(std::iter::once(&server)).map(|hop| hop.name.clone()).collect())
        .unwrap_or_default();
    TunnelStatus {
        state: state_name.to_string(),
        state_emoji: status_emoji(state_name),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        ssh_engine: runtime
            .map(TunnelRuntime::ssh_engine)
            .unwrap_or_else(|| if ssh_route.is_empty() { config.settings.ssh_engine.clone() } else { "embedded".into() }),
        server_id: server.id,
        server_name: server.name,
        ssh_route,
        #[cfg(feature = "embedded-ssh")]
        ssh_transport: runtime.and_then(|runtime| match &runtime.handle {
            TunnelHandle::Embedded(tunnel) => Some(tunnel.ssh_stats()),
            _ => None,
        }),
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
fn get_config(state: State<'_, AppState>) -> Result<AppConfig, String> {
    let config = load_config()?;
    report_config_recovery(&state);
    Ok(config)
}

#[tauri::command]
fn save_server(server: ServerProfile) -> Result<AppConfig, String> {
    validate_server(&server)?;
    mutate_config(|config| {
        if let Some(existing) = config.servers.iter_mut().find(|item| item.id == server.id) {
            *existing = server.clone();
        } else {
            config.servers.push(server.clone());
        }
        for profile in &config.servers {
            server_jumps(config, profile)?;
        }
        if config.selected_server_id.is_empty() {
            config.selected_server_id = server.id;
        }
        Ok(())
    })
}

#[tauri::command]
fn delete_server(id: String) -> Result<AppConfig, String> {
    mutate_config(|config| {
        if let Some(server) = config.servers.iter().find(|server| server.jump_server_id.as_deref() == Some(id.as_str())) {
            return Err(format!("Сервер используется в цепочке «{}». Сначала измените её.", server.name));
        }
        config.servers.retain(|server| server.id != id);
        if config.selected_server_id == id {
            config.selected_server_id = config
                .servers
                .first()
                .map(|server| server.id.clone())
                .unwrap_or_default();
        }
        Ok(())
    })
}

#[tauri::command]
fn select_server(id: String) -> Result<AppConfig, String> {
    mutate_config(|config| {
        if !config.servers.iter().any(|server| server.id == id) {
            return Err("Server profile not found".to_string());
        }
        config.selected_server_id = id;
        Ok(())
    })
}

#[tauri::command]
fn save_proxy(proxy: ProxyProfile) -> Result<AppConfig, String> {
    validate_proxy(&proxy)?;
    mutate_config(|config| {
        if let Some(existing) = config.proxies.iter_mut().find(|item| item.id == proxy.id) {
            *existing = proxy.clone();
        } else {
            config.proxies.push(proxy.clone());
        }
        if config.selected_proxy_id.is_empty() {
            config.selected_proxy_id = proxy.id;
        }
        Ok(())
    })
}

#[tauri::command]
fn delete_proxy(id: String) -> Result<AppConfig, String> {
    mutate_config(|config| {
        config.proxies.retain(|proxy| proxy.id != id);
        if config.selected_proxy_id == id {
            config.selected_proxy_id = config
                .proxies
                .first()
                .map(|proxy| proxy.id.clone())
                .unwrap_or_default();
        }
        Ok(())
    })
}

#[tauri::command]
fn select_proxy(id: String) -> Result<AppConfig, String> {
    mutate_config(|config| {
        if !config.proxies.iter().any(|proxy| proxy.id == id) {
            return Err("Proxy profile not found".to_string());
        }
        config.selected_proxy_id = id;
        Ok(())
    })
}

#[tauri::command]
fn save_settings(settings: AppSettings) -> Result<AppConfig, String> {
    validate_settings(&settings)?;
    mutate_config(|config| {
        config.settings = settings;
        Ok(())
    })
}

#[tauri::command]
fn default_settings() -> AppSettings {
    AppSettings::default()
}

#[tauri::command(async)]
fn reset_config(state: State<'_, AppState>) -> Result<AppConfig, String> {
    state.control.cancel();
    let redirect_result = stop_application_redirect(&state);
    {
        let mut runtime_guard = state.runtime.lock().map_err(|error| error.to_string())?;
        if let Some(runtime) = runtime_guard.take() {
            runtime.stop(&state.logs);
        }
    }
    redirect_result?;
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

#[tauri::command(async)]
fn test_server(server: ServerProfile, state: State<'_, AppState>) -> Result<String, String> {
    validate_server(&server)?;
    let config = load_config()?;
    if server.jump_server_id.is_some() || config.settings.ssh_engine == "embedded" {
        #[cfg(feature = "embedded-ssh")]
        {
            log_line(
                &state.logs,
                "Проверяем SSH, ключ и совместимость агента встроенным движком…",
            );
            let is_relay = config.servers.iter().any(|profile| profile.jump_server_id.as_deref() == Some(server.id.as_str()));
            let result = embedded_ssh::diagnose_server_with_options(
                embedded_server(&server), embedded_route(&config, &server)?, !is_relay,
            );
            match &result {
                Ok(message) => log_line(&state.logs, message),
                Err(error) => log_line(&state.logs, error),
            }
            return result;
        }
        #[cfg(not(feature = "embedded-ssh"))]
        return Err("Встроенный SSH недоступен в этой сборке".into());
    }
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
    let output = background_command("ssh")
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
        .arg("--")
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

#[tauri::command(async)]
fn start_tunnel(state: State<'_, AppState>) -> Result<String, String> {
    let config = load_config()?;
    let server = selected_server(&config)?;
    let proxy = selected_proxy(&config)?;
    validate_server(&server)?;
    server_jumps(&config, &server)?;
    validate_proxy(&proxy)?;
    validate_settings(&config.settings)?;

    if config.settings.clear_logs_on_start {
        if let Ok(mut logs) = state.logs.lock() {
            logs.clear();
        }
    }

    if state.closing.load(Ordering::SeqCst) {
        return Err("Application is closing".into());
    }
    if !state.control.request(config) {
        return Ok("Tunnel is already running or connecting".into());
    }
    *state.ip_cache.lock().unwrap() = None;
    *state.latency.lock().unwrap() = None;
    Ok("Tunnel startup requested".into())
}

// OpenSSH must not outlive a failed/cancelled startup attempt.
struct StartupChild(Option<Child>);
impl Drop for StartupChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn start_session_workers(app: tauri::AppHandle) {
    let telemetry_app = app.clone();
    thread::spawn(move || {
        let mut last_ip = Instant::now() - Duration::from_secs(60);
        let mut ip_generation = 0;
        loop {
            let state = telemetry_app.state::<AppState>();
            if state.closing.load(Ordering::SeqCst) {
                break;
            }
            let snapshot = state.control.snapshot();
            if snapshot.phase == Phase::Running {
                if ip_generation != snapshot.generation {
                    last_ip = Instant::now() - Duration::from_secs(60);
                    ip_generation = snapshot.generation;
                }
                if let Some(plan) = snapshot.plan {
                    if let (Ok(server), Ok(proxy)) = (selected_server(&plan), selected_proxy(&plan))
                    {
                        #[cfg(feature = "embedded-ssh")]
                        let embedded_latency = state.runtime.lock().ok().and_then(|slot| {
                            slot.as_ref().filter(|runtime| runtime.generation == snapshot.generation)
                                .and_then(|runtime| match &runtime.handle {
                                    TunnelHandle::Embedded(tunnel) => {
                                        let heartbeat = tunnel.transport_stats().diagnostics.heartbeat;
                                        let fresh = heartbeat.last_ack_at_ms.is_some_and(|at| now_epoch().saturating_mul(1000).saturating_sub(at) <= heartbeat.interval_ms.saturating_mul(3).max(15_000));
                                        Some(heartbeat.last_rtt_ms.filter(|_| fresh))
                                    },
                                    _ => None,
                                })
                        });
                        #[cfg(not(feature = "embedded-ssh"))]
                        let embedded_latency: Option<Option<u64>> = None;
                        let latency = embedded_latency.unwrap_or_else(|| server_jumps(&plan, &server).ok().and_then(|jumps| {
                            let entry = jumps.first().copied().unwrap_or(&server);
                            tcp_latency_ms(&entry.host, entry.port, Duration::from_secs(2))
                        }));
                        if state.control.current(snapshot.generation) {
                            *state.latency.lock().unwrap() = latency;
                        }
                        if last_ip.elapsed() >= Duration::from_secs(60) {
                            last_ip = Instant::now();
                            match fetch_ipinfo(&plan.settings.ipinfo_token, Some(&proxy)) {
                                Ok(Some(info)) if state.control.current(snapshot.generation) => {
                                    *state.ip_cache.lock().unwrap() = Some((
                                        now_epoch(),
                                        format!("{}:{}", proxy.host, proxy.port),
                                        info,
                                    ));
                                }
                                Err(error) => log_line(&state.logs, format!("IPinfo: {error}")),
                                _ => {}
                            }
                        }
                    }
                }
            }
            // This sampler never owns the runtime or blocks Stop.
            for _ in 0..20 {
                if state.closing.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });
    thread::spawn(move || {
        let mut last_sample = Instant::now();
        loop {
            let state = app.state::<AppState>();
            if state.closing.load(Ordering::SeqCst) {
                break;
            }
            let snapshot = state.control.snapshot();
            let mut failure = None;
            let retired = {
                let mut slot = state.runtime.lock().unwrap();
                if let Some(runtime) = slot.as_mut() {
                    if snapshot.plan.is_none() || runtime.generation != snapshot.generation {
                        slot.take()
                    } else {
                        match runtime.try_wait() {
                            Ok(None) => {
                                if last_sample.elapsed() >= Duration::from_secs(1) {
                                    update_traffic(&state, runtime);
                                    last_sample = Instant::now();
                                }
                                if let Some(plan) = &snapshot.plan {
                                    *state.cached_status.lock().unwrap() =
                                        build_status("running", Some(runtime), plan, &state);
                                }
                                None
                            }
                            result => {
                                failure = Some(match result {
                                    Ok(Some(error)) => error,
                                    Err(error) => error,
                                    _ => unreachable!(),
                                });
                                slot.take()
                            }
                        }
                    }
                } else {
                    None
                }
            };
            if let Some(runtime) = retired {
                let generation = runtime.generation;
                runtime.stop(&state.logs);
                finish_session(&state);
                if let Err(error) = stop_application_redirect(&state) {
                    log_line(&state.logs, error);
                }
                if let Some(error) = failure {
                    log_line(&state.logs, format!("Tunnel failed: {error}"));
                    let retry = snapshot
                        .plan
                        .as_ref()
                        .filter(|plan| {
                            plan.settings.auto_reconnect
                                && secret_tunnel_session_control::retryable(&error)
                        })
                        .map(|plan| {
                            Duration::from_secs(plan.settings.reconnect_delay_seconds.max(1).into())
                        });
                    state
                        .control
                        .failed(generation, error, retry, Instant::now());
                }
            }
            if let Some(attempt) = state.control.attempt(Instant::now()) {
                let plan = attempt.plan.unwrap();
                let generation = attempt.generation;
                *state.cached_status.lock().unwrap() =
                    build_status("starting", None, &plan, &state);
                let result = (|| {
                    let server = selected_server(&plan)?;
                    let proxy = selected_proxy(&plan)?;
                    let engine = if server.jump_server_id.is_some() { "embedded" } else { plan.settings.ssh_engine.as_str() };
                    match engine {
                        "system" => {
                            start_system_ssh_tunnel(&state, plan.clone(), server, proxy, generation)
                        }
                        "embedded" => start_embedded_ssh_tunnel(
                            &state,
                            plan.clone(),
                            server,
                            proxy,
                            generation,
                        ),
                        _ => Err("Unknown SSH engine".into()),
                    }
                })();
                match result {
                    Ok(runtime) => {
                        let mut runtime = Some(runtime);
                        if !state.control.publish(generation, || {
                            *state.runtime.lock().unwrap() = runtime.take();
                        }) {
                            runtime.unwrap().stop(&state.logs);
                        } else {
                            log_line(
                                &state.logs,
                                "Tunnel ready; interception can be enabled explicitly",
                            );
                        }
                    }
                    Err(error) => {
                        let retry = if plan.settings.auto_reconnect
                            && secret_tunnel_session_control::retryable(&error)
                        {
                            Some(Duration::from_secs(
                                plan.settings.reconnect_delay_seconds.max(1).into(),
                            ))
                        } else {
                            None
                        };
                        log_line(&state.logs, format!("Tunnel startup: {error}"));
                        state
                            .control
                            .failed(generation, error, retry, Instant::now());
                    }
                }
            }
            if state.control.snapshot().plan.is_none() {
                state.control.stopped();
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
}

#[cfg(not(feature = "embedded-ssh"))]
fn start_embedded_ssh_tunnel(
    state: &AppState,
    _config: AppConfig,
    _server: ServerProfile,
    _proxy: ProxyProfile,
    _generation: u64,
) -> Result<TunnelRuntime, String> {
    let message = "Embedded Rust SSH engine is disabled in this build. Build with Cargo feature 'embedded-ssh' or switch SSH engine to System OpenSSH.".to_string();
    log_line(&state.logs, &message);
    Err(message)
}

#[cfg(feature = "embedded-ssh")]
fn start_embedded_ssh_tunnel(
    state: &AppState,
    config: AppConfig,
    server: ServerProfile,
    proxy: ProxyProfile,
    generation: u64,
) -> Result<TunnelRuntime, String> {
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

    // Per-application traffic counts from zero for each tunnel session.
    embedded_ssh::app_usage::reset();
    let handle = embedded_ssh::start_with_options_cancellable(
        embedded_server(&server),
        embedded_ssh::EmbeddedProxy {
            host: proxy.host.clone(),
            port: proxy.port,
        },
        Arc::clone(&state.logs),
        embedded_route(&config, &server)?,
        &|| !state.control.current(generation) || state.closing.load(Ordering::SeqCst),
    )?;

    Ok(TunnelRuntime {
        generation,
        handle: TunnelHandle::Embedded(handle),
        server,
        proxy,
        started_at: now_epoch(),
        last_error: String::new(),
        last_traffic: None,
    })
}

fn start_system_ssh_tunnel(
    state: &AppState,
    config: AppConfig,
    server: ServerProfile,
    proxy: ProxyProfile,
    generation: u64,
) -> Result<TunnelRuntime, String> {
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
    // OpenSSH listens privately; the public relay owns the configured SOCKS port.
    let reservation = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let upstream = reservation.local_addr().map_err(|e| e.to_string())?;
    let listener =
        TcpListener::bind((proxy.host.as_str(), proxy.port)).map_err(|e| e.to_string())?;
    let relay =
        traffic_relay::TrafficRelay::start(listener, upstream).map_err(|e| e.to_string())?;
    let proxy_addr = upstream.to_string();
    let destination = format!("{}@{}", server.user, server.host);

    log_line(
        &state.logs,
        format!(
            "Starting '{}' with System OpenSSH on SSH port {} -> {} ({})",
            server.name, server.port, proxy_addr, proxy.name
        ),
    );

    let mut command = background_command("ssh");
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
    drop(reservation);
    let mut child = command
        .arg("-p")
        .arg(server.port.to_string())
        .arg("-D")
        .arg(&proxy_addr)
        .arg("-i")
        .arg(&key_path)
        .arg("--")
        .arg(destination)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;

    if let Some(stdout) = child.stdout.take() {
        attach_log_reader("ssh stdout", stdout, Arc::clone(&state.logs));
    }
    let stderr = child
        .stderr
        .take()
        .map(|reader| attach_log_reader("ssh stderr", reader, Arc::clone(&state.logs)));

    let mut startup = StartupChild(Some(child));
    let child = startup.0.as_mut().unwrap();
    let pid = child.id();
    for _ in 0..120 {
        if !state.control.current(generation) || state.closing.load(Ordering::SeqCst) {
            return Err("Tunnel startup cancelled".into());
        }
        thread::sleep(Duration::from_millis(50));
        if let Some(exit_status) = child.try_wait().map_err(|error| error.to_string())? {
            let diagnostic = stderr
                .as_ref()
                .and_then(|reader| reader.recv_timeout(Duration::from_millis(100)).ok())
                .unwrap_or_default();
            let message = format!("ssh exited during startup with {exit_status}: {diagnostic}");
            log_line(&state.logs, &message);
            return Err(message);
        }
        if tcp_connects("127.0.0.1", upstream.port(), Duration::from_millis(200)) {
            log_line(&state.logs, format!("Tunnel is running, ssh pid={pid}"));
            if config.settings.verify_proxy_on_start {
                match verify_proxy_cancellable(&proxy, &|| {
                    !state.control.current(generation) || state.closing.load(Ordering::SeqCst)
                }) {
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
            return Ok(TunnelRuntime {
                generation,
                handle: TunnelHandle::System(startup.0.take().unwrap(), relay),
                server,
                proxy,
                started_at: now_epoch(),
                last_error: String::new(),
                last_traffic: None,
            });
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    let message = "Timed out waiting for local SOCKS listener".to_string();
    log_line(&state.logs, &message);
    Err(message)
}

#[tauri::command(async)]
fn stop_tunnel(state: State<'_, AppState>) -> Result<String, String> {
    state.control.cancel();
    Ok("Tunnel stop requested".into())
}

#[tauri::command(async)]
fn restart_tunnel(state: State<'_, AppState>) -> Result<String, String> {
    let config = load_config()?;
    validate_server(&selected_server(&config)?)?;
    validate_proxy(&selected_proxy(&config)?)?;
    validate_settings(&config.settings)?;
    state.control.cancel();
    *state.ip_cache.lock().unwrap() = None;
    *state.latency.lock().unwrap() = None;
    state.control.request(config);
    Ok("Tunnel restart requested".into())
}

#[tauri::command]
fn tunnel_status(state: State<'_, AppState>) -> Result<TunnelStatus, String> {
    let snapshot = state.control.snapshot();
    let mut status = state
        .cached_status
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
    status.state = snapshot.phase.label().into();
    status.state_emoji = status_emoji(&status.state);
    if snapshot.phase != Phase::Running {
        status.pid = None;
        status.started_at = None;
        status.ip.clear();
        status.country_flag.clear();
        status.country_code.clear();
        status.country_name.clear();
    }
    if !snapshot.error.is_empty() {
        status.last_error = snapshot.error;
    }
    Ok(status)
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

#[tauri::command(async)]
fn redirect_status(state: State<'_, AppState>) -> Result<app_redirect::RedirectStatus, String> {
    let mut status = state.redirect.lock().map_err(|e| e.to_string())?.status();
    let config = load_config()?;
    let executable = if config.settings.redirect_cli_path.trim().is_empty() {
        default_redirect_cli()
    } else {
        config.settings.redirect_cli_path
    };
    #[cfg(target_os = "macos")]
    {
        status.component_available = macos_redirect::component_available(Path::new(&executable));
    }
    #[cfg(not(target_os = "macos"))]
    {
        status.component_available = cfg!(windows)
            && Path::new(&executable).is_file()
            && Path::new(&executable)
                .parent()
                .map(|directory| {
                    [
                        "ProxyBridgeCore.dll",
                        "WinDivert.dll",
                        "WinDivert64.sys",
                        "source-revision.txt",
                    ]
                    .iter()
                    .all(|file| directory.join(file).is_file())
                })
                .unwrap_or(false);
    }
    Ok(status)
}

#[tauri::command]
fn discord_redirect_processes() -> Vec<String> {
    app_redirect::discord_processes()
}

#[tauri::command]
fn check_domain_rule(pattern: String, domain: String) -> Result<bool, String> {
    let regex = secret_tunnel_routing::compile(&pattern)?;
    if domain.trim().is_empty() {
        return Ok(false);
    }
    Ok(regex.is_match(&secret_tunnel_routing::normalize(&domain)?))
}

#[tauri::command(async)]
fn scan_installed_applications() -> Result<Vec<installed_apps::InstalledApp>, String> {
    application_catalog()
}

#[tauri::command(async)]
fn resolve_application_paths(paths: Vec<String>) -> Result<Vec<InstalledApp>, String> {
    if paths.len() > 64 {
        return Err("Выберите не более 64 приложений за один раз".into());
    }
    #[cfg(target_os = "macos")]
    return paths
        .iter()
        .map(|path| macos_apps::describe(Path::new(path)))
        .collect();
    #[cfg(not(target_os = "macos"))]
    Err("Чтение .app доступно только на macOS".into())
}

/// PNG data URLs of application icons by path, for the application picker.
#[tauri::command(async)]
fn application_icons(paths: Vec<String>) -> Result<std::collections::HashMap<String, String>, String> {
    if paths.len() > 400 {
        return Err("Слишком много приложений за один запрос".into());
    }
    #[cfg(target_os = "macos")]
    return Ok(macos_apps::icons(&paths));
    #[cfg(not(target_os = "macos"))]
    {
        let _ = paths;
        Ok(Default::default())
    }
}

fn stop_application_redirect(state: &AppState) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return state.redirect.lock().map_err(|e| e.to_string())?.stop();
    #[cfg(not(target_os = "macos"))]
    {
        state.redirect.lock().map_err(|e| e.to_string())?.stop();
        Ok(())
    }
}

/// Programs the selected applications started that were given a rule while running.
#[tauri::command]
fn redirect_followed(state: State<'_, AppState>) -> Result<Vec<serde_json::Value>, String> {
    #[cfg(target_os = "macos")]
    {
        let followed = state.redirect.lock().map_err(|e| e.to_string())?.followed();
        return Ok(followed.into_iter().filter_map(|f| serde_json::to_value(f).ok()).collect());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = state;
        Ok(Vec::new())
    }
}

/// Programs of the selected applications that went around the tunnel.
#[tauri::command]
fn redirect_leaks(state: State<'_, AppState>) -> Result<Vec<serde_json::Value>, String> {
    #[cfg(target_os = "macos")]
    {
        let leaks = state.redirect.lock().map_err(|e| e.to_string())?.leaks();
        return Ok(leaks.into_iter().filter_map(|leak| serde_json::to_value(leak).ok()).collect());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = state;
        Ok(Vec::new())
    }
}

#[tauri::command(async)]
fn start_redirect(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let config = load_config()?;
    let proxy = {
        let mut runtime = state.runtime.lock().map_err(|e| e.to_string())?;
        let runtime = runtime.as_mut().ok_or("Сначала запустите SSH-туннель")?;
        if let Some(error) = runtime.try_wait()? {
            return Err(format!("SSH-туннель уже завершился: {error}"));
        }
        if !cfg!(target_os = "macos") && runtime.ssh_engine() != "embedded" {
            return Err("Для TCP и UDP выберите Embedded Rust SSH и перезапустите туннель".into());
        }
        (runtime.proxy.clone(), runtime.ssh_engine() == "embedded")
    };
    let (proxy, udp) = proxy;
    #[cfg(target_os = "macos")]
    {
        // Rules come from the applications as installed now, so programs
        // added by an update and bundled tools signed under their own
        // identifier (ChatGPT's `codex`) are intercepted too.
        let apps = macos_apps::selected(&config.settings.redirect_processes);
        let mut ids = config.settings.redirect_processes.clone();
        let mut seen: std::collections::HashSet<String> = ids.iter().map(|id| id.to_ascii_lowercase()).collect();
        for pattern in apps.iter().flat_map(|app| app.patterns.iter()) {
            if seen.insert(pattern.to_ascii_lowercase()) {
                ids.push(pattern.clone());
            }
        }
        let selection = macos_redirect::Selection {
            bundles: apps.iter().map(|app| (app.name.clone(), PathBuf::from(&app.executable))).collect(),
            covered: seen,
        };
        // Checks the rules and refuses domain rules, which macOS lacks.
        macos_redirect::profile(&ids, &config.settings.domain_rules, &proxy.host, proxy.port, udp)?;
        let executable = if config.settings.redirect_cli_path.trim().is_empty() {
            default_redirect_cli()
        } else {
            config.settings.redirect_cli_path.clone()
        };
        let rules = ids.len();
        let plan = macos_redirect::Plan {
            ids,
            host: proxy.host.clone(),
            port: proxy.port,
            udp,
            follow: config.settings.follow_child_processes,
        };
        let generation = state.redirect.lock().map_err(|e| e.to_string())?.start(
            Path::new(&executable),
            plan,
            Arc::clone(&state.logs),
            selection,
        )?;
        log_line(
            &state.logs,
            format!("Запрошен запуск перехвата macOS: {} приложений, {} правил; ожидаем подтверждение расширения", apps.len(), rules),
        );
        // Follow the applications: programs they start get a rule within a
        // couple of seconds, without dropping connections.
        let wake = state.redirect.lock().map_err(|e| e.to_string())?.waker();
        std::thread::spawn(move || loop {
            // Look every second, or right away when a connection went
            // direct: the program that made it is usually still running.
            {
                let (flag, signal) = &*wake;
                let Ok(guard) = flag.lock() else { break };
                let Ok((mut guard, _)) = signal.wait_timeout_while(guard, std::time::Duration::from_secs(1), |woken| !*woken) else { break };
                *guard = false;
            }
            let state = app.state::<AppState>();
            let Ok(mut redirect) = state.redirect.lock() else { break };
            if redirect.follow(generation).is_none() {
                break;
            }
            drop(redirect);
            std::thread::sleep(std::time::Duration::from_millis(250));
        });
        Ok("Запускаем расширение ProxyBridge. При первом запуске разрешите расширение и сетевую конфигурацию в настройках macOS".into())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = udp;
        let own_exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let own_exe = own_exe
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("Не удалось определить имя клиента")?;
        let value = app_redirect::domain_profile(
            &config.settings.redirect_processes,
            &config.settings.domain_rules,
            &proxy.host,
            proxy.port,
            own_exe,
        )?;
        let profile_path = app_config_dir("secret-tunnel")?.join("applications.pbprofile");
        let executable = if config.settings.redirect_cli_path.trim().is_empty() {
            default_redirect_cli()
        } else {
            config.settings.redirect_cli_path.clone()
        };
        state.redirect.lock().map_err(|e| e.to_string())?.start(
            Path::new(&executable),
            &profile_path,
            &value,
            Arc::clone(&state.logs),
            config.settings.hide_sensitive,
        )?;
        log_line(&state.logs, "Перехват выбранных приложений включён");
        Ok(
            "Перехват включён. Перезапустите выбранные программы, чтобы создать новые соединения"
                .into(),
        )
    }
}

#[tauri::command(async)]
fn stop_redirect(state: State<'_, AppState>) -> Result<String, String> {
    stop_application_redirect(&state)?;
    log_line(
        &state.logs,
        "Перехват приложений выключен; новые соединения идут напрямую",
    );
    Ok("Перехват выключен".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .setup(|app| {
            if let Ok(dir) = app_config_dir("secret-tunnel") {
                *app.state::<AppState>().sessions.lock().unwrap() = stats::SessionStore::load(dir.join("sessions.json"));
            }
            let initial = load_config().unwrap_or_else(|_| default_config());
            report_config_recovery(&app.state::<AppState>());
            *app.state::<AppState>().cached_status.lock().unwrap() =
                build_status("stopped", None, &initial, &app.state::<AppState>());
            start_session_workers(app.handle().clone());
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
                shutdown(&window.app_handle().state::<AppState>());
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            redirect_status,
            discord_redirect_processes,
            check_domain_rule,
            scan_installed_applications,
            resolve_application_paths,
            application_icons,
            session_history,
            clear_session_history,
            redirect_leaks,
            redirect_followed,
            start_redirect,
            stop_redirect,
            save_server,
            delete_server,
            select_server,
            save_proxy,
            delete_proxy,
            select_proxy,
            save_settings,
            default_settings,
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
        .build(tauri::generate_context!())
        .expect("error while building Secret Tunnel application")
        .run(|app, event| {
            // Quitting from the menu or with Cmd+Q closes no window first.
            if matches!(event, tauri::RunEvent::Exit) {
                shutdown(&app.state::<AppState>());
            }
        });
}

/// Stop interception and the tunnel and close the running session. Safe to
/// call more than once.
fn shutdown(state: &AppState) {
    state.closing.store(true, Ordering::SeqCst);
    state.control.cancel();
    if let Ok(mut redirect) = state.redirect.lock() {
        let _ = redirect.stop();
    }
    let runtime = match state.runtime.lock() {
        Ok(mut runtime_guard) => runtime_guard.take(),
        Err(_) => None,
    };
    if let Some(runtime) = runtime {
        runtime.stop(&state.logs);
    }
    finish_session(state);
}

#[cfg(test)]
mod recovery_storage_tests {
    use super::*;
    #[test]
    fn openssh_startup_preserves_terminal_auth_errors_for_retry_policy() {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let output = std::io::Cursor::new(
            b"user@fixture: Permission denied (publickey).\nConnection closed\n".to_vec(),
        );
        let diagnostic = attach_log_reader("fixture", output, logs.clone())
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(!secret_tunnel_session_control::retryable(&diagnostic));
        assert!(diagnostic.contains("Permission denied"));
        assert_eq!(logs.lock().unwrap().len(), 2);
    }
    #[test]
    fn corrupt_primary_restores_backup_and_does_not_overwrite_it_with_corruption() {
        let root = std::env::temp_dir().join(format!(
            "st-recovery-{}-{}",
            std::process::id(),
            now_epoch()
        ));
        let path = root.join("config.json");
        let mut config = AppConfig {
            selected_server_id: String::new(),
            selected_proxy_id: String::new(),
            servers: vec![],
            proxies: vec![],
            settings: AppSettings::default(),
        };
        write_config_at(&path, &config).unwrap();
        config.selected_server_id = "new-version".into();
        write_config_at(&path, &config).unwrap();
        for broken in ["{broken", "{}", "null", "{\"proxies\":[]}"] {
            fs::write(&path, broken).unwrap();
            let recovered = read_config_at(&path).unwrap();
            assert!(recovered.selected_server_id.is_empty());
            write_config_at(&path, &config).unwrap();
        }
        assert_eq!(
            read_config_at(&path).unwrap().selected_server_id,
            "new-version"
        );
        assert!(
            parse_config(&fs::read_to_string(path.with_extension("json.bak")).unwrap())
                .unwrap()
                .selected_server_id
                .is_empty()
        );
        fs::write(
            &path,
            r#"{"servers":[],"proxy":{"host":"127.0.0.1","port":1081}}"#,
        )
        .unwrap();
        write_config_at(&path, &config).unwrap();
        assert_eq!(
            parse_config(&fs::read_to_string(path.with_extension("json.bak")).unwrap())
                .unwrap()
                .proxies[0]
                .port,
            1081
        );
        fs::remove_dir_all(root).unwrap();
    }
}
