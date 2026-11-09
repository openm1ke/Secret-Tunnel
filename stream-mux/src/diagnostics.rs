use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub fn unix_millis() -> u64 {
    millis(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default(),
    )
}

fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TcpStage {
    Open,
    Dns,
    Connect,
    Relay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpEvent {
    pub at_ms: u64,
    pub flow: u32,
    pub stage: TcpStage,
    pub elapsed_ms: u64,
    pub outcome: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Diagnostics {
    pub at_ms: u64,
    pub bytes_tx: u64,
    pub bytes_rx: u64,
    pub udp_flow_credit: u64,
    pub udp_credit_bytes: u64,
    pub udp_credit_slots: u64,
    pub udp_credit_both: u64,
    pub udp_tx_attempts: u64,
    pub udp_received: u64,
    pub udp_window_bytes: usize,
    pub udp_window_slots: usize,
    pub udp_credit_return: Delay,
    pub udp_receive_hold: Delay,
    pub udp_credit_queue: Delay,
    pub udp_credit_write: Delay,
    pub receive_slot_rejections: u64,
    pub udp_byte_budget: u64,
    pub udp_slots: u64,
    pub udp_dns: u64,
    pub udp_send: u64,
    pub tx_queue: Delay,
    pub rx_hold: Delay,
    pub write_batch: Delay,
    pub tcp_errors: u64,
    pub tcp_slow: u64,
    pub tcp_cancelled: u64,
    pub tcp_probes: u64,
    pub tcp_probe_errors: u64,
    pub last_tcp: Option<TcpEvent>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Delay {
    pub samples: u64,
    pub max_ms: u64,
    pub over_100_ms: u64,
}

#[derive(Default)]
pub(crate) struct Timing {
    samples: AtomicU64,
    max_ms: AtomicU64,
    over_100_ms: AtomicU64,
}

impl Timing {
    pub(crate) fn observe(&self, duration: Duration) {
        let ms = millis(duration);
        self.samples.fetch_add(1, Relaxed);
        self.max_ms.fetch_max(ms, Relaxed);
        if ms >= 100 {
            self.over_100_ms.fetch_add(1, Relaxed);
        }
    }

    fn snapshot(&self) -> Delay {
        Delay {
            samples: self.samples.load(Relaxed),
            max_ms: self.max_ms.load(Relaxed),
            over_100_ms: self.over_100_ms.load(Relaxed),
        }
    }
}

#[derive(Default)]
pub(crate) struct Metrics {
    pub(crate) udp_flow_credit: AtomicU64,
    pub(crate) udp_credit_bytes: AtomicU64,
    pub(crate) udp_credit_slots: AtomicU64,
    pub(crate) udp_credit_both: AtomicU64,
    pub(crate) udp_tx_attempts: AtomicU64,
    pub(crate) udp_received: AtomicU64,
    pub(crate) udp_credit_return: Timing,
    pub(crate) udp_receive_hold: Timing,
    pub(crate) udp_credit_queue: Timing,
    pub(crate) udp_credit_write: Timing,
    pub(crate) receive_slot_rejections: AtomicU64,
    pub(crate) udp_byte_budget: AtomicU64,
    pub(crate) udp_slots: AtomicU64,
    pub(crate) udp_dns: AtomicU64,
    pub(crate) udp_send: AtomicU64,
    pub(crate) tx_queue: Timing,
    pub(crate) rx_hold: Timing,
    pub(crate) write_batch: Timing,
    tcp: Mutex<(u64, u64, Option<TcpEvent>)>,
    tcp_cancelled: AtomicU64,
    tcp_probes: AtomicU64,
    tcp_probe_errors: AtomicU64,
}

pub fn is_flow_cancellation(error: &str) -> bool {
    matches!(
        error,
        "Flow cancelled"
            | "Destination setup cancelled"
            | "Flow closed before destination acceptance"
    )
}

impl Metrics {
    pub(crate) fn record_tcp(
        &self,
        flow: u32,
        stage: TcpStage,
        elapsed: Duration,
        result: Result<(), &str>,
        probe: bool,
    ) {
        if probe {
            if matches!(stage, TcpStage::Open | TcpStage::Connect)
                || matches!(stage, TcpStage::Dns) && result.is_err()
            {
                self.tcp_probes.fetch_add(1, Relaxed);
                if result.is_err() {
                    self.tcp_probe_errors.fetch_add(1, Relaxed);
                }
            }
            return;
        }
        if result.is_err_and(is_flow_cancellation) {
            self.tcp_cancelled.fetch_add(1, Relaxed);
            return;
        }
        if result.is_ok() && elapsed < Duration::from_millis(500) {
            return;
        }
        let mut tcp = self.tcp.lock().unwrap();
        let outcome = match result {
            Ok(()) => {
                tcp.1 += 1;
                "slow".into()
            }
            Err(error) => {
                tcp.0 += 1;
                error
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(120)
                    .collect()
            }
        };
        tcp.2 = Some(TcpEvent {
            at_ms: unix_millis(),
            flow,
            stage,
            elapsed_ms: millis(elapsed),
            outcome,
        });
    }

    pub(crate) fn snapshot(&self, bytes_tx: u64, bytes_rx: u64) -> Diagnostics {
        let tcp = self.tcp.lock().unwrap();
        Diagnostics {
            at_ms: unix_millis(),
            bytes_tx,
            bytes_rx,
            udp_flow_credit: self.udp_flow_credit.load(Relaxed),
            udp_credit_bytes: self.udp_credit_bytes.load(Relaxed),
            udp_credit_slots: self.udp_credit_slots.load(Relaxed),
            udp_credit_both: self.udp_credit_both.load(Relaxed),
            udp_tx_attempts: self.udp_tx_attempts.load(Relaxed),
            udp_received: self.udp_received.load(Relaxed),
            udp_window_bytes: 0,
            udp_window_slots: 0,
            udp_credit_return: self.udp_credit_return.snapshot(),
            udp_receive_hold: self.udp_receive_hold.snapshot(),
            udp_credit_queue: self.udp_credit_queue.snapshot(),
            udp_credit_write: self.udp_credit_write.snapshot(),
            receive_slot_rejections: self.receive_slot_rejections.load(Relaxed),
            udp_byte_budget: self.udp_byte_budget.load(Relaxed),
            udp_slots: self.udp_slots.load(Relaxed),
            udp_dns: self.udp_dns.load(Relaxed),
            udp_send: self.udp_send.load(Relaxed),
            tx_queue: self.tx_queue.snapshot(),
            rx_hold: self.rx_hold.snapshot(),
            write_batch: self.write_batch.snapshot(),
            tcp_errors: tcp.0,
            tcp_slow: tcp.1,
            tcp_cancelled: self.tcp_cancelled.load(Relaxed),
            tcp_probes: self.tcp_probes.load(Relaxed),
            tcp_probe_errors: self.tcp_probe_errors.load(Relaxed),
            last_tcp: tcp.2.clone(),
        }
    }
}

pub(crate) const MARKER: &[u8] = b"\xffSTD1";
pub(crate) const MAX_BYTES: usize = 4096;

#[derive(Serialize, Deserialize)]
pub(crate) struct PeerReport {
    pub(crate) metrics: Diagnostics,
    pub(crate) last_udp_error: String,
}
