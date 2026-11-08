use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::{c_char, CStr},
    sync::{Mutex, OnceLock},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DomainRule {
    pub id: String,
    pub pattern: String,
    pub enabled: bool,
    pub action: DomainAction,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DomainAction {
    Tunnel,
    Direct,
}

pub fn compile(pattern: &str) -> Result<Regex, String> {
    if pattern.is_empty() || pattern.len() > 1024 {
        return Err("Регулярка должна содержать от 1 до 1024 байт".into());
    }
    RegexBuilder::new(&format!("\\A(?:{pattern})\\z"))
        .case_insensitive(true)
        .size_limit(1_000_000)
        .dfa_size_limit(1_000_000)
        .build()
        .map_err(|e| e.to_string())
}
pub fn normalize(domain: &str) -> Result<String, String> {
    let value = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if value.len() > 253
        || value.parse::<std::net::IpAddr>().is_ok()
        || !value.split('.').all(|s| {
            !s.is_empty()
                && s.len() <= 63
                && !s.starts_with('-')
                && !s.ends_with('-')
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(
            "Введите домен без https://, порта и пути; для IDN используйте punycode".into(),
        );
    }
    Ok(value)
}
pub fn validate(rules: &[DomainRule]) -> Result<(), String> {
    if rules.len() > 128 {
        return Err("Максимум 128 доменных правил".into());
    }
    let mut ids = std::collections::HashSet::new();
    for (i, rule) in rules.iter().enumerate() {
        if rule.id.is_empty()
            || rule.id.len() > 64
            || !rule
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            || !ids.insert(&rule.id)
        {
            return Err(format!("Правило {}: некорректный или повторный ID", i + 1));
        }
        compile(&rule.pattern).map_err(|e| format!("Правило {}: {e}", i + 1))?;
    }
    Ok(())
}
pub fn encoded(pattern: &str) -> String {
    format!(
        "stregex:{}",
        pattern
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}
fn decode(value: &str) -> Option<String> {
    let value = value.strip_prefix("stregex:")?;
    if value.len() > 2048 || value.len() % 2 != 0 {
        return None;
    }
    String::from_utf8(
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|b| u8::from_str_radix(std::str::from_utf8(b).ok()?, 16).ok())
            .collect::<Option<Vec<_>>>()?,
    )
    .ok()
}
// Only NUL-terminated strings owned by the native engine may cross this boundary.
#[no_mangle]
pub unsafe extern "C" fn st_regex_match(pattern: *const c_char, domain: *const c_char) -> i32 {
    if pattern.is_null() || domain.is_null() {
        return 0;
    }
    let (Ok(pattern), Ok(domain)) = (
        CStr::from_ptr(pattern).to_str(),
        CStr::from_ptr(domain).to_str(),
    ) else {
        return 0;
    };
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Regex>>>> = OnceLock::new();
    let Ok(mut cache) = CACHE.get_or_init(Default::default).lock() else {
        return 0;
    };
    if !cache.contains_key(pattern) {
        if cache.len() >= 128 {
            return 0;
        }
        cache.insert(
            pattern.to_string(),
            decode(pattern).and_then(|p| compile(&p).ok()),
        );
    }
    let Some(Some(regex)) = cache.get(pattern) else {
        return 0;
    };
    if domain.is_empty() {
        return 1;
    } // native registration/validation
    normalize(domain)
        .map(|d| regex.is_match(&d) as i32)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_domain_case_and_safety() {
        let r = compile(r"(?:.*\.)?example\.com").unwrap();
        assert!(r.is_match("EXAMPLE.COM"));
        assert!(r.is_match("cdn.example.com"));
        assert!(!r.is_match("example.com.evil"));
        assert!(!r.is_match("notexample.com"));
        assert!(compile("(?=x)").is_err());
        assert!(compile(r"(a)\1").is_err());
        assert!(normalize("https://example.com/path").is_err());
        assert_eq!(normalize("EXAMPLE.COM.").unwrap(), "example.com");
    }
    #[test]
    fn encoded_regex_preserves_delimiters() {
        let p = r"[a-z]{1,4}\.example\.com";
        assert_eq!(decode(&encoded(p)).unwrap(), p);
        let p = std::ffi::CString::new(encoded(p)).unwrap();
        let d = std::ffi::CString::new("test.example.com").unwrap();
        assert_eq!(unsafe { st_regex_match(p.as_ptr(), d.as_ptr()) }, 1);
    }
}
