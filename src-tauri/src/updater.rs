//! Updates from the project's GitHub releases: the desktop app (the macOS
//! zip, the Windows installer) and the server agent. Downloads are checked
//! against the release's SHA256SUMS.txt before anything is installed.
use crate::agent_update::valid_version;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const RELEASES_API: &str = "https://api.github.com/repos/openm1ke/Secret-Tunnel/releases";
pub const RELEASES_PAGE: &str = "https://github.com/openm1ke/Secret-Tunnel/releases";
static INSTALLING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Default, Serialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Release {
    pub version: String,
    pub current: String,
    pub newer: bool,
    pub notes: String,
    pub page: String,
    pub published_at: String,
    /// The file this computer installs, if the release has one for it.
    pub app: Option<Asset>,
    pub agent: Option<Asset>,
    #[serde(skip)]
    sums: Option<Asset>,
    /// Installed with the Windows installer, so it can update itself; a
    /// portable copy is updated by hand.
    pub installable: bool,
}

/// Download of the app update, for the progress bar.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Download {
    pub running: bool,
    pub version: String,
    pub received: u64,
    pub total: u64,
    /// Downloaded and its checksum matches.
    pub ready: bool,
    pub error: String,
    #[serde(skip)]
    asset: Option<VerifiedAsset>,
}

#[derive(Clone, Debug)]
pub struct VerifiedAsset {
    _directory: Arc<tempfile::TempDir>,
    file: PathBuf,
    digest: String,
}

impl VerifiedAsset {
    pub fn path(&self) -> &Path {
        &self.file
    }

    fn verify(&self) -> Result<(), String> {
        if sha256_file(&self.file)? != self.digest {
            return Err("Контрольная сумма не совпала. Скачайте обновление заново.".into());
        }
        Ok(())
    }
}

fn download() -> &'static Mutex<Download> {
    static STATE: OnceLock<Mutex<Download>> = OnceLock::new();
    STATE.get_or_init(Default::default)
}

pub fn status() -> Download {
    download().lock().map(|d| d.clone()).unwrap_or_default()
}

/// `a` is a later version than `b`: numeric parts compared in order.
pub fn is_newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| {
        v.trim_start_matches('v')
            .split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

fn curl(proxy: Option<(&str, u16)>) -> std::process::Command {
    let mut command = crate::background_command("curl");
    command.args([
        "-q",
        "-sSfL",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "10",
        "-A",
        concat!("secret-tunnel/", env!("CARGO_PKG_VERSION")),
    ]);
    if let Some((host, port)) = proxy {
        command.args([
            "--noproxy",
            "",
            "--socks5-hostname",
            &format!("{host}:{port}"),
        ]);
    }
    command.stdin(Stdio::null());
    command
}

fn fetch(url: &str, proxy: Option<(&str, u16)>, limit_seconds: u32) -> Result<Vec<u8>, String> {
    let output = curl(proxy)
        .args([
            "-m",
            &limit_seconds.to_string(),
            "-H",
            "Accept: application/vnd.github+json",
            url,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if reason.is_empty() {
            "GitHub не ответил".into()
        } else {
            format!("GitHub не ответил: {reason}")
        });
    }
    Ok(output.stdout)
}

fn try_routes<T>(
    first: Option<(&str, u16)>,
    second: Option<(&str, u16)>,
    mut attempt: impl FnMut(Option<(&str, u16)>) -> Result<T, String>,
) -> Result<T, String> {
    match attempt(first) {
        Ok(value) => Ok(value),
        Err(first_error) => {
            if first == second {
                std::thread::sleep(Duration::from_millis(300));
            }
            attempt(second).map_err(|error| {
                if error == first_error {
                    error
                } else {
                    format!("{first_error}\nПовтор: {error}")
                }
            })
        }
    }
}

fn app_suffix() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("-macos-universal.zip")
    } else if cfg!(windows) {
        Some("-windows-x64-setup.exe")
    } else {
        None
    }
}

/// The Windows installer leaves an uninstaller next to the program; a
/// portable copy has none.
fn installed_with_installer() -> bool {
    if cfg!(windows) {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("uninstall.exe").is_file()))
            .unwrap_or(false)
    } else {
        cfg!(target_os = "macos") && macos_bundle().is_ok()
    }
}

/// The latest published release, tried directly and then through the tunnel.
pub fn latest(proxy: Option<(&str, u16)>) -> Result<Release, String> {
    fetch_release("latest", None, proxy)
}

