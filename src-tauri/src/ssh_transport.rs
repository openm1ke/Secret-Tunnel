//! Observe the outer socket without duplicating it or extending its lifetime.
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CipherPolicy {
    #[default]
    Auto,
    Aes128Gcm,
    Chacha20Poly1305,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SshTuning {
    pub cipher: CipherPolicy,
    pub channel_buffer_messages: usize,
    pub channel_receive_window_bytes: u32,
    pub tcp_interactive_bytes: Option<usize>,
}

impl Default for SshTuning {
    fn default() -> Self {
        Self {
            cipher: CipherPolicy::Auto,
            channel_buffer_messages: 32,
            // Bounds the download of the whole session: 16 flows at 200 ms RTT
            // reached 42.6 Mbit/s with 2 MiB and 83 Mbit/s with 4 MiB.
            channel_receive_window_bytes: 4 * 1024 * 1024,
            tcp_interactive_bytes: Some(96 * 1024),
        }
    }
}

impl SshTuning {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=128).contains(&self.channel_buffer_messages)
            || !(65536..=8 * 1024 * 1024).contains(&self.channel_receive_window_bytes)
            || self.tcp_interactive_bytes.is_some_and(|bytes| {
                !(64 * 1024..=8 * 1024 * 1024).contains(&bytes) || bytes % 1024 != 0
            })
        {
            return Err("Некорректные пределы очереди или окна SSH".into());
        }
        Ok(())
    }

    pub(super) fn configure(&self, config: &mut russh::client::Config) {
        use std::borrow::Cow;
        config.channel_buffer_size = self.channel_buffer_messages;
        config.window_size = self.channel_receive_window_bytes;
        config.preferred.compression = Cow::Borrowed(&[russh::compression::NONE]);
        match self.cipher {
            CipherPolicy::Auto => {}
            CipherPolicy::Aes128Gcm => {
                config.preferred.cipher = Cow::Borrowed(&[russh::cipher::AES_128_GCM]);
            }
            CipherPolicy::Chacha20Poly1305 => {
                config.preferred.cipher = Cow::Borrowed(&[russh::cipher::CHACHA20_POLY1305]);
            }
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TcpSnapshot {
    pub at_ms: u64,
    pub connected: bool,
    pub source: &'static str,
    pub error_code: Option<i32>,
    pub rtt_us: Option<u32>,
    pub rto_us: Option<u32>,
    pub min_rtt_us: Option<u32>,
    pub unacked_packets: Option<u32>,
    pub not_sent_bytes: Option<u32>,
    pub send_queue_bytes: Option<u32>,
    pub bytes_in_flight: Option<u32>,
    pub congestion_window_bytes: Option<u64>,
    pub total_retransmits: Option<u32>,
    pub bytes_retransmitted: Option<u64>,
    pub retransmit_timeouts: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SshSnapshot {
    pub tuning: SshTuning,
    pub outer_tcp: TcpSnapshot,
}

#[cfg(unix)]
type Socket = std::os::fd::RawFd;
#[cfg(windows)]
type Socket = std::os::windows::io::RawSocket;

#[derive(Debug, Default)]
pub(super) struct TcpObserver(Mutex<Option<Socket>>);

impl TcpObserver {
    pub(super) fn snapshot(&self) -> TcpSnapshot {
        let guard = self.0.lock().unwrap();
        match *guard {
            Some(socket) => platform_snapshot(socket),
            None => TcpSnapshot {
                at_ms: secret_tunnel_mux::unix_millis(),
                source: "closed",
                ..Default::default()
            },
        }
    }
}

pub(super) struct ObservedStream {
    stream: TcpStream,
    observer: Arc<TcpObserver>,
}

impl ObservedStream {
    pub(super) fn new(stream: TcpStream, observer: Arc<TcpObserver>) -> Self {
        #[cfg(unix)]
        use std::os::fd::AsRawFd;
        #[cfg(windows)]
        use std::os::windows::io::AsRawSocket;
        #[cfg(unix)]
        let socket = stream.as_raw_fd();
        #[cfg(windows)]
        let socket = stream.as_raw_socket();
        *observer.0.lock().unwrap() = Some(socket);
        Self { stream, observer }
    }
}

impl Drop for ObservedStream {
    fn drop(&mut self) {
        // snapshot holds this lock during the OS call. Clear before closing the
        // stream, so it can never inspect a recycled descriptor/socket handle.
        *self.observer.0.lock().unwrap() = None;
    }
}

impl AsyncRead for ObservedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for ObservedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn linux_info(bytes: &[u8]) -> TcpSnapshot {
    // Linux UAPI tcp_info has an append-only layout. Check the length returned
    // by getsockopt for every field; older Android kernels omit its tail.
    let u32_at = |offset| {
        bytes
            .get(offset..offset + 4)
            .map(|b| u32::from_ne_bytes(b.try_into().unwrap()))
    };
    let u64_at = |offset| {
        bytes
            .get(offset..offset + 8)
            .map(|b| u64::from_ne_bytes(b.try_into().unwrap()))
    };
    TcpSnapshot {
        at_ms: secret_tunnel_mux::unix_millis(),
        connected: true,
        source: "linux_tcp_info",
        rto_us: u32_at(8),
        unacked_packets: u32_at(24),
        rtt_us: u32_at(68),
        congestion_window_bytes: u32_at(80)
            .zip(u32_at(16))
            .map(|(segments, mss)| u64::from(segments) * u64::from(mss)),
        total_retransmits: u32_at(100),
        not_sent_bytes: u32_at(144),
        min_rtt_us: u32_at(148),
        bytes_retransmitted: u64_at(208),
        ..Default::default()
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn platform_snapshot(socket: Socket) -> TcpSnapshot {
    let mut buffer = [0u8; 256];
    let mut length = buffer.len() as libc::socklen_t;
    // socket belongs to ObservedStream and cannot close while the observer is locked.
    let result = unsafe {
        libc::getsockopt(
            socket,
            libc::IPPROTO_TCP,
            libc::TCP_INFO,
            buffer.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 {
        return TcpSnapshot {
            at_ms: secret_tunnel_mux::unix_millis(),
            connected: true,
            source: "unavailable",
            error_code: std::io::Error::last_os_error().raw_os_error(),
            ..Default::default()
        };
    }
    let mut snapshot = linux_info(&buffer[..(length as usize).min(buffer.len())]);
    let mut queued: libc::c_int = 0;
    if unsafe { libc::ioctl(socket, libc::TIOCOUTQ, &mut queued) } == 0 {
        snapshot.send_queue_bytes = u32::try_from(queued).ok();
    }
    snapshot
}

#[cfg(windows)]
fn platform_snapshot(socket: Socket) -> TcpSnapshot {
    use windows_sys::Win32::Networking::WinSock::{
        TCP_INFO_v0, WSAGetLastError, WSAIoctl, SIO_TCP_INFO,
    };
    let version = 0u32;
    let mut info: TCP_INFO_v0 = unsafe { std::mem::zeroed() };
    let mut returned = 0u32;
    let result = unsafe {
        WSAIoctl(
            socket as usize,
            SIO_TCP_INFO,
            (&version as *const u32).cast(),
            std::mem::size_of_val(&version) as u32,
            (&mut info as *mut TCP_INFO_v0).cast(),
            std::mem::size_of_val(&info) as u32,
            &mut returned,
            std::ptr::null_mut(),
            None,
        )
    };
    if result != 0 || returned < std::mem::size_of_val(&info) as u32 {
        return TcpSnapshot {
            at_ms: secret_tunnel_mux::unix_millis(),
            connected: true,
            source: "unavailable",
            error_code: (result != 0).then(|| unsafe { WSAGetLastError() }),
            ..Default::default()
        };
    }
    TcpSnapshot {
        at_ms: secret_tunnel_mux::unix_millis(),
        connected: true,
        source: "windows_tcp_info_v0",
        rtt_us: Some(info.RttUs),
        min_rtt_us: Some(info.MinRttUs),
        bytes_in_flight: Some(info.BytesInFlight),
        congestion_window_bytes: Some(u64::from(info.Cwnd)),
        bytes_retransmitted: Some(u64::from(info.BytesRetrans)),
        retransmit_timeouts: Some(info.TimeoutEpisodes),
        ..Default::default()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
fn platform_snapshot(_: Socket) -> TcpSnapshot {
    TcpSnapshot {
        at_ms: secret_tunnel_mux::unix_millis(),
        connected: true,
        source: "unsupported",
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn observes_real_socket_and_does_not_keep_it_alive() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (mut peer, _) = listener.accept().await.unwrap();
        let observer = Arc::new(TcpObserver::default());
        let mut stream = ObservedStream::new(stream, observer.clone());
        stream.write_all(b"hello").await.unwrap();
        let mut payload = [0; 5];
        peer.read_exact(&mut payload).await.unwrap();
        assert_eq!(&payload, b"hello");
        let stats = observer.snapshot();
        assert!(stats.connected);
        #[cfg(any(target_os = "linux", target_os = "android", windows))]
        assert!(stats.rtt_us.is_some(), "{stats:?}");
        drop(stream);
        assert!(!observer.snapshot().connected);
        let mut tail = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            peer.read_to_end(&mut tail),
        )
        .await
        .unwrap()
        .unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn older_kernel_layout_does_not_fabricate_missing_counters() {
        let mut bytes = [0u8; 104];
        bytes[68..72].copy_from_slice(&12_345u32.to_ne_bytes());
        bytes[100..104].copy_from_slice(&7u32.to_ne_bytes());
        let stats = linux_info(&bytes);
        assert_eq!(stats.rtt_us, Some(12_345));
        assert_eq!(stats.total_retransmits, Some(7));
        assert!(stats.not_sent_bytes.is_none());
        assert!(stats.bytes_retransmitted.is_none());
    }

    #[test]
    fn tuning_rejects_unbounded_queues_and_unsupported_ciphers() {
        assert!(serde_json::from_str::<SshTuning>(r#"{"cipher":"none"}"#).is_err());
        let tuning: SshTuning = serde_json::from_str(r#"{"channel_buffer_messages":0}"#).unwrap();
        assert!(tuning.validate().is_err());
        let mut config = russh::client::Config::default();
        let tuning = SshTuning {
            cipher: CipherPolicy::Aes128Gcm,
            channel_buffer_messages: 8,
            channel_receive_window_bytes: 1024 * 1024,
            tcp_interactive_bytes: None,
        };
        tuning.validate().unwrap();
        tuning.configure(&mut config);
        assert_eq!(
            config.preferred.cipher.as_ref(),
            &[russh::cipher::AES_128_GCM]
        );
        assert_eq!(
            config.preferred.compression.as_ref(),
            &[russh::compression::NONE]
        );
    }
}
