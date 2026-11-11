//! Per-flow FIFO with bounded preference for short datagrams and flow setup.
//! The existing byte/message permits remain attached to queued frames.
use crate::{tcp_flight::SHORT_DATAGRAM, Kind, Queued};
use std::collections::{HashMap, VecDeque};

const INTERACTIVE_BURST: usize = 8;

pub(super) fn interactive(frame: &Queued) -> bool {
    frame.frame.kind != Kind::Data || (frame.udp_data && frame.frame.data.len() <= SHORT_DATAGRAM)
}

#[derive(Default)]
pub(super) struct Scheduler {
    flows: HashMap<u32, VecDeque<Queued>>,
    interactive: VecDeque<u32>,
    bulk: VecDeque<u32>,
    consecutive_interactive: usize,
}

impl Scheduler {
    pub(super) fn push(&mut self, frame: Queued) {
        let id = frame.frame.id;
        if let Some(flow) = self.flows.get_mut(&id) {
            flow.push_back(frame);
        } else {
            let urgent = interactive(&frame);
            self.flows.insert(id, VecDeque::from([frame]));
            self.list(id, urgent);
        }
    }
    fn list(&mut self, id: u32, urgent: bool) {
        if urgent {
            self.interactive.push_back(id);
        } else {
            self.bulk.push_back(id);
        }
    }
    pub(super) fn next(&mut self) -> Option<Queued> {
        let id = if self.consecutive_interactive < INTERACTIVE_BURST {
            self.interactive
                .pop_front()
                .or_else(|| self.bulk.pop_front())
        } else {
            self.bulk
                .pop_front()
                .or_else(|| self.interactive.pop_front())
        }?;
        // Keep this flow unlisted until the writer commits, discards or defers
        // this frame. A deferred Accepted/EOF cannot be bypassed by its DATA.
        self.flows.get_mut(&id).unwrap().pop_front()
    }
    pub(super) fn complete(&mut self, id: u32, served: Option<bool>, deferred: Option<Queued>) {
        if let Some(urgent) = served {
            if urgent {
                self.consecutive_interactive = self.consecutive_interactive.saturating_add(1);
            } else {
                self.consecutive_interactive = 0;
            }
        }
        let flow = self.flows.get_mut(&id).unwrap();
        if let Some(frame) = deferred {
            flow.push_front(frame);
        }
        if let Some(head) = flow.front() {
            let urgent = interactive(head);
            self.list(id, urgent);
        } else {
            self.flows.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{wire::Frame, Counters};
    use std::{sync::Arc, time::Instant};

    fn frame(id: u32, kind: Kind, udp: bool, size: usize) -> Queued {
        Queued {
            frame: Frame {
                id,
                kind,
                data: vec![0; size],
            },
            queued_at: Instant::now(),
            priority: false,
            udp_credit: false,
            udp_data: udp,
            cancelled: None,
            bytes: None,
            _slot: None,
            counters: Arc::new(Counters::default()),
        }
    }

    #[test]
    fn short_datagrams_bypass_other_flows_but_never_their_own_acceptance() {
        let mut scheduler = Scheduler::default();
        for id in 1..=16 {
            for _ in 0..4 {
                scheduler.push(frame(id, Kind::Data, false, 16384));
            }
        }
        scheduler.push(frame(100, Kind::Accepted, false, 0));
        scheduler.push(frame(100, Kind::Data, true, 170));
        let accepted = scheduler.next().unwrap();
        assert_eq!(
            (accepted.frame.id, accepted.frame.kind),
            (100, Kind::Accepted)
        );
        // A frame which did not fit the current write batch stays at the head.
        scheduler.complete(100, None, Some(accepted));
        let accepted = scheduler.next().unwrap();
        assert_eq!(accepted.frame.kind, Kind::Accepted);
        scheduler.complete(100, Some(true), None);
        let voice = scheduler.next().unwrap();
        assert_eq!((voice.frame.id, voice.frame.kind), (100, Kind::Data));
    }

    #[test]
    fn continuous_interactive_traffic_cannot_starve_bulk_and_bulk_flows_rotate() {
        let mut scheduler = Scheduler::default();
        for _ in 0..100 {
            scheduler.push(frame(100, Kind::Data, true, 170));
        }
        for id in 1..=4 {
            for _ in 0..4 {
                scheduler.push(frame(id, Kind::Data, false, 16384));
            }
        }
        let mut bulk = Vec::new();
        let mut since_bulk = 0;
        for _ in 0..100 {
            let packet = scheduler.next().unwrap();
            let urgent = interactive(&packet);
            if !urgent {
                bulk.push(packet.frame.id);
                since_bulk = 0;
            } else {
                since_bulk += 1;
                assert!(since_bulk <= INTERACTIVE_BURST);
            }
            scheduler.complete(packet.frame.id, Some(urgent), None);
            if bulk.len() == 4 {
                break;
            }
        }
        assert_eq!(bulk, vec![1, 2, 3, 4]);
    }
}
