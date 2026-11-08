//! Bounded, credit-controlled streams over one authenticated byte transport.
//! Application consumption, rather than receipt from SSH, replenishes credit.
mod capacity;
mod datagrams;
pub use capacity::socket_capacity;
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
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot, watch, Notify, OwnedSemaphorePermit, Semaphore},
};
use wire::Frame;
pub use wire::{Kind, MAX_PAYLOAD, TCP_CHUNK};

pub type Result<T> = std::result::Result<T, String>;
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
}
impl Default for Config {
    fn default() -> Self {
        Self {
            max_flows: 1024,
            receive_window: 65536,
            receive_slots: 64,
            outbound_bytes: 8 * 1024 * 1024,
        }
    }
}
impl Config {
    fn validate(&self) -> Result<()> {
        if !(1..=8192).contains(&self.max_flows)
            || !(65536..=1048576).contains(&self.receive_window)
            || !(4..=128).contains(&self.receive_slots)
            || self.max_flows * self.receive_window > 256 * 1024 * 1024
            || !(65536..=64 * 1024 * 1024).contains(&self.outbound_bytes)
        {
            return Err("Invalid multiplex transport resource budget".into());
        }
        Ok(())
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
}
fn peak(current: &AtomicUsize, highest: &AtomicUsize, amount: usize) {
    highest.fetch_max(
        current.fetch_add(amount, Ordering::Relaxed) + amount,
        Ordering::Relaxed,
    );
}

struct Window {
    counts: Mutex<(usize, usize)>,
    max: (usize, usize),
    ready: Notify,
}
impl Window {
    fn new(bytes: usize, slots: usize) -> Arc<Self> {
        Arc::new(Self {
            counts: Mutex::new((bytes, slots)),
            max: (bytes, slots),
            ready: Notify::new(),
        })
    }
    fn reserve(self: &Arc<Self>, bytes: usize) -> Option<Reservation> {
        let mut counts = self.counts.lock().unwrap();
        if counts.0 < bytes || counts.1 == 0 {
            return None;
        }
        counts.0 -= bytes;
        counts.1 -= 1;
        Some(Reservation {
            window: self.clone(),
            bytes,
            committed: false,
        })
    }
    fn grant(&self, bytes: usize, slots: usize) -> Result<()> {
        let mut counts = self.counts.lock().unwrap();
        if bytes == 0
            || slots == 0
            || bytes > self.max.0 - counts.0
            || slots > self.max.1 - counts.1
        {
            return Err("Invalid flow credit".into());
        }
        counts.0 += bytes;
        counts.1 += slots;
        self.ready.notify_waiters();
        Ok(())
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
            let _ = self.window.grant(self.bytes, 1);
        }
    }
}

