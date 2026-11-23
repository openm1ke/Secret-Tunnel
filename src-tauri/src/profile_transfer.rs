use super::*;
use secret_tunnel_profile_transfer as exchange;

#[derive(Serialize)]
pub struct ExportedProfiles { link: String, code: String, qr_svg: Option<String>, server_count: usize, skipped: Vec<String> }

#[tauri::command]
pub async fn transfer_export(server_ids: Vec<String>) -> Result<ExportedProfiles, String> {
    #[cfg(feature = "embedded-ssh")]
    return tauri::async_runtime::spawn_blocking(move || {
        use russh::keys::ssh_key::LineEnding;
        if server_ids.is_empty() || server_ids.len() > 32 { return Err("Выберите от 1 до 32 серверов".into()); }
        let config = load_config()?;
        let mut roots = Vec::new(); let mut skipped = Vec::new();
        for id in &server_ids {
            let profile = server_by_id(&config, id)?;
            if server_ids.len() > 1 && (profile.host.trim().is_empty() || profile.user.trim().is_empty() || !Path::new(&expand_home(&profile.key_path)).is_file()) {
                skipped.push(profile.name);
            } else { roots.push(id.clone()); }
        }
        if roots.is_empty() { return Err("Нет настроенных серверов с доступным SSH-ключом".into()); }
        let mut profiles: Vec<ServerProfile> = Vec::new();
        for id in &roots {
            let target = server_by_id(&config, id)?;
            for profile in server_jumps(&config, &target)?.into_iter().chain(std::iter::once(&target)) {
                if !profiles.iter().any(|p| p.id == profile.id) { profiles.push(profile.clone()); }
            }
        }
        let mut servers = Vec::new();
        for profile in profiles {
            let path = expand_home(&profile.key_path);
            if fs::metadata(&path).map_err(|_| format!("Не найден ключ сервера «{}»", profile.name))?.len() > 1_048_576 {
                return Err("Файл SSH-ключа больше 1 МБ".into());
            }
            let key = russh::keys::load_secret_key(&path, None)
                .map_err(|e| embedded_ssh::setup::friendly_error(&e.to_string()))?;
            let pin = match profile.host_key_sha256.as_ref().filter(|pin| exchange::valid_pin(pin)) {
                Some(pin) => pin.clone(),
                None => {
                    let observed = Arc::new(Mutex::new(None));
                    let mut route = embedded_route(&config, &profile)?;
                    route.observed_host_key = Some(observed.clone());
                    for hop in &mut route.jumps { hop.server.strict_host_key_checking = "yes".into(); }
                    let mut endpoint = embedded_server(&profile);
                    endpoint.strict_host_key_checking = "yes".into();
                    embedded_ssh::diagnose_server_with_options(endpoint, route, false)
                        .map_err(|e| format!("Подтвердите ключ сервера «{}» перед переносом. {}", profile.name, embedded_ssh::setup::friendly_error(&e)))?;
                    let pin = observed.lock().map_err(|e| e.to_string())?.clone().ok_or("Сервер не вернул отпечаток")?;
                    pin
                }
            };
            servers.push(exchange::Server { id: profile.id.clone(), name: profile.name.clone(), host: profile.host.clone(), port: profile.port,
                user: profile.user.clone(), host_key_sha256: pin, private_key: key.to_openssh(LineEnding::LF).map_err(|e| e.to_string())?.to_string(), jump_id: profile.jump_server_id.clone() });
        }
        let document = exchange::Document { version: 1, roots, servers };
        let code = exchange::generate_code();
        let link = exchange::encrypt(&document, &code)?;
        let qr_svg = exchange::qr_svg(&link);
        Ok(ExportedProfiles { link, code, qr_svg, server_count: document.servers.len(), skipped })
    }).await.map_err(|_| "Не удалось подготовить перенос".to_string())?;
    #[cfg(not(feature = "embedded-ssh"))]
    { let _ = server_ids; Err("Перенос ключей требует сборку со встроенным SSH".into()) }
}

#[tauri::command]
pub async fn transfer_preview(payload: String, code: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let document = exchange::decrypt(&payload, &code)?;
        serde_json::to_value(document.preview()).map_err(|e| e.to_string())
    }).await.map_err(|_| "Не удалось расшифровать серверы".to_string())?
}

