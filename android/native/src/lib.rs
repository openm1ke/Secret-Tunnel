use jni::{
    objects::{JObject, JString, JValue},
    sys::jstring,
    JNIEnv,
};
use secret_tunnel_core::embedded_ssh::{
    self, PriorityUdpRouter, EmbeddedJump, EmbeddedOptions, EmbeddedProxy, EmbeddedServer,
    EmbeddedTunnelHandle, FlowWindow,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};

#[derive(Deserialize)]
struct Config {
    #[serde(default = "android_name")]
    name: String,
    host: String,
    port: u16,
    user: String,
    key_path: String,
    host_key_sha256: String,
    #[serde(default)]
    socks_port: u16,
    #[serde(default)]
    ssh_jumps: Vec<JumpConfig>,
    check_agent: Option<bool>,
    #[serde(default)]
    device_key_path: String,
    #[serde(default)]
    new_session: bool,
    #[serde(default)]
    max_connections: usize,
    #[serde(default)]
    udp_receive_window: Option<FlowWindow>,
    #[serde(default)]
    agent_udp_receive_window: Option<FlowWindow>,
    #[serde(default)]
    ssh_tuning: embedded_ssh::SshTuning,
    #[serde(default)]
    priority_udp_ssh: bool,
}

fn android_name() -> String { "Android".into() }

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
    jumps.into_iter().map(|jump| {
        if jump.host.trim().is_empty() || jump.user.trim().is_empty() || jump.port == 0
            || jump.key_path.is_empty() || jump.host_key_sha256.is_empty()
        {
            return Err(format!("Настройте адрес, ключ и отпечаток сервера «{}»", jump.name));
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
    }).collect()
}
#[derive(Default)]
struct State {
    tunnel: Option<Session>,
    starting: bool,
    error: String,
    totals: (u64, u64),
    transport: Option<embedded_ssh::TransportSnapshot>,
    checks: Arc<secret_tunnel_core::connection_checks::Checks>,
    socks_port: u16,
}

struct Session {
    primary: EmbeddedTunnelHandle,
    priority: Option<EmbeddedTunnelHandle>,
    priority_error: Option<String>,
}

impl Session {
    fn totals(&self) -> (u64, u64) {
        let (rx, tx) = self.primary.traffic_totals();
        let (priority_rx, priority_tx) = self.priority.as_ref()
            .map(EmbeddedTunnelHandle::traffic_totals).unwrap_or_default();
        (rx.saturating_add(priority_rx), tx.saturating_add(priority_tx))
    }

