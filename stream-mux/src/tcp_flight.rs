//! Session-wide TCP credit budget: bytes sent and not yet received by the peer.
//! Permits return on peer wire receipt, not on acceptance by an SSH writer.
//! Per-flow application-consumption credit remains a separate constraint.
//!
//! Without a call the budget keeps the path free of a standing queue. A fixed
//! 8 MiB let sixteen downloads park 0.9 s of data in the agent and the SSH path
//! on an 80 Mbit/s link, and a request on any other connection waited behind it
//! (speed test loaded ping 109-188 ms on a 52 ms path). It starts at 1 MiB and
//! doubles per round trip until `Delivered` comes back late, then drops to what
//! the path carried without a queue, so the queue drains; it then grows by a
//! quarter per round trip. The uncongested delay here is the lowest of the last
//! 10-20 s, so a path that really became slower is not throttled for the rest
//! of the session; the regular drains keep that estimate honest.
//!
//! A budget that is kept full makes its waiters queue, and permits are granted
//! in order: a ping reply on an idle connection waited behind sixteen downloads
//! and the loaded ping got worse as the budget got tighter. A flow with almost
//! nothing in flight therefore draws on a small shared reserve first.
//!
//! During a call, while a voice-like UDP flow is active, the budget is tighter.
//! "Voice-like" is a run of short datagrams on one flow. Counting them per
//! session let six DNS lookups (twelve short datagrams on a dozen flows) cap all
//! TCP for the whole hold time on every page load.
//!
//! While active, the budget follows queueing delay instead of staying at the
//! configured size. It starts there, so a call is protected at once as before,
//! grows by about one TCP chunk per round trip while `Delivered` returns near
//! the uncongested path delay and halves when a queue builds up, never below
//! the configured size. Growth is additive because the delay signal arrives a
//! round trip late: doubling overran a 2 Mbit/s, 300 ms path before the queue
//! showed and doubled voice p95. A fixed 96 KiB
//! capped every download at 96 KiB per round trip (14.8 Mbit/s at 53 ms) and
//! held small TCP messages, such as an app's own pings, behind it; apps that
//! merely look like a call (reconnect bursts, latency probes) triggered that.
use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MAX: usize = 8 * 1024 * 1024;
/// Budget without a call: where it starts, and the least it shrinks to.
const BULK_START: usize = 1024 * 1024;
const BULK_FLOOR: usize = 256 * 1024;
/// Queueing delay that ends growth without a call; an eighth of the path
/// delay on long paths, so their jitter does not count as a queue.
const BULK_QUEUE: Duration = Duration::from_millis(10);
/// A flow with at most this much in flight, the new bytes included, is light.
const LIGHT_FLOW: usize = 2 * crate::TCP_CHUNK;
/// Reserve shared by light flows, beside the budget.
const LIGHT_RESERVE: usize = 512 * 1024;
/// Age of the uncongested-delay estimate without a call.
const BASE_WINDOW: Duration = Duration::from_secs(10);
const DETECTION: Duration = Duration::from_millis(500);
const HOLD: Duration = Duration::from_secs(15);
/// Short datagrams one flow needs within `DETECTION` to activate the budget.
const SHORT_RUN: usize = 12;
/// An encoded datagram up to this size counts as short.
pub(super) const SHORT_DATAGRAM: usize = 512;

/// Bytes one flow may carry within `DETECTION` and still look like a call.
///
/// A QUIC download sends short ACKs too, enough to reach `SHORT_RUN`, but they
/// travel with megabytes of full-size datagrams. A call stays far below this:
/// voice is tens of kbit/s and a video call a few Mbit/s, while 384 KiB per
/// 500 ms is 6.3 Mbit/s.
const BULK_BYTES: usize = 384 * 1024;
/// Full-size datagrams allowed per short one within a window. A video call
/// sends about three video packets per voice packet; a QUIC download sends
/// eight or more data packets per ACK, at any rate.
const BULK_RATIO: usize = 4;

/// One `Delivered` round trip against the uncongested path while active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delay {
    /// Essentially no queue: the budget may grow.
    Clear,
    Hold,
    /// A queue that would delay voice: halve the budget.
    Queued,
}
fn delay(base: Duration, sample: Duration) -> Delay {
    if sample <= base + (base / 8).max(Duration::from_millis(10)) {
        Delay::Clear
    } else if sample >= base + (base / 4).max(Duration::from_millis(30)) {
        Delay::Queued
    } else {
        Delay::Hold
    }
}