struct Queued {
    frame: Frame,
    priority: bool,
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
    peer_window: (usize, usize),
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
}
impl Inner {
    fn fail(&self, error: impl Into<String>) {
        if !*self.closed.borrow() {
            *self.error.lock().unwrap() = error.into();
        }
        let _ = self.closed.send(true);
        for state in self.flows.lock().unwrap().values() {
            state.window.ready.notify_waiters();
        }
    }
    fn control(&self, frame: Frame) -> Result<()> {
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
                priority: !ordered,
                cancelled: None,
                bytes: None,
                _slot: Some(slot),
                counters: self.counters.clone(),
            })
            .map_err(|_| "Multiplex writer stopped".to_owned())
    }
    fn remove(&self, id: u32) {
        if self.flows.lock().unwrap().remove(&id).is_some() {
            self.counters.active.fetch_sub(1, Ordering::Relaxed);
        }
    }
    fn state(
        self: &Arc<Self>,
        id: u32,
        kind: FlowKind,
        opening: Option<oneshot::Sender<Result<()>>>,
    ) -> Result<(Arc<State>, mpsc::Receiver<Event>)> {
        let mut flows = self.flows.lock().unwrap();
        if flows.len() >= self.limit || flows.contains_key(&id) {
            self.counters.rejected.fetch_add(1, Ordering::Relaxed);
            return Err("Connection capacity reached".into());
        }
        let (packets, receiver) = mpsc::channel(self.config.receive_slots + 2);
        let (closed, _) = watch::channel(false);
        let state = Arc::new(State {
            id,
            kind,
            inner: Arc::downgrade(self),
            window: Window::new(self.peer_window.0, self.peer_window.1),
            packets,
            opening: Mutex::new(opening),
            ready: AtomicBool::new(false),
            cancelled: Arc::new(AtomicBool::new(false)),
            closed,
            received_bytes: AtomicUsize::new(0),
            received_slots: AtomicUsize::new(0),
            sent_eof: AtomicBool::new(false),
            peer_eof: AtomicBool::new(false),
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
                config.max_flows as u32,
                config.receive_window as u32,
                config.receive_slots as u32,
            ),
        )
        .await
        .map_err(|_| "Multiplex negotiation timed out".to_owned())?
        .map_err(|e| e.to_string())?;
        let limit = config.max_flows.min(peer.0 as usize);
        let (incoming_tx, incoming) = mpsc::channel(limit);
        let (data_tx, data_rx) = mpsc::unbounded_channel();
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let (closed, _) = watch::channel(false);
        let inner = Arc::new(Inner {
            limit,
            peer_window: (peer.1 as usize, peer.2 as usize),
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
            control_slots: Arc::new(Semaphore::new(limit * (config.receive_slots + 4) + 64)),
            config,
            closed,
            error: Default::default(),
            counters: Default::default(),
            peer_stats: Default::default(),
            udp_error: Default::default(),
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
        let owner = Arc::new(IoOwner {
            inner: inner.clone(),
            tasks: vec![reader, writer],
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
        Snapshot {
            active: c.active.load(Ordering::Relaxed),
            peak: c.peak.load(Ordering::Relaxed),
            limit: self.inner.limit,
            rejected: c.rejected.load(Ordering::Relaxed),
            receive_queue_bytes: c.rx_queue.load(Ordering::Relaxed),
            receive_queue_peak: c.rx_peak.load(Ordering::Relaxed),
            send_queue_bytes: c.tx_queue.load(Ordering::Relaxed),
            send_queue_peak: c.tx_peak.load(Ordering::Relaxed),
            receive_budget: self.inner.limit * self.inner.config.receive_window,
            send_budget: self.inner.config.outbound_bytes,
            frames_tx: c.frames_tx.load(Ordering::Relaxed),
            frames_rx: c.frames_rx.load(Ordering::Relaxed),
            write_batches: c.batches.load(Ordering::Relaxed),
            bytes_tx: c.bytes_tx.load(Ordering::Relaxed),
            bytes_rx: c.bytes_rx.load(Ordering::Relaxed),
            udp_drops: c.udp_drops.load(Ordering::Relaxed),
            protocol_errors: c.protocol_errors.load(Ordering::Relaxed),
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
    pub fn publish_stats(&self) -> bool {
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
        data.extend_from_slice(&error.as_bytes()[..error.len().min(256)]);
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
                priority: true,
                cancelled: None,
                bytes: None,
                _slot: Some(slot),
                counters: self.inner.counters.clone(),
            })
            .is_ok()
    }
    pub async fn open(&self, kind: FlowKind, target: Vec<u8>) -> Result<Flow> {
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
            let (state, receiver) = self.inner.state(id, kind, Some(ack))?;
            let flow = Flow {
                state,
                receiver,
                retired: false,
                eof: false,
            };
            self.inner.control(Frame {
                id,
                kind: if kind == FlowKind::Tcp {
                    Kind::OpenTcp
                } else {
                    Kind::OpenUdp
                },
                data: target,
            })?;
            (flow, ready)
        };
        let (flow, ready) = flow;
        let mut closed = self.inner.closed.subscribe();
        tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(12), ready) => result.map_err(|_| "Destination setup timed out".to_owned())?.map_err(|_| "Destination setup cancelled".to_owned())??,
            _ = closed.changed() => return Err("Multiplex transport closed".into()),
        }
        Ok(flow)
    }
}

