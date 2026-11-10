//! Bounded end-to-end probes. RTT includes waiting in mux/SSH/TCP queues.
use crate::{diagnostics::unix_millis, wire::Frame, Inner, Kind};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

const ACTIVE_INTERVAL: Duration = Duration::from_secs(5);
const IDLE_INTERVAL: Duration = Duration::from_secs(15);
const RECENT: usize = 64;
const MIN_TIMEOUT_MS: u64 = 60_000;
const MAX_TIMEOUT_MS: u64 = 180_000;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HeartbeatSnapshot {
    pub active: bool,
    pub probes: u64,
    pub acknowledged: u64,
    pub unexpected_pongs: u64,
    pub timeouts: u64,
    pub last_ack_at_ms: Option<u64>,
    pub last_rtt_ms: Option<u64>,
    pub smoothed_rtt_ms: Option<u64>,
    pub jitter_ms: Option<u64>,
    pub min_rtt_ms: Option<u64>,
    pub p95_rtt_ms: Option<u64>,
    pub recent_samples: usize,
    pub pending_ms: Option<u64>,
    pub timeout_ms: u64,
    pub interval_ms: u64,
}

#[derive(Default)]
pub(super) struct Heartbeat {
    active: bool,
    sequence: u64,
    pending: Option<(u64, Instant)>,
    last_probe: Option<Instant>,
    acknowledged: u64,
    unexpected_pongs: u64,
    timeouts: u64,
    last_ack_at_ms: Option<u64>,
    last_us: Option<u64>,
    smoothed_us: Option<u64>,
    jitter_us: u64,
    recent_us: VecDeque<u64>,
    payload_bytes: (u64, u64),
    payload_at: Option<Instant>,
}

pub(super) enum Action {
    Wait,
    Probe(u64),
    Expired,
}

impl Heartbeat {
    pub(super) fn new(initiator: bool) -> Self {
        Self {
            active: initiator,
            ..Default::default()
        }
    }
    pub(super) fn activate(&mut self) {
        self.active = true;
    }
    pub(super) fn payload_progress(&mut self, tx: u64, rx: u64, now: Instant) {
        if self.payload_bytes != (tx, rx) {
            self.payload_bytes = (tx, rx);
            self.payload_at = Some(now);
        }
    }
    fn interval(&self, now: Instant) -> Duration {
        if self
            .payload_at
            .is_some_and(|at| now.duration_since(at) <= IDLE_INTERVAL)
        {
            ACTIVE_INTERVAL
        } else {
            IDLE_INTERVAL
        }
    }
    #[cfg(test)]
    pub(super) fn expire_for_test(&mut self) {
        if let Some((_, sent)) = &mut self.pending {
            *sent = Instant::now() - Duration::from_secs(181);
        }
    }
    fn timeout_ms(&self) -> u64 {
        // Be conservative on mobile handovers and long chains. Only observed
        // matching acknowledgements may lengthen the deadline.
        self.smoothed_us
            .unwrap_or(0)
            .saturating_add(self.jitter_us.saturating_mul(4))
            .saturating_mul(8)
            .div_ceil(1000)
            .clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS)
    }
    pub(super) fn step(&mut self, now: Instant) -> Action {
        if !self.active {
            return Action::Wait;
        }
        if let Some((_, sent)) = self.pending {
            if now.duration_since(sent) >= Duration::from_millis(self.timeout_ms()) {
                self.timeouts += 1;
                return Action::Expired;
            }
            return Action::Wait;
        }
        if self
            .last_probe
            .is_some_and(|last| now.duration_since(last) < self.interval(now))
        {
            return Action::Wait;
        }
        self.sequence += 1;
        self.pending = Some((self.sequence, now));
        self.last_probe = Some(now);
        Action::Probe(self.sequence)
    }
    pub(super) fn acknowledge(&mut self, nonce: u64, now: Instant) {
        let Some((expected, sent)) = self.pending else {
            self.unexpected_pongs += 1;
            return;
        };
        if expected != nonce {
            self.unexpected_pongs += 1;
            return;
        }
        self.pending = None;
        let us = now.duration_since(sent).as_micros().min(u64::MAX as u128) as u64;
        self.acknowledged += 1;
        self.last_ack_at_ms = Some(unix_millis());
        self.last_us = Some(us);
        if let Some(previous) = self.smoothed_us {
            self.jitter_us = (self.jitter_us.saturating_mul(3) + previous.abs_diff(us)) / 4;
            self.smoothed_us = Some((previous.saturating_mul(7) + us) / 8);
        } else {
            self.smoothed_us = Some(us);
            self.jitter_us = us / 2;
        }
        if self.recent_us.len() == RECENT {
            self.recent_us.pop_front();
        }
        self.recent_us.push_back(us);
    }
    pub(super) fn snapshot(&self, now: Instant) -> HeartbeatSnapshot {
        let mut recent: Vec<_> = self.recent_us.iter().copied().collect();
        recent.sort_unstable();
        HeartbeatSnapshot {
            active: self.active,
            probes: self.sequence,
            acknowledged: self.acknowledged,
            unexpected_pongs: self.unexpected_pongs,
            timeouts: self.timeouts,
            last_ack_at_ms: self.last_ack_at_ms,
            last_rtt_ms: self.last_us.map(|v| v / 1000),
            smoothed_rtt_ms: self.smoothed_us.map(|v| v / 1000),
            jitter_ms: self.smoothed_us.map(|_| self.jitter_us / 1000),
            min_rtt_ms: recent.first().map(|v| v / 1000),
            p95_rtt_ms: (!recent.is_empty())
                .then(|| recent[(recent.len() * 95).div_ceil(100) - 1] / 1000),
            recent_samples: recent.len(),
            pending_ms: self
                .pending
                .map(|(_, sent)| now.duration_since(sent).as_millis().min(u64::MAX as u128) as u64),
            timeout_ms: self.timeout_ms(),
            interval_ms: self.interval(now).as_millis() as u64,
        }
    }
}

