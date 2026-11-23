//! Offline profile exchange. Links, QR codes and files contain the same authenticated
//! ciphertext. The independently generated 100-bit transfer code is never embedded.
use aes_gcm::{aead::{Aead, KeyInit, Payload}, Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, io::{Read, Write}};
use zeroize::{Zeroize, Zeroizing};

pub const LINK_PREFIX: &str = "secrettunnel://import#stp1.";
pub const MAX_INPUT: usize = 1_048_576;
pub const MAX_PLAIN: usize = 768 * 1024;
const MAGIC: &[u8; 4] = b"STP\x01";
const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const AUTH_ERROR: &str = "Неверный код переноса или повреждённые данные. Проверьте код и повторите попытку.";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub host_key_sha256: String,
    pub private_key: String,
    #[serde(default)]
    pub jump_id: Option<String>,
}
impl Drop for Server { fn drop(&mut self) { self.private_key.zeroize(); } }
impl Server {
    pub fn replace_key(&mut self, canonical: String) { self.private_key.zeroize(); self.private_key = canonical; }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub version: u8,
    pub roots: Vec<String>,
    pub servers: Vec<Server>,
}

#[derive(Serialize)]
pub struct PreviewServer<'a> {
    pub id: &'a str, pub name: &'a str, pub host: &'a str, pub port: u16,
    pub user: &'a str, pub host_key_sha256: &'a str, pub jump_id: &'a Option<String>,
}
#[derive(Serialize)]
pub struct Preview<'a> { pub roots: &'a [String], pub servers: Vec<PreviewServer<'a>> }

impl Document {
    pub fn preview(&self) -> Preview<'_> {
        Preview { roots: &self.roots, servers: self.servers.iter().map(|s| PreviewServer {
            id: &s.id, name: &s.name, host: &s.host, port: s.port, user: &s.user,
            host_key_sha256: &s.host_key_sha256, jump_id: &s.jump_id,
        }).collect() }
    }
    pub fn validate(&self) -> Result<(), String> {
        let invalid = || "Некорректный файл серверов. Создайте экспорт заново в Secret Tunnel.".to_owned();
        if self.version != 1 || self.servers.is_empty() || self.servers.len() > 32 || self.roots.is_empty() || self.roots.len() > 32 { return Err(invalid()); }
        let ids: HashSet<_> = self.servers.iter().map(|s| s.id.as_str()).collect();
        if ids.len() != self.servers.len() || self.roots.iter().any(|id| !ids.contains(id.as_str())) || self.roots.iter().collect::<HashSet<_>>().len() != self.roots.len() { return Err(invalid()); }
        for server in &self.servers {
            if server.id.is_empty() || server.id.len() > 128 || server.name.trim().is_empty() || server.name.chars().count() > 64 || server.name.chars().any(char::is_control)
                || server.host.is_empty() || server.host.len() > 253 || server.host.starts_with('-')
                || server.host.chars().any(|c| c.is_control() || c.is_whitespace() || "/@?#\\".contains(c))
                || server.port == 0 || server.user.is_empty() || server.user.len() > 128 || server.user.starts_with('-')
                || server.user.chars().any(|c| c.is_control() || c.is_whitespace())
                || server.private_key.len() > 16 * 1024 || !server.private_key.contains("PRIVATE KEY-----")
                || !valid_pin(&server.host_key_sha256) { return Err(invalid()); }
            let mut next = Some(server.id.as_str());
            let mut seen = HashSet::new();
            while let Some(id) = next {
                if !seen.insert(id) || seen.len() > 4 { return Err("В цепочке допустимо до трёх промежуточных серверов, без повторений.".into()); }
                next = self.servers.iter().find(|s| s.id == id).ok_or_else(invalid)?.jump_id.as_deref();
            }
        }
        Ok(())
    }
}

pub fn valid_pin(pin: &str) -> bool {
    pin.len() == 50 && pin.starts_with("SHA256:") && pin.as_bytes()[7..].iter().all(|b| b.is_ascii_alphanumeric() || b"+/".contains(b))
}

