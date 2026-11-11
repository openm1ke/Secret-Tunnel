//! Bounded, credit-controlled streams over one authenticated byte transport.
//! Application consumption, rather than receipt from SSH, replenishes credit.
pub mod agent;
mod capacity;
mod datagrams;
mod diagnostics;
mod elastic;
mod heartbeat;
mod scheduler;
mod tcp_flight;
pub use capacity::socket_capacity;
pub use diagnostics::{is_flow_cancellation, unix_millis, Diagnostics, TcpStage};
pub use heartbeat::HeartbeatSnapshot;
pub use tcp_flight::TcpFlightSnapshot;
mod relay;
pub use datagrams::DatagramBuffers;
#[cfg(test)]
mod tests;
mod wire;
pub use relay::{relay, Direction};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot, watch, Notify, OwnedSemaphorePermit, Semaphore},
};
use wire::Frame;
pub use wire::{Kind, MAX_PAYLOAD, TCP_CHUNK};

pub type Result<T> = std::result::Result<T, String>;
pub const TCP_DESTINATION_TIMEOUT: Duration = Duration::from_secs(10);
pub const FLOW_OPEN_TIMEOUT: Duration = Duration::from_secs(12);
/// Receive bytes one session may promise its peer across all flows.
///
/// Accounting, not allocation: buffers grow only while a local consumer leaves
/// data unread, at most one window per flow. Sized so 1024 flows can each hold
/// a full `TCP_RECEIVE_WINDOW`.
const MAX_RECEIVE_BUDGET: usize = 1024 * 1024 * 1024;
const BASE_FLOW_SLOT_LIMIT: usize = 128;
pub const UDP_FLOW_SLOT_LIMIT: usize = 192;
/// TCP receive window every STMUX003 endpoint accepts and starts sending within.
pub const BASE_TCP_WINDOW: usize = 65536;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FlowWindow {
    pub bytes: usize,
    pub slots: usize,
}

impl FlowWindow {
    fn valid(self, kind: FlowKind) -> bool {
        let max_slots = match kind {
            FlowKind::Tcp => BASE_FLOW_SLOT_LIMIT,
            FlowKind::Udp => UDP_FLOW_SLOT_LIMIT,
        };
        (65536..=1048576).contains(&self.bytes) && (4..=max_slots).contains(&self.slots)
    }
}

pub const UDP_WINDOW: FlowWindow = FlowWindow {
    bytes: 131072,
    slots: BASE_FLOW_SLOT_LIMIT,
};
/// TCP receive window announced by production endpoints.
///
/// A sender may have at most one window unacknowledged, and credit returns only
/// after the receiver's consumer took the bytes, so one TCP flow carries at most
/// `window / RTT`: 64 KiB gives 10 Mbit/s at 50 ms and 2.6 Mbit/s at 200 ms on
/// any link, 256 KiB 42 and 10 Mbit/s, 1 MiB 168 and 42 Mbit/s. Each side
/// announces its own window in the handshake, so a peer with the 64 KiB default
/// of [`Config`] interoperates and keeps its own direction. Senders grow into it
/// per flow (`elastic`), and the session budget (`tcp_flight`) keeps the sum of
/// all flows from building a queue, so a larger window costs latency only when
/// a single connection really is the bottleneck's only user.
pub const TCP_RECEIVE_WINDOW: usize = 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowKind {
    Tcp,
    Udp,
}
#[derive(Clone, Copy)]
pub enum Role {
    Client,
    Server,
}
#[derive(Clone, Debug)]
pub struct Config {
    pub max_flows: usize,
    pub receive_window: usize,
    pub receive_slots: usize,
    pub outbound_bytes: usize,
    pub udp_window: Option<FlowWindow>,
    /// Start probes only after discovery confirmed peer heartbeat support.
    /// A passive peer starts its own probes after receiving the first Ping.
    pub heartbeat_initiator: bool,
    pub tcp_interactive_bytes: Option<usize>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            max_flows: 1024,
            receive_window: BASE_TCP_WINDOW,
            receive_slots: 64,
            outbound_bytes: 8 * 1024 * 1024,
            udp_window: None,
            heartbeat_initiator: false,
            tcp_interactive_bytes: None,
        }
    }
}
impl Config {
    fn validate(&self) -> Result<()> {
        if !(1..=8192).contains(&self.max_flows)
            || !self.receive_for(FlowKind::Tcp).valid(FlowKind::Tcp)
            || !self.receive_for(FlowKind::Udp).valid(FlowKind::Udp)
            || self.max_receive_window() > self.max_flows * self.receive_window
            || !(65536..=64 * 1024 * 1024).contains(&self.outbound_bytes)
            || self.tcp_interactive_bytes.is_some_and(|bytes| {
                !(65536..=8 * 1024 * 1024).contains(&bytes) || bytes % 1024 != 0
            })
        {
            return Err("Invalid multiplex transport resource budget".into());
        }
        Ok(())
    }

    fn receive_for(&self, kind: FlowKind) -> FlowWindow {
        let tcp = FlowWindow {
            bytes: self.receive_window,
            slots: self.receive_slots,
        };
        match kind {
            FlowKind::Tcp => tcp,
            FlowKind::Udp => self.udp_window.unwrap_or(tcp),
        }
    }

    fn max_receive_window(&self) -> usize {
        self.receive_window
            .max(self.receive_for(FlowKind::Udp).bytes)
    }

    /// Flows this endpoint announces: every one of them may hold its largest
    /// window without the session exceeding the hard receive cap. Larger windows
    /// therefore lower the flow limit instead of failing or rejecting later.
    fn admitted_flows(&self) -> usize {
        self.max_flows
            .min(MAX_RECEIVE_BUDGET / self.max_receive_window())
    }
}

#[derive(Default)]
struct Counters {
    active: AtomicUsize,
    peak: AtomicUsize,
    rejected: AtomicU64,
    rx_queue: AtomicUsize,
    rx_peak: AtomicUsize,
    tx_queue: AtomicUsize,
    tx_peak: AtomicUsize,
    frames_tx: AtomicU64,
    frames_rx: AtomicU64,
    batches: AtomicU64,
    bytes_tx: AtomicU64,
    bytes_rx: AtomicU64,
    udp_drops: AtomicU64,
    protocol_errors: AtomicU64,
    diagnostics: diagnostics::Metrics,
}
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub active: usize,
    pub peak: usize,
    pub limit: usize,
    pub rejected: u64,
    pub receive_queue_bytes: usize,
    pub receive_queue_peak: usize,
    pub send_queue_bytes: usize,
    pub send_queue_peak: usize,
    pub receive_budget: usize,
    pub send_budget: usize,
    pub frames_tx: u64,
    pub frames_rx: u64,
    pub write_batches: u64,
    pub bytes_tx: u64,
    pub bytes_rx: u64,
    pub udp_drops: u64,
    pub protocol_errors: u64,
    pub diagnostics: Diagnostics,
    pub peer_udp_window: Option<FlowWindow>,
    /// TCP receive window this endpoint announced: bounds each incoming flow.
    pub tcp_receive_window: usize,
    /// TCP receive window of the peer: bounds each outgoing flow.
    pub peer_tcp_window: usize,
    pub peer: Option<PeerSnapshot>,
}
#[derive(Debug, Clone, Default)]
pub struct PeerSnapshot {
    pub active: u64,
    pub peak: u64,
    pub limit: u64,
    pub rejected: u64,
    pub receive_queue_bytes: u64,
    pub receive_queue_peak: u64,
    pub receive_budget: u64,
    pub send_queue_bytes: u64,
    pub send_queue_peak: u64,
    pub send_budget: u64,
    pub udp_drops: u64,
    pub protocol_errors: u64,
    pub last_udp_error: String,
    pub diagnostics: Option<Diagnostics>,
}
fn peak(current: &AtomicUsize, highest: &AtomicUsize, amount: usize) {
    highest.fetch_max(
        current.fetch_add(amount, Ordering::Relaxed) + amount,
        Ordering::Relaxed,
    );
}

