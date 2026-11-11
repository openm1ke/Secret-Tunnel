//! Sender-side growth of a TCP flow inside a peer receive window larger than
//! the 64 KiB baseline.
//!
//! One flow carries at most `window / RTT`, so the baseline caps a connection at
//! 10 Mbit/s on a 50 ms path. Using a larger window unconditionally is worse on a
//! slow link: sixteen flows would queue megabytes ahead of the first voice
//! datagrams before the interactive TCP budget engages. A flow therefore starts at
//! the baseline and earns more only while its credit returns about as fast as the
//! path allows; a standing queue stops the growth and a long one halves the limit.
//! The worst case is the fixed baseline every STMUX003 peer already has.
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

/// Limit a flow starts from, returns to after idling, and never goes below.
pub(super) const FLOOR: usize = crate::BASE_TCP_WINDOW;
/// A resumed flow re-earns its limit: the path may have filled meanwhile.
pub(super) const IDLE_RESET: Duration = Duration::from_secs(1);

/// Lowest credit round trip seen on this transport.
///
/// It is never aged. One SSH/TCP connection keeps one network path, and any
/// estimate that follows recent samples drifts upwards on a saturated link until
/// a standing queue looks normal. If the path really becomes slower, flows stay
/// at the floor until the session reconnects, which is the pre-elastic behaviour.
#[derive(Default)]
pub(super) struct Path(Mutex<Option<Duration>>);

impl Path {
    pub(super) fn observe(&self, sample: Duration) -> Duration {
        let mut lowest = self.0.lock().unwrap();
        let base = lowest.map_or(sample, |known| known.min(sample));
        *lowest = Some(base);
        base
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Grow,
    Hold,
    Shrink,
}

/// Classify one credit round trip against the uncongested path.
pub(super) fn step(base: Duration, sample: Duration) -> Step {
    if sample <= base + (base / 2).max(Duration::from_millis(25)) {
        Step::Grow
    } else if sample >= base * 2 + Duration::from_millis(100) {
        Step::Shrink
    } else {
        Step::Hold
    }
}

/// Elastic state of one flow's send window; guarded by the window's mutex.
pub(super) struct Sending {
    /// Bytes this flow may have unacknowledged, within the peer's window.
    pub(super) limit: usize,
    /// Unacknowledged packets, oldest first; credit returns in the same order.
    sent: std::collections::VecDeque<(Instant, usize)>,
    reduced_at: Option<Instant>,
    idle_since: Option<Instant>,
}

impl Sending {
    pub(super) fn new() -> Self {
        Self {
            limit: FLOOR,
            sent: Default::default(),
            reduced_at: None,
            idle_since: None,
        }
    }

    /// Whether `bytes` more may be sent with `in_flight` bytes unacknowledged.
    pub(super) fn admits(&mut self, in_flight: usize, bytes: usize, now: Instant) -> bool {
        if self
            .idle_since
            .is_some_and(|since| now.duration_since(since) >= IDLE_RESET)
        {
            self.limit = FLOOR;
        }
        in_flight + bytes <= self.limit
    }

    pub(super) fn sent(&mut self, bytes: usize, now: Instant) {
        self.idle_since = None;
        self.sent.push_back((now, bytes));
    }

    /// A reservation that was never written to the transport.
    pub(super) fn unsent(&mut self, bytes: usize, now: Instant) {
        if let Some(index) = self.sent.iter().rposition(|(_, size)| *size == bytes) {
            self.sent.remove(index);
        }
        if self.sent.is_empty() {
            self.idle_since.get_or_insert(now);
        }
    }