pub(super) async fn run(inner: Arc<Inner>) {
    let mut closed = inner.closed.subscribe();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *closed.borrow() {
            return;
        }
        tokio::select! { _ = closed.changed() => return, _ = tick.tick() => {} }
        let now = Instant::now();
        let action = {
            let mut heartbeat = inner.heartbeat.lock().unwrap();
            heartbeat.payload_progress(
                inner
                    .counters
                    .bytes_tx
                    .load(std::sync::atomic::Ordering::Relaxed),
                inner
                    .counters
                    .bytes_rx
                    .load(std::sync::atomic::Ordering::Relaxed),
                now,
            );
            heartbeat.step(now)
        };
        match action {
            Action::Wait => {}
            Action::Expired => {
                inner.fail("End-to-end heartbeat timed out");
                return;
            }
            Action::Probe(nonce) => {
                if inner
                    .control(Frame {
                        kind: Kind::Ping,
                        id: 0,
                        data: nonce.to_be_bytes().to_vec(),
                    })
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_matching_pongs_complete_a_probe_and_history_is_bounded() {
        let mut heartbeat = Heartbeat::new(true);
        let start = Instant::now();
        for n in 0..80u64 {
            let sent = start + Duration::from_secs(n * 5);
            heartbeat.payload_progress(n + 1, 0, sent);
            let Action::Probe(nonce) = heartbeat.step(sent) else {
                panic!("missing probe");
            };
            heartbeat.acknowledge(nonce + 1, sent + Duration::from_millis(10));
            assert!(heartbeat.pending.is_some());
            heartbeat.acknowledge(nonce, sent + Duration::from_millis(100));
        }
        let snapshot = heartbeat.snapshot(start + Duration::from_secs(400));
        assert_eq!(snapshot.acknowledged, 80);
        assert_eq!(snapshot.unexpected_pongs, 80);
        assert_eq!(snapshot.recent_samples, RECENT);
        assert_eq!(snapshot.p95_rtt_ms, Some(100));
        assert_eq!(snapshot.smoothed_rtt_ms, Some(100));
        assert_eq!(snapshot.timeout_ms, MIN_TIMEOUT_MS);
    }
    #[test]
    fn peer_activity_does_not_hide_a_missing_ack_and_timeout_is_bounded() {
        let mut heartbeat = Heartbeat::new(false);
        let now = Instant::now();
        assert!(matches!(heartbeat.step(now), Action::Wait));
        heartbeat.activate();
        assert!(matches!(heartbeat.step(now), Action::Probe(_)));
        heartbeat.activate();
        heartbeat.acknowledge(999, now + Duration::from_secs(59));
        assert!(matches!(
            heartbeat.step(now + Duration::from_secs(60)),
            Action::Expired
        ));
        heartbeat.smoothed_us = Some(120_000_000);
        assert_eq!(heartbeat.timeout_ms(), MAX_TIMEOUT_MS);
    }
    #[test]
    fn idle_probes_slow_down_and_control_traffic_does_not_keep_them_busy() {
        let mut heartbeat = Heartbeat::new(true);
        let now = Instant::now();
        let Action::Probe(nonce) = heartbeat.step(now) else {
            panic!("no initial probe");
        };
        heartbeat.acknowledge(nonce, now + Duration::from_millis(100));
        assert!(matches!(
            heartbeat.step(now + Duration::from_secs(5)),
            Action::Wait
        ));
        heartbeat.payload_progress(100, 0, now + Duration::from_secs(6));
        assert!(matches!(
            heartbeat.step(now + Duration::from_secs(6)),
            Action::Probe(_)
        ));
        assert_eq!(
            heartbeat.snapshot(now + Duration::from_secs(7)).interval_ms,
            5000
        );
        heartbeat.activate();
        heartbeat.payload_progress(100, 0, now + Duration::from_secs(30));
        assert_eq!(
            heartbeat
                .snapshot(now + Duration::from_secs(30))
                .interval_ms,
            15000
        );
    }
}