    fn stop(self) -> embedded_ssh::TransportSnapshot {
        if let Some(priority) = self.priority {
            priority.stop();
        }
        self.primary.stop_with_stats()
    }
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
fn output(env: &mut JNIEnv, value: String) -> jstring {
    env.new_string(value)
        .map(JString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}
fn start(env: &mut JNIEnv, config: JString, service: JObject) -> Result<(), String> {
    let raw: String = env.get_string(&config).map_err(|e| e.to_string())?.into();
    let cfg: Config = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    if cfg.host.trim().is_empty()
        || cfg.user.trim().is_empty()
        || cfg.port == 0
        || cfg.socks_port == 0
    {
        return Err("Укажите сервер, пользователя и порты 1–65535".into());
    }
    let jumps = ssh_jumps(cfg.ssh_jumps)?;
    // Keep build-time overrides exact for comparisons using identical APKs.
    let build_kib = option_env!("ST_ANDROID_UDP_RECEIVE_KIB");
    let build_slots = option_env!("ST_ANDROID_UDP_RECEIVE_SLOTS");
    let udp_receive_kib = build_kib
        .unwrap_or("1024")
        .parse::<usize>()
        .ok()
        .filter(|kib| (64..=1024).contains(kib))
        .ok_or("Некорректный размер окна UDP в сборке")?;
    let udp_receive_slots = build_slots
        .unwrap_or("512")
        .parse()
        .map_err(|_| "Некорректный лимит UDP-слотов в сборке")?;
    {
        let mut s = state().lock().map_err(|e| e.to_string())?;
        if s.starting || s.tunnel.is_some() {
            return Err("Туннель уже запущен".into());
        }
        s.starting = true;
        s.error.clear();
        if cfg.new_session {
            s.totals = (0, 0);
            s.transport = None;
        }
    }
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    if cfg.new_session {
        logs().lock().unwrap().clear();
    }
    *observed().lock().unwrap() = None;
    let result = (|| {
        let vm = Arc::new(env.get_java_vm().map_err(|e| e.to_string())?);
        let service = env.new_global_ref(service).map_err(|e| e.to_string())?;
        let protect = Arc::new(move |fd: i32| {
            vm.attach_current_thread()
                .ok()
                .and_then(|mut env| {
                    env.call_method(service.as_obj(), "protect", "(I)Z", &[JValue::Int(fd)])
                        .ok()
                })
                .and_then(|v| v.z().ok())
                .unwrap_or(false)
        });
        let priority_router = (cfg.priority_udp_ssh && cfg.ssh_tuning.tcp_interactive_bytes == Some(64 * 1024))
            .then(PriorityUdpRouter::default);
        let server = EmbeddedServer {
                name: cfg.name,
                host: cfg.host,
                port: cfg.port,
                user: cfg.user,
                key_path: cfg.key_path,
                strict_host_key_checking: "yes".into(),
        };
        let options = EmbeddedOptions {
            host_key_sha256: Some(cfg.host_key_sha256),
            observed_host_key: Some(observed()),
            protect_socket: Some(protect),
            jumps,
            ssh_tuning: cfg.ssh_tuning,
            probe_ipv6: true,
            udp_receive_window: Some(cfg.udp_receive_window.unwrap_or(FlowWindow {
                bytes: udp_receive_kib * 1024,
                slots: udp_receive_slots,
            })),
            cap_default_udp_receive_slots: cfg.udp_receive_window.is_none()
                && build_kib.is_none()
                && build_slots.is_none(),
            agent_udp_receive_window: cfg.agent_udp_receive_window,
            bind_physical_interface: false,
            route_note: None,
            max_connections: if cfg.max_connections == 0 {
                512
            } else {
                cfg.max_connections.clamp(64, 2048)
            },
            priority_udp: priority_router.clone(),
        };
        let cancelled = || GENERATION.load(Ordering::SeqCst) != generation;
        let primary = embedded_ssh::start_with_options_cancellable(
            server.clone(),
            EmbeddedProxy {
                host: "127.0.0.1".into(),
                port: cfg.socks_port,
            },
            logs(),
            options.clone(),
            &cancelled,
        )?;
        let mut session = Session { primary, priority: None, priority_error: None };
        if let Some(router) = priority_router {
            let result = if session.primary.priority_udp_port().is_some() {
                embedded_ssh::start_priority_udp_transport(server, logs(), options, &cancelled)
            } else {
                Err("Локальный SOCKS для второго SSH недоступен".into())
            };
            match result {
                Ok(priority) => {
                    router.attach(&priority);
                    secret_tunnel_core::log_line(&logs(), "Отдельный SSH для UDP подключён");
                    session.priority = Some(priority);
                }
                Err(error) => {
                    secret_tunnel_core::log_line(&logs(), format!("UDP отмеченных приложений через основной SSH: {error}"));
                    session.priority_error = Some(error);
                }
            }
        }
        Ok::<_, String>(session)
    })();
    let mut s = state().lock().map_err(|e| e.to_string())?;
    s.starting = false;
    match result {
        Ok(tunnel) if GENERATION.load(Ordering::SeqCst) == generation => {
            s.tunnel = Some(tunnel);
            s.socks_port = cfg.socks_port;
            s.checks = Default::default();
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
#[no_mangle]
pub extern "system" fn Java_dev_secrettunnel_android_NativeBridge_start(
    mut env: JNIEnv,
    _this: JObject,
    config: JString,
    service: JObject,
) -> jstring {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        start(&mut env, config, service)
    }))
    .unwrap_or_else(|_| Err("Ошибка Rust SSH".into()));
    output(
        &mut env,
        match result {
            Ok(()) => String::new(),
            Err(e) => e,
        },
    )
}
#[no_mangle]
pub extern "system" fn Java_dev_secrettunnel_android_NativeBridge_cancel(
    _env: JNIEnv,
    _this: JObject,
) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}
#[no_mangle]
pub extern "system" fn Java_dev_secrettunnel_android_NativeBridge_stop(
    _env: JNIEnv,
    _this: JObject,
) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    let tunnel = {
        let mut s = state().lock().unwrap();
        s.checks = Default::default();
        s.socks_port = 0;
        let t = s.tunnel.take();
        if let Some(t) = &t {
            let (rx, tx) = t.totals();
            s.totals.0 = s.totals.0.saturating_add(rx);
            s.totals.1 = s.totals.1.saturating_add(tx);
        }
        t
    };
    if let Some(tunnel) = tunnel {
        let transport = tunnel.stop();
        state().lock().unwrap().transport = Some(transport);
    }
}
#[no_mangle]
pub extern "system" fn Java_dev_secrettunnel_android_NativeBridge_status(
    mut env: JNIEnv,
    _this: JObject,
) -> jstring {
    let mut s = state().lock().unwrap();
    if let Some(error) = s.tunnel.as_ref().and_then(|t| t.primary.try_wait()) {
        s.error = error;
    }
    if let Some(tunnel) = s.tunnel.as_mut() {
        if tunnel.priority_error.is_none() {
            if let Some(error) = tunnel.priority.as_ref().and_then(EmbeddedTunnelHandle::try_wait) {
                secret_tunnel_core::log_line(&logs(), format!("UDP отмеченных приложений через основной SSH: {error}"));
                tunnel.priority_error = Some(error);
            }
        }
    }
    let current = s
        .tunnel
        .as_ref()
        .map(Session::totals)
        .unwrap_or_default();
    let totals = (
        s.totals.0.saturating_add(current.0),
        s.totals.1.saturating_add(current.1),
    );
    let stats = s
        .tunnel
        .as_ref()
        .map(|t| t.primary.transport_stats())
        .or_else(|| s.transport.clone())
        .unwrap_or_default();
    let peer=stats.peer.as_ref().map(|p|json!({"active":p.active,"peak":p.peak,"limit":p.limit,"rejected":p.rejected,"udp_drops":p.udp_drops,"last_udp_error":p.last_udp_error,"protocol_errors":p.protocol_errors,"diagnostics":p.diagnostics,
        "receive_queue_bytes":p.receive_queue_bytes,"receive_queue_peak":p.receive_queue_peak,"receive_budget":p.receive_budget,"send_queue_bytes":p.send_queue_bytes,"send_queue_peak":p.send_queue_peak,"send_budget":p.send_budget}));
    let running = s.tunnel.is_some() && s.error.is_empty();
    let priority = s.tunnel.as_ref().filter(|t| running && (t.priority.is_some() || t.priority_error.is_some()));
    let separate = priority.is_some_and(|t| t.priority.is_some() && t.priority_error.is_none());
    let transport = json!({"version":3,"ssh_channels":u8::from(running)+u8::from(separate),"active":stats.active,"peak":stats.peak,"limit":stats.limit,"rejected":stats.rejected,"server":peer,"peer_udp_window":stats.peer_udp_window,
        "receive_queue_bytes":stats.receive_queue_bytes,"receive_queue_peak":stats.receive_queue_peak,"receive_budget":stats.receive_budget,
        "send_queue_bytes":stats.send_queue_bytes,"send_queue_peak":stats.send_queue_peak,"send_budget":stats.send_budget,
        "frames_tx":stats.frames_tx,"frames_rx":stats.frames_rx,"write_batches":stats.write_batches,"udp_drops":stats.udp_drops,"protocol_errors":stats.protocol_errors,"diagnostics":stats.diagnostics});
    let mut transport = transport;
    transport["ssh"] = json!(s.tunnel.as_ref().map(|t| t.primary.ssh_stats()));
    let priority_status = priority.map(|t| {
        let (rx, tx) = t.priority.as_ref().map(EmbeddedTunnelHandle::traffic_totals).unwrap_or_default();
        json!({"separate":separate,"error":t.priority_error,"port":t.primary.priority_udp_port(),"rx":rx,"tx":tx})
    });
    let checks = if running { s.checks.snapshot() } else { Default::default() };
    let value=json!({"running":running,"starting":s.starting,"error":s.error,"rx":totals.0,"tx":totals.1,"transport":transport,"agent":s.tunnel.as_ref().map(|t|t.primary.agent_summary()),"ipv6_supported":s.tunnel.as_ref().and_then(|t|t.primary.ipv6_supported()).unwrap_or(false),"checks":checks,"priority_udp_ssh":priority_status,"observed_host_key":*observed().lock().unwrap(),"logs":*logs().lock().unwrap()}).to_string();
    output(&mut env, value)
}