struct Window {
    counts: Mutex<WindowCredit>,
    max: (usize, usize),
    ready: Notify,
}
struct WindowCredit {
    bytes: usize,
    slots: usize,
    udp_sample: Option<Instant>,
    shortage_sample: Option<Instant>,
    /// Present for a TCP flow whose peer window exceeds the baseline.
    sending: Option<elastic::Sending>,
}

#[derive(Debug)]
enum CreditShortage {
    Bytes,
    Slots,
    Both,
}

#[derive(Debug)]
struct WindowShortage {
    cause: CreditShortage,
    first_rejection: bool,
}
#[derive(Default)]
struct CreditTiming {
    since_enqueue: Option<Duration>,
    since_shortage: Option<Duration>,
}

impl Window {
    fn new(window: FlowWindow, elastic: bool) -> Arc<Self> {
        let FlowWindow { bytes, slots } = window;
        Arc::new(Self {
            counts: Mutex::new(WindowCredit {
                bytes,
                slots,
                udp_sample: None,
                shortage_sample: None,
                sending: elastic.then(elastic::Sending::new),
            }),
            max: (bytes, slots),
            ready: Notify::new(),
        })
    }
    fn reserve(self: &Arc<Self>, bytes: usize) -> std::result::Result<Reservation, WindowShortage> {
        let mut counts = self.counts.lock().unwrap();
        let counts = &mut *counts;
        let now = Instant::now();
        // An elastic flow may hold less than the peer's whole window in flight.
        let in_flight = self.max.0 - counts.bytes;
        let over_limit = counts
            .sending
            .as_mut()
            .is_some_and(|sending| !sending.admits(in_flight, bytes, now));
        let shortage = match (counts.bytes < bytes || over_limit, counts.slots == 0) {
            (true, true) => Some(CreditShortage::Both),
            (true, false) => Some(CreditShortage::Bytes),
            (false, true) => Some(CreditShortage::Slots),
            (false, false) => None,
        };
        if let Some(cause) = shortage {
            let first_rejection = counts.shortage_sample.is_none();
            counts.shortage_sample.get_or_insert_with(Instant::now);
            return Err(WindowShortage { cause, first_rejection });
        }
        counts.bytes -= bytes;
        counts.slots -= 1;
        if let Some(sending) = &mut counts.sending {
            sending.sent(bytes, now);
        }
        Ok(Reservation {
            window: self.clone(),
            bytes,
            committed: false,
        })
    }
    fn sample_udp_credit(&self) {
        self.counts
            .lock()
            .unwrap()
            .udp_sample
            .get_or_insert_with(Instant::now);
    }

    /// Return credit: from the peer (timed against `path` for elastic flows)
    /// or from a reservation that was never sent (`path` is `None`).
    fn grant(
        &self,
        bytes: usize,
        slots: usize,
        from_peer: bool,
        path: Option<&elastic::Path>,
    ) -> Result<CreditTiming> {
        let mut counts = self.counts.lock().unwrap();
        if bytes == 0
            || slots == 0
            || bytes > self.max.0 - counts.bytes
            || slots > self.max.1 - counts.slots
        {
            return Err("Invalid flow credit".into());
        }
        counts.bytes += bytes;
        counts.slots += slots;
        if let Some(sending) = &mut counts.sending {
            let now = Instant::now();
            match path {
                Some(path) if from_peer => sending.credited(bytes, slots, self.max.0, path, now),
                _ => sending.unsent(bytes, now),
            }
        }
        self.ready.notify_waiters();
        Ok(if from_peer {
            CreditTiming {
                since_enqueue: counts.udp_sample.take().map(|t| t.elapsed()),
                since_shortage: counts.shortage_sample.take().map(|t| t.elapsed()),
            }
        } else {
            CreditTiming::default()
        })
    }
}
struct Reservation {
    window: Arc<Window>,
    bytes: usize,
    committed: bool,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.window.grant(self.bytes, 1, false, None);
        }
    }
}

struct Queued {
    frame: Frame,
    queued_at: Instant,
    priority: bool,
    udp_credit: bool,
    udp_data: bool,
    cancelled: Option<Arc<AtomicBool>>,
    bytes: Option<OwnedSemaphorePermit>,
    _slot: Option<OwnedSemaphorePermit>,
    counters: Arc<Counters>,
}
impl Drop for Queued {
    fn drop(&mut self) {
        if self.bytes.is_some() {
            self.counters
                .tx_queue
                .fetch_sub(self.frame.data.len(), Ordering::Relaxed);
        }
    }
}
struct Inner {
    config: Config,
    limit: usize,
    receive_budget: usize,
    receive_bytes: Arc<Semaphore>,
    receive_slots: Arc<Semaphore>,
    peer_limits: wire::Limits,
    remote_odd: bool,
    next: AtomicU64,
    last_remote: AtomicU32,
    open_gate: Mutex<()>,
    flows: Mutex<HashMap<u32, Arc<State>>>,
    incoming: mpsc::Sender<Incoming>,
    data: mpsc::UnboundedSender<Queued>,
    control: mpsc::UnboundedSender<Queued>,
    bytes: Arc<Semaphore>,
    slots: Arc<Semaphore>,
    control_slots: Arc<Semaphore>,
    closed: watch::Sender<bool>,
    error: Mutex<String>,
    counters: Arc<Counters>,
    peer_stats: Mutex<Option<PeerSnapshot>>,
    udp_error: Mutex<String>,
    heartbeat: Mutex<heartbeat::Heartbeat>,
    tcp_flight: Arc<tcp_flight::TcpFlight>,
    /// Uncongested credit round trip of this transport, for elastic TCP flows.
    path: elastic::Path,
}
impl Inner {
    fn fail(&self, error: impl Into<String>) {
        if !*self.closed.borrow() {
            *self.error.lock().unwrap() = error.into();
        }
        self.closed.send_replace(true);
        self.tcp_flight.close();
        for state in self.flows.lock().unwrap().values() {
            state.window.ready.notify_waiters();
        }
    }
    fn control(&self, frame: Frame) -> Result<()> {
        self.queue_control(frame, false)
    }

