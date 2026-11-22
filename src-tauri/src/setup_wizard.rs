use super::*;
use tauri::ipc::Channel;

#[derive(Deserialize)]
pub struct AccessRequest {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub fingerprint: String,
    pub key_path: Option<String>,
    pub password: Option<String>,
}

#[tauri::command]
pub async fn wizard_probe(host: String, port: u16) -> Result<String, String> {
    #[cfg(feature = "embedded-ssh")]
    return tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        if host.trim().is_empty() || port == 0 { return Err("Укажите адрес и SSH-порт сервера".into()); }
        embedded_ssh::probe_server(host.trim(), port, Vec::new()).map_err(|e| embedded_ssh::setup::friendly_error(&e))
    }).await.map_err(|e| e.to_string())?;
    #[cfg(not(feature = "embedded-ssh"))]
    { let _ = (host, port); Err("Мастер требует сборку со встроенным SSH".into()) }
}

#[tauri::command]
pub async fn wizard_access(request: AccessRequest, progress: Channel<String>) -> Result<String, String> {
    #[cfg(feature = "embedded-ssh")]
    return tauri::async_runtime::spawn_blocking(move || access(request, progress))
        .await.map_err(|_| "Мастер не завершил проверку. Повторите попытку.".to_string())?
        .map_err(|e| embedded_ssh::setup::friendly_error(&e));
    #[cfg(not(feature = "embedded-ssh"))]
    { let _ = (request, progress); Err("Мастер требует сборку со встроенным SSH".into()) }
}

#[cfg(feature = "embedded-ssh")]
fn access(request: AccessRequest, progress: Channel<String>) -> Result<String, String> {
    let notify = |text: &str| { let _ = progress.send(text.to_owned()); };
    if request.id.len() < 12 || request.id.len() > 64 || !request.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err("Некорректный идентификатор настройки".into());
    }
    if request.fingerprint.len() != 50 || !request.fingerprint.starts_with("SHA256:") {
        return Err("Подтвердите отпечаток сервера".into());
    }
    let mut profile = default_config().servers.remove(0);
    profile.id = request.id;
    profile.name = if request.name.trim().is_empty() { "Мой сервер".into() } else { request.name.trim().into() };
    profile.host = request.host.trim().into();
    profile.port = request.port;
    profile.user = request.user.trim().into();
    profile.host_key_sha256 = Some(request.fingerprint.clone());
    let config = load_config()?;
    if config.servers.iter().any(|s| s.id == profile.id && (s.host != profile.host || s.port != profile.port || s.user != profile.user)) {
        return Err("Настройка уже сохранена для другого сервера. Начните новый мастер.".into());
    }
    profile.key_path = match request.key_path.filter(|path| !path.trim().is_empty()) {
        Some(path) => expand_home(&path),
        None => {
            notify("Создаём отдельный ключ приложения…");
            let directory = app_config_dir("secret-tunnel")?.join("keys");
            fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
            let path = directory.join(format!("{}.key", profile.id));
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)] {
                use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(mut file) => {
                    use std::io::Write;
                    let written = (|| -> Result<(), String> {
                        let key = embedded_ssh::setup::generate_key()?;
                        file.write_all(key.as_bytes()).map_err(|e| e.to_string())?;
                        file.sync_all().map_err(|e| e.to_string())
                    })();
                    if let Err(error) = written {
                        drop(file);
                        let _ = fs::remove_file(&path);
                        return Err(error);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {},
                Err(e) => return Err(e.to_string()),
            }
            path.to_string_lossy().into_owned()
        }
    };
    validate_server(&profile)?;
    let options = embedded_ssh::EmbeddedOptions {
        host_key_sha256: profile.host_key_sha256.clone(),
        bind_physical_interface: config.settings.bypass_vpn_for_ssh,
        ..Default::default()
    };
    if let Some(password) = request.password.filter(|value| !value.is_empty()) {
        notify("Входим по паролю и добавляем публичный ключ…");
        embedded_ssh::setup::enroll_password(embedded_server(&profile), request.fingerprint, password, config.settings.bypass_vpn_for_ssh)?;
        notify("Повторный вход по новому ключу прошёл.");
    } else {
        notify("Проверяем вход по выбранному ключу…");
        embedded_ssh::diagnose_server_with_options(embedded_server(&profile), options, false)?;
    }
    notify("Сохраняем подключение…");
    let id = profile.id.clone();
    mutate_config(|config| {
        config.servers.retain(|s| s.id != profile.id);
        config.servers.push(profile);
        if config.selected_server_id.is_empty() || config.selected_server_id == "example" {
            config.selected_server_id = id.clone();
        }
        if config.proxies.is_empty() {
            config.proxies = default_config().proxies;
            config.selected_proxy_id = config.proxies[0].id.clone();
        }
        config.settings.ssh_engine = "embedded".into();
        Ok(())
    })?;
    Ok(id)
}