#[no_mangle]
pub extern "system" fn Java_dev_secrettunnel_android_NativeBridge_checkConnection(
    mut env: JNIEnv,
    _this: JObject,
) -> jstring {
    let state = state().lock().unwrap();
    let result = if state.tunnel.is_some() && state.error.is_empty() {
        state.checks.start(([127, 0, 0, 1], state.socks_port).into(), true)
            .err().map(|error| error.to_string()).unwrap_or_default()
    } else {
        "Сначала подключите SSH".into()
    };
    output(&mut env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_secrettunnel_android_NativeBridge_trafficTotals(
    env: JNIEnv,
    _: JObject,
) -> jni::sys::jlongArray {
    let totals = {
        let s = state().lock().unwrap();
        let current = s.tunnel.as_ref().map(Session::totals).unwrap_or_default();
        [s.totals.0.saturating_add(current.0) as i64, s.totals.1.saturating_add(current.1) as i64]
    };
    let Ok(array) = env.new_long_array(2) else { return std::ptr::null_mut() };
    if env.set_long_array_region(&array, 0, &totals).is_err() {
        return std::ptr::null_mut();
    }
    array.into_raw()
}

#[no_mangle]
pub extern "system" fn Java_dev_secrettunnel_android_NativeBridge_probe(
    mut env: JNIEnv,
    _: JObject,
    config: JString,
) -> jstring {
    let result = (|| -> Result<String, String> {
        let raw: String = env.get_string(&config).map_err(|e| e.to_string())?.into();
        let cfg: ProbeConfig = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if cfg.host.trim().is_empty() || cfg.port == 0 {
            return Err("Invalid server address or port".into());
        }
        embedded_ssh::probe_server(&cfg.host, cfg.port, ssh_jumps(cfg.ssh_jumps)?)
    })();
    output(
        &mut env,
        match result {
            Ok(pin) => json!({"fingerprint":pin}).to_string(),
            Err(e) => json!({"error":e}).to_string(),
        },
    )
}

#[no_mangle]
pub extern "system" fn Java_dev_secrettunnel_android_NativeBridge_checkServer(
    mut env: JNIEnv,
    _: JObject,
    config: JString,
) -> jstring {
    let result = (|| -> Result<String, String> {
        let raw: String = env.get_string(&config).map_err(|e| e.to_string())?.into();
        let cfg: Config = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if cfg.host.trim().is_empty()
            || cfg.user.trim().is_empty()
            || cfg.port == 0
            || cfg.host_key_sha256.is_empty()
        {
            return Err("Укажите сервер, ключ и подтверждённый отпечаток".into());
        }
        embedded_ssh::diagnose_server_with_options(
            EmbeddedServer {
                name: cfg.name,
                host: cfg.host,
                port: cfg.port,
                user: cfg.user,
                key_path: cfg.key_path,
                strict_host_key_checking: "yes".into(),
            },
            EmbeddedOptions {
                host_key_sha256: Some(cfg.host_key_sha256),
                jumps: ssh_jumps(cfg.ssh_jumps)?,
                ssh_tuning: cfg.ssh_tuning,
                ..Default::default()
            },
            cfg.check_agent.unwrap_or(true),
        )
    })();
    output(
        &mut env,
        match result {
            Ok(summary) => json!({"ok":true,"summary":summary}).to_string(),
            Err(error) => json!({"ok":false,"error":error}).to_string(),
        },
    )
}

#[no_mangle]
pub extern "system" fn Java_dev_secrettunnel_android_NativeBridge_generateKey(
    mut env: JNIEnv,
    _: JObject,
) -> jstring {
    let result =
        (|| -> Result<String, String> {
            use russh::keys::ssh_key::{Algorithm, LineEnding, PrivateKey};
            let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
                .map_err(|e| e.to_string())?;
            Ok(json!({"private":key.to_openssh(LineEnding::LF).map_err(|e| e.to_string())?.as_str(),
            "public":key.public_key().to_openssh().map_err(|e| e.to_string())?}).to_string())
        })();
    output(
        &mut env,
        result.unwrap_or_else(|e| json!({"error":e}).to_string()),
    )
}

#[no_mangle]
pub extern "system" fn Java_dev_secrettunnel_android_NativeBridge_registerKey(
    mut env: JNIEnv,
    _: JObject,
    config: JString,
    public: JString,
) -> jstring {
    let result = (|| -> Result<(), String> {
        let raw: String = env.get_string(&config).map_err(|e| e.to_string())?.into();
        let cfg: Config = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let public: String = env.get_string(&public).map_err(|e| e.to_string())?.into();
        let jumps = ssh_jumps(cfg.ssh_jumps)?;
        let mut server = EmbeddedServer {
            name: cfg.name,
            host: cfg.host,
            port: cfg.port,
            user: cfg.user,
            key_path: cfg.key_path,
            strict_host_key_checking: "yes".into(),
        };
        embedded_ssh::register_device_key(server.clone(), cfg.host_key_sha256.clone(), public, jumps.clone())?;
        if cfg.device_key_path.is_empty() {
            return Err("Missing device key verification path".into());
        }
        server.key_path = cfg.device_key_path;
        embedded_ssh::verify_login(server, cfg.host_key_sha256, jumps)
    })();
    output(&mut env, result.err().unwrap_or_default())
}