    fn queue_control(&self, frame: Frame, udp_credit: bool) -> Result<()> {
        let slot = self
            .control_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                self.fail("Multiplex control budget exhausted");
                "Multiplex control budget exhausted".to_owned()
            })?;
        // Acceptance must precede this flow's first DATA, including protocols
        // with a server-first banner. Priority scheduling may bypass controls.
        let ordered = frame.kind == Kind::Accepted;
        let queue = if ordered { &self.data } else { &self.control };
        queue
            .send(Queued {
                frame,
                queued_at: Instant::now(),
                priority: !ordered,
                udp_credit,
                udp_data: false,
                cancelled: None,
                bytes: None,
                _slot: Some(slot),
                counters: self.counters.clone(),
            })
            .map_err(|_| "Multiplex writer stopped".to_owned())
    }
    /// Count a UDP datagram of `state` towards a voice-like run of that flow.
    /// DNS never counts: a resolver that shares one flow is not a call either.
    /// Large datagrams count only towards the flow's volume, see `tcp_flight`.
    fn note_datagram(&self, state: &State, data: &[u8]) {
        if !self.tcp_flight.enabled() || tcp_flight::datagram_port(data) == Some(53) {
            return;
        }
        let now = Instant::now();
        if state.short_run.lock().unwrap().note(now, data.len()) {
            self.tcp_flight.activate(now);
        }
    }
    fn remove(&self, id: u32) {
        let removed = self.flows.lock().unwrap().remove(&id);
        if let Some(state) = removed {
            self.tcp_flight.retire(
                id,
                &state.outbound_closed,
                state.cancelled.load(Ordering::Relaxed),
            );
            self.counters.active.fetch_sub(1, Ordering::Relaxed);
        }
    }
    fn state(
        self: &Arc<Self>,
        id: u32,
        kind: FlowKind,
        probe: bool,
        opening: Option<oneshot::Sender<Result<()>>>,
    ) -> Result<(Arc<State>, mpsc::Receiver<Event>)> {
        let mut flows = self.flows.lock().unwrap();
        if flows.len() >= self.limit || flows.contains_key(&id) {
            self.counters.rejected.fetch_add(1, Ordering::Relaxed);
            return Err("Connection capacity reached".into());
        }
        let receive_window = self.config.receive_for(kind);
        let receive_reservation = self
            .receive_bytes
            .clone()
            .try_acquire_many_owned(receive_window.bytes as u32)
            .map_err(|_| {
                self.counters.rejected.fetch_add(1, Ordering::Relaxed);
                "Receive buffer budget reached".to_owned()
            })?;
        let receive_slot_reservation = self
            .receive_slots
            .clone()
            .try_acquire_many_owned(receive_window.slots as u32)
            .map_err(|_| {
                self.counters.rejected.fetch_add(1, Ordering::Relaxed);
                self.counters
                    .diagnostics
                    .receive_slot_rejections
                    .fetch_add(1, Ordering::Relaxed);
                "Receive packet budget reached".to_owned()
            })?;
        let (packets, receiver) = mpsc::channel(receive_window.slots + 2);
        let (closed, _) = watch::channel(false);
        let state = Arc::new(State {
            id,
            kind,
            probe,
            receive_window,
            _receive_reservation: receive_reservation,
            _receive_slot_reservation: receive_slot_reservation,
            inner: Arc::downgrade(self),
            window: {
                let window = self.peer_limits.window(kind);
                // UDP never waits for credit, and a baseline window has no room to grow.
                Window::new(window, kind == FlowKind::Tcp && window.bytes > BASE_TCP_WINDOW)
            },
            packets,
            opening: Mutex::new(opening),
            ready: AtomicBool::new(false),
            cancelled: Arc::new(AtomicBool::new(false)),
            outbound_closed: AtomicBool::new(false),
            closed,
            received_bytes: AtomicUsize::new(0),
            received_slots: AtomicUsize::new(0),
            sent_eof: AtomicBool::new(false),
            peer_eof: AtomicBool::new(false),
            short_run: Mutex::new(tcp_flight::ShortRun::new(Instant::now())),
        });
        flows.insert(id, state.clone());
        peak(&self.counters.active, &self.counters.peak, 1);
        Ok((state, receiver))
    }
}
struct IoOwner {
    inner: Arc<Inner>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for IoOwner {
    fn drop(&mut self) {
        self.inner.fail("Multiplex transport closed");
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[derive(Clone)]
pub struct Endpoint {
    inner: Arc<Inner>,
    _io: Arc<IoOwner>,
    incoming: Arc<tokio::sync::Mutex<mpsc::Receiver<Incoming>>>,
}
impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("stats", &self.snapshot())
            .finish()
    }
}
impl Endpoint {
    pub async fn closed(&self) {
        let mut closed = self.inner.closed.subscribe();
        if !*closed.borrow() {
            let _ = closed.changed().await;
        }
    }
    pub async fn start<S>(mut stream: S, role: Role, config: Config) -> Result<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        config.validate()?;
        let peer = tokio::time::timeout(
            Duration::from_secs(5),
            wire::handshake(
                &mut stream,
                matches!(role, Role::Server),
                wire::Limits {
                    flows: config.admitted_flows(),
                    tcp: config.receive_for(FlowKind::Tcp),
                    udp: config.udp_window,
                },
            ),
        )
        .await
        .map_err(|_| "Multiplex negotiation timed out".to_owned())?
        .map_err(|e| e.to_string())?;
        let limit = config.admitted_flows().min(peer.flows);
        // Every flow reserves its whole window for its lifetime, and idle UDP
        // associations (one per DNS query behind a TUN adapter) live for tens of
        // seconds. Size both budgets for the largest window, so the advertised
        // flow limit is reachable with any TCP/UDP mix instead of rejecting new
        // flows at a fraction of it. Buffers are still allocated on demand, and
        // `admitted_flows` keeps the byte budget within its hard cap.
        let largest_window = config.max_receive_window();
        let receive_budget = limit * largest_window;
        let largest_window_slots = config.receive_slots.max(config.receive_for(FlowKind::Udp).slots);
        let receive_slot_budget = limit * largest_window_slots;
        let (incoming_tx, incoming) = mpsc::channel(limit);
        let (data_tx, data_rx) = mpsc::unbounded_channel();
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let (closed, _) = watch::channel(false);
        let inner = Arc::new(Inner {
            limit,
            receive_budget,
            receive_bytes: Arc::new(Semaphore::new(receive_budget)),
            receive_slots: Arc::new(Semaphore::new(receive_slot_budget)),
            peer_limits: peer,
            remote_odd: matches!(role, Role::Server),
            next: AtomicU64::new(if matches!(role, Role::Client) { 1 } else { 2 }),
            last_remote: AtomicU32::new(0),
            open_gate: Mutex::new(()),
            flows: Default::default(),
            incoming: incoming_tx,
            data: data_tx,
            control: control_tx,
            bytes: Arc::new(Semaphore::new(config.outbound_bytes)),
            slots: Arc::new(Semaphore::new(4096)),
            control_slots: Arc::new(Semaphore::new(receive_slot_budget + limit * 4 + 64)),
            heartbeat: Mutex::new(heartbeat::Heartbeat::new(config.heartbeat_initiator)),
            tcp_flight: tcp_flight::TcpFlight::new(
                config.outbound_bytes,
                config.tcp_interactive_bytes,
            ),
            config,
            closed,
            error: Default::default(),
            counters: Default::default(),
            peer_stats: Default::default(),
            udp_error: Default::default(),
            path: Default::default(),
        });
        let (reader, writer) = tokio::io::split(stream);
        let read_inner = inner.clone();
        let write_inner = inner.clone();
        let reader = tokio::spawn(async move {
            read_loop(reader, read_inner).await;
        });
        let writer = tokio::spawn(async move {
            write_loop(writer, data_rx, control_rx, write_inner).await;
        });
        let probe_inner = inner.clone();
        let heartbeat = tokio::spawn(async move { heartbeat::run(probe_inner).await; });
        let owner = Arc::new(IoOwner {
            inner: inner.clone(),
            tasks: vec![reader, writer, heartbeat],
        });
        Ok(Self {
            inner,
            _io: owner,
            incoming: Arc::new(tokio::sync::Mutex::new(incoming)),
        })
    }
    pub fn error(&self) -> Option<String> {
        if *self.inner.closed.borrow() {
            Some(self.inner.error.lock().unwrap().clone())
        } else {
            None
        }
    }
    pub fn shutdown(&self) {
        self.inner.fail("Multiplex transport stopped");
    }
    pub fn snapshot(&self) -> Snapshot {
        let c = &self.inner.counters;
        let udp_window = self.inner.config.receive_for(FlowKind::Udp);
        let mut diagnostics = c.diagnostics.snapshot(
            c.bytes_tx.load(Ordering::Relaxed),
            c.bytes_rx.load(Ordering::Relaxed),
        );
        diagnostics.udp_window_bytes = udp_window.bytes;
        diagnostics.udp_window_slots = udp_window.slots;
        diagnostics.heartbeat = self.inner.heartbeat.lock().unwrap().snapshot(Instant::now());
        diagnostics.tcp_flight = self.inner.tcp_flight.snapshot();
        Snapshot {
            active: c.active.load(Ordering::Relaxed),
            peak: c.peak.load(Ordering::Relaxed),
            limit: self.inner.limit,
            rejected: c.rejected.load(Ordering::Relaxed),
            receive_queue_bytes: c.rx_queue.load(Ordering::Relaxed),
            receive_queue_peak: c.rx_peak.load(Ordering::Relaxed),
            send_queue_bytes: c.tx_queue.load(Ordering::Relaxed),
            send_queue_peak: c.tx_peak.load(Ordering::Relaxed),
            receive_budget: self.inner.receive_budget,
            send_budget: self.inner.config.outbound_bytes,
            frames_tx: c.frames_tx.load(Ordering::Relaxed),
            frames_rx: c.frames_rx.load(Ordering::Relaxed),
            write_batches: c.batches.load(Ordering::Relaxed),
            bytes_tx: c.bytes_tx.load(Ordering::Relaxed),
            bytes_rx: c.bytes_rx.load(Ordering::Relaxed),
            udp_drops: c.udp_drops.load(Ordering::Relaxed),
            protocol_errors: c.protocol_errors.load(Ordering::Relaxed),
            diagnostics,
            peer_udp_window: Some(self.inner.peer_limits.window(FlowKind::Udp)),
            tcp_receive_window: self.inner.config.receive_window,
            peer_tcp_window: self.inner.peer_limits.window(FlowKind::Tcp).bytes,
            peer: self.inner.peer_stats.lock().unwrap().clone(),
        }
    }
    pub async fn accept(&self) -> Option<Incoming> {
        let mut closed = self.inner.closed.subscribe();
        if *closed.borrow() {
            return None;
        }
        tokio::select! { value = async { self.incoming.lock().await.recv().await } => value, _ = closed.changed() => None }
    }
    pub fn publish_stats(&self, detailed: bool) -> bool {
        let s = self.snapshot();
        let fields = [
            s.active as u64,
            s.peak as u64,
            s.limit as u64,
            s.rejected,
            s.receive_queue_bytes as u64,
            s.receive_queue_peak as u64,
            s.receive_budget as u64,
            s.send_queue_bytes as u64,
            s.send_queue_peak as u64,
            s.send_budget as u64,
            s.udp_drops,
            s.protocol_errors,
        ];
        let mut data = Vec::with_capacity(352);
        for value in fields {
            data.extend_from_slice(&value.to_be_bytes());
        }
        let error = self.inner.udp_error.lock().unwrap();
        if detailed {
            let report = diagnostics::PeerReport {
                metrics: s.diagnostics,
                last_udp_error: error.clone(),
            };
            let Ok(encoded) = serde_json::to_vec(&report) else {
                return false;
            };
            if encoded.len() > diagnostics::MAX_BYTES {
                return false;
            }
            data.extend_from_slice(diagnostics::MARKER);
            data.extend_from_slice(&encoded);
        } else {
            data.extend_from_slice(&error.as_bytes()[..error.len().min(256)]);
        }
        drop(error);
        let Ok(slot) = self.inner.control_slots.clone().try_acquire_owned() else {
            return false;
        };
        self.inner
            .control
            .send(Queued {
                frame: Frame {
                    kind: Kind::Stats,
                    id: 0,
                    data,
                },
                queued_at: Instant::now(),
                priority: true,
                udp_credit: false,
                udp_data: false,
                cancelled: None,
                bytes: None,
                _slot: Some(slot),
                counters: self.inner.counters.clone(),
            })
            .is_ok()
    }
    pub async fn open(&self, kind: FlowKind, target: Vec<u8>) -> Result<Flow> {
        self.open_flow(kind, target, false).await
    }

