//! Desktop and Android compile the same SSH/SOCKS/UDP implementation.
#[path = "../../src-tauri/src/embedded_ssh.rs"]
pub mod embedded_ssh;
#[path = "../../src-tauri/src/udp_protocol.rs"]
pub mod udp_protocol;

pub fn log_line(logs: &std::sync::Arc<std::sync::Mutex<Vec<String>>>, line: impl Into<String>) {
    if let Ok(mut logs) = logs.lock() {
        if logs.len() >= 300 {
            logs.remove(0);
        }
        logs.push(line.into());
    }
}
