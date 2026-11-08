use jni::{
    objects::{JObject, JString, JValue},
    sys::jstring,
    JNIEnv,
};
use secret_tunnel_core::embedded_ssh::{
    self, EmbeddedOptions, EmbeddedProxy, EmbeddedServer, EmbeddedTunnelHandle,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};

#[derive(Deserialize)]
struct Config {
    host: String,
    port: u16,
    user: String,
    key_path: String,
    host_key_sha256: String,
    socks_port: u16,
    #[serde(default)]
    device_key_path: String,
    #[serde(default)]
    new_session: bool,
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
        embedded_ssh::start_with_options(
            EmbeddedServer {
                name: "Android".into(),
                host: cfg.host,
                port: cfg.port,
                user: cfg.user,
                key_path: cfg.key_path,
                strict_host_key_checking: "yes".into(),
            },
            EmbeddedProxy {
                host: "127.0.0.1".into(),
                port: cfg.socks_port,
            },
            logs(),
            EmbeddedOptions {
                host_key_sha256: Some(cfg.host_key_sha256),
                observed_host_key: Some(observed()),
                protect_socket: Some(protect),
            },
        )
    })();
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
#[no_mangle]
pub extern "system" fn Java_dev_secrettunnel_android_NativeBridge_status(
    mut env: JNIEnv,
    _this: JObject,
) -> jstring {
    let mut s = state().lock().unwrap();
    if let Some(error) = s.tunnel.as_ref().and_then(|t| t.try_wait()) {
        s.error = error;
    }
    let current = s
        .tunnel
        .as_ref()
        .map(|t| t.traffic_totals())
        .unwrap_or_default();
    let totals = (
        s.totals.0.saturating_add(current.0),
        s.totals.1.saturating_add(current.1),
    );
    let value=json!({"running":s.tunnel.is_some()&&s.error.is_empty(),"starting":s.starting,"error":s.error,"rx":totals.0,"tx":totals.1,"observed_host_key":*observed().lock().unwrap(),"logs":*logs().lock().unwrap()}).to_string();
    output(&mut env, value)
}

#[no_mangle]
pub extern "system" fn Java_dev_secrettunnel_android_NativeBridge_probe(
    mut env: JNIEnv,
    _: JObject,
    host: JString,
    port: i32,
) -> jstring {
    let result = (|| -> Result<String, String> {
        let host: String = env.get_string(&host).map_err(|e| e.to_string())?.into();
        if host.trim().is_empty() || !(1..=65535).contains(&port) {
            return Err("Invalid server address or port".into());
        }
        embedded_ssh::probe_server(&host, port as u16)
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
        let mut server = EmbeddedServer {
            name: "Android".into(),
            host: cfg.host,
            port: cfg.port,
            user: cfg.user,
            key_path: cfg.key_path,
            strict_host_key_checking: "yes".into(),
        };
        embedded_ssh::register_device_key(server.clone(), cfg.host_key_sha256.clone(), public)?;
        if cfg.device_key_path.is_empty() {
            return Err("Missing device key verification path".into());
        }
        server.key_path = cfg.device_key_path;
        embedded_ssh::verify_login(server, cfg.host_key_sha256)
    })();
    output(&mut env, result.err().unwrap_or_default())
}