#[tauri::command]
pub async fn transfer_import(payload: String, code: String) -> Result<Vec<String>, String> {
    #[cfg(feature = "embedded-ssh")]
    return tauri::async_runtime::spawn_blocking(move || import_document(exchange::decrypt(&payload, &code)?))
        .await.map_err(|_| "Не удалось сохранить серверы".to_string())?;
    #[cfg(not(feature = "embedded-ssh"))]
    { let _ = (payload, code); Err("Перенос ключей требует сборку со встроенным SSH".into()) }
}

#[cfg(feature = "embedded-ssh")]
fn import_document(document: exchange::Document) -> Result<Vec<String>, String> {
    use russh::keys::ssh_key::{LineEnding, PrivateKey};
    document.validate()?;
    let mut prepared = Vec::new();
    // Validate every key before creating files or changing the configuration.
    for server in &document.servers {
        let private = PrivateKey::from_openssh(&server.private_key).map_err(|_| "В переносе есть некорректный SSH-ключ")?;
        if private.is_encrypted() { return Err("Перенос содержит ключ с парольной фразой. Создайте отдельный ключ в мастере.".into()); }
        prepared.push(private.to_openssh(LineEnding::LF).map_err(|e| e.to_string())?);
    }
    let ids: std::collections::HashMap<_, _> = document.servers.iter().map(|s| (s.id.clone(), format!("transfer-{:032x}", rand::random::<u128>()))).collect();
    let directory = app_config_dir("secret-tunnel")?.join("keys");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?; }
    let mut files = Vec::new();
    let result = (|| {
        let mut profiles = Vec::new();
        for (server, private) in document.servers.iter().zip(&prepared) {
            let id = ids[&server.id].clone();
            let path = directory.join(format!("{id}.key"));
            let mut options = fs::OpenOptions::new(); options.write(true).create_new(true);
            #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
            let mut file = options.open(&path).map_err(|e| e.to_string())?;
            files.push(path.clone());
            use std::io::Write;
            file.write_all(private.as_bytes()).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            let mut profile = default_config().servers.remove(0);
            profile.id = id; profile.name = server.name.clone(); profile.host = server.host.clone(); profile.port = server.port;
            profile.user = server.user.clone(); profile.key_path = path.to_string_lossy().into_owned();
            profile.host_key_sha256 = Some(server.host_key_sha256.clone());
            profile.jump_server_id = server.jump_id.as_ref().map(|id| ids[id].clone());
            profile.strict_host_key_checking = "yes".into();
            validate_server(&profile)?; profiles.push(profile);
        }
        let roots: Vec<_> = document.roots.iter().map(|id| ids[id].clone()).collect();
        mutate_config(|config| {
            config.servers.extend(profiles);
            if config.selected_server_id.is_empty() || config.selected_server_id == "example" { config.selected_server_id = roots[0].clone(); }
            if config.proxies.is_empty() { config.proxies = default_config().proxies; config.selected_proxy_id = config.proxies[0].id.clone(); }
            Ok(())
        })?;
        Ok(roots)
    })();
    if result.is_err() { for file in files { let _ = fs::remove_file(file); } }
    result
}

#[tauri::command]
pub async fn transfer_read_file(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut data = Vec::new();
        fs::File::open(path).map_err(|e| e.to_string())?.take(exchange::MAX_INPUT as u64 + 1).read_to_end(&mut data).map_err(|e| e.to_string())?;
        if data.len() > exchange::MAX_INPUT { return Err("Файл переноса больше 1 МБ".into()); }
        let text = String::from_utf8(data).map_err(|_| "Это не файл переноса Secret Tunnel")?;
        if !text.trim().trim_start_matches('\u{feff}').starts_with(exchange::LINK_PREFIX) { return Err("Это не файл переноса Secret Tunnel".into()); }
        Ok(text)
    }).await.map_err(|_| "Не удалось открыть файл".to_string())?
}

