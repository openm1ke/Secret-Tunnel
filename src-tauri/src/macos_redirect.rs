//! ProxyBridge macOS CLI adapter (profile contract checked against 4.1.2,
//! cb897b017a88a142ec803c3af138b0ad75c2ae12). No Windows profile translation.
use crate::app_redirect::{DomainStats, RedirectStatus};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

pub const EXECUTABLE: &str = "/Applications/ProxyBridge.app/Contents/MacOS/ProxyBridge";
const GUARDIAN_ARG: &str = "--macos-redirect-guardian";
/// Guardian command: hand the running rules over to the next CLI.
const HANDOFF: &str = "handoff";
const PROXY_ID: &str = "secret-tunnel-socks";
const DISCORD_UDP_PROXY_ID: &str = "secret-tunnel-discord-udp";
const BYPASS: &[&str] = &[
    "dev.secrettunnel.desktop",
    "com.apple.ssh",
    "org.openbsd.ssh",
    "com.interceptsuite.ProxyBridge",
    "com.interceptsuite.ProxyBridge.extension",
    "ssh",
    "secret-tunnel",
    "ProxyBridge",
];

fn component(executable: &Path) -> Result<PathBuf, String> {
    let executable = executable
        .canonicalize()
        .map_err(|_| "Установите ProxyBridge 4.1.2 или новее в /Applications".to_string())?;
    let app = executable
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or("Некорректный путь ProxyBridge")?;
    if !app.starts_with("/Applications") || app.join("Contents/MacOS/ProxyBridge") != executable {
        return Err("Выберите /Applications/ProxyBridge.app/Contents/MacOS/ProxyBridge".into());
    }
    let info = plist::Value::from_file(app.join("Contents/Info.plist"))
        .map_err(|_| "Не найден Info.plist установленного ProxyBridge".to_string())?;
    let info = info
        .as_dictionary()
        .ok_or("Некорректный Info.plist ProxyBridge")?;
    let version: Vec<u32> = info
        .get("CFBundleShortVersionString")
        .and_then(plist::Value::as_string)
        .unwrap_or_default()
        .split('.')
        .take(3)
        .filter_map(|part| part.parse().ok())
        .collect();
    if info
        .get("CFBundleIdentifier")
        .and_then(plist::Value::as_string)
        != Some("com.interceptsuite.ProxyBridge")
        || version.len() != 3
        || (version[0], version[1], version[2]) < (4, 1, 2)
    {
        return Err("Требуется ProxyBridge 4.1.2 или новее с поддержкой CLI".into());
    }
    Ok(executable)
}

pub fn component_available(executable: &Path) -> bool {
    component(executable).is_ok()
}

pub fn validate_ids(ids: &[String]) -> Result<(), String> {
    if ids.len() > 1024 {
        return Err("Максимум 1024 bundle ID".into());
    }
    for id in ids {
        if id.len() > 255
            || id.is_empty()
            || id.split('.').any(str::is_empty)
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        {
            return Err(format!(
                "Укажите точный bundle ID или идентификатор подписи macOS без масок: {id}"
            ));
        }
        if BYPASS.iter().any(|own| id.eq_ignore_ascii_case(own)) {
            return Err("Нельзя перехватывать собственный транспорт или движок".into());
        }
    }
    Ok(())
}

pub fn profile(
    ids: &[String],
    domains: &[secret_tunnel_routing::DomainRule],
    host: &str,
    port: u16,
    udp: bool,
) -> Result<Value, String> {
    profile_with_discord_udp(ids, domains, host, port, udp, None)
}

pub(crate) fn profile_with_discord_udp(
    ids: &[String],
    domains: &[secret_tunnel_routing::DomainRule],
    host: &str,
    port: u16,
    udp: bool,
    discord_udp_port: Option<u16>,
) -> Result<Value, String> {
    validate_ids(ids)?;
    if ids.is_empty() {
        return Err("Выберите хотя бы одно приложение".into());
    }
    if domains.iter().any(|rule| rule.enabled) {
        return Err("Доменные regex-правила пока доступны только в Windows. Отключите их перед запуском перехвата macOS".into());
    }
    if !host
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
        || port == 0
    {
        return Err("Перехват требует локальный SOCKS на loopback-адресе".into());
    }
    if discord_udp_port.is_some_and(|secondary| !udp || secondary == 0 || secondary == port) {
        return Err("Для UDP Discord нужен отдельный локальный SOCKS".into());
    }
    let rule = |process: &str, protocol: &str, action: &str| {
        json!({
            "processNames": process, "targetHosts": "*", "targetPorts": "*",
            "protocol": protocol, "action": action, "enabled": true,
        })
    };
    let mut rules = vec![rule(&BYPASS.join(";"), "BOTH", "DIRECT")];
    let mut seen = std::collections::HashSet::new();
    for id in ids.iter().filter(|id| seen.insert(id.to_ascii_lowercase())) {
        if discord_udp_port.is_some() && is_discord_process(id) {
            rules.push(rule(id, "UDP", DISCORD_UDP_PROXY_ID));
            rules.push(rule(id, "TCP", PROXY_ID));
        } else {
            rules.push(rule(id, if udp { "BOTH" } else { "TCP" }, PROXY_ID));
        }
        // OpenSSH -D has no UDP support. Do not silently let selected UDP go direct.
        if !udp {
            rules.push(rule(id, "UDP", "BLOCK"));
        }
    }
    rules.push(rule("*", "BOTH", "DIRECT"));
    let proxy = |id, name, port| json!({"id": id, "name": name, "type": "socks5",
        "host": host, "port": port, "sendDomain": false});
    let mut proxies = vec![proxy(PROXY_ID, "SSH SOCKS", port)];
    if let Some(port) = discord_udp_port {
        proxies.push(proxy(DISCORD_UDP_PROXY_ID, "Discord UDP test", port));
    }
    Ok(json!({"name": "Secret Tunnel", "dnsEnabled": false,
        "proxyConfigs": proxies, "proxyRules": rules}))
}

