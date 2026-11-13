//! Isolated userspace WFP engine lifecycle and routing telemetry.
use secret_tunnel_routing::{DomainAction, DomainRule};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, Stdio},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct RedirectState {
    child: Option<Child>,
    error: String,
    stats: Arc<Mutex<DomainStats>>,
}
#[derive(Serialize)]
pub struct RedirectStatus {
    pub running: bool,
    pub starting: bool,
    pub platform: String,
    pub requires_administrator: bool,
    pub supports_system_ssh: bool,
    pub supports_domains: bool,
    pub setup_message: String,
    pub pid: Option<u32>,
    pub administrator: bool,
    pub last_error: String,
    pub component_available: bool,
    pub domain_stats: DomainStats,
}
#[derive(Clone, Default, Serialize)]
pub struct RuleStats {
    pub id: String,
    pub observed: u64,
    pub tunnel: u64,
    pub direct: u64,
}
#[derive(Clone, Default, Serialize)]
pub struct DomainStats {
    pub rules: Vec<RuleStats>,
    pub unknown: u64,
    pub events: u64,
    pub dropped_output_lines: u64,
    #[serde(skip)]
    patterns: Vec<(DomainRule, DomainPredicate)>,
}
// Keep compilation off the log reader's hot path without exposing Regex in JSON.
type DomainPredicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;
impl DomainStats {
    fn from_profile(value: &Value) -> Self {
        let domains: Vec<DomainRule> =
            serde_json::from_value(value["SecretTunnelDomains"].clone()).unwrap_or_default();
        Self {
            rules: domains
                .iter()
                .map(|r| RuleStats {
                    id: r.id.clone(),
                    ..Default::default()
                })
                .collect(),
            patterns: domains
                .into_iter()
                .filter_map(|r| {
                    let re = secret_tunnel_routing::compile(&r.pattern).ok()?;
                    let predicate: DomainPredicate = Arc::new(move |d| re.is_match(d));
                    Some((r, predicate))
                })
                .collect(),
            ..Default::default()
        }
    }
    fn event(&mut self, line: &str) -> bool {
        if let Some((prefix, _)) = line.split_once(" output line(s) dropped") {
            if let Some(count) = prefix.strip_prefix('[').and_then(|n| n.parse::<u64>().ok()) {
                self.dropped_output_lines = self.dropped_output_lines.saturating_add(count);
            }
            return true;
        }
        let Some((_, body)) = line.split_once("[ST_ROUTE] ") else {
            return false;
        };
        let parts: Vec<_> = body.split_whitespace().collect();
        if parts.len() != 3 {
            return true;
        }
        let Ok(rule_id) = parts[0].parse::<usize>() else {
            return true;
        };
        self.events += 1;
        if parts[2] == "-" {
            self.unknown += 1;
            return true;
        }
        let hex = parts[2];
        if hex.len() > 510 || hex.len() % 2 != 0 || !hex.is_ascii() {
            return true;
        }
        let bytes: Option<Vec<u8>> = hex
            .as_bytes()
            .chunks_exact(2)
            .map(|b| u8::from_str_radix(std::str::from_utf8(b).ok()?, 16).ok())
            .collect();
        let Some(domain) = bytes
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|d| secret_tunnel_routing::normalize(&d).ok())
        else {
            return true;
        };
        for (i, ((rule, matches), stats)) in
            self.patterns.iter().zip(self.rules.iter_mut()).enumerate()
        {
            if matches(&domain) {
                stats.observed += 1;
                if rule.enabled && rule_id == i + 2 {
                    if parts[1] == "tunnel" {
                        stats.tunnel += 1;
                    } else if parts[1] == "direct" {
                        stats.direct += 1;
                    }
                }
            }
        }
        true
    }
}
pub fn administrator() -> bool {
    #[cfg(windows)]
    {
        #[link(name = "shell32")]
        extern "system" {
            fn IsUserAnAdmin() -> i32;
        }
        unsafe { IsUserAnAdmin() != 0 }
    }
    #[cfg(not(windows))]
    {
        false
    }
}
pub fn profile(
    processes: &[String],
    host: &str,
    port: u16,
    own_exe: &str,
) -> Result<Value, String> {
    if !host
        .parse::<std::net::IpAddr>()
        .map(|i| i.is_loopback())
        .unwrap_or(false)
        || port == 0
    {
        return Err("Перехват требует локальный SOCKS на loopback-адресе".into());
    }
    if processes.len() > 1024 {
        return Err("Максимум 1024 процесса".into());
    }
    let mut names = Vec::new();
    for process in processes {
        let name = process.trim().replace('/', "\\").to_lowercase();
        let b = name.as_bytes();
        let absolute = b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
        let base = name.rsplit('\\').next().unwrap_or("");
        let valid_name = base.ends_with(".exe")
            && base
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | ' '));
        let valid_path = if absolute {
            !name.chars().any(|c| {
                c.is_control() || matches!(c, '*' | '?' | ';' | ',' | '"' | '<' | '>' | '|')
            }) && !name[3..].contains(':')
                && !name[3..]
                    .split('\\')
                    .any(|p| p.is_empty() || matches!(p, "." | ".."))
        } else {
            !name.contains('\\') && !name.contains(':')
        };
        if name.len() > 250 || !valid_name || !valid_path {
            return Err(format!(
                "Укажите имя exe или полный локальный путь без масок: {process}"
            ));
        }
        if base == own_exe.to_lowercase()
            || base.starts_with("secrettunnel")
            || base.starts_with("secret-tunnel")
            || matches!(
                base,
                "ssh.exe" | "proxybridge_cli.exe" | "proxybridge.exe" | "proxybridgesvc.exe"
            )
        {
            return Err("Нельзя перехватывать собственный транспорт или движок".into());
        }
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let rule = |name: &str, process: &str, action: &str| json!({"Name":name,"ProcessName":process,"TargetHosts":"*","TargetPorts":"*","TargetDomains":"*","Protocol":"BOTH","Action":action,"IsEnabled":true,"ProxyConfigId":1,"LocalhostViaProxy":false});
    let mut rules = vec![rule(
        "Transport bypass",
        &format!("{own_exe};ssh.exe;ProxyBridge_CLI.exe;ProxyBridgeSvc.exe"),
        "DIRECT",
    )];
    let mut chunk = String::new();
    for name in names {
        if !chunk.is_empty() && chunk.len() + name.len() + 1 > 4096 {
            rules.push(rule("Selected applications", &chunk, "PROXY"));
            chunk.clear();
        }
        if !chunk.is_empty() {
            chunk.push(';');
        }
        chunk.push_str(&name);
    }
    if !chunk.is_empty() {
        rules.push(rule("Selected applications", &chunk, "PROXY"));
    }
    rules.push(rule("Other applications", "*", "DIRECT"));
    Ok(
        json!({"Version":"1.0","IsTrafficLoggingEnabled":true,"LocalhostViaProxy":false,"ProxyConfigs":[{"Id":1,"Name":"Secret Tunnel","Type":"SOCKS5","Host":host,"Port":port.to_string(),"Username":"","Password":"","SendDomainToProxy":true}],"ProxyRules":rules}),
    )
}
pub fn domain_profile(
    processes: &[String],
    domains: &[DomainRule],
    host: &str,
    port: u16,
    own: &str,
) -> Result<Value, String> {
    domain_profile_with_priority_udp(processes, domains, host, port, own, None)
}

