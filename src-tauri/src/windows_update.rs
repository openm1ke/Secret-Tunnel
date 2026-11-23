//! The app must not exit until its independent installer helper acknowledges it.
use std::{
    io::Read,
    path::Path,
    process::Child,
    time::{Duration, Instant},
};

pub fn last_error(config: &Path) -> Option<String> {
    let mut text = String::new();
    std::fs::File::open(config.join("updates/last-error.txt"))
        .ok()?
        .take(16 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    let text = text.trim_start_matches('\u{feff}').trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn wait_ready(
    child: &mut Child,
    ready: &Path,
    config: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(last_error(config).unwrap_or_else(|| format!(
                "Помощник обновления завершился до запуска установки ({status}). Приложение оставлено открытым."
            )));
        }
        if std::fs::read_to_string(ready).is_ok_and(|text| text.trim() == "ready") {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            return Err("Помощник обновления не подтвердил готовность. Приложение оставлено открытым; попробуйте запустить скачанный установщик вручную.".into());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(windows)]
struct PreparedUpdate {
    child: Child,
    directory: std::path::PathBuf,
}

#[cfg(windows)]
pub fn powershell() -> Result<std::path::PathBuf, String> {
    let root = std::env::var_os("SystemRoot").ok_or("Не найдена системная папка Windows")?;
    let program = Path::new(&root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    if !program.is_file() {
        return Err("Не найден Windows PowerShell. Запустите скачанный установщик вручную.".into());
    }
    Ok(program)
}

#[cfg(windows)]
fn prepare(
    installer: &Path,
    digest: &str,
    app: &Path,
    parent_pid: u32,
    config: &Path,
) -> Result<PreparedUpdate, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };
    let program = powershell()?;
    let updates = config.join("updates");
    std::fs::create_dir_all(&updates)
        .map_err(|e| format!("Не удалось подготовить обновление: {e}"))?;
    let stage = tempfile::Builder::new()
        .prefix("install-")
        .tempdir_in(&updates)
        .map_err(|e| e.to_string())?;
    // The original download is owned by the old process. The helper needs its own copy.
    let copied = stage.path().join("setup.exe");
    std::fs::copy(installer, &copied)
        .map_err(|e| format!("Не удалось сохранить установщик для перезапуска: {e}"))?;
    let ready = stage.path().join("ready");
    let log =
        std::fs::File::create(stage.path().join("helper-output.log")).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(updates.join("last-error.txt"));
    let script: Vec<u8> = include_str!("update-windows.ps1")
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut child = Command::new(program)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &STANDARD.encode(script),
        ])
        .current_dir(stage.path())
        .env("ST_UPDATE_PID", parent_pid.to_string())
        .env("ST_UPDATE_INSTALLER", &copied)
        .env("ST_UPDATE_SHA256", digest)
        .env("ST_UPDATE_APP", app)
        .env("ST_UPDATE_READY", &ready)
        .env("ST_UPDATE_ERROR", updates.join("last-error.txt"))
        .env("ST_UPDATE_LOG", stage.path().join("update.log"))
        // DETACHED_PROCESS ignores CREATE_NO_WINDOW and does not detach a Windows job.
        .creation_flags(0x0800_0000)
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("Не удалось запустить помощник обновления: {e}"))?;
    if let Err(error) = wait_ready(&mut child, &ready, config, Duration::from_secs(30)) {
        let _ = child.kill();
        let _ = child.wait();
        let diagnostic = std::fs::read(stage.path().join("helper-output.log")).unwrap_or_default();
        let _ = std::fs::write(updates.join("last-helper.log"), diagnostic);
        let _ = std::fs::write(updates.join("last-error.txt"), &error);
        return Err(error);
    }
    Ok(PreparedUpdate {
        child,
        directory: stage.keep(),
    })
}