fn is_discord_process(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    matches!(id.as_str(), "discord" | "com.hnc.discord" | "com.hnc.discord.helper")
        || id.starts_with("com.hnc.discord.helper.")
        || id == "discord helper"
        || id.starts_with("discord helper (")
}

#[derive(Default)]
struct Progress {
    ready: bool,
    message: String,
    error: String,
}

/// The selected applications and every identifier the running rules cover.
#[derive(Clone, Default)]
pub struct Selection {
    /// Application name and bundle path.
    pub bundles: Vec<(String, PathBuf)>,
    /// Lower-case identifiers and process names in the rules.
    pub covered: HashSet<String>,
}

/// What the rules are built from.
#[derive(Clone)]
pub struct Plan {
    pub ids: Vec<String>,
    pub host: String,
    pub port: u16,
    pub udp: bool,
    pub discord_udp_port: Option<u16>,
    /// Add programs the selected applications start, while running.
    pub follow: bool,
}

/// A program started by a selected application whose connections went direct.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Leak {
    pub app: String,
    pub process: String,
    pub path: String,
    /// The rule that would intercept it: its signing identifier, else its name.
    pub rule: String,
    pub flows: u64,
}

/// A program of a selected application that was given a rule while running.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Followed {
    pub app: String,
    pub process: String,
    pub rule: String,
}

/// Shells and small utilities an application runs to do its work. They make
/// no connections of their own, and a rule naming them would send every
/// shell on the Mac through the tunnel.
const NOT_FOLLOWED: &[&str] = &[
    "zsh", "bash", "sh", "dash", "fish", "tcsh", "csh", "ksh", "login", "env", "sudo", "su", "nohup",
    "xargs", "sleep", "cat", "ls", "grep", "sed", "awk", "ps", "kill", "tee", "head", "tail", "sort",
    "uniq", "wc", "tr", "cut", "find", "mkdir", "rm", "cp", "mv", "chmod", "which", "dirname",
    "basename", "date", "uname", "sw_vers", "defaults", "open", "osascript", "launchctl", "codesign",
    "security", "plutil", "mdfind", "pgrep", "pkill", "lsof", "sysctl", "id", "whoami", "stat",
    "getconf", "locale", "arch", "ioreg", "hostname", "tput", "stty", "xattr", "ditto", "tar", "unzip",
];

#[derive(Default)]
pub struct RedirectState {
    child: Option<Child>,
    progress: Arc<Mutex<Progress>>,
    started: Option<Instant>,
    error: String,
    selection: Selection,
    /// Connections the engine let go direct, by process name.
    direct: Arc<Mutex<HashMap<String, u64>>>,
    /// Running programs of the selected applications: process name → app.
    related: Arc<Mutex<HashMap<String, String>>>,
    executable: Option<PathBuf>,
    logs: Option<Arc<Mutex<Vec<String>>>>,
    plan: Option<Plan>,
    followed: Vec<Followed>,
    /// Signing identifiers already read, by executable path.
    identities: HashMap<PathBuf, Option<String>>,
    /// Changes whenever interception starts or stops; ends a follow loop.
    generation: u64,
    /// Programs followed since the app started, applied again on the next
    /// start so short-lived tools are covered from their first connection.
    learned: Vec<Followed>,
    /// Set when a connection went direct: the follow loop looks right away.
    wake: Arc<(Mutex<bool>, std::sync::Condvar)>,
    watch: Option<Arc<Watch>>,
}