/// Datagrams sent and received on one flow within the current window.
pub(super) struct ShortRun {
    since: Instant,
    count: usize,
    large: usize,
    bytes: usize,
    /// Bytes of the previous window, so a sustained download stays bulk while
    /// a new window has only begun.
    previous_bytes: usize,
}
impl ShortRun {
    pub(super) fn new(now: Instant) -> Self {
        Self { since: now, count: 0, large: 0, bytes: 0, previous_bytes: 0 }
    }
    /// Count one datagram of `len` bytes; true while the flow keeps a run of
    /// short datagrams at a call-like volume.
    pub(super) fn note(&mut self, now: Instant, len: usize) -> bool {
        let mut elapsed = now.duration_since(self.since);
        if elapsed > DETECTION {
            self.previous_bytes = if elapsed > DETECTION * 2 { 0 } else { self.bytes };
            self.since = now;
            self.count = 0;
            self.large = 0;
            self.bytes = 0;
            elapsed = Duration::ZERO;
        }
        self.bytes += len;
        if len > SHORT_DATAGRAM {
            self.large += 1;
            return false;
        }
        self.count += 1;
        // Compare rates, not totals: a download reaches twelve ACKs within tens
        // of milliseconds of a window, long before it accumulates its volume.
        let span = elapsed.max(DETECTION / 5).as_micros();
        let allowed = BULK_BYTES as u128 * span / DETECTION.as_micros();
        self.count >= SHORT_RUN
            && self.large <= self.count * BULK_RATIO
            && self.previous_bytes <= BULK_BYTES
            && self.bytes as u128 <= allowed
    }
}