#[tauri::command]
pub async fn wizard_finish(server_id: String, install: bool, progress: Channel<String>) -> Result<String, String> {
    #[cfg(feature = "embedded-ssh")]
    return tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let notify = |text: &str| { let _ = progress.send(text.to_owned()); };
        let config = load_config()?;
        let server = server_by_id(&config, &server_id)?;
        let route = embedded_route(&config, &server)?;
        notify("Проверяем серверный агент…");
        if install {
            notify("Скачиваем агент и сверяем SHA-256…");
            let release = updater::release_for(env!("CARGO_PKG_VERSION"), None)?;
            let asset = release.agent.as_ref().ok_or("В релизе нет агента для Linux x86_64")?;
            let archive = updater::fetch_verified(&release, asset, None, |_| {})?;
            notify("Устанавливаем агент в домашнюю папку пользователя…");
            let (status, _, _) = embedded_ssh::run_on_server_blocking(
                embedded_server(&server), route.clone(), &agent_update::agent_install_command(&release.version),
                fs::read(archive.path()).map_err(|e| e.to_string())?, Duration::from_secs(90),
            )?;
            if status != 0 { return Err("Не удалось установить агент. Требуется Linux x86_64 и доступ на запись в домашнюю папку.".into()); }
        }
        notify("Проверяем подключение к агенту и поддержку TCP/UDP…");
        let summary = embedded_ssh::diagnose_server_with_options(embedded_server(&server), route, true)?;
        notify("Подключение проверено и сохранено.");
        Ok(summary)
    }).await.map_err(|_| "Проверка агента прервалась. Повторите попытку.".to_string())?
        .map_err(|e| embedded_ssh::setup::friendly_error(&e));
    #[cfg(not(feature = "embedded-ssh"))]
    { let _ = (server_id, install, progress); Err("Мастер требует сборку со встроенным SSH".into()) }
}

#[cfg(all(test, feature = "embedded-ssh"))]
mod tests {
    use super::*;

    /// An isolated SSH account is supplied by the local integration runner.
    #[test]
    #[ignore = "requires WIZARD_TEST_FIXTURE and SECRET_TUNNEL_TEST_CONFIG_DIR"]
    fn desktop_setup_enrolls_and_checks_agent() {
        let path = std::env::var("WIZARD_TEST_FIXTURE").expect("disposable SSH fixture");
        assert!(std::env::var_os("SECRET_TUNNEL_TEST_CONFIG_DIR").is_some());
        let fixture: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let mut initial = default_config();
        initial.settings.bypass_vpn_for_ssh = false;
        save_config(initial).unwrap();
        let host = fixture["host"].as_str().unwrap().to_string();
        let user = fixture["user"].as_str().unwrap().to_string();
        let password = fixture["password"].as_str().unwrap().to_string();
        let port = fixture["port"].as_u64().unwrap() as u16;
        let pin = tauri::async_runtime::block_on(wizard_probe(host.clone(), port)).unwrap();
        assert_eq!(pin, fixture["fingerprint"]);
        let request = |password: String, key_path: Option<String>, id: &str| AccessRequest {
            id: id.into(), name: "Ubuntu test".into(), host: host.clone(), port, user: user.clone(),
            fingerprint: pin.clone(), key_path, password: Some(password),
        };
        let progress = || Channel::new(|_| Ok(()));
        let id = "setup-desktop-password-case";
        let error = tauri::async_runtime::block_on(wizard_access(request("wrong".into(), None, id), progress())).unwrap_err();
        assert!(error.contains("Сервер не принял"), "{error}");
        assert!(!load_config().unwrap().servers.iter().any(|server| server.id == id));
        let enrolled = tauri::async_runtime::block_on(wizard_access(request(password, None, id), progress())).unwrap();
        assert_eq!(enrolled, id);
        let config = load_config().unwrap();
        let profile = server_by_id(&config, id).unwrap();
        assert_eq!(profile.host_key_sha256, Some(pin.clone()));
        let configured = || wizard_finish(id.to_string(), false, progress());
        if tauri::async_runtime::block_on(configured()).is_err() {
            tauri::async_runtime::block_on(wizard_finish(id.to_string(), true, progress())).unwrap();
        }
        let summary = tauri::async_runtime::block_on(configured()).unwrap();
        assert!(summary.contains("STMUX003: OK"));
        let existing = fixture["key_path"].as_str().unwrap().to_owned();
        let imported = tauri::async_runtime::block_on(wizard_access(request(String::new(), Some(existing), "setup-desktop-imported-key"), progress())).unwrap();
        assert!(tauri::async_runtime::block_on(wizard_finish(imported, false, progress())).unwrap().contains("STMUX003: OK"));
        let serialized = fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(!serialized.contains("PRIVATE KEY"));
        assert!(!serialized.contains(fixture["password"].as_str().unwrap()));
    }
}
