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
const PROXY_ID: &str = "secret-tunnel-socks";
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
    let rule = |process: &str, protocol: &str, action: &str| {
        json!({
            "processNames": process, "targetHosts": "*", "targetPorts": "*",
            "protocol": protocol, "action": action, "enabled": true,
        })
    };
    let mut rules = vec![rule(&BYPASS.join(";"), "BOTH", "DIRECT")];
    let mut seen = std::collections::HashSet::new();
    for id in ids.iter().filter(|id| seen.insert(id.to_ascii_lowercase())) {
        rules.push(rule(id, if udp { "BOTH" } else { "TCP" }, PROXY_ID));
        // OpenSSH -D has no UDP support. Do not silently let selected UDP go direct.
        if !udp {
            rules.push(rule(id, "UDP", "BLOCK"));
        }
    }
    rules.push(rule("*", "BOTH", "DIRECT"));
    Ok(json!({"name": "Secret Tunnel", "dnsEnabled": false,
        "proxyConfigs": [{"id": PROXY_ID, "name": "SSH SOCKS", "type": "socks5",
            "host": host, "port": port, "sendDomain": false}], "proxyRules": rules}))
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

#[derive(Default)]
pub struct RedirectState {
    child: Option<Child>,
    progress: Arc<Mutex<Progress>>,
    started: Option<Instant>,
    error: String,
    selection: Selection,
    /// Connections the engine let go direct, by process name.
    direct: Arc<Mutex<HashMap<String, u64>>>,
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

/// One `ps -axo pid=,ppid=,comm=` line. Columns are right-aligned, so fields
/// are separated by runs of spaces; the path itself may contain spaces.
fn process_line(line: &str) -> Option<(u32, (u32, PathBuf))> {
    let (pid, rest) = line.trim_start().split_once(char::is_whitespace)?;
    let (ppid, path) = rest.trim_start().split_once(char::is_whitespace)?;
    let path = path.trim_start();
    (!path.is_empty()).then(|| Some((pid.parse().ok()?, (ppid.parse().ok()?, PathBuf::from(path)))))?
}

/// The process of an engine line that went direct:
/// `[12:00:00] TCP node -> 1.2.3.4:443 via Direct`.
fn direct_process(line: &str) -> Option<&str> {
    let (left, right) = line.split_once(" -> ")?;
    if !right.ends_with(" via Direct") {
        return None;
    }
    let left = left.trim_start();
    let left = if left.starts_with('[') { left.split_once("] ")?.1 } else { left };
    let (_, process) = left.split_once(' ')?; // protocol
    Some(process.trim()).filter(|process| !process.is_empty())
}

impl RedirectState {
    pub fn status(&mut self) -> RedirectStatus {
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(exit)) => {
                    self.child = None;
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

    pub fn start(
        &mut self,
        executable: &Path,
        value: &Value,
        logs: Arc<Mutex<Vec<String>>>,
        hide_sensitive: bool,
        selection: Selection,
    ) -> Result<(), String> {
        if self.child.is_some() {
            return Err("Перехват уже запущен или ожидает разрешения macOS".into());
        }
        self.selection = selection;
        self.direct = Arc::default();
        let executable = component(executable)?;
        self.progress = Arc::new(Mutex::new(Progress {
            message: "Ожидаем запуск расширения. При первом запуске разрешите ProxyBridge в настройках macOS".into(),
            ..Default::default()
        }));
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
        if let Some(stdout) = child.stdout.take() {
            let progress = self.progress.clone();
            let logs = logs.clone();
            let direct = self.direct.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if let Some(process) = direct_process(&line) {
                        if let Ok(mut direct) = direct.lock() {
                            if direct.len() < 2048 || direct.contains_key(process) {
                                *direct.entry(process.to_owned()).or_default() += 1;
                            }
                        }
                    }
                    #[cfg(feature = "embedded-ssh")]
                    crate::embedded_ssh::app_usage::note_route(&line);
                    let lifecycle = line.starts_with("Extension:")
                        || line.starts_with("Network configuration:")
                        || line == "Redirecting traffic. Press Ctrl+C to stop."
                        || line == "Rules cleared, tunnel stopped.";
                    if lifecycle {
                        if let Ok(mut p) = progress.lock() {
                            if line == "Redirecting traffic. Press Ctrl+C to stop." {
                                p.ready = true;
                                p.message = "Системное расширение перенаправляет новые соединения выбранных приложений".into();
                            } else {
                                p.message = line.clone();
                            }
                        }
                    }
                    if !hide_sensitive || lifecycle {
                        super::log_line(&logs, format!("Redirect macOS: {line}"));
                    }
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
                    if !hide_sensitive {
                        super::log_line(&logs, format!("Redirect macOS: {line}"));
                    }
                }
            });
        }
        self.started = Some(Instant::now());
        self.child = Some(child);
        self.error.clear();
        Ok(())
    }

    /// Programs of the selected applications, or started by them, whose
    /// connections went direct because no rule names them. macOS reports a
    /// flow under the program's own signing identifier, not its parent's, so
    /// tools an application launches (node, uv, python, MCP servers) are not
    /// covered by the application's rule.
    pub fn leaks(&self) -> Vec<Leak> {
        if self.child.is_none() || self.selection.bundles.is_empty() {
            return Vec::new();
        }
        let direct = self.direct.lock().map(|d| d.clone()).unwrap_or_default();
        if direct.is_empty() {
            return Vec::new();
        }
        let Ok(output) = std::process::Command::new("/bin/ps").args(["-axo", "pid=,ppid=,comm="]).output() else {
            return Vec::new();
        };
        let processes: HashMap<u32, (u32, PathBuf)> =
            String::from_utf8_lossy(&output.stdout).lines().filter_map(process_line).collect();
        let owner = |path: &Path| {
            self.selection.bundles.iter().find(|(_, bundle)| path.starts_with(bundle)).map(|(name, _)| name.clone())
        };
        let mut leaks: HashMap<String, Leak> = HashMap::new();
        for (pid, (_, path)) in &processes {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            let Some(&flows) = direct.get(name) else { continue };
            // `ps` shows a program started through PATH by its bare name.
            let path = &(if path.is_absolute() { None } else { executable_path(*pid) }).unwrap_or_else(|| path.clone());
            // The application it belongs to, or the nearest selected ancestor.
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
            let rule = crate::macos_apps::signing_identifier(path)
                .filter(|id| validate_ids(std::slice::from_ref(id)).is_ok())
                .or_else(|| Some(name.to_owned()).filter(|n| validate_ids(std::slice::from_ref(n)).is_ok()));
            let Some(rule) = rule else { continue };
            if self.selection.covered.contains(&rule.to_ascii_lowercase()) || self.selection.covered.contains(&name.to_ascii_lowercase()) {
                continue;
            }
            leaks.entry(rule.clone()).or_insert(Leak { app, process: name.to_owned(), path: path.to_string_lossy().into_owned(), rule, flows });
        }
        let mut leaks: Vec<_> = leaks.into_values().collect();
        leaks.sort_by(|a, b| b.flows.cmp(&a.flows).then_with(|| a.rule.cmp(&b.rule)));
        leaks
    }

    pub fn stop(&mut self) -> Result<(), String> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
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
        let (closed, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = input.read(&mut [0u8; 1]);
            let _ = closed.send(());
        });
        let outcome = loop {
            if let Some(exit) = cli.try_wait().map_err(|e| e.to_string())? {
                break Ok(exit.code().unwrap_or(1));
            }
            if rx.try_recv().is_ok() {
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
    fn process_lines_with_aligned_columns_and_spaces_in_paths() {
        assert_eq!(
            process_line("  801     1 /Applications/Secret Tunnel.app/Contents/MacOS/secret-tunnel"),
            Some((801, (1, PathBuf::from("/Applications/Secret Tunnel.app/Contents/MacOS/secret-tunnel"))))
        );
        assert_eq!(process_line("88417 88335 codex"), Some((88417, (88335, PathBuf::from("codex")))));
        assert_eq!(process_line("  12  "), None);
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