    /// Peer credit for `slots` packets carrying `bytes`; `window` is its maximum.
    pub(super) fn credited(
        &mut self,
        bytes: usize,
        slots: usize,
        window: usize,
        path: &Path,
        now: Instant,
    ) {
        let mut oldest = None;
        for _ in 0..slots {
            if let Some((at, _)) = self.sent.pop_front() {
                oldest.get_or_insert(at);
            }
        }
        if self.sent.is_empty() {
            self.idle_since = Some(now);
        }
        let Some(sent_at) = oldest else { return };
        let sample = now.duration_since(sent_at);
        match step(path.observe(sample), sample) {
            // One window of fast credit doubles the limit, as in TCP slow start.
            Step::Grow => self.limit = (self.limit + bytes).min(window),
            Step::Hold => {}
            // Packets sent before a reduction still report the old queue.
            Step::Shrink => {
                if self
                    .reduced_at
                    .is_none_or(|at| now.duration_since(at) >= sample)
                {
                    self.limit = (self.limit / 2).max(FLOOR);
                    self.reduced_at = Some(now);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK: usize = 16384;
    const WINDOW: usize = 256 * 1024;
    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn thresholds_scale_with_the_path_and_keep_a_hold_band() {
        // 50 ms path: up to 75 ms is fast, from 200 ms a queue is standing.
        assert_eq!(step(ms(50), ms(50)), Step::Grow);
        assert_eq!(step(ms(50), ms(75)), Step::Grow);
        assert_eq!(step(ms(50), ms(76)), Step::Hold);
        assert_eq!(step(ms(50), ms(199)), Step::Hold);
        assert_eq!(step(ms(50), ms(200)), Step::Shrink);
        // 300 ms path: the same absolute delays are still fast.
        assert_eq!(step(ms(300), ms(450)), Step::Grow);
        assert_eq!(step(ms(300), ms(451)), Step::Hold);
        assert_eq!(step(ms(300), ms(700)), Step::Shrink);
        // A very short path keeps an absolute 25 ms allowance for scheduling noise.
        assert_eq!(step(ms(2), ms(27)), Step::Grow);
    }

    #[test]
    fn the_path_base_is_the_lowest_sample_and_never_drifts_up() {
        let path = Path::default();
        assert_eq!(path.observe(ms(80)), ms(80));
        assert_eq!(path.observe(ms(50)), ms(50));
        for _ in 0..1000 {
            assert_eq!(path.observe(ms(900)), ms(50));
        }
    }

    /// Send one full limit, then credit every packet after `round_trip`.
    fn round(sending: &mut Sending, path: &Path, start: Instant, round_trip: Duration) {
        let packets = sending.limit / CHUNK;
        for index in 0..packets {
            assert!(sending.admits(index * CHUNK, CHUNK, start));
            sending.sent(CHUNK, start);
        }
        assert!(!sending.admits(packets * CHUNK, CHUNK, start));
        for _ in 0..packets {
            sending.credited(CHUNK, 1, WINDOW, path, start + round_trip);
        }
    }

    #[test]
    fn fast_credit_doubles_the_limit_up_to_the_peer_window() {
        let (mut sending, path, start) = (Sending::new(), Path::default(), Instant::now());
        let mut limits = vec![sending.limit];
        for index in 0..4 {
            round(&mut sending, &path, start + ms(50) * index, ms(50));
            limits.push(sending.limit);
        }
        assert_eq!(limits, [65536, 131072, 262144, 262144, 262144]);
    }

    #[test]
    fn a_standing_queue_stops_growth_and_a_long_one_halves_once_per_round_trip() {
        let (mut sending, path, start) = (Sending::new(), Path::default(), Instant::now());
        round(&mut sending, &path, start, ms(50));
        round(&mut sending, &path, start + ms(50), ms(50));
        assert_eq!(sending.limit, WINDOW);
        // 120 ms on a 50 ms path: neither fast nor a long queue.
        round(&mut sending, &path, start + ms(100), ms(120));
        assert_eq!(sending.limit, WINDOW);
        // 400 ms: sixteen late credits of one round reduce the limit once.
        round(&mut sending, &path, start + ms(300), ms(400));
        assert_eq!(sending.limit, WINDOW / 2);
        round(&mut sending, &path, start + ms(800), ms(400));
        assert_eq!(sending.limit, FLOOR);
        // Never below what every peer already allows.
        round(&mut sending, &path, start + ms(1300), ms(400));
        assert_eq!(sending.limit, FLOOR);
    }

    #[test]
    fn a_slow_first_round_on_a_slow_link_does_not_earn_more() {
        // 2 Mbit/s, 300 ms: sixteen flows share the link, so only the first
        // packets of the session return near the path delay.
        let (path, start) = (Path::default(), Instant::now());
        assert_eq!(path.observe(ms(364)), ms(364));
        let mut sending = Sending::new();
        for index in 0..4 {
            sending.sent(CHUNK, start);
            let _ = index;
        }
        for delay in [1_400, 2_400, 3_400, 4_400] {
            sending.credited(CHUNK, 1, WINDOW, &path, start + ms(delay));
        }
        assert_eq!(sending.limit, FLOOR);
    }

    #[test]
    fn an_idle_flow_restarts_from_the_floor_and_unsent_reservations_do_not_count() {
        let (mut sending, path, start) = (Sending::new(), Path::default(), Instant::now());
        round(&mut sending, &path, start, ms(50));
        round(&mut sending, &path, start + ms(50), ms(50));
        assert_eq!(sending.limit, WINDOW);
        // Still warm shortly after the last credit.
        assert!(sending.admits(0, WINDOW, start + ms(600)));
        assert!(!sending.admits(0, WINDOW, start + ms(100) + IDLE_RESET));
        assert_eq!(sending.limit, FLOOR);
        // A cancelled reservation leaves no packet to time.
        let later = start + ms(5_000);
        sending.sent(CHUNK, later);
        sending.unsent(CHUNK, later);
        sending.credited(CHUNK, 1, WINDOW, &path, later + ms(10_000));
        assert_eq!(sending.limit, FLOOR);
    }
}