pub fn generate_code() -> String {
    let raw: String = (0..20).map(|_| ALPHABET[rand::random_range(0..32)] as char).collect();
    raw.as_bytes().chunks(4).map(|part| std::str::from_utf8(part).unwrap()).collect::<Vec<_>>().join("-")
}
fn normalized_code(code: &str) -> Result<Zeroizing<String>, String> {
    if code.len() > 128 { return Err("Введите код переноса из 20 символов.".into()); }
    let normalized = Zeroizing::new(code.chars().filter(|c| !c.is_ascii_whitespace() && *c != '-').map(|c| c.to_ascii_uppercase()).collect::<String>());
    if normalized.len() != 20 || !normalized.bytes().all(|b| ALPHABET.contains(&b)) { return Err("Введите код переноса из 20 символов. Дефисы и пробелы можно пропустить.".into()); }
    Ok(normalized)
}
fn derive(code: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
    let code = normalized_code(code)?;
    let params = Params::new(19_456, 2, 1, Some(32)).map_err(|e| e.to_string())?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params).hash_password_into(code.as_bytes(), salt, key.as_mut()).map_err(|e| e.to_string())?;
    Ok(key)
}
pub fn encrypt(document: &Document, code: &str) -> Result<String, String> {
    document.validate()?;
    let plain = Zeroizing::new(serde_json::to_vec(document).map_err(|e| e.to_string())?);
    if plain.len() > MAX_PLAIN { return Err("Слишком много данных. Перенесите серверы по частям.".into()); }
    let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
    compressor.write_all(&plain).map_err(|e| e.to_string())?;
    let compressed = Zeroizing::new(compressor.finish().map_err(|e| e.to_string())?);
    let salt = rand::random::<[u8; 16]>();
    let nonce = rand::random::<[u8; 12]>();
    let key = derive(code, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| "Не удалось создать шифр")?;
    let encrypted = cipher.encrypt(Nonce::from_slice(&nonce), Payload { msg: &compressed, aad: MAGIC }).map_err(|_| "Не удалось зашифровать серверы")?;
    let mut envelope = Vec::with_capacity(32 + encrypted.len());
    envelope.extend(MAGIC); envelope.extend(salt); envelope.extend(nonce); envelope.extend(encrypted);
    let link = format!("{LINK_PREFIX}{}", URL_SAFE_NO_PAD.encode(envelope));
    if link.len() > MAX_INPUT { return Err("Слишком много данных. Перенесите серверы по частям.".into()); }
    Ok(link)
}
pub fn decrypt(input: &str, code: &str) -> Result<Document, String> {
    if input.len() > MAX_INPUT { return Err("Файл переноса больше 1 МБ.".into()); }
    let text = input.trim().strip_prefix('\u{feff}').unwrap_or(input.trim());
    let encoded = text.strip_prefix(LINK_PREFIX).or_else(|| text.strip_prefix("stp1.")).ok_or("Это не ссылка или файл переноса Secret Tunnel.")?;
    let envelope = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| "Повреждён файл переноса")?;
    if envelope.len() < 48 || &envelope[..4] != MAGIC { return Err("Неподдерживаемый формат переноса".into()); }
    let key = derive(code, &envelope[4..20])?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| AUTH_ERROR)?;
    let compressed = Zeroizing::new(cipher.decrypt(Nonce::from_slice(&envelope[20..32]), Payload { msg: &envelope[32..], aad: MAGIC }).map_err(|_| AUTH_ERROR)?);
    let mut plain = Zeroizing::new(Vec::new());
    ZlibDecoder::new(compressed.as_slice()).take(MAX_PLAIN as u64 + 1).read_to_end(&mut plain).map_err(|_| "Повреждены данные переноса")?;
    if plain.len() > MAX_PLAIN { return Err("Распакованные данные слишком велики.".into()); }
    let document: Document = serde_json::from_slice(&plain).map_err(|_| "Некорректные данные серверов")?;
    document.validate()?;
    Ok(document)
}
pub fn qr_svg(link: &str) -> Option<String> {
    if link.len() > 2300 { return None; }
    let code = qrcode::QrCode::with_error_correction_level(link.as_bytes(), qrcode::EcLevel::M).ok()?;
    Some(code.render::<qrcode::render::svg::Color>().min_dimensions(320, 320).dark_color(qrcode::render::svg::Color("#000000")).light_color(qrcode::render::svg::Color("#ffffff")).build())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn example() -> Document {
        Document { version: 1, roots: vec!["target".into()], servers: vec![Server {
            id: "target".into(), name: "Demo".into(), host: "203.0.113.10".into(), port: 2222,
            user: "demo".into(), host_key_sha256: format!("SHA256:{}", "A".repeat(43)),
            private_key: "-----BEGIN OPENSSH PRIVATE KEY-----\nfixture\n-----END OPENSSH PRIVATE KEY-----".into(), jump_id: None,
        }] }
    }
    #[test] fn transfer_round_trip_is_encrypted_and_randomized() {
        let document = example(); let code = generate_code();
        let first = encrypt(&document, &code).unwrap(); let second = encrypt(&document, &code).unwrap();
        assert_ne!(first, second); assert!(!first.contains("203.0.113")); assert!(!first.contains("PRIVATE KEY"));
        let restored = decrypt(&first, &code.to_lowercase().replace('-', " ")).unwrap();
        assert_eq!(restored.servers[0].private_key, document.servers[0].private_key);
        assert!(!serde_json::to_string(&restored.preview()).unwrap().contains("private_key"));
        assert!(qr_svg(&first).is_some());
    }
    #[test] fn wrong_code_and_modified_ciphertext_are_rejected() {
        let code = generate_code(); let link = encrypt(&example(), &code).unwrap();
        assert!(matches!(decrypt(&link, &generate_code()), Err(error) if error == AUTH_ERROR));
        let mut envelope = URL_SAFE_NO_PAD.decode(link.strip_prefix(LINK_PREFIX).unwrap()).unwrap();
        envelope[40] ^= 1;
        let altered = format!("{LINK_PREFIX}{}", URL_SAFE_NO_PAD.encode(envelope));
        assert!(matches!(decrypt(&altered, &code), Err(error) if error == AUTH_ERROR));
        assert!(decrypt(&"x".repeat(MAX_INPUT + 1), &code).is_err());
        assert!(decrypt("https://example.com/untrusted", &code).is_err());
    }
    #[test] fn chains_reject_missing_hops_cycles_and_duplicate_ids() {
        let mut document = example(); document.servers[0].jump_id = Some("missing".into()); assert!(document.validate().is_err());
        document.servers[0].jump_id = Some("target".into()); assert!(document.validate().is_err());
        document.servers[0].jump_id = None; document.servers.push(document.servers[0].clone()); assert!(document.validate().is_err());
    }
    #[test] fn authenticated_compression_bomb_is_bounded() {
        // A sender knows the code, so authentication alone cannot bound decompression.
        let code = generate_code();
        let mut compressor = ZlibEncoder::new(Vec::new(), Compression::best());
        compressor.write_all(&vec![b' '; MAX_PLAIN + 1]).unwrap();
        let compressed = compressor.finish().unwrap();
        let salt = [1; 16]; let nonce = [2; 12];
        let key = derive(&code, &salt).unwrap();
        let cipher = Aes256Gcm::new_from_slice(key.as_ref()).unwrap();
        let encrypted = cipher.encrypt(Nonce::from_slice(&nonce), Payload { msg: &compressed, aad: MAGIC }).unwrap();
        let envelope = [MAGIC.as_slice(), &salt, &nonce, &encrypted].concat();
        let link = format!("{LINK_PREFIX}{}", URL_SAFE_NO_PAD.encode(envelope));
        assert!(matches!(decrypt(&link, &code), Err(error) if error == "Распакованные данные слишком велики."));
    }
    #[test] fn large_exports_remain_importable_without_a_qr() {
        let mut document = example();
        document.servers[0].private_key.push_str(&URL_SAFE_NO_PAD.encode(rand::random::<[u8; 4096]>()));
        let code = generate_code(); let link = encrypt(&document, &code).unwrap();
        assert!(link.len() > 2300); assert!(qr_svg(&link).is_none());
        assert_eq!(decrypt(&link, &code).unwrap().servers[0].private_key, document.servers[0].private_key);
    }
}