struct State {
    id: u32,
    kind: FlowKind,
    inner: Weak<Inner>,
    window: Arc<Window>,
    packets: mpsc::Sender<Event>,
    opening: Mutex<Option<oneshot::Sender<Result<()>>>>,
    ready: AtomicBool,
    cancelled: Arc<AtomicBool>,
    closed: watch::Sender<bool>,
    received_bytes: AtomicUsize,
    received_slots: AtomicUsize,
    sent_eof: AtomicBool,
    peer_eof: AtomicBool,
}
impl State {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        let _ = self.closed.send(true);
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
            inner
                .counters
                .rx_queue
                .fetch_sub(self.data.len(), Ordering::Relaxed);
            if !self.state.cancelled.load(Ordering::Relaxed) {
                let mut data = Vec::with_capacity(8);
                data.extend_from_slice(&(self.data.len() as u32).to_be_bytes());
                data.extend_from_slice(&1u32.to_be_bytes());
                let _ = inner.control(Frame {
                    id: self.state.id,
                    kind: Kind::Credit,
                    data,
                });
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
        let Some(mut credit) = self.state.window.reserve(data.len()) else {
            self.note_udp_drop();
            return Ok(false);
        };
        let Ok(bytes) = inner
            .bytes
            .clone()
            .try_acquire_many_owned(data.len() as u32)
        else {
            self.note_udp_drop();
            return Ok(false);
        };
        let Ok(slot) = inner.slots.clone().try_acquire_owned() else {
            self.note_udp_drop();
            return Ok(false);
        };
        peak(
            &inner.counters.tx_queue,
            &inner.counters.tx_peak,
            data.len(),
        );
        inner
            .data
            .send(Queued {
                frame: Frame {
                    id: self.state.id,
                    kind: Kind::Data,
                    data,
                },
                priority: false,
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
    pub fn note_udp_drop(&self) {
        if let Some(inner) = self.state.inner.upgrade() {
            inner.counters.udp_drops.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn note_udp_failure(&self, reason: &str) {
        self.note_udp_drop();
        if let Some(inner) = self.state.inner.upgrade() {
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
                if let Some(reservation) = self.state.window.reserve(data.len()) {
                    break Some(reservation);
                }
                tokio::select! { _ = notified => (), _ = all_closed.changed() => return Err("Transport closed".into()), _ = flow_closed.changed() => return Err("Flow closed".into()) }
            }
        } else {
            None
        };
        let acquire = async {
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
            Ok::<_, String>((bytes, slot))
        };
        let (bytes, slot) = tokio::select! {
            value = acquire => value?,
            _ = all_closed.changed() => return Err("Transport closed".into()),
            _ = flow_closed.changed() => return Err("Flow closed".into()),
        };
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
                priority: false,
                cancelled: if kind == Kind::Close {
                    None
                } else {
                    Some(self.state.cancelled.clone())
                },
                bytes,
                _slot: Some(slot),
                counters: inner.counters.clone(),
            })
            .map_err(|_| "Writer stopped")?;
        if let Some(reservation) = &mut reservation {
            reservation.committed = true;
        }
        Ok(())
    }
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
    if kind == Kind::Stats {
        if id != 0 || !(96..=352).contains(&data.len()) {
            return Err("Invalid transport statistics".into());
        }
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
            last_udp_error: String::from_utf8_lossy(&data[96..]).into_owned(),
        });
        return Ok(());
    }
    if matches!(kind, Kind::OpenTcp | Kind::OpenUdp) {
        if (id % 2 == 1) != inner.remote_odd || data.len() > 512 {
            return Err("Invalid open frame".into());
        }
        if id <= inner.last_remote.swap(id, Ordering::Relaxed) {
            return Err("Flow ID reused or reordered".into());
        }
        let flow_kind = if kind == Kind::OpenTcp {
            FlowKind::Tcp
        } else {
            FlowKind::Udp
        };
        match inner.state(id, flow_kind, None) {
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
    let Some(state) = state else {
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
            inner.remove(id);
        }
        Kind::Credit => {
            if data.len() != 8 {
                return Err("Invalid credit frame".into());
            }
            state.window.grant(
                u32::from_be_bytes(data[..4].try_into().unwrap()) as usize,
                u32::from_be_bytes(data[4..].try_into().unwrap()) as usize,
            )?;
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
            } else {
                state.cancel();
                inner.remove(id);
            }
        }
        Kind::Abort => {
            if data.len() > 256 {
                return Err("Invalid abort frame".into());
            }
            state.cancel();
            inner.remove(id);
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
            if bytes > inner.config.receive_window || slots > inner.config.receive_slots {
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
            peak(
                &inner.counters.rx_queue,
                &inner.counters.rx_peak,
                data.len(),
            );
            let received = Received {
                data,
                state: state.clone(),
            };
            state
                .packets
                .try_send(Event::Data(received))
                .map_err(|_| "Flow queue exceeded credit")?;
        }
        _ => return Err("Unexpected frame".into()),
    }
    Ok(())
}

fn next_ready(
    data: &mut mpsc::UnboundedReceiver<Queued>,
    control: &mut mpsc::UnboundedReceiver<Queued>,
    pending: &mut Option<Queued>,
    controls: &mut usize,
) -> Option<Queued> {
    if *controls < 32 {
        if let Ok(frame) = control.try_recv() {
            *controls += 1;
            return Some(frame);
        }
    }
    if let Some(frame) = pending.take().or_else(|| data.try_recv().ok()) {
        *controls = 0;
        return Some(frame);
    }
    if let Ok(frame) = control.try_recv() {
        *controls += 1;
        return Some(frame);
    }
    None
}
async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut data: mpsc::UnboundedReceiver<Queued>,
    mut control: mpsc::UnboundedReceiver<Queued>,
    inner: Arc<Inner>,
) {
    let mut closed = inner.closed.subscribe();
    let mut pending = None;
    let mut controls = 0usize;
    let mut buffer = Vec::with_capacity(131072);
    loop {
        let first = match next_ready(&mut data, &mut control, &mut pending, &mut controls) {
            Some(frame) => frame,
            None => {
                tokio::select! { biased; _ = closed.changed() => return, Some(frame) = control.recv() => { controls += 1; frame }, Some(frame) = data.recv() => { controls = 0; frame }, else => return }
            }
        };
        buffer.clear();
        let mut batch = Vec::new();
        let mut next = Some(first);
        while let Some(frame) = next {
            if frame
                .cancelled
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
            {
                next = next_ready(&mut data, &mut control, &mut pending, &mut controls);
                continue;
            }
            if !buffer.is_empty() && buffer.len() + wire::HEADER + frame.frame.data.len() > 65536 {
                if frame.priority {
                    if let Err(error) = frame.frame.encode(&mut buffer) {
                        inner.fail(error.to_string());
                        return;
                    }
                    batch.push(frame);
                } else {
                    pending = Some(frame);
                }
                break;
            }
            if let Err(error) = frame.frame.encode(&mut buffer) {
                inner.fail(error.to_string());
                return;
            }
            batch.push(frame);
            if batch.len() >= 32 {
                break;
            }
            next = next_ready(&mut data, &mut control, &mut pending, &mut controls);
        }
        if buffer.is_empty() {
            continue;
        }
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            writer.write_all(&buffer).await?;
            writer.flush().await
        })
        .await;
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
            }
        }
        drop(batch);
        tokio::task::yield_now().await;
    }
}
