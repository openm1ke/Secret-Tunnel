//! First connection uses a host key explicitly accepted in the setup wizard.
use super::{authenticate_ssh, handshake, load_identity, EmbeddedOptions, EmbeddedServer, TcpObserver};
use russh::{client, ChannelMsg};
use russh::keys::ssh_key::{Algorithm, LineEnding, PrivateKey};
use std::sync::Arc;
use std::time::Duration;

pub fn generate_key() -> Result<String, String> {
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).map_err(|e| e.to_string())?;
    Ok(key.to_openssh(LineEnding::LF).map_err(|e| e.to_string())?.to_string())
}

pub fn key_command(public: &str) -> Result<String, String> {
    if !public.starts_with("ssh-ed25519 ") || public.len() > 512 ||
        !public.chars().all(|c| c.is_ascii_alphanumeric() || " +/=@._-".contains(c)) {
        return Err("Некорректный публичный ключ".into());
    }
    // A retry reuses the device key. The lock also serializes simultaneous registrations.
    Ok(format!(r#"set -eu
umask 077
mkdir -p "$HOME/.ssh"
exec 9>>"$HOME/.ssh/.secret-tunnel-key-lock"
flock -w 5 9 || exit 73
chmod 700 "$HOME/.ssh"
touch "$HOME/.ssh/authorized_keys"
chmod 600 "$HOME/.ssh/authorized_keys"
grep -qxF '{public}' "$HOME/.ssh/authorized_keys" || printf '\n%s\n' '{public}' >> "$HOME/.ssh/authorized_keys"
"#))
}

pub fn enroll_password(server: EmbeddedServer, pin: String, password: String, bind_physical: bool) -> Result<(), String> {
    if pin.len() != 50 || !pin.starts_with("SHA256:") {
        return Err("Сначала подтвердите отпечаток сервера".into());
    }
    if password.is_empty() { return Err("Введите пароль сервера".into()); }
    let identity = load_identity(&server)?;
    let public = identity.public_key().to_openssh().map_err(|e| e.to_string())?;
    let command = key_command(&public)?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(35), async {
            let options = EmbeddedOptions { host_key_sha256: Some(pin.clone()), bind_physical_interface: bind_physical, ..Default::default() };
            let mut session = handshake(&server, (server.host.as_str(), server.port), options.clone(), Arc::new(TcpObserver::default())).await?;
            let result = async {
                let auth = session.authenticate_password(&server.user, password).await.map_err(|_| "Ошибка входа по паролю. Проверьте логин и настройки SSH.".to_string())?;
                if !auth.success() { return Err("Сервер не принял логин или пароль. Проверьте данные из письма хостера. Если вход по паролю выключен, выберите готовый SSH-ключ.".into()); }
                append_key(&session, command).await
            }.await;
            let _ = session.disconnect(russh::Disconnect::ByApplication, "", "English").await;
            result?;
            // A successful append is insufficient: the server may ignore this authorized_keys file.
            let mut verification = handshake(&server, (server.host.as_str(), server.port), options, Arc::new(TcpObserver::default())).await?;
            let result = authenticate_ssh(&server, &mut verification, identity).await;
            let _ = verification.disconnect(russh::Disconnect::ByApplication, "", "English").await;
            result.map_err(|_| "Ключ добавлен, но сервер не разрешил вход по нему. Проверьте PublicKeyAuthentication и AuthorizedKeysFile в настройках SSH.".into())
        }).await.map_err(|_| "Сервер не завершил настройку за 35 с. Проверьте сеть и повторите попытку.".to_string())?
    })
}

async fn append_key<H: client::Handler>(session: &client::Handle<H>, command: String) -> Result<(), String> {
    let mut channel = session.channel_open_session().await.map_err(|e| e.to_string())?;
    channel.exec(true, command).await.map_err(|e| e.to_string())?;
    let mut status = None;
    while let Some(message) = channel.wait().await {
        if let ChannelMsg::ExitStatus { exit_status } = message { status = Some(exit_status); }
    }
    match status {
        Some(0) => Ok(()),
        Some(73) => Err("Другой процесс добавляет ключ. Повторите попытку через несколько секунд.".into()),
        _ => Err("Не удалось записать публичный ключ. Проверьте права на домашнюю папку пользователя.".into()),
    }
}

pub fn friendly_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("encrypted") || lower.contains("passphrase") {
        "Ключ защищён парольной фразой. Выберите ключ без парольной фразы или создайте отдельный ключ через вход по паролю сервера.".into()
    } else if lower.contains("authentication failed") {
        "Сервер не принял ключ. Проверьте пользователя и выберите ключ, уже разрешённый на сервере.".into()
    } else if lower.contains("host key") || lower.contains("fingerprint") {
        "Ключ сервера изменился. Получите отпечаток заново и сверьте его в панели хостера.".into()
    } else if lower.contains("refused") {
        "Сервер отклонил подключение. Проверьте SSH-порт и разрешение доступа в панели хостера.".into()
    } else if lower.contains("timeout") || lower.contains("timed out") || lower.contains("before the connection timeout") {
        "Сервер не ответил вовремя. Проверьте адрес, порт, сеть и доступ к серверу в панели хостера.".into()
    } else if lower.contains("resolve") || lower.contains("lookup") || lower.contains("nodename") {
        "Не удалось найти сервер. Проверьте адрес или укажите IP из письма хостера.".into()
    } else { raw.chars().take(400).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_command_rejects_shell_input() {
        assert!(key_command("ssh-ed25519 abc'; id #").is_err());
        assert!(key_command("ssh-ed25519 abc\ncommand").is_err());
        let key = generate_key().unwrap();
        let private = PrivateKey::from_openssh(&key).unwrap();
        assert!(key_command(&private.public_key().to_openssh().unwrap()).is_ok());
    }
}