pub fn release_for(version: &str, proxy: Option<(&str, u16)>) -> Result<Release, String> {
    if !valid_version(version) {
        return Err("Некорректный номер версии".into());
    }
    fetch_release(&format!("tags/v{version}"), Some(version), proxy)
        .map_err(|error| format!("Релиз v{version} недоступен: {error}"))
}

fn fetch_release(
    endpoint: &str,
    expected: Option<&str>,
    proxy: Option<(&str, u16)>,
) -> Result<Release, String> {
    let url = format!("{RELEASES_API}/{endpoint}");
    let body = try_routes(None, proxy, |route| fetch(&url, route, 15))?;
    let json =
        serde_json::from_slice(&body).map_err(|_| "GitHub вернул непонятный ответ".to_string())?;
    parse_release(&json, expected)
}

fn parse_release(json: &serde_json::Value, expected: Option<&str>) -> Result<Release, String> {
    let version = json["tag_name"]
        .as_str()
        .unwrap_or_default()
        .trim_start_matches('v')
        .to_string();
    if !valid_version(&version)
        || expected.is_some_and(|expected| version != expected)
        || json["draft"] == true
        || json["prerelease"] == true
    {
        return Err("GitHub вернул неподходящий релиз".into());
    }
    let find = |name: String| {
        let url = format!("{RELEASES_PAGE}/download/v{version}/{name}");
        let entry = json["assets"].as_array()?.iter().find(|asset| {
            asset["name"].as_str() == Some(name.as_str())
                && asset["browser_download_url"].as_str() == Some(url.as_str())
        })?;
        Some(Asset {
            name,
            url,
            size: entry["size"].as_u64().filter(|&size| size > 0)?,
        })
    };
    let current = env!("CARGO_PKG_VERSION").to_string();
    Ok(Release {
        newer: is_newer(&version, &current),
        notes: json["body"].as_str().unwrap_or_default().to_string(),
        page: format!("{RELEASES_PAGE}/tag/v{version}"),
        published_at: json["published_at"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        app: app_suffix().and_then(|suffix| find(format!("SecretTunnel-{version}{suffix}"))),
        agent: find(format!("SecretTunnel-{version}-server-linux-x86_64.tar.gz")),
        sums: find("SHA256SUMS.txt".into()),
        installable: installed_with_installer(),
        version,
        current,
    })
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// The digest SHA256SUMS.txt lists for `name` (lines are `<hex>  ./name`).
pub fn listed_digest(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (digest, file) = line.split_once(char::is_whitespace)?;
        let file = file.trim().trim_start_matches('*').trim_start_matches("./");
        (file == name && digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| digest.to_ascii_lowercase())
    })
}

/// Downloads `asset` into a fresh folder and checks it against the release's sums.
pub fn fetch_verified(
    release: &Release,
    asset: &Asset,
    proxy: Option<(&str, u16)>,
    mut progress: impl FnMut(u64),
) -> Result<VerifiedAsset, String> {
    let sums = release
        .sums
        .as_ref()
        .ok_or("В релизе нет SHA256SUMS.txt — установка без проверки отменена")?;
    let listed = String::from_utf8(try_routes(proxy, None, |route| {
        fetch(&sums.url, route, 30)
    })?)
    .map_err(|e| e.to_string())?;
    let expected = listed_digest(&listed, &asset.name)
        .ok_or_else(|| format!("{} нет в SHA256SUMS.txt", asset.name))?;
    let directory = tempfile::Builder::new()
        .prefix("secret-tunnel-update-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let file = directory.path().join(&asset.name);
    try_routes(proxy, None, |route| {
        progress(0);
        download_file(asset, &file, route, &mut progress)
    })?;
    let verified = VerifiedAsset {
        _directory: Arc::new(directory),
        file,
        digest: expected,
    };
    verified.verify()?;
    Ok(verified)
}

fn download_file(
    asset: &Asset,
    file: &Path,
    proxy: Option<(&str, u16)>,
    progress: &mut impl FnMut(u64),
) -> Result<(), String> {
    if file.exists() {
        std::fs::remove_file(file).map_err(|error| error.to_string())?;
    }
    let mut child = curl(proxy)
        .args([
            "-m",
            "1800",
            "--speed-limit",
            "1024",
            "--speed-time",
            "30",
            "-o",
        ])
        .arg(file)
        .arg(&asset.url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl: {e}"))?;
    let finished = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        progress(std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0));
        std::thread::sleep(Duration::from_millis(250));
    };
    if !finished.success() {
        let mut reason = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut reason);
        }
        return Err(format!("Загрузка не удалась: {}", reason.trim()));
    }
    let size = std::fs::metadata(file)
        .map_err(|error| error.to_string())?
        .len();
    if size != asset.size {
        return Err("Размер загруженного файла отличается от указанного в релизе".into());
    }
    progress(size);
    Ok(())
}