pub fn domain_profile_with_priority_udp(
    processes: &[String],
    domains: &[DomainRule],
    host: &str,
    port: u16,
    own: &str,
    priority_udp: Option<(u16, &[String])>,
) -> Result<Value, String> {
    secret_tunnel_routing::validate(domains)?;
    let mut value = profile(processes, host, port, own)?;
    let rules = value["ProxyRules"].as_array_mut().unwrap();
    for (i, d) in domains.iter().enumerate() {
        rules.insert(i+1,json!({"Name":format!("Domain {}",d.id),"ProcessName":"*","TargetHosts":"*","TargetPorts":"*","TargetDomains":secret_tunnel_routing::encoded(&d.pattern),"Protocol":"BOTH","Action":if d.action==DomainAction::Tunnel {"PROXY"} else {"DIRECT"},"IsEnabled":d.enabled,"ProxyConfigId":1,"LocalhostViaProxy":false}));
    }
    if let Some((secondary, marked)) = priority_udp {
        if secondary == 0 || secondary == port {
            return Err("Для второго SSH нужен отдельный локальный SOCKS".into());
        }
        let normalize = |name: &str| name.trim().replace('/', "\\").to_lowercase();
        let marked: std::collections::HashSet<_> = marked.iter().map(|name| normalize(name)).collect();
        // Generate UDP rules only from validated application rules. Explicit
        // domain exceptions retain their earlier position in the profile.
        let mut priority_rules = Vec::new();
        for rule in rules.iter().skip(domains.len() + 1).filter(|r| r["Action"] == "PROXY") {
            let names = rule["ProcessName"].as_str().unwrap_or_default().split(';')
                .filter(|name| marked.contains(*name)).collect::<Vec<_>>().join(";");
            if names.is_empty() { continue; }
            let mut udp_rule = rule.clone();
            udp_rule["Name"] = json!("UDP via second SSH");
            udp_rule["ProcessName"] = json!(names);
            udp_rule["Protocol"] = json!("UDP");
            udp_rule["ProxyConfigId"] = json!(2);
            priority_rules.push(udp_rule);
        }
        if !priority_rules.is_empty() {
            rules.splice(domains.len() + 1..domains.len() + 1, priority_rules);
            let proxies = value["ProxyConfigs"].as_array_mut().unwrap();
            let mut second = proxies[0].clone();
            second["Id"] = json!(2);
            second["Name"] = json!("UDP");
            second["Port"] = json!(secondary.to_string());
            proxies.push(second);
        }
    }
    value["SecretTunnelDomains"] = serde_json::to_value(domains).map_err(|e| e.to_string())?;
    Ok(value)
}
pub fn discord_processes() -> Vec<String> {
    let mut processes = vec!["Discord.exe".into()];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        for (dir, exe) in [
            ("Discord", "Discord.exe"),
            ("DiscordCanary", "DiscordCanary.exe"),
            ("DiscordPTB", "DiscordPTB.exe"),
        ] {
            let dir = std::path::PathBuf::from(&local).join(dir);
            if dir.is_dir() {
                if !processes.iter().any(|n| n == exe) {
                    processes.push(exe.into());
                }
                let updater = dir.join("Update.exe");
                if updater.is_file() {
                    processes.push(updater.to_string_lossy().into_owned());
                }
            }
        }
    }
    processes
}
impl RedirectState {
    pub fn status(&mut self) -> RedirectStatus {
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(exit)) => {
                    self.error = format!("Движок перехвата завершился: {exit}");
                    self.child = None;
                }
                Err(e) => self.error = e.to_string(),
                _ => {}
            }
        }
        RedirectStatus {
            running: self.child.is_some(),
            starting: false,
            platform: std::env::consts::OS.into(),
            requires_administrator: cfg!(windows),
            supports_system_ssh: false,
            supports_domains: cfg!(windows),
            setup_message: String::new(),
            pid: self.child.as_ref().map(Child::id),
            administrator: administrator(),
            last_error: self.error.clone(),
            component_available: false,
            domain_stats: self.stats.lock().map(|s| s.clone()).unwrap_or_default(),
        }
    }
    pub fn start(
        &mut self,
        executable: &Path,
        profile_path: &Path,
        value: &Value,
        logs: Arc<Mutex<Vec<String>>>,
        hide_sensitive: bool,
    ) -> Result<(), String> {
        if self.status().running {
            return Err("Перехват уже запущен; остановите его перед изменением правил".into());
        }
        if !cfg!(windows) {
            return Err("Перехват пока поддерживается только в Windows".into());
        }
        if !administrator() {
            return Err("Запустите Secret Tunnel от имени администратора".into());
        }
        let executable = executable
            .canonicalize()
            .map_err(|_| "Не найден ProxyBridge_CLI.exe".to_string())?;
        if executable.file_name().and_then(|n| n.to_str()) != Some("ProxyBridge_CLI.exe") {
            return Err("Ожидался ProxyBridge_CLI.exe".into());
        }
        let directory = executable.parent().ok_or("Нет каталога движка")?;
        for file in [
            "ProxyBridgeCore.dll",
            "WinDivert.dll",
            "WinDivert64.sys",
            "source-revision.txt",
        ] {
            if !directory.join(file).is_file() {
                return Err(format!("В комплекте отсутствует {file}"));
            }
        }
        let revision = std::fs::read_to_string(directory.join("source-revision.txt"))
            .map_err(|e| e.to_string())?;
        if !revision.contains("cb897b017a88a142ec803c3af138b0ad75c2ae12")
            || !revision.contains("Secret Tunnel patch: stdin stop + parent EOF cleanup.")
        {
            return Err(
                "Требуется адаптированный CLI с остановкой при закрытии Secret Tunnel".into(),
            );
        }
        if value["SecretTunnelDomains"]
            .as_array()
            .is_some_and(|r| !r.is_empty())
            && !revision.contains("domain-regex-v1")
        {
            return Err("Для доменов нужен комплект domain-regex-v1. Выберите новый компонент в разделе Приложения → Дополнительно".into());
        }
        if value["ProxyConfigs"].as_array().is_some_and(|proxies| proxies.len() > 1)
            && !revision.contains("webview-owner-v1")
        {
            return Err("Для второго SSH обновите компонент перехвата в разделе «Приложения» → «Дополнительно».".into());
        }
        self.stats = Arc::new(Mutex::new(DomainStats::from_profile(value)));
        std::fs::write(
            profile_path,
            serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let mut child =
            super::background_command(executable.to_str().ok_or("Некорректный путь движка")?)
                .args([
                    "--profile",
                    profile_path.to_str().ok_or("Некорректный путь профиля")?,
                    "--verbose",
                    "3",
                ])
                .current_dir(directory)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?;
        let (ready, result) = mpsc::channel();
        if let Some(stdout) = child.stdout.take() {
            let logs = logs.clone();
            let stats = self.stats.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    #[cfg(feature = "embedded-ssh")]
                    crate::embedded_ssh::app_usage::note_route(&line);
                    if line.contains("ProxyBridge is running.") {
                        let _ = ready.send(());
                    }
                    if stats.lock().map(|mut s| s.event(&line)).unwrap_or(false) {
                        continue;
                    }
                    let message = if hide_sensitive {
                        if line.contains("ProxyBridge is running.") {
                            "Перехват приложений запущен".to_string()
                        } else if line.starts_with("[CONN]") {
                            format!(
                                "{} → адрес скрыт",
                                line.split(" -> ").next().unwrap_or("[CONN]")
                            )
                        } else {
                            continue;
                        }
                    } else {
                        line
                    };
                    super::log_line(&logs, format!("Redirect: {message}"));
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            super::attach_log_reader("Redirect error", stderr, logs);
        }
        self.child = Some(child);
        self.error.clear();
        if result.recv_timeout(Duration::from_secs(12)).is_err() {
            self.stop();
            self.error =
                "Движок не подтвердил запуск. Проверьте Runtime logs и комплект драйвера".into();
            return Err(self.error.clone());
        }
        Ok(())
    }
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if let Some(mut input) = child.stdin.take() {
                let _ = input.write_all(b"stop\n");
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    _ if Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    _ => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        }
    }
}
impl Drop for RedirectState {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scoped_processes_and_transport() {
        for name in [
            "*.exe",
            "Discord.exe;*",
            "ssh.exe",
            "C:Discord.exe",
            r"C:\Tools\..\Discord.exe",
            r"\\server\share\App.exe",
        ] {
            assert!(profile(&[name.into()], "127.0.0.1", 1080, "client.exe").is_err());
        }
        let p = profile(
            &[
                "Discord.exe".into(),
                "Discord.exe".into(),
                r"C:\Users\Test User\Discord\Update.exe".into(),
            ],
            "127.0.0.1",
            1080,
            "SecretTunnel.exe",
        )
        .unwrap();
        assert_eq!(p["ProxyRules"][0]["Action"], "DIRECT");
        assert_eq!(
            p["ProxyRules"][1]["ProcessName"],
            r"discord.exe;c:\users\test user\discord\update.exe"
        );
        assert!(profile(&[], "0.0.0.0", 1080, "client.exe").is_err());
    }
    #[test]
    fn domain_order_disabled_and_counters() {
        let rules = vec![
            DomainRule {
                id: "exception".into(),
                pattern: r"(?:.*\.)?example\.com".into(),
                enabled: true,
                action: DomainAction::Direct,
            },
            DomainRule {
                id: "all".into(),
                pattern: ".*".into(),
                enabled: false,
                action: DomainAction::Tunnel,
            },
        ];
        let p = domain_profile(
            &["Firefox.exe".into()],
            &rules,
            "127.0.0.1",
            1080,
            "client.exe",
        )
        .unwrap();
        assert_eq!(p["ProxyRules"][1]["Action"], "DIRECT");
        assert_eq!(p["ProxyRules"][2]["IsEnabled"], false);
        assert_eq!(p["ProxyRules"][3]["ProcessName"], "firefox.exe");
        let mut stats = DomainStats::from_profile(&p);
        stats.event("[LOG]  [ST_ROUTE] 2 direct 6578616d706c652e636f6d");
        stats.event("[LOG]  [ST_ROUTE] 4 tunnel -");
        assert_eq!(stats.rules[0].direct, 1);
        assert_eq!(stats.rules[1].observed, 1);
        assert_eq!(stats.rules[1].tunnel, 0);
        assert_eq!(stats.unknown, 1);
        assert!(domain_profile(&[], &rules, "127.0.0.1", 1080, "client.exe").is_ok());
    }
    #[test]
    fn marked_windows_app_gets_only_a_udp_override_after_domain_rules() {
        let apps = ["Dorion.exe", "Firefox.exe"].map(str::to_owned);
        let marked = ["dorion.EXE", "unselected.exe"].map(str::to_owned);
        let domains = [DomainRule {
            id: "exception".into(), pattern: "example.com".into(),
            enabled: true, action: DomainAction::Direct,
        }];
        let p = domain_profile_with_priority_udp(&apps, &domains, "127.0.0.1", 1080, "client.exe", Some((1081, &marked))).unwrap();
        assert_eq!(p["ProxyRules"][1]["Action"], "DIRECT");
        assert_eq!(p["ProxyRules"][2]["ProcessName"], "dorion.exe");
        assert_eq!(p["ProxyRules"][2]["Protocol"], "UDP");
        assert_eq!(p["ProxyRules"][2]["ProxyConfigId"], 2);
        assert_eq!(p["ProxyRules"][3]["ProcessName"], "dorion.exe;firefox.exe");
        assert_eq!(p["ProxyRules"][3]["Protocol"], "BOTH");
        assert_eq!(p["ProxyRules"][3]["ProxyConfigId"], 1);
        assert_eq!(p["ProxyConfigs"][1]["Port"], "1081");
    }

    #[test]
    fn long_lists_are_chunked() {
        let names: Vec<_> = (0..100)
            .map(|i| format!(r"C:\{}\App{i}.exe", "Directory".repeat(20)))
            .collect();
        let p = profile(&names, "127.0.0.1", 1080, "client.exe").unwrap();
        let selected: Vec<_> = p["ProxyRules"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["Action"] == "PROXY")
            .collect();
        assert!(selected.len() > 1);
        assert!(selected
            .iter()
            .all(|r| r["ProcessName"].as_str().unwrap().len() <= 4096));
        assert_eq!(
            selected
                .iter()
                .map(|r| r["ProcessName"].as_str().unwrap().split(';').count())
                .sum::<usize>(),
            100
        );
    }
}
