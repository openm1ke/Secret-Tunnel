use serde::{Deserialize, Serialize};
#[cfg(not(target_os = "macos"))]
use std::process::Stdio;

#[derive(Clone, Deserialize, Serialize)]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub executable: String,
    pub patterns: Vec<String>,
    pub source: String,
    /// macOS: the bundle's own identifier. Helper identifiers in `patterns`
    /// can be shared by many applications (Sparkle, Mozilla), so only this
    /// one says whether the application itself is selected.
    #[serde(default)]
    pub primary: String,
}

pub fn scan() -> Result<Vec<InstalledApp>, String> {
    #[cfg(target_os = "macos")]
    return super::macos_apps::scan();
    #[cfg(not(target_os = "macos"))]
    {
        if !cfg!(windows) {
            return Err("Поиск установленных программ пока доступен только в Windows".into());
        }
        let powershell =
            std::path::PathBuf::from(std::env::var_os("SystemRoot").ok_or("Не найден SystemRoot")?)
                .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let output =
            super::background_command(powershell.to_str().ok_or("Некорректный путь PowerShell")?)
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-STA",
                    "-Command",
                    include_str!("../scripts/scan-applications.ps1"),
                ])
                .stdin(Stdio::null())
                .output()
                .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err("Не удалось прочитать список приложений Windows".into());
        }
        let mut apps: Vec<InstalledApp> = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("Ошибка чтения списка приложений: {e}"))?;
        // Only present choices supported by the routing validator.
        apps.retain(|app| {
            super::app_redirect::profile(
                &app.patterns,
                "127.0.0.1",
                1080,
                "SecretTunnel-programs.exe",
            )
            .is_ok()
        });
        apps.sort_by_cached_key(|app| app.name.to_lowercase());
        apps.dedup_by(|a, b| a.id.eq_ignore_ascii_case(&b.id));
        Ok(apps)
    }
}
