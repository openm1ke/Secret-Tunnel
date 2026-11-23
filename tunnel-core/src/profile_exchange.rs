//! FFI adapters share key validation as well as the envelope format.
use crate::profile_transfer::{self as transfer, Document};
use russh::keys::ssh_key::LineEnding;

fn keys(document: &mut Document) -> Result<(), String> {
    document.validate()?;
    for server in &mut document.servers {
        let key = russh::keys::decode_secret_key(&server.private_key, None)
            .map_err(|_| "Не удалось прочитать SSH-ключ. Используйте ключ без парольной фразы.".to_owned())?;
        let canonical = key.to_openssh(LineEnding::LF).map_err(|_| "Не удалось прочитать SSH-ключ")?.to_string();
        server.replace_key(canonical);
    }
    Ok(())
}
pub fn export(raw: &str) -> Result<String, String> {
    if raw.len() > transfer::MAX_PLAIN { return Err("Слишком много данных для переноса".into()); }
    let mut document: Document = serde_json::from_str(raw).map_err(|_| "Некорректные данные серверов")?;
    keys(&mut document)?;
    let code = transfer::generate_code();
    let link = transfer::encrypt(&document, &code)?;
    Ok(serde_json::json!({"qr_available":transfer::qr_svg(&link).is_some(),"link":link,"code":code,"server_count":document.servers.len()}).to_string())
}
pub fn open(payload: &str, code: &str, preview: bool) -> Result<String, String> {
    let mut document = transfer::decrypt(payload, code)?;
    keys(&mut document)?;
    if preview { serde_json::to_string(&document.preview()) } else { serde_json::to_string(&document) }.map_err(|_| "Не удалось прочитать перенос".into())
}