/// The executable of a running process.
fn executable_path(pid: u32) -> Option<PathBuf> {
    let mut buffer = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let length = unsafe { libc::proc_pidpath(pid as i32, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if length <= 0 {
        return None;
    }
    buffer.truncate(length as usize);
    Some(PathBuf::from(String::from_utf8(buffer).ok()?))
}

/// A running program that belongs to a selected application: inside its
/// bundle, or started by one of its processes.
#[derive(Clone)]
struct Member {
    pid: u32,
    app: String,
    name: String,
    path: PathBuf,
}

/// Every process with its parent and executable, read from the kernel
/// without starting `ps`, so following can look often.
fn processes() -> HashMap<u32, (u32, PathBuf)> {
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return HashMap::new();
    }
    let mut pids = vec![0 as libc::pid_t; count as usize + 64];
    let size = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
    pids.truncate(count.max(0) as usize);
    pids.into_iter()
        .filter(|&pid| pid > 0)
        .filter_map(|pid| {
            let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
            let read = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
            if read != size {
                return None;
            }
            Some((pid as u32, (info.pbi_ppid, executable_path(pid as u32)?)))
        })
        .collect()
}

/// Every running program of the selected applications.
fn members(bundles: &[(String, PathBuf)]) -> Vec<Member> {
    let processes = processes();
    let owner = |path: &Path| bundles.iter().find(|(_, bundle)| path.starts_with(bundle)).map(|(name, _)| name.clone());
    let mut found = Vec::new();
    for (pid, (_, path)) in &processes {
        let mut app = owner(path);
        let mut current = *pid;
        for _ in 0..32 {
            if app.is_some() {
                break;
            }
            let Some((parent, _)) = processes.get(&current) else { break };
            let Some((_, parent_path)) = processes.get(parent) else { break };
            app = owner(parent_path);
            current = *parent;
        }
        let Some(app) = app else { continue };
        let path = path.clone();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
        found.push(Member { pid: *pid, app, name, path });
    }
    found
}

/// Programs the selected applications start, reported by the kernel as
/// they start (kqueue process events), so even a tool that connects and
/// exits within a moment is seen and gets a rule for its next run.
#[derive(Default)]
struct Watch {
    /// Processes of the selected applications to watch, with their app.
    roots: Mutex<HashMap<u32, String>>,
    /// Programs started since the follow loop last looked.
    spawned: Mutex<Vec<Member>>,
    alive: std::sync::atomic::AtomicBool,
}

fn child_pids(pid: u32) -> Vec<u32> {
    let mut pids = vec![0 as libc::pid_t; 256];
    let size = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    let count = unsafe { libc::proc_listchildpids(pid as libc::pid_t, pids.as_mut_ptr().cast(), size) };
    pids.truncate(count.max(0) as usize);
    pids.into_iter().filter(|&p| p > 0).map(|p| p as u32).collect()
}

fn watch_processes(watch: Arc<Watch>, wake: Arc<(Mutex<bool>, std::sync::Condvar)>) {
    use std::sync::atomic::Ordering;
    let queue = unsafe { libc::kqueue() };
    if queue < 0 {
        return;
    }
    let register = |pid: u32| {
        let event = libc::kevent {
            ident: pid as usize,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_CLEAR,
            fflags: libc::NOTE_FORK | libc::NOTE_EXEC | libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        unsafe { libc::kevent(queue, &event, 1, std::ptr::null_mut(), 0, std::ptr::null()) == 0 }
    };
    let note = |pid: u32, app: &str| {
        let Some(path) = executable_path(pid) else { return };
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { return };
        if NOT_FOLLOWED.contains(&name.as_str()) {
            return;
        }
        if let Ok(mut spawned) = watch.spawned.lock() {
            if spawned.len() < 256 {
                spawned.push(Member { pid, app: app.to_owned(), name, path });
            }
        }
        let (flag, signal) = &*wake;
        if let Ok(mut flag) = flag.lock() {
            *flag = true;
            signal.notify_one();
        }
    };
    let mut watched: HashMap<u32, String> = HashMap::new();
    let mut events: Vec<libc::kevent> = vec![unsafe { std::mem::zeroed() }; 64];
    while watch.alive.load(Ordering::Relaxed) {
        let roots = watch.roots.lock().map(|r| r.clone()).unwrap_or_default();
        for (pid, app) in roots {
            if !watched.contains_key(&pid) && register(pid) {
                watched.insert(pid, app);
            }
        }
        let timeout = libc::timespec { tv_sec: 1, tv_nsec: 0 };
        let count = unsafe { libc::kevent(queue, std::ptr::null(), 0, events.as_mut_ptr(), events.len() as libc::c_int, &timeout) };
        for event in events.iter().take(count.max(0) as usize) {
            let pid = event.ident as u32;
            let Some(app) = watched.get(&pid).cloned() else { continue };
            if event.fflags & libc::NOTE_EXEC != 0 {
                note(pid, &app);
            }
            if event.fflags & libc::NOTE_FORK != 0 {
                for child in child_pids(pid) {
                    if !watched.contains_key(&child) && register(child) {
                        watched.insert(child, app.clone());
                        // The child may have run its program already.
                        note(child, &app);
                    }
                }
            }
            if event.fflags & libc::NOTE_EXIT != 0 {
                watched.remove(&pid);
            }
        }
        if watched.len() > 8192 {
            watched.clear();
        }
    }
    unsafe { libc::close(queue) };
}

#[derive(Debug, PartialEq)]
enum Route {
    Tunnel,
    Direct,
    Block,
}

/// A connection line of the engine:
/// `[12:00:00] TCP node -> 1.2.3.4:443 via SOCKS5 127.0.0.1:1080`.
#[derive(Debug, PartialEq)]
struct FlowLine<'a> {
    protocol: &'a str,
    process: &'a str,
    destination: &'a str,
    port: &'a str,
    route: Route,
}

fn flow_line(line: &str) -> Option<FlowLine<'_>> {
    let (left, right) = line.split_once(" -> ")?;
    let left = left.trim_start();
    let left = if left.starts_with('[') { left.split_once("] ")?.1 } else { left };
    let (protocol, process) = left.split_once(' ')?;
    if !matches!(protocol, "TCP" | "UDP") {
        return None;
    }
    let (target, via) = right.split_once(" via ")?;
    let (destination, port) = target.rsplit_once(':')?;
    let route = match via {
        "Direct" => Route::Direct,
        "BLOCK" => Route::Block,
        proxy if proxy.starts_with("SOCKS") => Route::Tunnel,
        _ => return None,
    };
    let destination = destination.trim_start_matches('[').trim_end_matches(']');
    Some(FlowLine { protocol, process: process.trim(), destination, port, route })
}

/// The process of an engine line that went direct.
#[cfg(test)]
fn direct_process(line: &str) -> Option<&str> {
    flow_line(line).filter(|flow| flow.route == Route::Direct).map(|flow| flow.process)
}

/// A readable journal entry for a connection, with the host name the
/// application looked up when the tunnel's DNS saw it.
fn describe_flow(flow: &FlowLine<'_>, app: Option<&str>) -> String {
    let verb = match flow.route {
        Route::Tunnel => "через туннель",
        Route::Direct => "напрямую",
        Route::Block => "заблокировано",
    };
    let who = match app {
        Some(app) if !flow.process.eq_ignore_ascii_case(app) && !flow.process.starts_with(app) => format!("{} ({app})", flow.process),
        _ => flow.process.to_owned(),
    };
    #[cfg(feature = "embedded-ssh")]
    let name = crate::embedded_ssh::dns_names::lookup(flow.destination);
    #[cfg(not(feature = "embedded-ssh"))]
    let name: Option<String> = None;
    let target = match name {
        Some(name) => format!("{name} ({})", flow.destination),
        None if flow.destination == "unknown" => "неизвестный адрес".to_owned(),
        None => flow.destination.to_owned(),
    };
    let port = if flow.port == "unknown" { String::new() } else { format!(":{}", flow.port) };
    format!("Соединение {verb}: {who} → {target}{port} {}", flow.protocol)
}