/// Starts downloading the app update in the background.
pub fn start_download(release: Release, proxy: Option<(String, u16)>) -> Result<(), String> {
    let asset = release
        .app
        .clone()
        .ok_or("В релизе нет сборки для этой системы")?;
    {
        let mut state = download().lock().map_err(|e| e.to_string())?;
        if INSTALLING.load(Ordering::Acquire) {
            return Err("Установка обновления уже началась".into());
        }
        if state.running {
            return Err("Обновление уже скачивается".into());
        }
        *state = Download {
            running: true,
            version: release.version.clone(),
            total: asset.size,
            ..Default::default()
        };
    }
    std::thread::spawn(move || {
        let proxy = proxy.as_ref().map(|(host, port)| (host.as_str(), *port));
        let result = fetch_verified(&release, &asset, proxy, |received| {
            if let Ok(mut state) = download().lock() {
                state.received = received;
            }
        });
        if let Ok(mut state) = download().lock() {
            state.running = false;
            match result {
                Ok(asset) => {
                    state.ready = true;
                    state.asset = Some(asset);
                }
                Err(error) => state.error = error,
            }
        }
    });
    Ok(())
}

/// Keep the previous bundle and restore it if the launch request fails.
#[cfg(target_os = "macos")]
const MAC_SWAP: &str = r#"pid=$1; old=$2; new=$3; backup=$4; launcher=$5
remaining=300
while kill -0 "$pid" 2>/dev/null; do
    [ "$remaining" -gt 0 ] || { echo 'Application did not exit' >&2; exit 1; }
    remaining=$((remaining - 1))
    sleep 0.2
done
if ! mv "$old" "$backup"; then
    "$launcher" -n -a "$old"
    exit 1
fi
if mv "$new" "$old"; then
    if "$launcher" -n -a "$old"; then
        exit 0
    fi
    mv "$old" "$new" || exit 1
fi
mv "$backup" "$old" || exit 1
"$launcher" -n -a "$old"
exit 1"#;

#[cfg(target_os = "macos")]
fn mac_swap_command(
    pid: u32,
    old: &Path,
    new: &Path,
    backup: &Path,
    launcher: &Path,
) -> std::process::Command {
    let mut command = std::process::Command::new("/bin/sh");
    command
        .args(["-c", MAC_SWAP, "secret-tunnel-update"])
        .arg(pid.to_string())
        .arg(old)
        .arg(new)
        .arg(backup)
        .arg(launcher);
    command
}

