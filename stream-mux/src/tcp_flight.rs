//! Session-wide TCP credit budget while short UDP is active.
//! Permits return on peer wire receipt, not on acceptance by an SSH writer.
//! Per-flow application-consumption credit remains a separate constraint.
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
const DETECTION: Duration = Duration::from_millis(500);
const HOLD: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TcpFlightSnapshot {
    pub interactive: bool,
    pub configured_bytes: Option<usize>,
    pub effective_bytes: usize,
    pub held_bytes: usize,
    pub flows_with_credit: usize,
}
struct Packet {
    payload: usize,
    permit: OwnedSemaphorePermit,
}
struct State {
    limit: usize,
    debt: usize,
    held: HashMap<u32, VecDeque<Packet>>,
    short_since: Instant,
    short_count: usize,
    active_until: Option<Instant>,
}
pub(super) struct TcpFlight {
    available: Arc<Semaphore>,
    state: Mutex<State>,
    normal: usize,
    target: Option<usize>,
}
pub(super) struct Reservation {
    budget: Arc<TcpFlight>,
    permit: Option<OwnedSemaphorePermit>,
    payload: usize,
}
impl TcpFlight {
    pub(super) fn new(normal: usize, target: Option<usize>) -> Arc<Self> {
        let normal = normal.min(MAX);
        Arc::new(Self {
            available: Arc::new(Semaphore::new(normal)),
            normal,
            target,
            state: Mutex::new(State {
                limit: normal,
                debt: 0,
                held: HashMap::new(),
                short_since: Instant::now(),
                short_count: 0,
                active_until: None,
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
    pub(super) fn note_short(&self, now: Instant) {
        if self.target.is_none() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if now.duration_since(state.short_since) > DETECTION {
            state.short_since = now;
            state.short_count = 0;
        }
        state.short_count += 1;
        if state.short_count >= 12 {
            state.active_until = Some(now + HOLD);
            self.resize(&mut state, self.target.unwrap().min(self.normal));
        }
    }
    pub(super) fn tick(&self, now: Instant) {
        let mut state = self.state.lock().unwrap();
        if state.active_until.is_some_and(|until| now >= until) {
            state.active_until = None;
            self.resize(&mut state, self.normal);
        }
    }
    pub(super) async fn reserve(self: &Arc<Self>, payload: usize) -> Result<Reservation> {
        // Bound metadata too: a one-byte TCP frame still occupies one KiB of
        // the session budget until acknowledged or cancelled.
        let charge = payload.div_ceil(1024) * 1024;
        let permit = self
            .available
            .clone()
            .acquire_many_owned(charge as u32)
            .await
            .map_err(|_| "TCP flight budget closed")?;
        Ok(Reservation {
            budget: self.clone(),
            permit: Some(permit),
            payload,
        })
    }
    fn give_back(&self, state: &mut State, mut permit: OwnedSemaphorePermit) {
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
        for _ in 0..slots {
            returned.push(packets.pop_front().unwrap().permit);
        }
        if packets.is_empty() {
            state.held.remove(&id);
        }
        for permit in returned {
            self.give_back(&mut state, permit);
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
                .map(|p| p.permit.num_permits())
                .sum(),
            flows_with_credit: state.held.len(),
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
    fn activate(budget: &TcpFlight, now: Instant) {
        for _ in 0..12 {
            budget.note_short(now);
        }
    }
    #[test]
    fn isolated_datagrams_do_not_activate_and_short_gaps_do_not_release_bulk() {
        let budget = TcpFlight::new(MAX, Some(65536));
        let now = Instant::now();
        for i in 0..20 {
            budget.note_short(now + Duration::from_secs(i));
        }
        assert!(!budget.snapshot().interactive);
        let now = now + Duration::from_secs(21);
        activate(&budget, now);
        budget.tick(now + Duration::from_secs(3));
        assert!(budget.snapshot().interactive);
        assert_eq!(budget.snapshot().effective_bytes, 65536);
        budget.tick(now + HOLD);
        assert!(!budget.snapshot().interactive);
        assert_eq!(budget.snapshot().effective_bytes, MAX);
    }
    #[tokio::test]
    async fn shrink_waits_for_peer_credit_and_graceful_retirement_does_not_refund_early() {
        let budget = TcpFlight::new(MAX, Some(65536));
        let closed = AtomicBool::new(false);
        for id in 0..16 {
            budget
                .reserve(16384)
                .await
                .unwrap()
                .commit(id, &closed, || Ok(()))
                .unwrap();
        }
        activate(&budget, Instant::now());
        budget.retire(0, &closed, false);
        assert_eq!(budget.snapshot().held_bytes, 262144);
        assert!(budget.acknowledge(0, 1, 1).is_err());
        let waiting = {
            let budget = budget.clone();
            tokio::spawn(async move { budget.reserve(16384).await })
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
        let reservation = budget.reserve(1).await.unwrap();
        assert!(reservation
            .commit(1, &closed, || Err("writer stopped".into()))
            .is_err());
        assert_eq!(budget.available.available_permits(), 65536);
        let reservation = budget.reserve(16384).await.unwrap();
        budget.retire(2, &closed, true);
        assert!(reservation
            .commit(2, &closed, || panic!("must not enqueue retired data"))
            .is_err());
        budget.tick(now + Duration::from_secs(16));
        assert_eq!(budget.available.available_permits(), MAX);
        assert!(!budget.snapshot().interactive);
    }
    #[tokio::test]
    async fn closing_budget_wakes_waiters_without_a_peer_ack() {
        let budget = TcpFlight::new(65536, Some(65536));
        let held = budget.reserve(65536).await.unwrap();
        let waiting = {
            let budget = budget.clone();
            tokio::spawn(async move { budget.reserve(1).await })
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