    pub async fn probe_tcp(&self, target: Vec<u8>) -> Result<Flow> {
        self.open_flow(FlowKind::Tcp, target, true).await
    }

    async fn open_flow(&self, kind: FlowKind, target: Vec<u8>, probe: bool) -> Result<Flow> {
        if let Some(error) = self.error() {
            return Err(error);
        }
        if target.len() > 512 {
            return Err("Destination is too long".into());
        }
        let flow = {
            let _gate = self.inner.open_gate.lock().unwrap();
            let id = self.inner.next.fetch_add(2, Ordering::Relaxed);
            if id > u32::MAX as u64 {
                return Err("Flow IDs exhausted; reconnect transport".into());
            }
            let id = id as u32;
            let (ack, ready) = oneshot::channel();
            let (state, receiver) = self.inner.state(id, kind, probe, Some(ack))?;
            let flow = Flow {
                state,
                receiver,
                retired: false,
                eof: false,
            };
            self.inner.control(Frame {
                id,
                kind: match kind {
                    FlowKind::Tcp if probe && self.inner.config.udp_window.is_some() => {
                        Kind::ProbeTcp
                    }
                    FlowKind::Tcp => Kind::OpenTcp,
                    FlowKind::Udp => Kind::OpenUdp,
                },
                data: target,
            })?;
            (flow, ready)
        };
        let (flow, ready) = flow;
        let mut closed = self.inner.closed.subscribe();
        let started = Instant::now();
        let result = tokio::select! {
            result = tokio::time::timeout(FLOW_OPEN_TIMEOUT, ready) => match result {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err("Destination setup cancelled".to_owned()),
                Err(_) => Err("Destination setup timed out".to_owned()),
            },
            _ = closed.changed() => Err("Multiplex transport closed".into()),
        };
        if kind == FlowKind::Tcp {
            flow.record_tcp(
                TcpStage::Open,
                started.elapsed(),
                result.as_ref().map(|_| ()).map_err(String::as_str),
            );
        }
        result?;
        Ok(flow)
    }
}