#[cfg(windows)]
pub fn launch(
    installer: &Path,
    digest: &str,
    app: &Path,
    parent_pid: u32,
    config: &Path,
) -> Result<(), String> {
    let prepared = prepare(installer, digest, app, parent_pid, config)?;
    // Closing our process handle does not terminate the acknowledged helper.
    drop(prepared.child);
    drop(prepared.directory);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    fn readiness_fixture() {
        let Ok(mode) = std::env::var("ST_UPDATE_READINESS_FIXTURE") else {
            return;
        };
        if mode == "exit" {
            std::process::exit(17);
        }
        if mode == "ready" {
            std::fs::write(
                std::env::var_os("ST_UPDATE_FIXTURE_READY").unwrap(),
                "ready",
            )
            .unwrap();
        }
        std::thread::sleep(Duration::from_secs(30));
    }
    fn child(mode: &str, ready: &Path) -> Child {
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "windows_update::tests::readiness_fixture",
                "--nocapture",
            ])
            .env("ST_UPDATE_READINESS_FIXTURE", mode)
            .env("ST_UPDATE_FIXTURE_READY", ready)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }
    #[test]
    fn readiness_requires_acknowledgement_from_a_live_helper() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let mut helper = child("ready", &ready);
        let result = wait_ready(&mut helper, &ready, dir.path(), Duration::from_secs(5));
        let alive = helper.try_wait().unwrap().is_none();
        let _ = helper.kill();
        let _ = helper.wait();
        assert!(result.is_ok());
        assert!(alive);
    }
    #[test]
    fn early_exit_and_missing_acknowledgement_do_not_allow_app_exit() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let mut helper = child("exit", &ready);
        assert!(
            wait_ready(&mut helper, &ready, dir.path(), Duration::from_secs(5))
                .unwrap_err()
                .contains("17")
        );
        let _ = helper.wait();
        let mut helper = child("idle", &ready);
        let result = wait_ready(&mut helper, &ready, dir.path(), Duration::from_millis(100));
        let alive = helper.try_wait().unwrap().is_none();
        let _ = helper.kill();
        let _ = helper.wait();
        assert!(result.is_err());
        assert!(alive);
    }

    #[cfg(windows)]
    struct StopOnDrop(Child);
    #[cfg(windows)]
    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_helper_waits_installs_restarts_and_recovers_without_losing_settings() {
        use sha2::{Digest, Sha256};
        use std::os::windows::process::CommandExt;
        let fixtures = tempfile::tempdir().unwrap();
        let binary = fixtures.path().join("fixture.exe");
        if let Some(prebuilt) = std::env::var_os("ST_WINDOWS_UPDATE_TEST_FIXTURE") {
            std::fs::copy(prebuilt, &binary).unwrap();
        } else {
            let source = fixtures.path().join("fixture.rs");
            std::fs::write(
                &source,
                include_str!("../../scripts/fixtures/windows-update-fixture.rs"),
            )
            .unwrap();
            let output = Command::new("rustc")
                .args(["--edition", "2021"])
                .arg(&source)
                .arg("-o")
                .arg(&binary)
                .creation_flags(0x0800_0000)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        // Parallels host shares expose prebuilt EXEs read-only; local downloads are writable.
        let mut permissions = std::fs::metadata(&binary).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&binary, permissions).unwrap();
        let digest = format!("{:x}", Sha256::digest(std::fs::read(&binary).unwrap()));
        for installer_exit in [0, 42] {
            let root = tempfile::Builder::new()
                .prefix("Обновление с пробелом '")
                .tempdir()
                .unwrap();
            let installed = root.path().join("Installed app's folder");
            let config = root.path().join("config");
            std::fs::create_dir_all(&installed).unwrap();
            std::fs::create_dir_all(&config).unwrap();
            let app = installed.join("SecretTunnel.exe");
            std::fs::copy(&binary, &app).unwrap();
            std::fs::write(
                installed.join("fixture.txt"),
                root.path().to_string_lossy().as_bytes(),
            )
            .unwrap();
            std::fs::write(config.join("config.json"), "preserved-settings").unwrap();
            let mut old = StopOnDrop(
                Command::new(&app)
                    .arg("--wait")
                    .creation_flags(0x0800_0000)
                    .spawn()
                    .unwrap(),
            );
            let download = tempfile::tempdir().unwrap();
            let original = download.path().join("downloaded-setup.exe");
            std::fs::copy(&binary, &original).unwrap();
            let prepared = prepare(&original, &digest, &app, old.0.id(), &config).unwrap();
            drop(download);
            assert!(!original.exists());
            let mut helper = StopOnDrop(prepared.child);
            std::fs::write(
                prepared.directory.join("fixture.txt"),
                format!(
                    "{}\n{installer_exit}\n{}",
                    root.path().display(),
                    app.display()
                ),
            )
            .unwrap();
            assert!(old.0.try_wait().unwrap().is_none());
            assert!(!root.path().join("installer-args").exists());
            old.0.kill().unwrap();
            old.0.wait().unwrap();
            let started = Instant::now();
            let status = loop {
                if let Some(status) = helper.0.try_wait().unwrap() {
                    break status;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(20),
                    "helper timed out: {:?}",
                    last_error(&config)
                );
                std::thread::sleep(Duration::from_millis(25));
            };
            assert_eq!(
                status.success(),
                installer_exit == 0,
                "{:?}; fixture error: {:?}",
                last_error(&config),
                std::fs::read_to_string(root.path().join("installer-error"))
            );
            assert_eq!(
                std::fs::read_to_string(root.path().join("installer-args")).unwrap(),
                format!("/P /UPDATE /D={}", installed.display())
            );
            assert_eq!(
                std::fs::read_to_string(root.path().join("installer-cwd")).unwrap(),
                prepared.directory.to_string_lossy()
            );
            let started = Instant::now();
            while !root.path().join("app-restarted").exists() {
                assert!(
                    started.elapsed() < Duration::from_secs(5),
                    "app did not reopen"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
            assert_eq!(
                std::fs::read_to_string(config.join("config.json")).unwrap(),
                "preserved-settings"
            );
            if installer_exit == 0 {
                assert!(last_error(&config).is_none());
                assert!(root.path().join("app-replaced").exists());
            } else {
                assert!(last_error(&config).unwrap().contains("42"));
                assert!(prepared.directory.join("setup.exe").exists());
            }
        }
        // An invalid staged file is rejected before the old process is told to quit.
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app.exe");
        std::fs::copy(&binary, &app).unwrap();
        let mut old = StopOnDrop(
            Command::new(&app)
                .arg("--wait")
                .creation_flags(0x0800_0000)
                .spawn()
                .unwrap(),
        );
        assert!(prepare(&binary, &"0".repeat(64), &app, old.0.id(), root.path()).is_err());
        assert!(old.0.try_wait().unwrap().is_none());
        assert!(last_error(root.path()).unwrap().contains("сумма"));
    }
}