#[tauri::command]
pub async fn transfer_write_file(path: String, payload: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if payload.len() > exchange::MAX_INPUT || !payload.starts_with(exchange::LINK_PREFIX) { return Err("Некорректные данные переноса".into()); }
        fs::write(path, payload).map_err(|e| e.to_string())
    }).await.map_err(|_| "Не удалось сохранить файл".to_string())?
}

#[cfg(all(test, feature = "embedded-ssh"))]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires an isolated SECRET_TUNNEL_TEST_CONFIG_DIR; produces optional cross-device fixture"]
    fn desktop_transfer_preserves_existing_profiles_and_chains() {
        assert!(std::env::var_os("SECRET_TUNNEL_TEST_CONFIG_DIR").is_some());
        let directory = app_config_dir("secret-tunnel").unwrap(); fs::create_dir_all(&directory).unwrap();
        let first_key = embedded_ssh::setup::generate_key().unwrap();
        let second_key = embedded_ssh::setup::generate_key().unwrap();
        let mut config = default_config();
        let mut relay = config.servers[0].clone(); relay.id = "relay".into(); relay.name = "Demo RU".into(); relay.host = "203.0.113.10".into(); relay.user = "demo".into();
        relay.host_key_sha256 = Some(format!("SHA256:{}", "A".repeat(43)));
        relay.key_path = directory.join("relay.key").to_string_lossy().into_owned(); fs::write(&relay.key_path, &first_key).unwrap();
        let mut target = relay.clone(); target.id = "target".into(); target.name = "Demo FR".into(); target.host = "203.0.113.20".into(); target.jump_server_id = Some(relay.id.clone());
        target.key_path = directory.join("target.key").to_string_lossy().into_owned(); fs::write(&target.key_path, &second_key).unwrap();
        config.servers = vec![relay.clone(), target.clone()]; config.selected_server_id = relay.id.clone(); config.settings.ssh_engine = "system".into();
        save_config(config.clone()).unwrap();
        let exported = tauri::async_runtime::block_on(transfer_export(vec![target.id.clone()])).unwrap();
        assert_eq!(exported.server_count, 2); assert!(exported.qr_svg.is_some());
        let preview = tauri::async_runtime::block_on(transfer_preview(exported.link.clone(), exported.code.clone())).unwrap();
        assert_eq!(preview["servers"].as_array().unwrap().len(), 2);
        assert!(!preview.to_string().contains("private_key"));
        let original = fs::read(config_path().unwrap()).unwrap();
        assert!(tauri::async_runtime::block_on(transfer_import(exported.link.clone(), exchange::generate_code())).is_err());
        assert_eq!(fs::read(config_path().unwrap()).unwrap(), original);
        let roots = tauri::async_runtime::block_on(transfer_import(exported.link.clone(), exported.code.clone())).unwrap();
        let updated = load_config().unwrap(); assert_eq!(updated.servers.len(), 4); assert_eq!(updated.selected_server_id, "relay");
        assert_eq!(serde_json::to_value(&updated.settings).unwrap(), serde_json::to_value(&config.settings).unwrap());
        let imported = server_by_id(&updated, &roots[0]).unwrap();
        assert_ne!(imported.id, target.id); assert!(requires_embedded_ssh(&imported));
        let hop = server_by_id(&updated, imported.jump_server_id.as_ref().unwrap()).unwrap();
        assert_ne!(hop.id, relay.id); assert_eq!(fs::read_to_string(&hop.key_path).unwrap(), first_key);
        assert_eq!(fs::read_to_string(&imported.key_path).unwrap(), second_key);
        assert_eq!(fs::read_to_string(&relay.key_path).unwrap(), first_key);
        if let Ok(fixture) = std::env::var("PROFILE_TRANSFER_FIXTURE") {
            let path = PathBuf::from(fixture);
            fs::write(path.with_extension("svg"), exported.qr_svg.as_ref().unwrap()).unwrap();
            fs::write(path.with_extension("stprofile"), &exported.link).unwrap();
            assert_eq!(tauri::async_runtime::block_on(transfer_read_file(path.with_extension("stprofile").to_string_lossy().into_owned())).unwrap(), exported.link);
            fs::write(path, serde_json::json!({"link":exported.link,"code":exported.code,"private_key":second_key}).to_string()).unwrap();
        }
    }
}