struct State {
    id: u32,
    kind: FlowKind,
    probe: bool,
    receive_window: FlowWindow,
    _receive_reservation: OwnedSemaphorePermit,
    _receive_slot_reservation: OwnedSemaphorePermit,
    inner: Weak<Inner>,
    window: Arc<Window>,
    packets: mpsc::Sender<Event>,
    opening: Mutex<Option<oneshot::Sender<Result<()>>>>,
    ready: AtomicBool,
    cancelled: Arc<AtomicBool>,
    outbound_closed: AtomicBool,
    closed: watch::Sender<bool>,
    received_bytes: AtomicUsize,
    received_slots: AtomicUsize,
    sent_eof: AtomicBool,
    peer_eof: AtomicBool,
    /// Short datagrams of this flow in both directions; see `tcp_flight`.
    short_run: Mutex<tcp_flight::ShortRun>,
}
impl State {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.closed.send_replace(true);
        if let Some(ack) = self.opening.lock().unwrap().take() {
            let _ = ack.send(Err("Flow closed before destination acceptance".into()));
        }
        self.window.ready.notify_waiters();
    }
}
enum Event {
    Data(Received),
    Eof,
}
pub struct Received {
    data: Vec<u8>,
    state: Arc<State>,
    arrived_at: Instant,
}
impl std::ops::Deref for Received {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.data
    }
}
impl Drop for Received {
    fn drop(&mut self) {
        self.state
            .received_bytes
            .fetch_sub(self.data.len(), Ordering::Relaxed);
        self.state.received_slots.fetch_sub(1, Ordering::Relaxed);
        if let Some(inner) = self.state.inner.upgrade() {
            let metrics = &inner.counters.diagnostics;
            let held = self.arrived_at.elapsed();
            let udp = self.state.kind == FlowKind::Udp;
            metrics.rx_hold.observe(held);
            if udp {
                metrics.udp_receive_hold.observe(held);
            }
            inner
                .counters
                .rx_queue
                .fetch_sub(self.data.len(), Ordering::Relaxed);
            if !self.state.cancelled.load(Ordering::Relaxed) {
                let mut data = Vec::with_capacity(8);
                data.extend_from_slice(&(self.data.len() as u32).to_be_bytes());
                data.extend_from_slice(&1u32.to_be_bytes());
                let _ = inner.queue_control(
                    Frame {
                        id: self.state.id,
                        kind: Kind::Credit,
                        data,
                    },
                    udp,
                );
            }
        }
    }
}
pub struct Flow {
    state: Arc<State>,
    receiver: mpsc::Receiver<Event>,
    retired: bool,
    eof: bool,
}
#[derive(Clone)]
pub struct Sender {
    state: Arc<State>,
}
pub struct Incoming {
    pub kind: FlowKind,
    pub target: Vec<u8>,
    pub flow: Flow,
}
impl Flow {
    pub fn record_tcp(
        &self,
        stage: TcpStage,
        elapsed: Duration,
        result: std::result::Result<(), &str>,
    ) {
        if let Some(inner) = self.state.inner.upgrade() {
            inner.counters.diagnostics.record_tcp(
                self.id(),
                stage,
                elapsed,
                result,
                self.state.probe,
            );
        }
    }

