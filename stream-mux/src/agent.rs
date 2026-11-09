//! Bounded, versioned agent discovery before opening the data channel.
use serde::{Deserialize, Serialize};

pub const OUTPUT_LIMIT: usize = 8192;
pub const QUERY_COMMAND: &str = r#"if [ ! -f "$HOME/.local/bin/secret-tunnel-agent" ]; then exit 200; fi; if [ ! -x "$HOME/.local/bin/secret-tunnel-agent" ]; then exit 201; fi; exec "$HOME/.local/bin/secret-tunnel-agent" --capabilities"#;
pub const START_COMMAND: &str = r#"exec "$HOME/.local/bin/secret-tunnel-agent" --stdio-v3"#;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Capabilities {
    pub schema: u16,
    pub agent_version: String,
    pub os: String,
    pub arch: String,
    pub protocols: Vec<u16>,
    pub tcp: bool,
    pub udp: bool,
    pub max_flows: usize,
}
impl Capabilities {
    pub fn current(version: &str, max_flows: usize) -> Self {
        Self {
            schema: 1,
            agent_version: version.into(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            protocols: vec![3],
            tcp: true,
            udp: true,
            max_flows,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || !self.protocols.contains(&3) || !self.tcp || !self.udp {
            return Err("AGENT_INCOMPATIBLE: SSH и ключ проверены. Агент не поддерживает требуемый STMUX003 с TCP и UDP. Обновите ~/.local/bin/secret-tunnel-agent.".into());
        }
        if self.max_flows < 32 {
            return Err("AGENT_RESOURCE_LIMIT: SSH и ключ проверены. Серверу не хватает файловых дескрипторов для агента.".into());
        }
        if self.protocols.len() > 32
            || [&self.agent_version, &self.os, &self.arch]
                .iter()
                .any(|value| {
                    value.is_empty()
                        || value.len() > 64
                        || !value.bytes().all(|b| b.is_ascii_graphic())
                })
        {
            return Err(invalid_response());
        }
        Ok(())
    }
}

impl Capabilities {
    pub fn summary(&self) -> String {
        format!(
            "Агент {} · {}/{} · STMUX003 · TCP/UDP · лимит {}",
            self.agent_version, self.os, self.arch, self.max_flows
        )
    }
}
pub fn invalid_response() -> String {
    "AGENT_INVALID_RESPONSE: SSH и ключ проверены. Агент вернул некорректную диагностику; проверьте его версию и вывод shell startup-файлов.".into()
}
pub fn parse(exit: Option<u32>, stdout: &[u8], stderr: &[u8]) -> Result<Capabilities, String> {
    if stdout.len().saturating_add(stderr.len()) > OUTPUT_LIMIT {
        return Err(invalid_response());
    }
    match exit {
        Some(200) => return Err("AGENT_MISSING: SSH и ключ проверены. Не найден ~/.local/bin/secret-tunnel-agent. Установите серверный агент из релиза для архитектуры сервера.".into()),
        Some(201 | 126 | 127) => return Err("AGENT_NOT_EXECUTABLE: SSH и ключ проверены. Агент нельзя запустить. Проверьте права выполнения, архитектуру и системные библиотеки агента.".into()),
        Some(0) => {
            let caps: Capabilities = serde_json::from_slice(stdout).map_err(|_| invalid_response())?;
            caps.validate()?;
            return Ok(caps);
        }
        Some(_) if stdout.is_empty() => {
            let diagnostic = String::from_utf8_lossy(stderr);
            if diagnostic.contains("Usage: secret-tunnel-agent [--multiplex]") {
                return Err("AGENT_INCOMPATIBLE: SSH и ключ проверены. Агент устарел; установите текущую серверную сборку с диагностикой schema 1 и STMUX003.".into());
            }
        }
        _ => {}
    }
    Err("AGENT_CHECK_FAILED: SSH и ключ проверены. Не удалось проверить агент. Проверьте его установку; маршруты не включены.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_capabilities_and_preserves_compatible_future_fields() {
        let caps = Capabilities::current("0.1.0", 256);
        let mut value = serde_json::to_value(&caps).unwrap();
        value["future"] = serde_json::json!(true);
        assert_eq!(
            parse(Some(0), &serde_json::to_vec(&value).unwrap(), &[]).unwrap(),
            caps.clone()
        );
        for bad in ["schema", "protocols", "tcp", "udp", "max_flows"] {
            let mut value = serde_json::to_value(&caps).unwrap();
            value[bad] = match bad {
                "schema" => serde_json::json!(2),
                "protocols" => serde_json::json!([1, 2]),
                "max_flows" => serde_json::json!(0),
                _ => serde_json::json!(false),
            };
            assert!(parse(Some(0), &serde_json::to_vec(&value).unwrap(), &[]).is_err());
        }
    }
    #[test]
    fn missing_permissions_malformed_and_bounded_output_are_distinct() {
        assert!(parse(Some(200), &[], &[])
            .unwrap_err()
            .starts_with("AGENT_MISSING:"));
        for exit in [201, 126, 127] {
            assert!(parse(Some(exit), &[], &[])
                .unwrap_err()
                .starts_with("AGENT_NOT_EXECUTABLE:"));
        }
        for bytes in [b"{}".as_slice(), b"null", b"welcome\n{}", b"STUDP001"] {
            assert!(parse(Some(0), bytes, &[])
                .unwrap_err()
                .starts_with("AGENT_INVALID_RESPONSE:"));
        }
        assert!(parse(Some(0), &vec![0; OUTPUT_LIMIT + 1], &[]).is_err());
        assert!(parse(None, &[], &[]).is_err());
    }
    #[test]
    fn old_agent_is_rejected_without_a_fallback() {
        assert!(parse(
            Some(1),
            &[],
            b"Error: Usage: secret-tunnel-agent [--multiplex]"
        )
        .unwrap_err()
        .starts_with("AGENT_INCOMPATIBLE:"));
        assert!(parse(
            Some(1),
            b"STUDP001",
            b"Usage: secret-tunnel-agent [--multiplex]"
        )
        .is_err());
        assert!(parse(Some(1), &[], b"Segmentation fault").is_err());
    }
}