/// Port of a datagram in the SOCKS5 UDP form STMUX003 flows carry: destination
/// when sent by the client, source when relayed back by the agent.
pub(super) fn datagram_port(data: &[u8]) -> Option<u16> {
    if data.get(..3) != Some(&[0, 0, 0]) {
        return None;
    }
    let offset = match *data.get(3)? {
        1 => 8,
        4 => 20,
        3 => 5 + usize::from(*data.get(4)?),
        _ => return None,
    };
    Some(u16::from_be_bytes([*data.get(offset)?, *data.get(offset + 1)?]))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TcpFlightSnapshot {
    pub interactive: bool,
    pub configured_bytes: Option<usize>,
    pub effective_bytes: usize,
    pub held_bytes: usize,
    pub flows_with_credit: usize,
    /// Uncongested `Delivered` round trip the budget follows without a call.
    pub path_delay_ms: Option<u64>,
}
/// Lowest round trip of the last one to two `BASE_WINDOW`s.
#[derive(Default)]
struct Recent {
    since: Option<Instant>,
    current: Option<Duration>,
    previous: Option<Duration>,
}
impl Recent {
    fn observe(&mut self, sample: Duration, now: Instant) -> Duration {
        let since = *self.since.get_or_insert(now);
        let age = now.saturating_duration_since(since);
        if age >= BASE_WINDOW {
            self.previous = if age >= BASE_WINDOW * 2 { None } else { self.current };
            self.current = None;
            self.since = Some(now);
        }
        let current = self.current.map_or(sample, |known| known.min(sample));
        self.current = Some(current);
        self.previous.map_or(current, |previous| previous.min(current))
    }
}
struct Packet {
    payload: usize,
    permit: Permit,
    sent_at: Instant,
}
/// Charge for one packet, from the budget or from the light reserve.
struct Permit {
    permit: OwnedSemaphorePermit,
    light: bool,
}
impl Permit {
    fn bytes(&self) -> usize {
        self.permit.num_permits()
    }
}
struct State {
    limit: usize,
    debt: usize,
    held: HashMap<u32, VecDeque<Packet>>,
    active_until: Option<Instant>,
    /// Lowest `Delivered` round trip of the session; see `elastic::Path` for
    /// why it never ages.
    base: Option<Duration>,
    reduced_at: Option<Instant>,
    /// Uncongested delay for the budget without a call.
    recent: Recent,
    /// Doubling until the first queue, and again after a call.
    slow_start: bool,
}
pub(super) struct TcpFlight {
    available: Arc<Semaphore>,
    light: Arc<Semaphore>,
    state: Mutex<State>,
    normal: usize,
    target: Option<usize>,
}
pub(super) struct Reservation {
    budget: Arc<TcpFlight>,
    permit: Option<Permit>,
    payload: usize,
}
impl TcpFlight {
    pub(super) fn new(normal: usize, target: Option<usize>) -> Arc<Self> {
        Self::with_reserve(normal, target, LIGHT_RESERVE)
    }
    fn with_reserve(normal: usize, target: Option<usize>, light: usize) -> Arc<Self> {
        let normal = normal.min(MAX);
        // Without a negotiated budget TCP is not charged here at all.
        let limit = if target.is_some() { BULK_START.min(normal) } else { normal };
        Arc::new(Self {
            available: Arc::new(Semaphore::new(limit)),
            light: Arc::new(Semaphore::new(light)),
            normal,
            target,
            state: Mutex::new(State {
                limit,
                debt: 0,
                held: HashMap::new(),
                active_until: None,
                base: None,
                reduced_at: None,
                recent: Recent::default(),
                slow_start: true,
            }),
        })
    }
    fn resize(&self, state: &mut State, limit: usize) {
        if limit < state.limit {
            let amount = state.limit - limit;
            state.debt += amount - self.available.forget_permits(amount);
        } else {
            let amount = limit - state.limit;
            let cancel = amount.min(state.debt);
            state.debt -= cancel;
            self.available.add_permits(amount - cancel);
        }
        state.limit = limit;
    }
    /// Whether a TCP budget was negotiated for this session at all.
    pub(super) fn enabled(&self) -> bool {
        self.target.is_some()
    }
    fn floor(&self) -> usize {
        self.target.unwrap_or(self.normal).min(self.normal)
    }
    /// A flow carries a voice-like run: cap TCP and (re)start the hold time.
    /// A run that continues keeps the budget the delay has earned meanwhile.
    pub(super) fn activate(&self, now: Instant) {
        if self.target.is_none() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if state.active_until.replace(now + HOLD).is_none() {
            state.reduced_at = None;
            self.resize(&mut state, self.floor());
        }
    }
    /// Adapt the budget to one `Delivered` round trip of `bytes`.
    fn follow_delay(&self, state: &mut State, sample: Duration, bytes: usize, now: Instant) {
        let base = state.base.map_or(sample, |known| known.min(sample));
        state.base = Some(base);
        let recent = state.recent.observe(sample, now);
        if self.target.is_none() {
            return;
        }
        if state.active_until.is_none() {
            self.follow_bulk(state, recent, sample, bytes, now);
            return;
        }
        match delay(base, sample) {
            // Additive: `bytes` of a full limit add one TCP chunk. Light
            // packets were not charged, so they report delay but earn nothing.
            Delay::Clear if bytes > 0 => {
                let growth = (bytes * crate::TCP_CHUNK / state.limit.max(1)).max(1024);
                let limit = (state.limit + growth).min(self.normal);
                self.resize(state, limit);
            }
            Delay::Clear | Delay::Hold => {}
            // Packets sent before a reduction still report the old queue.
            Delay::Queued => {
                if state.reduced_at.is_none_or(|at| now.duration_since(at) >= sample) {
                    let limit = (state.limit / 2).max(self.floor());
                    self.resize(state, limit);
                    state.reduced_at = Some(now);
                }
            }
        }
    }
    /// Without a call: grow while `Delivered` shows no queue, drain one.
    fn follow_bulk(&self, state: &mut State, base: Duration, sample: Duration, bytes: usize, now: Instant) {
        // Bytes in flight when this packet was acknowledged, itself included.
        let flight = (state.limit.saturating_sub(self.available.available_permits()) + bytes).min(state.limit);
        if sample < base + (base / 8).max(BULK_QUEUE) {
            // Only a budget that limits the sender earns more; an idle one
            // would otherwise reach 8 MiB and release it in one burst.
            if bytes > 0 && flight >= state.limit / 2 {
                let growth = if state.slow_start { bytes } else { bytes / 4 };
                let limit = (state.limit + growth.max(1024)).min(self.normal);
                self.resize(state, limit);
            }
        } else if state.reduced_at.is_none_or(|at| now.duration_since(at) >= sample) {
            // What the path carried in one uncongested round trip.
            let drained = (flight as u128 * base.as_nanos() / sample.as_nanos().max(1)) as usize;
            let limit = drained.clamp(BULK_FLOOR.min(self.normal), self.normal);
            self.resize(state, limit.min(state.limit));
            state.reduced_at = Some(now);
            state.slow_start = false;
        }
    }
    pub(super) fn tick(&self, now: Instant) {
        let mut state = self.state.lock().unwrap();
        if state.active_until.is_some_and(|until| now >= until) {
            // Keep the call's budget and earn the rest back by delay: the queue
            // a jump to the full budget built is what the call was spared from.
            state.active_until = None;
            state.slow_start = true;
            state.reduced_at = None;
        }
    }
    /// Charge `payload` bytes of flow `id`.
    pub(super) async fn reserve(self: &Arc<Self>, id: u32, payload: usize) -> Result<Reservation> {
        // Bound metadata too: a one-byte TCP frame still occupies one KiB of
        // the session budget until acknowledged or cancelled.
        let charge = payload.div_ceil(1024) * 1024;
        let light = {
            let state = self.state.lock().unwrap();
            let in_flight: usize = state.held.get(&id).map_or(0, |packets| packets.iter().map(|p| p.permit.bytes()).sum());
            in_flight + charge <= LIGHT_FLOW
        };
        if light {
            if let Ok(permit) = self.light.clone().try_acquire_many_owned(charge as u32) {
                return Ok(Reservation {
                    budget: self.clone(),
                    permit: Some(Permit { permit, light: true }),
                    payload,
                });
            }
        }
        let permit = self
            .available
            .clone()
            .acquire_many_owned(charge as u32)
            .await
            .map_err(|_| "TCP flight budget closed")?;
        let permit = Permit { permit, light: false };
        Ok(Reservation {
            budget: self.clone(),
            permit: Some(permit),
            payload,
        })
    }
    fn give_back(&self, state: &mut State, permit: Permit) {
        let Permit { mut permit, light } = permit;
        if light {
            return;
        }
        let remove = state.debt.min(permit.num_permits());
        if remove > 0 {
            permit.split(remove).unwrap().forget();
            state.debt -= remove;
        }
        drop(permit);
    }
    pub(super) fn acknowledge(&self, id: u32, bytes: usize, slots: usize) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let Some(packets) = state.held.get(&id) else {
            return Ok(());
        };
        if slots == 0
            || slots > packets.len()
            || packets.iter().take(slots).map(|p| p.payload).sum::<usize>() != bytes
        {
            return Err("Invalid session TCP credit".into());
        }
        let mut returned = Vec::with_capacity(slots);
        let packets = state.held.get_mut(&id).unwrap();
        let sent_at = packets.front().map(|packet| packet.sent_at);
        for _ in 0..slots {
            returned.push(packets.pop_front().unwrap().permit);
        }
        if packets.is_empty() {
            state.held.remove(&id);
        }
        let charged = returned.iter().filter(|permit| !permit.light).map(Permit::bytes).sum();
        for permit in returned {
            self.give_back(&mut state, permit);
        }
        // Light packets still report the path delay; they charge nothing.
        if let Some(sent_at) = sent_at {
            let now = Instant::now();
            self.follow_delay(&mut state, now.duration_since(sent_at), charged, now);
        }
        Ok(())
    }
    pub(super) fn retire(&self, id: u32, outbound_closed: &AtomicBool, cancelled: bool) {
        let mut state = self.state.lock().unwrap();
        outbound_closed.store(true, Ordering::Relaxed);
        if cancelled {
            self.clear_locked(&mut state, id);
        }
        // Graceful retirement keeps permits until late Delivered/Close. EOF
        // enqueued locally is not evidence that the peer received the bytes.
    }
    fn clear_locked(&self, state: &mut State, id: u32) {
        if let Some(packets) = state.held.remove(&id) {
            for packet in packets {
                self.give_back(state, packet.permit);
            }
        }
    }
    pub(super) fn clear(&self, id: u32) {
        let mut state = self.state.lock().unwrap();
        self.clear_locked(&mut state, id);
    }
    pub(super) fn close(&self) {
        self.available.close();
        self.light.close();
    }
    pub(super) fn snapshot(&self) -> TcpFlightSnapshot {
        let state = self.state.lock().unwrap();
        TcpFlightSnapshot {
            interactive: state.active_until.is_some(),
            configured_bytes: self.target,
            effective_bytes: state.limit,
            held_bytes: state
                .held
                .values()
                .flatten()
                .map(|p| p.permit.bytes())
                .sum(),
            flows_with_credit: state.held.len(),
            path_delay_ms: state.recent.current.map(|current| {
                state.recent.previous.map_or(current, |previous| previous.min(current)).as_millis() as u64
            }),
        }
    }
}
impl Reservation {
    pub(super) fn commit(
        mut self,
        id: u32,
        closed: &AtomicBool,
        send: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let mut state = self.budget.state.lock().unwrap();
        if closed.load(Ordering::Relaxed) {
            return Err("Flow closed".into());
        }
        state.held.entry(id).or_default().push_back(Packet {
            payload: self.payload,
            permit: self.permit.take().unwrap(),
            sent_at: Instant::now(),
        });
        let result = send();
        if result.is_err() {
            let packets = state.held.get_mut(&id).unwrap();
            self.permit = Some(packets.pop_back().unwrap().permit);
            if packets.is_empty() {
                state.held.remove(&id);
            }
        }
        result
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(permit) = self.permit.take() {
            let mut state = self.budget.state.lock().unwrap();
            self.budget.give_back(&mut state, permit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// One flow's short datagrams, as `Inner::note_datagram` feeds them.
    fn note_short(budget: &TcpFlight, run: &mut ShortRun, now: Instant) {
        if run.note(now, 160) {
            budget.activate(now);
        }
    }
    fn activate(budget: &TcpFlight, now: Instant) {
        let mut run = ShortRun::new(now);
        for _ in 0..SHORT_RUN {
            note_short(budget, &mut run, now);
        }
    }
    #[test]
    fn isolated_datagrams_do_not_activate_and_short_gaps_do_not_release_bulk() {
        let budget = TcpFlight::new(MAX, Some(65536));
        let now = Instant::now();
        let mut run = ShortRun::new(now);
        for i in 0..20 {
            note_short(&budget, &mut run, now + Duration::from_secs(i));
        }
        assert!(!budget.snapshot().interactive);
        let now = now + Duration::from_secs(21);
        activate(&budget, now);
        budget.tick(now + Duration::from_secs(3));
        assert!(budget.snapshot().interactive);
        assert_eq!(budget.snapshot().effective_bytes, 65536);
        budget.tick(now + HOLD);
        assert!(!budget.snapshot().interactive);
        // The call's budget stays; delivery delay earns the rest back.
        assert_eq!(budget.snapshot().effective_bytes, 65536);
    }
    #[test]
    fn a_run_is_counted_per_flow_and_needs_twelve_within_the_window() {
        let budget = TcpFlight::new(MAX, Some(65536));
        let now = Instant::now();
        // Six lookups: twelve short datagrams, but two on each of six flows.
        for _ in 0..6 {
            let mut run = ShortRun::new(now);
            note_short(&budget, &mut run, now);
            note_short(&budget, &mut run, now + Duration::from_millis(40));
        }
        assert!(!budget.snapshot().interactive);
        // Eleven on one flow are not enough; the twelfth is, within 500 ms.
        let mut run = ShortRun::new(now);
        for index in 0..11 {
            note_short(&budget, &mut run, now + Duration::from_millis(20 * index));
        }
        assert!(!budget.snapshot().interactive);
        note_short(&budget, &mut run, now + Duration::from_millis(240));
        assert!(budget.snapshot().interactive);
        // Without a negotiated budget nothing activates.
        let disabled = TcpFlight::new(MAX, None);
        assert!(!disabled.enabled());
        activate(&disabled, now);
        assert!(!disabled.snapshot().interactive);
    }
    #[test]
    fn short_acks_of_a_bulk_download_do_not_look_like_a_call() {
        let budget = TcpFlight::new(MAX, Some(65536));
        let now = Instant::now();
        // QUIC with an ACK every eighth 1200-byte datagram, slow to fast.
        for mbit in [1u64, 2, 5, 20] {
            let gap = Duration::from_micros(1200 * 8 / mbit);
            let mut run = ShortRun::new(now);
            for index in 0..(2_000_000 / gap.as_micros() as u64) {
                let at = now + gap * index as u32;
                if run.note(at, 1200) {
                    budget.activate(at);
                }
                if index % 8 == 7 && run.note(at, 60) {
                    budget.activate(at);
                }
            }
            assert!(!budget.snapshot().interactive, "{mbit} Mbit/s QUIC looked like a call");
        }
        // A video call: 50 voice packets and 150 video packets per second each way.
        let mut run = ShortRun::new(now);
        for index in 0..100u64 {
            let at = now + Duration::from_millis(5 * index);
            let len = if index % 4 == 0 { 160 } else { 1200 };
            if run.note(at, len) {
                budget.activate(at);
            }
        }
        assert!(budget.snapshot().interactive);
    }
    #[test]
    fn delay_thresholds_scale_with_the_path() {
        let ms = Duration::from_millis;
        assert_eq!(delay(ms(50), ms(60)), Delay::Clear);
        assert_eq!(delay(ms(50), ms(61)), Delay::Hold);
        assert_eq!(delay(ms(50), ms(80)), Delay::Queued);
        assert_eq!(delay(ms(300), ms(337)), Delay::Clear);
        assert_eq!(delay(ms(300), ms(374)), Delay::Hold);
        assert_eq!(delay(ms(300), ms(375)), Delay::Queued);
    }
    /// Send and acknowledge one packet of `size` on flow `id`, `rtt` later.
    async fn round(budget: &Arc<TcpFlight>, id: u32, size: usize, start: Instant, rtt: Duration) {
        let closed = AtomicBool::new(false);
        budget.reserve(id, size).await.unwrap().commit(id, &closed, || Ok(())).unwrap();
        // Model the round trip by back-dating the send time.
        budget.state.lock().unwrap().held.get_mut(&id).unwrap().back_mut().unwrap().sent_at = start;
        let mut state = budget.state.lock().unwrap();
        let packet = state.held.get_mut(&id).unwrap().pop_front().unwrap();
        if state.held[&id].is_empty() {
            state.held.remove(&id);
        }
        let charged = packet.permit.bytes();
        budget.give_back(&mut state, packet.permit);
        budget.follow_delay(&mut state, rtt, charged, start + rtt);
    }
    #[tokio::test]
    async fn an_active_budget_grows_on_a_clear_path_and_shrinks_to_the_floor_on_a_queue() {
        let floor = 96 * 1024;
        let budget = TcpFlight::new(MAX, Some(floor));
        let ms = Duration::from_millis;
        let now = Instant::now();
        // Without a call an idle sender earns nothing, whatever the delay.
        round(&budget, 1, 16384, now, ms(50)).await;
        assert_eq!(budget.snapshot().effective_bytes, BULK_START);
        // Activation starts at the configured size, as the fixed budget did.
        budget.activate(now);
        assert_eq!(budget.snapshot().effective_bytes, floor);
        // Fast delivery: one round trip of a full limit adds about one chunk.
        for index in 0..64 {
            round(&budget, 1, 16384, now + ms(index), ms(52)).await;
        }
        let grown = budget.snapshot().effective_bytes;
        assert!(grown > floor + 6 * 16384 && grown < floor + 64 * 16384, "grew to {grown}");
        // A continuing call does not reset what the delay has earned.
        budget.activate(now + ms(100));
        assert_eq!(budget.snapshot().effective_bytes, grown);
        // A queue halves it, at most once per round trip, never below the floor.
        round(&budget, 1, 16384, now + ms(200), ms(150)).await;
        assert_eq!(budget.snapshot().effective_bytes, grown / 2);
        round(&budget, 1, 16384, now + ms(201), ms(150)).await;
        assert_eq!(budget.snapshot().effective_bytes, grown / 2);
        for index in 0..10 {
            round(&budget, 1, 16384, now + ms(500 + 200 * index), ms(150)).await;
        }
        assert_eq!(budget.snapshot().effective_bytes, floor);
        // Expiry keeps the floor and slow-starts from it.
        budget.tick(now + ms(100) + HOLD);
        assert_eq!(budget.snapshot().effective_bytes, floor);
        assert!(!budget.snapshot().interactive);
        for index in 0..6 {
            bulk_round(&budget, ms(50), now + HOLD + ms(200 + index)).await;
        }
        assert_eq!(budget.snapshot().effective_bytes, 2 * floor);
    }
    /// One 16 KiB `Delivered` while the sender keeps the whole budget in flight.
    async fn bulk_round(budget: &Arc<TcpFlight>, rtt: Duration, now: Instant) {
        let spare = budget.available.available_permits();
        let taken = (spare > 0).then(|| budget.available.clone().try_acquire_many_owned(spare as u32).unwrap());
        let mut state = budget.state.lock().unwrap();
        budget.follow_delay(&mut state, rtt, 16384, now);
        if let Some(permit) = taken {
            budget.give_back(&mut state, Permit { permit, light: false });
        }
    }
    #[tokio::test]
    async fn without_a_call_the_budget_doubles_until_a_queue_and_then_drains_it() {
        let budget = TcpFlight::new(MAX, Some(96 * 1024));
        let ms = Duration::from_millis;
        let now = Instant::now();
        assert_eq!(budget.snapshot().effective_bytes, BULK_START);
        // Slow start: one round trip of a full budget doubles it.
        for index in 0..64 {
            bulk_round(&budget, ms(50), now + ms(index)).await;
        }
        assert_eq!(budget.snapshot().effective_bytes, 2 * BULK_START);
        // A queue: down to what the path carried without it, once per round trip.
        bulk_round(&budget, ms(100), now + ms(100)).await;
        assert_eq!(budget.snapshot().effective_bytes, BULK_START);
        bulk_round(&budget, ms(100), now + ms(150)).await;
        assert_eq!(budget.snapshot().effective_bytes, BULK_START);
        // Then a quarter per round trip.
        for index in 0..64 {
            bulk_round(&budget, ms(50), now + ms(300 + index)).await;
        }
        assert_eq!(budget.snapshot().effective_bytes, BULK_START + BULK_START / 4);
        // A delay that is part of the path, not queue, is under the threshold.
        bulk_round(&budget, ms(59), now + ms(400)).await;
        assert!(budget.snapshot().effective_bytes > BULK_START + BULK_START / 4);
        // Never below the floor, never above the session budget.
        for index in 0..10 {
            bulk_round(&budget, ms(1000), now + ms(1000 + 1100 * index)).await;
        }
        assert_eq!(budget.snapshot().effective_bytes, BULK_FLOOR);
        assert!(!budget.snapshot().interactive);
        let budget = TcpFlight::new(BULK_START + 65536, Some(96 * 1024));
        for index in 0..64 {
            bulk_round(&budget, ms(50), now + ms(index)).await;
        }
        assert_eq!(budget.snapshot().effective_bytes, BULK_START + 65536);
    }
    #[test]
    fn the_uncongested_delay_ages_so_a_slower_path_is_not_throttled_for_good() {
        let mut recent = Recent::default();
        let ms = Duration::from_millis;
        let now = Instant::now();
        assert_eq!(recent.observe(ms(50), now), ms(50));
        assert_eq!(recent.observe(ms(120), now + ms(5_000)), ms(50));
        // The previous window still remembers the faster path.
        assert_eq!(recent.observe(ms(120), now + ms(12_000)), ms(50));
        assert_eq!(recent.observe(ms(120), now + ms(22_000)), ms(120));
        // After a long silence nothing old is kept.
        assert_eq!(recent.observe(ms(300), now + ms(60_000)), ms(300));
    }
    #[tokio::test]
    async fn a_light_flow_passes_a_full_budget() {
        let budget = TcpFlight::new(MAX, Some(65536));
        budget.activate(Instant::now());
        let closed = AtomicBool::new(false);
        // A download: its first 32 KiB are light, the rest fills the budget.
        for _ in 0..6 {
            budget.reserve(1, 16384).await.unwrap().commit(1, &closed, || Ok(())).unwrap();
        }
        assert_eq!(budget.available.available_permits(), 0);
        let blocked = {
            let budget = budget.clone();
            tokio::spawn(async move { budget.reserve(1, 16384).await })
        };
        // A ping reply on another connection does not wait behind it.
        let ping = tokio::time::timeout(Duration::from_millis(100), budget.reserve(2, 100))
            .await
            .expect("light flow waited")
            .unwrap();
        ping.commit(2, &closed, || Ok(())).unwrap();
        // More than a light flow carries goes through the budget.
        let large = {
            let budget = budget.clone();
            tokio::spawn(async move { budget.reserve(3, 3 * 16384).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!blocked.is_finished() && !large.is_finished());
        // Acknowledging light packets returns the reserve, not the budget.
        budget.acknowledge(2, 100, 1).unwrap();
        budget.acknowledge(1, 16384, 1).unwrap();
        assert_eq!(budget.light.available_permits(), LIGHT_RESERVE - 16384);
        assert_eq!(budget.available.available_permits(), 0);
        budget.acknowledge(1, 16384, 1).unwrap();
        assert_eq!(budget.light.available_permits(), LIGHT_RESERVE);
        // The download's first budget packet frees room for its next one.
        budget.acknowledge(1, 16384, 1).unwrap();
        tokio::time::timeout(Duration::from_secs(1), blocked).await.unwrap().unwrap().unwrap();
        budget.clear(1);
        large.abort();
    }
    #[test]
    fn the_datagram_port_is_read_from_every_address_form() {
        let ipv4 = [0, 0, 0, 1, 1, 1, 1, 1, 0, 53, 9, 9];
        assert_eq!(datagram_port(&ipv4), Some(53));
        let mut ipv6 = vec![0, 0, 0, 4];
        ipv6.extend_from_slice(&[0x20; 16]);
        ipv6.extend_from_slice(&[0x01, 0xbb, 7]);
        assert_eq!(datagram_port(&ipv6), Some(443));
        let mut domain = vec![0, 0, 0, 3, 11];
        domain.extend_from_slice(b"example.com");
        domain.extend_from_slice(&[0, 53]);
        assert_eq!(datagram_port(&domain), Some(53));
        // Truncated, fragmented or foreign payloads have no port.
        for invalid in [&[][..], &[0, 0, 0, 1, 1, 1, 1], &[0, 0, 1, 1, 1, 1, 1, 1, 0, 53], &[0, 0, 0, 9, 0, 0]] {
            assert_eq!(datagram_port(invalid), None);
        }
    }
    #[tokio::test]
    async fn shrink_waits_for_peer_credit_and_graceful_retirement_does_not_refund_early() {
        let budget = TcpFlight::with_reserve(MAX, Some(65536), 0);
        let closed = AtomicBool::new(false);
        for id in 0..16 {
            budget
                .reserve(id, 16384)
                .await
                .unwrap()
                .commit(id, &closed, || Ok(()))
                .unwrap();
        }
        activate(&budget, Instant::now());
        {
            // Deliveries report a standing queue, so the active budget stays at
            // its floor and only peer credit releases the excess.
            let mut state = budget.state.lock().unwrap();
            state.base = Some(Duration::from_millis(50));
            let sent_at = Instant::now() - Duration::from_millis(300);
            for packet in state.held.values_mut().flatten() {
                packet.sent_at = sent_at;
            }
        }
        budget.retire(0, &closed, false);
        assert_eq!(budget.snapshot().held_bytes, 262144);
        assert!(budget.acknowledge(0, 1, 1).is_err());
        let waiting = {
            let budget = budget.clone();
            tokio::spawn(async move { budget.reserve(100, 16384).await })
        };
        for id in 0..12 {
            budget.acknowledge(id, 16384, 1).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!waiting.is_finished());
        budget.acknowledge(12, 16384, 1).unwrap();
        let reservation = tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(reservation);
        for id in 13..16 {
            budget.clear(id);
        }
        assert_eq!(budget.available.available_permits(), 65536);
        assert_eq!(budget.snapshot().held_bytes, 0);
    }
    #[tokio::test]
    async fn cancellation_failed_enqueue_and_idle_recovery_return_exact_permits() {
        let budget = TcpFlight::new(MAX, Some(65536));
        let now = Instant::now();
        activate(&budget, now);
        let closed = AtomicBool::new(false);
        let reservation = budget.reserve(1, 1).await.unwrap();
        assert!(reservation
            .commit(1, &closed, || Err("writer stopped".into()))
            .is_err());
        assert_eq!(budget.available.available_permits(), 65536);
        let reservation = budget.reserve(2, 16384).await.unwrap();
        budget.retire(2, &closed, true);
        assert!(reservation
            .commit(2, &closed, || panic!("must not enqueue retired data"))
            .is_err());
        budget.tick(now + Duration::from_secs(16));
        assert_eq!(budget.available.available_permits(), 65536);
        assert_eq!(budget.light.available_permits(), LIGHT_RESERVE);
        assert!(!budget.snapshot().interactive);
    }
    #[tokio::test]
    async fn closing_budget_wakes_waiters_without_a_peer_ack() {
        let budget = TcpFlight::with_reserve(65536, Some(65536), 0);
        let held = budget.reserve(1, 65536).await.unwrap();
        let waiting = {
            let budget = budget.clone();
            tokio::spawn(async move { budget.reserve(2, 1).await })
        };
        budget.close();
        assert!(tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        drop(held);
    }
}