    pub fn sender(&self) -> Sender {
        Sender {
            state: self.state.clone(),
        }
    }
    pub fn id(&self) -> u32 {
        self.state.id
    }
    pub fn accept(&self) -> Result<()> {
        self.state.ready.store(true, Ordering::Relaxed);
        self.state
            .inner
            .upgrade()
            .ok_or("Transport closed")?
            .control(Frame {
                id: self.id(),
                kind: Kind::Accepted,
                data: vec![],
            })
    }
    pub fn reject(mut self, reason: &str) {
        if let Some(inner) = self.state.inner.upgrade() {
            let _ = inner.control(Frame {
                id: self.id(),
                kind: Kind::Rejected,
                data: reason.as_bytes()[..reason.len().min(256)].to_vec(),
            });
            inner.remove(self.id());
        }
        self.state.cancel();
        self.retired = true;
    }
    pub async fn recv(&mut self) -> Result<Option<Received>> {
        if self.eof {
            return Ok(None);
        }
        let inner = self.state.inner.upgrade().ok_or("Transport closed")?;
        let mut all_closed = inner.closed.subscribe();
        let mut flow_closed = self.state.closed.subscribe();
        if *all_closed.borrow() || *flow_closed.borrow() {
            return Err("Flow closed".into());
        }
        tokio::select! {
            event = self.receiver.recv() => match event {
                Some(Event::Data(packet)) => Ok(Some(packet)),
                Some(Event::Eof) => { self.eof = true; Ok(None) },
                None => Err("Flow receiver stopped".into()),
            },
            _ = all_closed.changed() => Err("Transport closed".into()),
            _ = flow_closed.changed() => Err("Flow closed".into()),
        }
    }
    pub async fn finish(&mut self) -> Result<()> {
        if self.state.kind == FlowKind::Tcp
            && (!self.eof || !self.state.sent_eof.load(Ordering::Relaxed))
        {
            return Err("TCP finish requires both EOF directions".into());
        }
        if let Some(inner) = self.state.inner.upgrade() {
            self.sender()
                .enqueue(Kind::Close, Vec::new(), false)
                .await?;
            inner.remove(self.id());
            self.retired = true;
        }
        Ok(())
    }
}
impl Drop for Flow {
    fn drop(&mut self) {
        if !self.retired {
            let was_cancelled = self.state.cancelled.swap(true, Ordering::Relaxed);
            self.state.cancel();
            if let Some(inner) = self.state.inner.upgrade() {
                inner.remove(self.id());
                if !was_cancelled {
                    let _ = inner.control(Frame {
                        id: self.id(),
                        kind: Kind::Abort,
                        data: vec![],
                    });
                }
            }
        }
    }
}
impl Sender {
    pub fn is_closed(&self) -> bool {
        self.state.cancelled.load(Ordering::Relaxed)
            || self
                .state
                .inner
                .upgrade()
                .is_none_or(|inner| *inner.closed.borrow())
    }
    pub async fn closed(&self) {
        let Some(inner) = self.state.inner.upgrade() else {
            return;
        };
        let mut all = inner.closed.subscribe();
        let mut flow = self.state.closed.subscribe();
        if *all.borrow() || *flow.borrow() {
            return;
        }
        tokio::select! { _ = all.changed() => (), _ = flow.changed() => () }
    }
    /// UDP remains an unreliable transport: overload drops are explicit and
    /// counted, instead of retaining an unbounded packet per blocked source.
    pub fn try_send_datagram(&self, data: Vec<u8>) -> Result<bool> {
        if self.state.kind != FlowKind::Udp || data.is_empty() || data.len() > MAX_PAYLOAD {
            return Err("Invalid datagram".into());
        }
        let inner = self.state.inner.upgrade().ok_or("Transport closed")?;
        if self.is_closed() {
            return Err("Flow closed".into());
        }
        let metrics = &inner.counters.diagnostics;
        inner.note_datagram(&self.state, &data);
        metrics.udp_tx_attempts.fetch_add(1, Ordering::Relaxed);
        let mut credit = match self.state.window.reserve(data.len()) {
            Ok(credit) => credit,
            Err(shortage) => {
                if shortage.first_rejection {
                    metrics.udp_credit_stall_events.fetch_add(1, Ordering::Relaxed);
                }
                let cause = match shortage.cause {
                    CreditShortage::Bytes => &metrics.udp_credit_bytes,
                    CreditShortage::Slots => &metrics.udp_credit_slots,
                    CreditShortage::Both => &metrics.udp_credit_both,
                };
                cause.fetch_add(1, Ordering::Relaxed);
                Self::note_udp_drop(&inner, &metrics.udp_flow_credit);
                return Ok(false);
            }
        };
        let Ok(bytes) = inner
            .bytes
            .clone()
            .try_acquire_many_owned(data.len() as u32)
        else {
            Self::note_udp_drop(&inner, &inner.counters.diagnostics.udp_byte_budget);
            return Ok(false);
        };
        let Ok(slot) = inner.slots.clone().try_acquire_owned() else {
            Self::note_udp_drop(&inner, &inner.counters.diagnostics.udp_slots);
            return Ok(false);
        };
        peak(
            &inner.counters.tx_queue,
            &inner.counters.tx_peak,
            data.len(),
        );
        self.state.window.sample_udp_credit();
        inner
            .data
            .send(Queued {
                frame: Frame {
                    id: self.state.id,
                    kind: Kind::Data,
                    data,
                },
                queued_at: Instant::now(),
                priority: false,
                udp_credit: false,
                udp_data: true,
                cancelled: Some(self.state.cancelled.clone()),
                bytes: Some(bytes),
                _slot: Some(slot),
                counters: inner.counters.clone(),
            })
            .map_err(|_| "Writer stopped")?;
        credit.committed = true;
        Ok(true)
    }
    pub async fn send(&self, data: Vec<u8>) -> Result<()> {
        self.enqueue(Kind::Data, data, true).await
    }
    pub async fn eof(&self) -> Result<()> {
        if self.state.sent_eof.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        self.enqueue(Kind::Eof, vec![], false).await
    }
    fn note_udp_drop(inner: &Inner, cause: &AtomicU64) {
        cause.fetch_add(1, Ordering::Relaxed);
        inner.counters.udp_drops.fetch_add(1, Ordering::Relaxed);
    }
    pub fn note_udp_failure(&self, failure: UdpFailure, reason: &str) {
        if let Some(inner) = self.state.inner.upgrade() {
            let metrics = &inner.counters.diagnostics;
            Self::note_udp_drop(
                &inner,
                match failure {
                    UdpFailure::Dns => &metrics.udp_dns,
                    UdpFailure::Send => &metrics.udp_send,
                },
            );
            *inner.udp_error.lock().unwrap() = reason.chars().take(256).collect();
        }
    }
    async fn enqueue(&self, kind: Kind, data: Vec<u8>, credit: bool) -> Result<()> {
        if kind == Kind::Data
            && (data.is_empty()
                || data.len()
                    > if self.state.kind == FlowKind::Tcp {
                        TCP_CHUNK
                    } else {
                        MAX_PAYLOAD
                    })
        {
            return Err("Invalid data size".into());
        }
        if credit && self.state.sent_eof.load(Ordering::Relaxed) {
            return Err("Data after EOF".into());
        }
        let inner = self.state.inner.upgrade().ok_or("Transport closed")?;
        if self.state.outbound_closed.load(Ordering::Relaxed) {
            return Err("Flow closed".into());
        }
        if credit && self.state.kind == FlowKind::Udp {
            inner.note_datagram(&self.state, &data);
        }
        let mut all_closed = inner.closed.subscribe();
        let mut flow_closed = self.state.closed.subscribe();
        let mut reservation = if credit {
            loop {
                let notified = self.state.window.ready.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if *all_closed.borrow() || self.state.cancelled.load(Ordering::Relaxed) {
                    return Err("Flow closed".into());
                }
                if let Ok(reservation) = self.state.window.reserve(data.len()) {
                    break Some(reservation);
                }
                tokio::select! { _ = notified => (), _ = all_closed.changed() => return Err("Transport closed".into()), _ = flow_closed.changed() => return Err("Flow closed".into()) }
            }
        } else {
            None
        };
        let acquire = async {
            // TCP waiting for delivery credit must leave the shared writer
            // slots and byte budget available for UDP.
            let flight = if credit
                && self.state.kind == FlowKind::Tcp
                && inner.config.tcp_interactive_bytes.is_some()
            {
                Some(inner.tcp_flight.reserve(self.state.id, data.len()).await?)
            } else {
                None
            };
            let bytes = if data.is_empty() {
                None
            } else {
                Some(
                    inner
                        .bytes
                        .clone()
                        .acquire_many_owned(data.len() as u32)
                        .await
                        .map_err(|_| "Budget closed")?,
                )
            };
            let slot = inner
                .slots
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| "Budget closed")?;
            Ok::<_, String>((bytes, slot, flight))
        };
        let (bytes, slot, flight) = tokio::select! {
            value = acquire => value?,
            _ = all_closed.changed() => return Err("Transport closed".into()),
            _ = flow_closed.changed() => return Err("Flow closed".into()),
        };
        let send = || {
            if bytes.is_some() {
                peak(
                    &inner.counters.tx_queue,
                    &inner.counters.tx_peak,
                    data.len(),
                );
            }
            inner
                .data
                .send(Queued {
                    frame: Frame {
                        id: self.state.id,
                        kind,
                        data,
                    },
                    queued_at: Instant::now(),
                    priority: false,
                    udp_credit: false,
                    udp_data: self.state.kind == FlowKind::Udp && kind == Kind::Data,
                    cancelled: if kind == Kind::Close {
                        None
                    } else {
                        Some(self.state.cancelled.clone())
                    },
                    bytes,
                    _slot: Some(slot),
                    counters: inner.counters.clone(),
                })
                .map_err(|_| "Writer stopped".to_string())
        };
        if let Some(flight) = flight {
            flight.commit(self.state.id, &self.state.outbound_closed, send)?;
        } else {
            send()?;
        }
        if let Some(reservation) = &mut reservation {
            reservation.committed = true;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub enum UdpFailure {
    Dns,
    Send,
}

async fn read_loop<R: AsyncRead + Unpin>(reader: R, inner: Arc<Inner>) {
    let mut reader = BufReader::with_capacity(65536, reader);
    let mut closed = inner.closed.subscribe();
    loop {
        let frame = tokio::select! { frame = wire::read(&mut reader) => match frame { Ok(frame) => frame, Err(e) => { inner.fail(format!("Transport read: {e}")); return; } }, _ = closed.changed() => return };
        let received = inner.counters.frames_rx.fetch_add(1, Ordering::Relaxed) + 1;
        if let Err(error) = dispatch(frame, &inner) {
            inner
                .counters
                .protocol_errors
                .fetch_add(1, Ordering::Relaxed);
            inner.fail(error);
            return;
        }
        if received % 32 == 0 {
            tokio::task::yield_now().await;
        }
    }
}
fn dispatch(frame: Frame, inner: &Arc<Inner>) -> Result<()> {
    let Frame { kind, id, data } = frame;
    if matches!(kind, Kind::Ping | Kind::Pong) {
        if id != 0 || data.len() != 8 { return Err("Invalid heartbeat frame".into()); }
        if kind == Kind::Ping {
            inner.heartbeat.lock().unwrap().activate();
            inner.control(Frame { kind: Kind::Pong, id: 0, data })?;
        } else {
            inner.heartbeat.lock().unwrap().acknowledge(u64::from_be_bytes(data.try_into().unwrap()), Instant::now());
        }
        return Ok(());
    }
    if kind == Kind::Stats {
        if id != 0 || data.len() < 96 {
            return Err("Invalid transport statistics".into());
        }
        let extra = &data[96..];
        let (last_udp_error, diagnostics) = match extra.strip_prefix(diagnostics::MARKER) {
            Some(encoded) if encoded.len() <= diagnostics::MAX_BYTES => {
                let report: diagnostics::PeerReport =
                    serde_json::from_slice(encoded).map_err(|_| "Invalid transport diagnostics")?;
                (report.last_udp_error, Some(report.metrics))
            }
            None if extra.len() <= 256 => (String::from_utf8_lossy(extra).into_owned(), None),
            _ => return Err("Invalid transport statistics".into()),
        };
        let number = |i: usize| u64::from_be_bytes(data[i * 8..i * 8 + 8].try_into().unwrap());
        *inner.peer_stats.lock().unwrap() = Some(PeerSnapshot {
            active: number(0),
            peak: number(1),
            limit: number(2),
            rejected: number(3),
            receive_queue_bytes: number(4),
            receive_queue_peak: number(5),
            receive_budget: number(6),
            send_queue_bytes: number(7),
            send_queue_peak: number(8),
            send_budget: number(9),
            udp_drops: number(10),
            protocol_errors: number(11),
            last_udp_error,
            diagnostics,
        });
        return Ok(());
    }
    if matches!(kind, Kind::OpenTcp | Kind::OpenUdp | Kind::ProbeTcp) {
        if kind == Kind::ProbeTcp && inner.config.udp_window.is_none() {
            return Err("TCP probe extension was not negotiated".into());
        }
        if (id % 2 == 1) != inner.remote_odd || data.len() > 512 {
            return Err("Invalid open frame".into());
        }
        if id <= inner.last_remote.swap(id, Ordering::Relaxed) {
            return Err("Flow ID reused or reordered".into());
        }
        let flow_kind = match kind {
            Kind::OpenUdp => FlowKind::Udp,
            _ => FlowKind::Tcp,
        };
        match inner.state(id, flow_kind, kind == Kind::ProbeTcp, None) {
            Ok((state, receiver)) => {
                let incoming = Incoming {
                    kind: flow_kind,
                    target: data,
                    flow: Flow {
                        state,
                        receiver,
                        retired: false,
                        eof: false,
                    },
                };
                if let Err(error) = inner.incoming.try_send(incoming) {
                    error
                        .into_inner()
                        .flow
                        .reject("Accept queue capacity reached");
                }
            }
            Err(reason) => inner.control(Frame {
                kind: Kind::Rejected,
                id,
                data: reason.into_bytes(),
            })?,
        }
        return Ok(());
    }
    let state = { inner.flows.lock().unwrap().get(&id).cloned() };
    if kind == Kind::Delivered {
        if id == 0
            || inner.config.tcp_interactive_bytes.is_none()
            || state
                .as_ref()
                .is_some_and(|flow| flow.kind != FlowKind::Tcp)
            || data.len() != 8
            || data[..4] == [0; 4]
            || data[4..] == [0; 4]
        {
            return Err("Unnegotiated or invalid TCP delivery acknowledgement".into());
        }
        // A graceful local retirement can precede the last delivery ACK.
        return inner.tcp_flight.acknowledge(
            id,
            u32::from_be_bytes(data[..4].try_into().unwrap()) as usize,
            u32::from_be_bytes(data[4..].try_into().unwrap()) as usize,
        );
    }
    let Some(state) = state else {
        if matches!(kind, Kind::Close | Kind::Abort) {
            inner.tcp_flight.clear(id);
        }
        return Ok(());
    }; // Late credit/data after cancellation.
    match kind {
        Kind::Accepted => {
            if !data.is_empty() {
                return Err("Invalid acceptance".into());
            }
            state.ready.store(true, Ordering::Relaxed);
            if let Some(ack) = state.opening.lock().unwrap().take() {
                let _ = ack.send(Ok(()));
            }
        }
        Kind::Rejected => {
            if let Some(ack) = state.opening.lock().unwrap().take() {
                let _ = ack.send(Err(String::from_utf8_lossy(&data).into_owned()));
            }
            state.cancel();
        }
        Kind::Credit => {
            if data.len() != 8 {
                return Err("Invalid credit frame".into());
            }
            let timing = state.window.grant(
                u32::from_be_bytes(data[..4].try_into().unwrap()) as usize,
                u32::from_be_bytes(data[4..].try_into().unwrap()) as usize,
                true,
                Some(&inner.path),
            )?;
            if let Some(elapsed) = timing.since_enqueue {
                inner
                    .counters
                    .diagnostics
                    .udp_credit_return
                    .observe(elapsed);
            }
            if state.kind == FlowKind::Udp {
                if let Some(elapsed) = timing.since_shortage {
                    let metrics = &inner.counters.diagnostics;
                    metrics.udp_credit_stall.observe(elapsed);
                    *metrics.last_udp_credit_stall.lock().unwrap() = Some(diagnostics::UdpCreditStall {
                        at_ms: diagnostics::unix_millis(), flow: id,
                        elapsed_ms: elapsed.as_millis().min(u64::MAX as u128) as u64,
                        credit_bytes: u32::from_be_bytes(data[..4].try_into().unwrap()) as usize,
                        credit_slots: u32::from_be_bytes(data[4..].try_into().unwrap()) as usize,
                    });
                }
            }
        }
        Kind::Close => {
            if !data.is_empty() {
                return Err("Invalid close frame".into());
            }
            if state.kind == FlowKind::Tcp {
                if !state.peer_eof.load(Ordering::Relaxed)
                    || !state.sent_eof.load(Ordering::Relaxed)
                {
                    return Err("Premature TCP close".into());
                }
                // Ordered graceful close must not discard queued DATA/EOF. The
                // application owns admission until it has consumed those bytes.
                inner.tcp_flight.clear(id);
            } else {
                state.cancel();
            }
        }
        Kind::Abort => {
            if data.len() > 256 {
                return Err("Invalid abort frame".into());
            }
            state.cancel();
            inner.tcp_flight.clear(id);
        }
        Kind::Eof => {
            if !data.is_empty() || state.peer_eof.swap(true, Ordering::Relaxed) {
                return Err("Invalid EOF".into());
            }
            state
                .packets
                .try_send(Event::Eof)
                .map_err(|_| "EOF queue overflow")?;
        }
        Kind::Data => {
            let maximum = if state.kind == FlowKind::Tcp {
                TCP_CHUNK
            } else {
                MAX_PAYLOAD
            };
            if !state.ready.load(Ordering::Relaxed)
                || state.peer_eof.load(Ordering::Relaxed)
                || data.is_empty()
                || data.len() > maximum
            {
                return Err("Invalid flow data".into());
            }
            let bytes = state
                .received_bytes
                .fetch_add(data.len(), Ordering::Relaxed)
                + data.len();
            let slots = state.received_slots.fetch_add(1, Ordering::Relaxed) + 1;
            if bytes > state.receive_window.bytes || slots > state.receive_window.slots {
                state
                    .received_bytes
                    .fetch_sub(data.len(), Ordering::Relaxed);
                state.received_slots.fetch_sub(1, Ordering::Relaxed);
                return Err("Peer exceeded granted flow credit".into());
            }
            inner
                .counters
                .bytes_rx
                .fetch_add(data.len() as u64, Ordering::Relaxed);
            if state.kind == FlowKind::Udp {
                inner.note_datagram(&state, &data);
                inner
                    .counters
                    .diagnostics
                    .udp_received
                    .fetch_add(1, Ordering::Relaxed);
            }
            peak(
                &inner.counters.rx_queue,
                &inner.counters.rx_peak,
                data.len(),
            );
            let delivered_bytes = data.len();
            let received = Received {
                data,
                state: state.clone(),
                arrived_at: Instant::now(),
            };
            state
                .packets
                .try_send(Event::Data(received))
                .map_err(|_| "Flow queue exceeded credit")?;
            if state.kind == FlowKind::Tcp && inner.config.tcp_interactive_bytes.is_some() {
                let mut delivered = Vec::with_capacity(8);
                delivered.extend_from_slice(&(delivered_bytes as u32).to_be_bytes());
                delivered.extend_from_slice(&1u32.to_be_bytes());
                inner.control(Frame {
                    id,
                    kind: Kind::Delivered,
                    data: delivered,
                })?;
            }
        }
        _ => return Err("Unexpected frame".into()),
    }
    Ok(())
}

fn next_ready(
    data: &mut mpsc::UnboundedReceiver<Queued>,
    control: &mut mpsc::UnboundedReceiver<Queued>,
    scheduled: &mut scheduler::Scheduler,
    controls: usize,
) -> Option<Queued> {
    // Bound ingestion work while looking beyond already queued bulk flows.
    for _ in 0..128 {
        match data.try_recv() {
            Ok(frame) => scheduled.push(frame),
            Err(_) => break,
        }
    }
    if controls < 32 {
        if let Ok(frame) = control.try_recv() {
            return Some(frame);
        }
    }
    scheduled.next().or_else(|| control.try_recv().ok())
}
async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut data: mpsc::UnboundedReceiver<Queued>,
    mut control: mpsc::UnboundedReceiver<Queued>,
    inner: Arc<Inner>,
) {
    let mut closed = inner.closed.subscribe();
    let mut scheduled = scheduler::Scheduler::default();
    let mut controls = 0usize;
    let mut buffer = Vec::with_capacity(65536);
    // Short writes also bound what ChannelStream can enqueue per message.
    // flush() alone does not confirm network delivery in russh.
    const BATCH_TARGET: usize = TCP_CHUNK + wire::HEADER;
    const WRITE_SLICE: usize = 8192;
    loop {
        if *closed.borrow() {
            return;
        }
        let first = match next_ready(&mut data, &mut control, &mut scheduled, controls) {
            Some(frame) => frame,
            None => {
                tokio::select! {
                    biased;
                    _ = closed.changed() => return,
                    Some(frame) = control.recv() => frame,
                    Some(frame) = data.recv() => {
                        scheduled.push(frame);
                        scheduled.next().unwrap()
                    },
                    else => return
                }
            }
        };
        buffer.clear();
        let mut batch = Vec::new();
        let mut next = Some(first);
        while let Some(frame) = next {
            let id = frame.frame.id;
            let from_data = !frame.priority;
            let urgent = scheduler::interactive(&frame);
            if frame
                .cancelled
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
            {
                if from_data {
                    scheduled.complete(id, None, None);
                }
                next = next_ready(&mut data, &mut control, &mut scheduled, controls);
                continue;
            }
            let over_target = !buffer.is_empty()
                && buffer.len() + wire::HEADER + frame.frame.data.len() > BATCH_TARGET;
            if over_target && from_data && !urgent {
                scheduled.complete(id, None, Some(frame));
                break;
            }
            if let Err(error) = frame.frame.encode(&mut buffer) {
                inner.fail(error.to_string());
                return;
            }
            if from_data {
                scheduled.complete(id, Some(urgent), None);
                controls = 0;
            } else {
                controls = controls.saturating_add(1);
            }
            batch.push(frame);
            if over_target || batch.len() >= 32 {
                break;
            }
            next = next_ready(&mut data, &mut control, &mut scheduled, controls);
        }
        if buffer.is_empty() {
            continue;
        }
        let started = Instant::now();
        let mut has_data = false;
        let mut has_udp_credit = false;
        for frame in &batch {
            if frame.frame.kind == Kind::Data {
                has_data = true;
                inner
                    .counters
                    .diagnostics
                    .tx_queue
                    .observe(started.duration_since(frame.queued_at));
                if scheduler::interactive(frame) {
                    inner
                        .counters
                        .diagnostics
                        .short_udp_queue
                        .observe(started.duration_since(frame.queued_at));
                } else {
                    inner
                        .counters
                        .diagnostics
                        .bulk_data_queue
                        .observe(started.duration_since(frame.queued_at));
                }
            }
            if frame.udp_credit {
                has_udp_credit = true;
                inner
                    .counters
                    .diagnostics
                    .udp_credit_queue
                    .observe(started.duration_since(frame.queued_at));
            }
        }
        inner
            .counters
            .diagnostics
            .write_batch_peak_bytes
            .fetch_max(buffer.len() as u64, Ordering::Relaxed);
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            for chunk in buffer.chunks(WRITE_SLICE) {
                writer.write_all(chunk).await?;
            }
            writer.flush().await
        })
        .await;
        if has_data {
            inner
                .counters
                .diagnostics
                .write_batch
                .observe(started.elapsed());
        }
        if has_udp_credit {
            inner
                .counters
                .diagnostics
                .udp_credit_write
                .observe(started.elapsed());
        }
        if !matches!(result, Ok(Ok(()))) {
            inner.fail("Transport write failed or timed out");
            return;
        }
        inner.counters.batches.fetch_add(1, Ordering::Relaxed);
        inner
            .counters
            .frames_tx
            .fetch_add(batch.len() as u64, Ordering::Relaxed);
        for frame in &batch {
            if frame.frame.kind == Kind::Data {
                inner
                    .counters
                    .bytes_tx
                    .fetch_add(frame.frame.data.len() as u64, Ordering::Relaxed);
                let count = if scheduler::interactive(frame) {
                    &inner.counters.diagnostics.short_udp_packets_written
                } else {
                    &inner.counters.diagnostics.bulk_data_packets_written
                };
                count.fetch_add(1, Ordering::Relaxed);
            }
        }
        drop(batch);
        tokio::task::yield_now().await;
    }
}
