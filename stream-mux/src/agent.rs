//! Bounded, versioned agent discovery before opening the data channel.
use crate::{FlowKind, FlowWindow};
use serde::{Deserialize, Serialize};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const OUTPUT_LIMIT: usize = 8192;
pub const QUERY_COMMAND: &str = r#"if [ ! -f "$HOME/.local/bin/secret-tunnel-agent" ]; then exit 200; fi; if [ ! -x "$HOME/.local/bin/secret-tunnel-agent" ]; then exit 201; fi; exec "$HOME/.local/bin/secret-tunnel-agent" --capabilities"#;
pub const START_COMMAND: &str = r#"exec "$HOME/.local/bin/secret-tunnel-agent" --stdio-v3"#;
pub const DIAGNOSTICS_COMMAND: &str = r#"exec "$HOME/.local/bin/secret-tunnel-agent" --stdio-v3-diagnostics"#;
pub const FLOW_CONTROL_COMMAND: &str = r#"exec "$HOME/.local/bin/secret-tunnel-agent" --stdio-v3-flow-control"#;
pub const UDP_CONFIG_COMMAND: &str = r#"exec "$HOME/.local/bin/secret-tunnel-agent" --stdio-v3-udp-config"#;
pub const QOS_COMMAND: &str = r#"exec "$HOME/.local/bin/secret-tunnel-agent" --stdio-v3-qos"#;
const UDP_CONFIG_MAGIC: &[u8; 8] = b"STUDPC01";

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
    #[serde(default)]
    pub diagnostics: bool,
    #[serde(default)]
    pub flow_control: bool,
    #[serde(default = "legacy_udp_slot_limit")]
    pub max_udp_slots: usize,
    #[serde(default)]
    pub udp_config: bool,
    #[serde(default)]
    pub heartbeat: bool,
    #[serde(default)]
    pub qos: bool,
    /// The agent grows TCP sending inside a peer window above the baseline and
    /// accepts [`ELASTIC_TCP_FLAG`] to announce a larger window of its own.
    #[serde(default)]
    pub elastic_tcp: bool,
}

/// Appended to a stdio command by a client that also sends elastically. Without
/// it the agent keeps the baseline window: an older client would fill a larger
/// one at once and queue megabytes ahead of voice on a slow link.
pub const ELASTIC_TCP_FLAG: &str = "--elastic-tcp";

/// The stdio command for an agent, with the elastic flag when both ends support it.
pub fn command(base: &str, elastic_tcp: bool) -> String {
    if elastic_tcp {
        format!("{base} {ELASTIC_TCP_FLAG}")
    } else {
        base.to_owned()
    }
}

fn legacy_udp_slot_limit() -> usize {
    crate::UDP_WINDOW.slots
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
            diagnostics: false,
            flow_control: false,
            max_udp_slots: crate::UDP_FLOW_SLOT_LIMIT,
            udp_config: false,
            heartbeat: true,
            qos: false,
            elastic_tcp: false,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionSettings {
    pub udp_window: FlowWindow,
    pub tcp_interactive_bytes: usize,
}
impl SessionSettings {
    fn valid(self) -> bool {
        self.udp_window.valid(FlowKind::Udp)
            && (65536..=8 * 1024 * 1024).contains(&self.tcp_interactive_bytes)
            && self.tcp_interactive_bytes % 1024 == 0
    }
}
pub async fn write_session_settings<W: AsyncWrite + Unpin>(
    writer: &mut W,
    settings: SessionSettings,
) -> io::Result<()> {
    if !settings.valid() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid session QoS settings",
        ));
    }
    let mut request = Vec::with_capacity(16);
    request.extend_from_slice(b"STQOS001");
    request.extend_from_slice(&(settings.udp_window.bytes as u32).to_be_bytes());
    request.extend_from_slice(&(settings.udp_window.slots as u16).to_be_bytes());
    request.extend_from_slice(&((settings.tcp_interactive_bytes / 1024) as u16).to_be_bytes());
    writer.write_all(&request).await?;
    writer.flush().await
}
pub async fn read_session_settings<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> io::Result<SessionSettings> {
    let mut request = [0; 16];
    reader.read_exact(&mut request).await?;
    let settings = SessionSettings {
        udp_window: FlowWindow {
            bytes: u32::from_be_bytes(request[8..12].try_into().unwrap()) as usize,
            slots: u16::from_be_bytes(request[12..14].try_into().unwrap()) as usize,
        },
        tcp_interactive_bytes: u16::from_be_bytes(request[14..16].try_into().unwrap()) as usize
            * 1024,
    };
    if &request[..8] != b"STQOS001" || !settings.valid() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid session QoS settings",
        ));
    }
    Ok(settings)
}

pub async fn write_udp_window_request<W: AsyncWrite + Unpin>(
    writer: &mut W,
    window: FlowWindow,
) -> io::Result<()> {
    if !window.valid(FlowKind::Udp) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid UDP window"));
    }
    let mut request = [0u8; 16];
    request[..8].copy_from_slice(UDP_CONFIG_MAGIC);
    request[8..12].copy_from_slice(&(window.bytes as u32).to_be_bytes());
    request[12..].copy_from_slice(&(window.slots as u32).to_be_bytes());
    writer.write_all(&request).await?;
    writer.flush().await
}

pub async fn read_udp_window_request<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> io::Result<FlowWindow> {
    let mut request = [0u8; 16];
    reader.read_exact(&mut request).await?;
    let window = FlowWindow {
        bytes: u32::from_be_bytes(request[8..12].try_into().unwrap()) as usize,
        slots: u32::from_be_bytes(request[12..].try_into().unwrap()) as usize,
    };
    if &request[..8] != UDP_CONFIG_MAGIC || !window.valid(FlowKind::Udp) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid UDP window request"));
    }
    Ok(window)
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

    #[tokio::test]
    async fn session_settings_are_bounded_and_reject_truncated_or_unknown_requests() {
        let settings = SessionSettings {
            udp_window: FlowWindow {
                bytes: 262144,
                slots: 192,
            },
            tcp_interactive_bytes: 65536,
        };
        let mut bytes = Vec::new();
        write_session_settings(&mut bytes, settings).await.unwrap();
        assert_eq!(
            read_session_settings(&mut bytes.as_slice()).await.unwrap(),
            settings
        );
        assert!(read_session_settings(&mut &bytes[..15]).await.is_err());
        bytes[0] = b'X';
        assert!(read_session_settings(&mut bytes.as_slice()).await.is_err());
        assert!(write_session_settings(
            &mut Vec::new(),
            SessionSettings {
                tcp_interactive_bytes: 1024,
                ..settings
            }
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn udp_window_request_rejects_invalid_limits_and_magic() {
        let mut frame = Vec::new();
        let window = FlowWindow { bytes: 256 * 1024, slots: 192 };
        write_udp_window_request(&mut frame, window).await.unwrap();
        assert_eq!(read_udp_window_request(&mut frame.as_slice()).await.unwrap(), window);
        assert!(read_udp_window_request(&mut &frame[..15]).await.is_err());
        let mut bad_magic = frame.clone();
        bad_magic[0] = b'X';
        assert!(read_udp_window_request(&mut bad_magic.as_slice()).await.is_err());
        frame[12..].copy_from_slice(&(crate::UDP_FLOW_SLOT_LIMIT as u32 + 1).to_be_bytes());
        assert!(read_udp_window_request(&mut frame.as_slice()).await.is_err());
    }

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