impl RedirectState {
    pub fn status(&mut self) -> RedirectStatus {
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(exit)) => {
                    self.child = None;
                    self.generation += 1;
                    self.end_watch();
                    let detail = self
                        .progress
                        .lock()
                        .map(|p| p.error.clone())
                        .unwrap_or_default();
                    self.error = if detail.is_empty() {
                        format!("Перехват macOS завершился: {exit}")
                    } else {
                        detail
                    };
                }
                Err(e) => self.error = e.to_string(),
                _ => {}
            }
        }
        if self.child.is_some()
            && self
                .started
                .is_some_and(|start| start.elapsed() > Duration::from_secs(330))
            && !self.progress.lock().map(|p| p.ready).unwrap_or(false)
        {
            let _ = self.stop();
            self.error = "Истекло время ожидания разрешения macOS. Проверьте Системные настройки → Конфиденциальность и безопасность".into();
        }
        let progress = self.progress.lock().unwrap_or_else(|p| p.into_inner());
        RedirectStatus {
            running: self.child.is_some() && progress.ready,
            starting: self.child.is_some() && !progress.ready,
            pid: self.child.as_ref().map(Child::id),
            administrator: false,
            last_error: self.error.clone(),
            component_available: false,
            platform: "macos".into(),
            requires_administrator: false,
            supports_system_ssh: true,
            supports_domains: false,
            setup_message: progress.message.clone(),
            domain_stats: DomainStats::default(),
        }
    }

    /// Start interception. Programs the applications are already running are
    /// included from the start.
    pub fn start(
        &mut self,
        executable: &Path,
        plan: Plan,
        logs: Arc<Mutex<Vec<String>>>,
        selection: Selection,
    ) -> Result<u64, String> {
        if self.child.is_some() {
            return Err("Перехват уже запущен или ожидает разрешения macOS".into());
        }
        self.executable = Some(component(executable)?);
        self.selection = selection;
        self.direct = Arc::default();
        self.followed.clear();
        self.logs = Some(logs);
        let follow = plan.follow;
        self.plan = Some(plan);
        self.end_watch();
        if follow {
            let watch = Arc::new(Watch::default());
            watch.alive.store(true, std::sync::atomic::Ordering::Relaxed);
            let (thread_watch, wake) = (watch.clone(), self.wake.clone());
            std::thread::spawn(move || watch_processes(thread_watch, wake));
            self.watch = Some(watch);
        }
        if follow {
            // Programs followed earlier in this session, for applications
            // still selected, then the ones running now.
            let names: HashSet<&String> = self.selection.bundles.iter().map(|(name, _)| name).collect();
            let learned: Vec<Followed> = self.learned.iter().filter(|f| names.contains(&f.app)).cloned().collect();
            self.adopt(learned);
            let found = self.unfollowed();
            self.adopt(found);
        } else {
            self.refresh_related();
        }
        let value = self.profile()?;
        self.progress = Arc::new(Mutex::new(Progress {
            message: "Ожидаем запуск расширения. При первом запуске разрешите ProxyBridge в настройках macOS".into(),
            ..Default::default()
        }));
        let child = self.spawn(&value)?;
        self.started = Some(Instant::now());
        self.child = Some(child);
        self.error.clear();
        self.generation += 1;
        Ok(self.generation)
    }

    fn profile(&self) -> Result<Value, String> {
        let plan = self.plan.as_ref().ok_or("Перехват не настроен")?;
        let mut ids = plan.ids.clone();
        ids.extend(self.followed.iter().map(|f| f.rule.clone()));
        profile_with_discord_udp(&ids, &[], &plan.host, plan.port, plan.udp, plan.discord_udp_port)
    }

    /// Start a guardian with a profile and read what it reports.
    fn spawn(&self, value: &Value) -> Result<Child, String> {
        let executable = self.executable.as_ref().ok_or("Не найден ProxyBridge")?;
        // This small process owns the CLI and watches our stdin pipe. EOF after
        // a GUI crash also sends SIGTERM, giving the extension time to clear rules.
        let mut child = super::background_command(
            std::env::current_exe()
                .map_err(|e| e.to_string())?
                .to_str()
                .ok_or("Некорректный путь клиента")?,
        )
        .arg(GUARDIAN_ARG)
        .arg(executable)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
        let mut payload = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        payload.push(b'\n');
        if let Err(e) = child
            .stdin
            .as_mut()
            .ok_or("Не открыт канал профиля")?
            .write_all(&payload)
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e.to_string());
        }
        let logs = self.logs.clone().unwrap_or_default();
        if let Some(stdout) = child.stdout.take() {
            #[cfg(feature = "macos-discord-udp-test")]
            let trace_discord = self.plan.as_ref().is_some_and(|p| p.discord_udp_port.is_some());
            let progress = self.progress.clone();
            let logs = logs.clone();
            let direct = self.direct.clone();
            let related = self.related.clone();
            let wake = self.wake.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    #[cfg(feature = "embedded-ssh")]
                    crate::embedded_ssh::app_usage::note_route(&line);
                    if let Some(flow) = flow_line(&line) {
                        #[cfg(feature = "macos-discord-udp-test")]
                        if trace_discord && is_discord_process(flow.process) {
                            eprintln!("DISCORD_ROUTE {line}");
                        }
                        let app = related.lock().ok().and_then(|r| r.get(flow.process).cloned());
                        if flow.route == Route::Direct {
                            let (flag, signal) = &*wake;
                            if let Ok(mut flag) = flag.lock() {
                                *flag = true;
                                signal.notify_one();
                            }
                            if let Ok(mut direct) = direct.lock() {
                                if direct.len() < 2048 || direct.contains_key(flow.process) {
                                    *direct.entry(flow.process.to_owned()).or_default() += 1;
                                }
                            }
                            // Other programs going direct is the normal case.
                            if app.is_none() {
                                continue;
                            }
                        }
                        super::log_line(&logs, describe_flow(&flow, app.as_deref()));
                        continue;
                    }
                    let lifecycle = line.starts_with("Extension:")
                        || line.starts_with("Network configuration:")
                        || line == "Redirecting traffic. Press Ctrl+C to stop."
                        || line == "Rules cleared, tunnel stopped.";
                    if lifecycle {
                        if let Ok(mut p) = progress.lock() {
                            if line == "Redirecting traffic. Press Ctrl+C to stop." {
                                p.ready = true;
                                p.message = "Системное расширение перенаправляет новые соединения выбранных приложений".into();
                            } else if !p.ready {
                                p.message = line.clone();
                            }
                        }
                    }
                    super::log_line(&logs, format!("Redirect macOS: {line}"));
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            let progress = self.progress.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if line.starts_with("ERROR:") {
                        if let Ok(mut p) = progress.lock() {
                            p.error = line.chars().take(1200).collect();
                        }
                    }
                    super::log_line(&logs, format!("Redirect macOS: {line}"));
                }
            });
        }
        Ok(child)
    }

    /// Apply the current rules without dropping connections: the running
    /// CLI is killed without cleanup, so the extension keeps its tunnel and
    /// rules, and a new CLI replaces only the rules (about a second).
    fn reload(&mut self) -> Result<(), String> {
        let value = self.profile()?;
        let Some(mut old) = self.child.take() else {
            return Err("Перехват не запущен".into());
        };
        if let Some(stdin) = old.stdin.as_mut() {
            let _ = stdin.write_all(format!("{HANDOFF}\n").as_bytes());
            let _ = stdin.flush();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !matches!(old.try_wait(), Ok(Some(_))) {
            if Instant::now() >= deadline {
                let _ = old.kill();
                let _ = old.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // The rules are still active; the new CLI only replaces them.
        self.progress = Arc::new(Mutex::new(Progress {
            ready: true,
            message: "Системное расширение перенаправляет новые соединения выбранных приложений".into(),
            ..Default::default()
        }));
        let child = self.spawn(&value)?;
        self.child = Some(child);
        Ok(())
    }

    /// Programs of the selected applications that no rule covers yet.
    fn unfollowed(&mut self) -> Vec<Followed> {
        let mut members = members(&self.selection.bundles);
        if let Some(watch) = &self.watch {
            if let Ok(mut roots) = watch.roots.lock() {
                roots.clear();
                roots.extend(members.iter().map(|m| (m.pid, m.app.clone())));
            }
            // Programs that started, and perhaps already exited, since the last look.
            if let Ok(mut spawned) = watch.spawned.lock() {
                members.append(&mut spawned);
            }
        }
        if let Ok(mut related) = self.related.lock() {
            related.clear();
            related.extend(members.iter().map(|m| (m.name.clone(), m.app.clone())));
        }
        let mut found: Vec<Followed> = Vec::new();
        for member in members {
            if NOT_FOLLOWED.contains(&member.name.as_str()) || self.selection.covered.contains(&member.name.to_ascii_lowercase()) {
                continue;
            }
            let identity = self
                .identities
                .entry(member.path.clone())
                .or_insert_with(|| crate::macos_apps::signing_identifier(&member.path))
                .clone();
            if identity.as_deref().is_some_and(|id| self.selection.covered.contains(&id.to_ascii_lowercase())) {
                continue;
            }
            let rule = identity
                .filter(|id| validate_ids(std::slice::from_ref(id)).is_ok())
                .or_else(|| Some(member.name.clone()).filter(|n| validate_ids(std::slice::from_ref(n)).is_ok()));
            let Some(rule) = rule else { continue };
            if !found.iter().any(|f| f.rule == rule) {
                found.push(Followed { app: member.app, process: member.name, rule });
            }
        }
        found
    }

    fn refresh_related(&mut self) {
        let members = members(&self.selection.bundles);
        if let Ok(mut related) = self.related.lock() {
            related.clear();
            related.extend(members.into_iter().map(|m| (m.name, m.app)));
        }
    }

    /// A rule matches a program by signing identifier or by file name, so
    /// only the rule itself is covered: another program with the same file
    /// name but a different signature is not.
    fn adopt(&mut self, found: Vec<Followed>) {
        for followed in found {
            if !self.selection.covered.insert(followed.rule.to_ascii_lowercase()) {
                continue;
            }
            if !self.learned.contains(&followed) {
                self.learned.push(followed.clone());
            }
            self.followed.push(followed);
        }
    }

    fn end_watch(&mut self) {
        if let Some(watch) = self.watch.take() {
            watch.alive.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Lets the follow loop sleep until a connection goes direct.
    pub fn waker(&self) -> Arc<(Mutex<bool>, std::sync::Condvar)> {
        self.wake.clone()
    }

    /// One pass of following the applications: give new programs a rule
    /// and apply it without dropping connections. Returns what was added.
    pub fn follow(&mut self, generation: u64) -> Option<Vec<Followed>> {
        if generation != self.generation || self.child.is_none() {
            return None;
        }
        let ready = self.progress.lock().map(|p| p.ready).unwrap_or(false);
        if !ready || !self.plan.as_ref().is_some_and(|plan| plan.follow) {
            self.refresh_related();
            return Some(Vec::new());
        }
        let found = self.unfollowed();
        if found.is_empty() {
            return Some(Vec::new());
        }
        self.adopt(found.clone());
        if let Err(error) = self.reload() {
            self.error = format!("Не удалось обновить правила перехвата: {error}");
            if let Some(logs) = &self.logs {
                super::log_line(logs, format!("Перехват: ошибка обновления правил: {error}"));
            }
            return None;
        }
        if let Some(logs) = &self.logs {
            let names: Vec<String> = found.iter().map(|f| format!("{} ({})", f.process, f.app)).collect();
            super::log_line(logs, format!("Перехват: без перезапуска добавлены {}", names.join(", ")));
        }
        Some(found)
    }

    pub fn followed(&self) -> Vec<Followed> {
        if self.child.is_none() {
            return Vec::new();
        }
        self.followed.clone()
    }

    /// Programs of the selected applications, or started by them, whose
    /// connections went direct because no rule names them. macOS reports a
    /// flow under the program's own signing identifier, not its parent's, so
    /// tools an application launches (node, uv, python, MCP servers) are not
    /// covered by the application's rule unless they are followed.
    pub fn leaks(&self) -> Vec<Leak> {
        if self.child.is_none() || self.selection.bundles.is_empty() {
            return Vec::new();
        }
        let direct = self.direct.lock().map(|d| d.clone()).unwrap_or_default();
        if direct.is_empty() {
            return Vec::new();
        }
        let mut leaks: HashMap<String, Leak> = HashMap::new();
        for member in members(&self.selection.bundles) {
            let Some(&flows) = direct.get(&member.name) else { continue };
            let rule = crate::macos_apps::signing_identifier(&member.path)
                .filter(|id| validate_ids(std::slice::from_ref(id)).is_ok())
                .or_else(|| Some(member.name.clone()).filter(|n| validate_ids(std::slice::from_ref(n)).is_ok()));
            let Some(rule) = rule else { continue };
            if self.selection.covered.contains(&rule.to_ascii_lowercase()) || self.selection.covered.contains(&member.name.to_ascii_lowercase()) {
                continue;
            }
            leaks.entry(rule.clone()).or_insert(Leak {
                app: member.app,
                process: member.name,
                path: member.path.to_string_lossy().into_owned(),
                rule,
                flows,
            });
        }
        let mut leaks: Vec<_> = leaks.into_values().collect();
        leaks.sort_by(|a, b| b.flows.cmp(&a.flows).then_with(|| a.rule.cmp(&b.rule)));
        leaks
    }

    pub fn stop(&mut self) -> Result<(), String> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        self.generation += 1;
        self.end_watch();
        // Closing the pipe is also the guardian's stop command.
        drop(child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            match child.try_wait() {
                Ok(Some(exit)) if exit.success() => {
                    self.error.clear();
                    return Ok(());
                }
                Ok(Some(exit)) => {
                    self.error = format!("Остановка перехвата macOS завершилась с ошибкой ({exit}). Проверьте ProxyBridge в системных настройках сети");
                    return Err(self.error.clone());
                }
                _ if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    self.error = "Не удалось подтвердить остановку расширения. Проверьте ProxyBridge в системных настройках сети".into();
                    return Err(self.error.clone());
                }
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
}
impl Drop for RedirectState {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Called before Tauri creates any windows. The parent supplies one compact JSON
/// line, then keeps stdin open for the lifetime of the routing session.
pub fn guardian_if_requested() -> bool {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).and_then(|s| s.to_str()) != Some(GUARDIAN_ARG) {
        return false;
    }
    let result = (|| -> Result<i32, String> {
        let executable = args.get(2).ok_or("Не указан ProxyBridge")?;
        let mut input = BufReader::new(std::io::stdin());
        let mut payload = String::new();
        input
            .by_ref()
            .take(4 * 1024 * 1024)
            .read_line(&mut payload)
            .map_err(|e| e.to_string())?;
        if !payload.ends_with('\n') {
            return Err("Профиль отсутствует или слишком большой".into());
        }
        let _: Value = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
        let mut cli =
            super::background_command(executable.to_str().ok_or("Некорректный путь ProxyBridge")?)
                .args(["--cli", "--profile", "-", "--verbose", "3"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?;
        let sent = cli
            .stdin
            .take()
            .ok_or("Не открыт канал ProxyBridge")?
            .write_all(payload.as_bytes());
        if let Err(e) = sent {
            let _ = terminate_cli(&mut cli);
            return Err(e.to_string());
        }
        let observed = Arc::new(Mutex::new((false, false)));
        let output_state = observed.clone();
        let output = cli.stdout.take().ok_or("Не открыт stdout ProxyBridge")?;
        let output_reader = std::thread::spawn(move || {
            let stdout = std::io::stdout();
            let mut writer = stdout.lock();
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                if let Ok(mut state) = output_state.lock() {
                    if line == "Redirecting traffic. Press Ctrl+C to stop." {
                        state.0 = true;
                    }
                    if line == "Rules cleared, tunnel stopped." {
                        state.1 = true;
                    }
                }
                // A dead GUI closes stdout too. Continue draining until CLI
                // cleanup completes instead of exiting on a broken parent pipe.
                let _ = writeln!(writer, "{line}");
                let _ = writer.flush();
            }
        });
        // EOF stops the CLI, which clears the rules. A `handoff` line instead
        // leaves the rules and the extension's tunnel running for the next CLI,
        // so rules change without dropping any connection.
        let (closed, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let handoff = input.read_line(&mut line).is_ok_and(|n| n > 0 && line.trim() == HANDOFF);
            let _ = closed.send(handoff);
        });
        let outcome = loop {
            if let Some(exit) = cli.try_wait().map_err(|e| e.to_string())? {
                break Ok(exit.code().unwrap_or(1));
            }
            if let Ok(handoff) = rx.try_recv() {
                if handoff {
                    let _ = cli.kill(); // SIGKILL: no cleanup, the rules stay.
                    let _ = cli.wait();
                    let _ = output_reader.join();
                    std::process::exit(0);
                }
                break terminate_cli(&mut cli);
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let _ = output_reader.join();
        let state = observed.lock().map_err(|e| e.to_string())?;
        // Upstream's five-second fallback can exit 0 without a provider reply.
        // Once ready, require its explicit cleanup acknowledgement as well.
        if outcome.as_ref().is_ok_and(|code| *code == 0) && state.0 && !state.1 {
            return Err("ProxyBridge завершился без подтверждения очистки правил. Проверьте системные настройки сети".into());
        }
        outcome
    })();
    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        }
    }
}

fn terminate_cli(cli: &mut Child) -> Result<i32, String> {
    if let Some(exit) = cli.try_wait().map_err(|e| e.to_string())? {
        return Ok(exit.code().unwrap_or(1));
    }
    // Child::kill is SIGKILL on Unix and would skip extension cleanup.
    if unsafe { libc::kill(cli.id() as i32, libc::SIGTERM) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        if let Some(exit) = cli.try_wait().map_err(|e| e.to_string())? {
            return Ok(exit.code().unwrap_or(1));
        }
        if Instant::now() >= deadline {
            let _ = cli.kill();
            let _ = cli.wait();
            return Err(
                "ProxyBridge не подтвердил очистку правил. Проверьте системные настройки сети"
                    .into(),
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_rules_bypass_transport_and_block_unsupported_udp() {
        let p = profile(&["org.example.App".into()], &[], "127.0.0.1", 1080, false).unwrap();
        let r = p["proxyRules"].as_array().unwrap();
        assert!(r[0]["processNames"]
            .as_str()
            .unwrap()
            .contains("com.apple.ssh"));
        assert_eq!(r[1]["action"], PROXY_ID);
        assert_eq!(r[1]["protocol"], "TCP");
        assert_eq!(r[2]["action"], "BLOCK");
        assert_eq!(r[2]["protocol"], "UDP");
        assert_eq!(r.last().unwrap()["action"], "DIRECT");
        assert_eq!(p["dnsEnabled"], false);
        let embedded = profile(&["org.example.App".into()], &[], "::1", 1080, true).unwrap();
        assert_eq!(embedded["proxyRules"][1]["protocol"], "BOTH");
        assert_eq!(embedded["proxyRules"].as_array().unwrap().len(), 3);
    }
    #[test]
    fn only_discord_udp_uses_the_test_proxy() {
        let ids = ["com.hnc.Discord", "com.hnc.Discord.helper.Renderer", "org.example.App"].map(str::to_owned);
        let p = profile_with_discord_udp(&ids, &[], "127.0.0.1", 1080, true, Some(1081)).unwrap();
        let rules = p["proxyRules"].as_array().unwrap();
        for pair in rules[1..5].chunks_exact(2) {
            assert_eq!(pair[0]["processNames"], pair[1]["processNames"]);
            assert_eq!(pair[0]["protocol"], "UDP");
            assert_eq!(pair[0]["action"], DISCORD_UDP_PROXY_ID);
            assert_eq!(pair[1]["protocol"], "TCP");
            assert_eq!(pair[1]["action"], PROXY_ID);
        }
        assert_eq!(rules[5]["protocol"], "BOTH");
        assert_eq!(rules[5]["action"], PROXY_ID);
        assert_eq!(p["proxyConfigs"][1]["port"], 1081);
        assert!(profile_with_discord_udp(&ids, &[], "127.0.0.1", 1080, false, Some(1081)).is_err());
        assert!(profile_with_discord_udp(&ids, &[], "127.0.0.1", 1080, true, Some(1080)).is_err());
    }
    #[test]
    fn rejects_broad_rules_transport_and_public_socks() {
        // Standalone signed tools need not have a reverse-DNS bundle ID.
        assert!(validate_ids(&["ShipIt".into(), "chrome_crashpad_handler".into()]).is_ok());
        for id in [
            "*",
            "com.example.*",
            "com.a;com.b",
            "Discord.exe/path",
            "com..app",
            "com.apple.ssh",
            "dev.secrettunnel.desktop",
        ] {
            assert!(validate_ids(&[id.into()]).is_err(), "{id}");
        }
        assert!(profile(&["org.example.App".into()], &[], "0.0.0.0", 1080, true).is_err());
        assert!(profile(&[], &[], "127.0.0.1", 1080, true).is_err());
        let domain = secret_tunnel_routing::DomainRule {
            id: "test".into(),
            pattern: "example".into(),
            enabled: true,
            action: secret_tunnel_routing::DomainAction::Tunnel,
        };
        assert!(profile(
            &["org.example.App".into()],
            &[domain],
            "127.0.0.1",
            1080,
            true
        )
        .is_err());
    }
    #[test]
    fn processes_are_read_from_the_kernel_with_their_parents() {
        let all = processes();
        let (parent, path) = all.get(&std::process::id()).expect("this test process");
        assert_eq!(Some(path.clone()), std::env::current_exe().ok());
        assert_eq!(*parent, std::os::unix::process::parent_id());
    }

    #[test]
    fn programs_a_watched_process_starts_are_reported_even_after_they_exit() {
        let watch = Arc::new(Watch::default());
        watch.alive.store(true, std::sync::atomic::Ordering::Relaxed);
        watch.roots.lock().unwrap().insert(std::process::id(), "Test".into());
        let wake: Arc<(Mutex<bool>, std::sync::Condvar)> = Arc::default();
        let thread = { let (w, k) = (watch.clone(), wake.clone()); std::thread::spawn(move || watch_processes(w, k)) };
        std::thread::sleep(Duration::from_millis(300));
        // A program that runs for a moment, like `nc -z`.
        std::process::Command::new("/usr/bin/true").status().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let seen = loop {
            let seen = watch.spawned.lock().unwrap().iter().any(|m| m.name == "true" && m.app == "Test");
            if seen || Instant::now() > deadline {
                break seen;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        watch.alive.store(false, std::sync::atomic::Ordering::Relaxed);
        thread.join().unwrap();
        assert!(seen, "{:?}", watch.spawned.lock().unwrap().iter().map(|m| m.name.clone()).collect::<Vec<_>>());
        assert!(*wake.0.lock().unwrap());
    }

    #[test]
    fn a_rule_covers_its_identifier_not_every_program_with_that_name() {
        let mut state = RedirectState::default();
        state.adopt(vec![Followed { app: "Claude".into(), process: "python".into(), rule: "python-0123abcd".into() }]);
        assert!(state.selection.covered.contains("python-0123abcd"));
        // Homebrew's Python has another signature and still needs its own rule.
        assert!(!state.selection.covered.contains("python"));
        // Adopting the same rule twice keeps one entry.
        state.adopt(vec![Followed { app: "Claude".into(), process: "python".into(), rule: "python-0123abcd".into() }]);
        assert_eq!(state.followed.len(), 1);
        assert_eq!(state.learned.len(), 1);
    }

    #[test]
    fn the_executable_of_a_running_process_is_found() {
        assert_eq!(executable_path(std::process::id()), std::env::current_exe().ok());
    }

    #[test]
    fn direct_lines_name_their_process() {
        assert_eq!(direct_process("[12:00:01] TCP node -> 1.2.3.4:443 via Direct"), Some("node"));
        assert_eq!(direct_process("[12:00:01] UDP Codex (Renderer) -> 1.2.3.4:53 via Direct"), Some("Codex (Renderer)"));
        assert_eq!(direct_process("[12:00:01] TCP node -> 1.2.3.4:443 via SOCKS5 127.0.0.1:1080"), None);
        assert_eq!(direct_process("Redirecting traffic. Press Ctrl+C to stop."), None);
    }

    #[test]
    fn a_program_started_by_a_selected_app_that_went_direct_is_a_leak() {
        // This test process stands in for a tool the selected app started.
        let me = std::env::current_exe().unwrap();
        let name = me.file_name().unwrap().to_str().unwrap().to_owned();
        let mut state = RedirectState::default();
        // Not running: nothing to report.
        assert!(state.leaks().is_empty());
        state.child = Some(std::process::Command::new("/bin/sleep").arg("5").spawn().unwrap());
        state.direct.lock().unwrap().insert(name.clone(), 3);
        // The test binary lives under target/; treat that directory as the app.
        let bundle = me.parent().unwrap().to_path_buf();
        state.selection = Selection { bundles: vec![("Test".into(), bundle)], covered: HashSet::new() };
        let leaks = state.leaks();
        let _ = state.child.take().map(|mut c| { let _ = c.kill(); c.wait() });
        assert!(leaks.iter().any(|leak| leak.process == name && leak.flows == 3 && leak.app == "Test"), "{leaks:?}");
    }

    #[test]
    fn connection_lines_are_parsed_and_described() {
        let flow = flow_line("[14:15:49] TCP Discord Helper (Renderer) -> 162.159.135.232:443 via SOCKS5 127.0.0.1:1080").unwrap();
        assert_eq!(flow, FlowLine { protocol: "TCP", process: "Discord Helper (Renderer)", destination: "162.159.135.232", port: "443", route: Route::Tunnel });
        assert_eq!(describe_flow(&flow, Some("Discord")), "Соединение через туннель: Discord Helper (Renderer) → 162.159.135.232:443 TCP");
        let flow = flow_line("[14:15:49] UDP node -> [2606:4700::1]:53 via Direct").unwrap();
        assert_eq!((flow.destination, &flow.route), ("2606:4700::1", &Route::Direct));
        assert_eq!(describe_flow(&flow, Some("ChatGPT")), "Соединение напрямую: node (ChatGPT) → 2606:4700::1:53 UDP");
        let flow = flow_line("[14:15:49] UDP x -> unknown:unknown via BLOCK").unwrap();
        assert_eq!(describe_flow(&flow, None), "Соединение заблокировано: x → неизвестный адрес UDP");
        assert!(flow_line("Profile \"Secret Tunnel\": 1 proxies, 3 rules applied").is_none());
    }

    #[test]
    fn programs_of_a_selected_app_without_a_rule_are_followed() {
        // This test binary stands in for a tool the selected app started.
        let me = std::env::current_exe().unwrap();
        let name = me.file_name().unwrap().to_str().unwrap().to_owned();
        let mut state = RedirectState::default();
        state.selection = Selection { bundles: vec![("Test".into(), me.parent().unwrap().to_path_buf())], covered: HashSet::new() };
        let found = state.unfollowed();
        let mine = found.iter().find(|f| f.process == name).expect("the running test binary");
        assert_eq!(mine.app, "Test");
        assert_eq!(state.related.lock().unwrap().get(&name).map(String::as_str), Some("Test"));
        // Once adopted it is covered and not found again.
        state.adopt(found);
        assert!(state.unfollowed().iter().all(|f| f.process != name));
    }

    #[test]
    fn graceful_stop_sends_sigterm_instead_of_sigkill() {
        let mut cli = std::process::Command::new("/bin/sh")
            .args([
                "-c",
                "trap 'exit 0' TERM; echo ready; while :; do sleep 0.1; done",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(cli.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(terminate_cli(&mut cli).unwrap(), 0);
    }
}