#[cfg(target_os = "macos")]
fn unpack_macos(archive: &Path, folder: &Path, version: &str) -> Result<PathBuf, String> {
    let unpacked = crate::background_command("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(archive)
        .arg(folder)
        .status()
        .map_err(|e| e.to_string())?;
    if !unpacked.success() {
        return Err("Не удалось распаковать обновление".into());
    }
    let bundle = folder.join("Secret Tunnel.app");
    let plist = plist::Value::from_file(bundle.join("Contents/Info.plist"))
        .map_err(|e| format!("Не удалось прочитать данные приложения: {e}"))?;
    let info = plist
        .as_dictionary()
        .ok_or("Не удалось прочитать данные приложения")?;
    for (key, expected) in [
        ("CFBundleIdentifier", "dev.secrettunnel.desktop"),
        ("CFBundleShortVersionString", version),
    ] {
        if info.get(key).and_then(plist::Value::as_string) != Some(expected) {
            return Err(format!(
                "Приложение в архиве не соответствует релизу ({key})"
            ));
        }
    }
    let signature = crate::background_command("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&bundle)
        .output()
        .map_err(|e| e.to_string())?;
    if !signature.status.success() {
        return Err(format!(
            "Подпись приложения не прошла проверку: {}",
            String::from_utf8_lossy(&signature.stderr).trim()
        ));
    }
    Ok(bundle)
}

#[cfg(target_os = "macos")]
fn macos_bundle() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let bundle = exe
        .ancestors()
        .nth(3)
        .ok_or("Не найдена папка приложения")?
        .to_path_buf();
    if bundle.extension().and_then(|e| e.to_str()) != Some("app") {
        return Err(
            "Обновление работает в установленном приложении, а не в сборке для разработки".into(),
        );
    }
    Ok(bundle)
}

#[cfg(not(target_os = "macos"))]
fn macos_bundle() -> Result<PathBuf, String> {
    Err("Только для macOS".into())
}

/// Hands the downloaded update to a helper that waits for this app to quit,
/// installs it and starts the new version. The caller quits the app next.
pub fn install() -> Result<(), String> {
    if INSTALLING.swap(true, Ordering::AcqRel) {
        return Err("Установка обновления уже началась".into());
    }
    let result = prepare_install();
    if result.is_err() {
        INSTALLING.store(false, Ordering::Release);
    }
    result
}

fn prepare_install() -> Result<(), String> {
    let state = status();
    let asset = state.asset.ok_or("Обновление ещё не скачано")?;
    if let Err(error) = asset.verify() {
        let mut state = download().lock().map_err(|e| e.to_string())?;
        state.ready = false;
        state.asset = None;
        return Err(error);
    }
    #[cfg(target_os = "macos")]
    {
        let bundle = macos_bundle()?;
        let parent = bundle.parent().ok_or("Не найдена папка приложения")?;
        // Both bundle moves must be renames on the same filesystem.
        let stage = tempfile::Builder::new()
            .prefix(".secret-tunnel-update-")
            .tempdir_in(parent)
            .map_err(|error| {
                format!(
                    "Не удалось подготовить обновление в {}: {error}",
                    parent.display()
                )
            })?;
        let fresh = unpack_macos(asset.path(), &stage.path().join("unpacked"), &state.version)?;
        let log =
            std::fs::File::create(stage.path().join("update.log")).map_err(|e| e.to_string())?;
        mac_swap_command(
            std::process::id(),
            &bundle,
            &fresh,
            &stage.path().join("Previous.app"),
            Path::new("/usr/bin/open"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .map_err(|e| e.to_string())?;
        let _ = stage.keep();
        if let Ok(mut state) = download().lock() {
            state.asset = None;
        }
        return Ok(());
    }
    #[cfg(windows)]
    {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        use std::os::windows::process::CommandExt;
        if !installed_with_installer() {
            return Err(format!(
                "Это портативная версия: скачайте новый архив со страницы {RELEASES_PAGE}"
            ));
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        let script: Vec<u8> = include_str!("update-windows.ps1")
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                &STANDARD.encode(script),
            ])
            .env("ST_UPDATE_PID", std::process::id().to_string())
            .env("ST_UPDATE_INSTALLER", asset.path())
            .env("ST_UPDATE_APP", exe)
            .env("ST_UPDATE_LOG", asset.path().with_file_name("update.log"))
            .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[allow(unreachable_code)]
    Err("Автообновление доступно на macOS и Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "embedded-ssh")]
    use crate::agent_update::agent_version_from_output;

    #[cfg(target_os = "macos")]
    fn mac_swap_fixture(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let old = dir.join("Secret Tunnel.app");
        let new = dir.join("unpacked/Secret Tunnel.app");
        let backup = dir.join("Previous.app");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("version"), "old").unwrap();
        std::fs::write(new.join("version"), "new").unwrap();
        (old, new, backup)
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_swap_replaces_the_bundle_after_the_app_quits() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new, backup) = mac_swap_fixture(dir.path());
        let mut app = std::process::Command::new("/bin/sleep")
            .arg("1")
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        let mut helper =
            mac_swap_command(app.id(), &old, &new, &backup, Path::new("/usr/bin/true"))
                .spawn()
                .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(std::fs::read_to_string(old.join("version")).unwrap(), "old");
        assert!(helper.try_wait().unwrap().is_none());
        // Reap the child before waiting for the helper: kill -0 sees a zombie.
        app.wait().unwrap();
        assert!(helper.wait().unwrap().success());
        assert!(started.elapsed() >= Duration::from_millis(800));
        assert_eq!(std::fs::read_to_string(old.join("version")).unwrap(), "new");
        assert_eq!(
            std::fs::read_to_string(backup.join("version")).unwrap(),
            "old"
        );
        assert!(!new.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_swap_restores_the_previous_bundle_when_open_fails() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (old, new, backup) = mac_swap_fixture(dir.path());
        let launcher = dir.path().join("open");
        std::fs::write(
            &launcher,
            "#!/bin/sh\n[ \"$(cat \"$3/version\")\" = old ] && touch \"$3/reopened\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut app = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = app.id();
        app.wait().unwrap();
        let status = mac_swap_command(pid, &old, &new, &backup, &launcher)
            .status()
            .unwrap();
        assert!(!status.success());
        assert_eq!(std::fs::read_to_string(old.join("version")).unwrap(), "old");
        assert!(old.join("reopened").exists());
        assert_eq!(std::fs::read_to_string(new.join("version")).unwrap(), "new");
        assert!(!backup.exists());
    }

    #[test]
    fn pinned_release_rejects_other_versions_and_repositories() {
        let asset = "SecretTunnel-0.3.23-server-linux-x86_64.tar.gz";
        let mut json = serde_json::json!({
            "tag_name": "v0.3.23",
            "assets": [{
                "name": asset,
                "size": 42,
                "browser_download_url": format!("{RELEASES_PAGE}/download/v0.3.23/{asset}")
            }]
        });
        assert!(parse_release(&json, Some("0.3.24")).is_err());
        assert_eq!(
            parse_release(&json, Some("0.3.23"))
                .unwrap()
                .agent
                .unwrap()
                .name,
            asset
        );
        json["assets"][0]["browser_download_url"] = serde_json::json!("https://example.com/agent");
        assert!(parse_release(&json, Some("0.3.23"))
            .unwrap()
            .agent
            .is_none());
        json["draft"] = serde_json::json!(true);
        assert!(parse_release(&json, Some("0.3.23")).is_err());
    }

    #[test]
    fn a_failed_route_falls_back_to_the_other_route() {
        let mut attempts = Vec::new();
        let proxy = Some(("127.0.0.1", 1080));
        let value = try_routes(proxy, None, |route| {
            attempts.push(route.map(|(_, port)| port));
            if route.is_some() {
                Err("unavailable".into())
            } else {
                Ok(42)
            }
        })
        .unwrap();
        assert_eq!(attempts, [Some(1080), None]);
        assert_eq!(value, 42);
    }

    #[cfg(feature = "embedded-ssh")]
    #[test]
    fn agent_version_requires_a_valid_agent_response() {
        assert_eq!(
            agent_version_from_output("banner\nsecret-tunnel-agent 0.3.23\n").unwrap(),
            Some("0.3.23")
        );
        assert_eq!(
            agent_version_from_output("secret-tunnel-agent нет\n").unwrap(),
            None
        );
        assert!(agent_version_from_output("bash: permission denied").is_err());
    }

    /// Live: verify app and agent archives coexist, and validate the macOS bundle.
    #[test]
    #[ignore]
    fn latest_release_downloads_and_verifies() {
        let release = latest(None).unwrap();
        let asset = release.app.clone().expect("no build for this platform");
        let file = fetch_verified(&release, &asset, None, |_| {}).unwrap();
        let pinned = release_for(&release.version, None).unwrap();
        let agent = pinned.agent.clone().unwrap();
        let agent_file = fetch_verified(&pinned, &agent, None, |_| {}).unwrap();
        assert_ne!(file.path().parent(), agent_file.path().parent());
        assert_eq!(std::fs::metadata(file.path()).unwrap().len(), asset.size);
        file.verify().unwrap();
        #[cfg(target_os = "macos")]
        {
            let dir = tempfile::tempdir().unwrap();
            unpack_macos(file.path(), dir.path(), &release.version).unwrap();
        }
        let path = agent_file.path().to_owned();
        drop(agent_file);
        assert!(!path.exists());
        assert!(file.path().exists());
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(is_newer("0.3.10", "0.3.9"));
        assert!(is_newer("v1.0.0", "0.9.99"));
        assert!(!is_newer("0.3.23", "0.3.23"));
        assert!(!is_newer("0.3.22", "0.3.23"));
        assert!(is_newer("0.4", "0.3.99"));
    }

    #[test]
    fn digest_is_read_from_the_release_sums() {
        let sums = "aaaa  ./other\n0123456789abcdef0123456789abcdef0123456789abcdef0123456789ABCDEF  ./SecretTunnel-0.3.23-android.apk\n";
        assert_eq!(
            listed_digest(sums, "SecretTunnel-0.3.23-android.apk").as_deref(),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert_eq!(listed_digest(sums, "missing"), None);
        assert_eq!(listed_digest(sums, "other"), None);
    }
}
