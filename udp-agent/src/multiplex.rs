use crate::{
    diagnostic_log::DiagnosticLog,
    udp_protocol::{self, PacketAddress},
};
use secret_tunnel_mux::{
    Config, DatagramBuffers, Endpoint, Flow, FlowKind, Role, TcpStage, UdpFailure,
};
use std::{
    collections::{HashMap, VecDeque},
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf, Stdin, Stdout},
    net::{TcpStream, UdpSocket},
    task::JoinSet,
};

struct Stdio {
    input: Stdin,
    output: Stdout,
}
impl AsyncRead for Stdio {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.input).poll_read(cx, buf)
    }
}
impl AsyncWrite for Stdio {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.output).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.output).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.output).poll_shutdown(cx)
    }
}

fn capacity() -> usize {
    secret_tunnel_mux::socket_capacity(2048)
}

pub enum Mode {
    Basic,
    Diagnostics,
    FlowControl,
    UdpConfig,
    Qos,
}

pub fn run(mode: Mode, elastic_tcp: bool) -> io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let result = runtime.block_on(serve(mode, elastic_tcp));
    // Stdin reads run on a blocking worker. An expired handshake can leave one
    // waiting for input, so this short-lived agent exits without joining it.
    runtime.shutdown_background();
    result
}

async fn serve(mode: Mode, elastic_tcp: bool) -> io::Result<()> {
    let limit = capacity();
    if limit < 32 {
        return Err(io::Error::other(
            "Not enough file descriptors for the agent",
        ));
    }
    let detailed = !matches!(mode, Mode::Basic);
    let mut stream = Stdio {
        input: tokio::io::stdin(),
        output: tokio::io::stdout(),
    };
    let qos = if matches!(mode, Mode::Qos) {
        Some(
            tokio::time::timeout(
                Duration::from_secs(5),
                secret_tunnel_mux::agent::read_session_settings(&mut stream),
            )
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Session settings timed out"))??,
        )
    } else {
        None
    };
    let udp_window = match mode {
        Mode::Qos => qos.map(|settings| settings.udp_window),
        Mode::Basic | Mode::Diagnostics => None,
        Mode::FlowControl => Some(secret_tunnel_mux::UDP_WINDOW),
        Mode::UdpConfig => Some(
            tokio::time::timeout(
                Duration::from_secs(5),
                secret_tunnel_mux::agent::read_udp_window_request(&mut stream),
            )
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "UDP configuration timed out"))??,
        ),
    };
    let endpoint = Endpoint::start(
        stream,
        Role::Server,
        Config {
            max_flows: limit,
            // The agent's receive window bounds the upload rate of each connection.
            // Only a client that grows its sending gradually gets the larger one.
            receive_window: if elastic_tcp {
                secret_tunnel_mux::TCP_RECEIVE_WINDOW
            } else {
                secret_tunnel_mux::BASE_TCP_WINDOW
            },
            udp_window,
            tcp_interactive_bytes: qos.map(|settings| settings.tcp_interactive_bytes),
            ..Default::default()
        },
    )
    .await
    .map_err(io::Error::other)?;
    let buffers = DatagramBuffers::default();
    let mut tasks = JoinSet::new();
    let mut telemetry = tokio::time::interval(Duration::from_secs(5));
    let log = DiagnosticLog::new(detailed);
    let mut reported = None;
    loop {
        tokio::select! {
            incoming=endpoint.accept()=>{
                let Some(incoming)=incoming else {break};let buffers=buffers.clone();
                tasks.spawn(async move {
                    let result=match incoming.kind {
                        FlowKind::Tcp=>tcp(incoming.flow,incoming.target).await,
                        FlowKind::Udp=>udp(incoming.flow,buffers).await,
                    };
                    if let Err(error)=result {
                        if !secret_tunnel_mux::is_flow_cancellation(&error) {
                            eprintln!("Multiplex flow: {error}");
                        }
                    }
                });
            }
            Some(result)=tasks.join_next(),if !tasks.is_empty()=>{if let Err(error)=result{eprintln!("Multiplex task: {error}");}}
            _=telemetry.tick()=>{
                let s = endpoint.snapshot();
                let d = &s.diagnostics;
                let signature = (s.active, s.peak, s.bytes_tx, s.bytes_rx, s.udp_drops, s.rejected,
                    d.tcp_errors, d.tcp_slow, d.tx_queue.over_100_ms, d.rx_hold.over_100_ms,
                    d.write_batch.over_100_ms,
                    (d.tcp_cancelled, d.tcp_probes, d.udp_tx_attempts, d.udp_received,
                     d.udp_credit_return.samples, d.heartbeat.probes, d.heartbeat.acknowledged,
                     d.heartbeat.timeouts, d.heartbeat.pending_ms.map(|ms|ms/5000)));
                if reported != Some(signature) {
                    if detailed { log.report(&s); }
                    if endpoint.publish_stats(detailed) { reported = Some(signature); }
                }
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    if detailed {
        log.report(&endpoint.snapshot());
    }
    eprintln!("Multiplex stopped: {:?}", endpoint.snapshot());
    Ok(())
}

async fn resolve(target: PacketAddress<'_>) -> io::Result<Vec<SocketAddr>> {
    match target {
        PacketAddress::Socket(address) => Ok(vec![address]),
        PacketAddress::Domain(host, port) => tokio::time::timeout(
            Duration::from_secs(3),
            tokio::net::lookup_host((host, port)),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "DNS timeout"))?
        .map(|addresses| addresses.take(8).collect()),
    }
}
async fn resolve_tcp(target: &[u8]) -> io::Result<Vec<SocketAddr>> {
    let parsed = udp_protocol::parse_packet(target)?;
    if !parsed.payload.is_empty() {
        return Err(io::Error::other("Invalid TCP target"));
    }
    resolve(parsed.address).await
}

async fn connect(mut addresses: Vec<SocketAddr>) -> io::Result<TcpStream> {
    if let Some(first) = addresses.first() {
        if let Some(other) = addresses
            .iter()
            .position(|next| next.is_ipv4() != first.is_ipv4())
        {
            addresses.swap(1, other);
        }
    }
    let mut remaining: VecDeque<_> = addresses.into();
    let mut attempts = JoinSet::new();
    if let Some(address) = remaining.pop_front() {
        attempts.spawn(TcpStream::connect(address));
    }
    let fallback = tokio::time::sleep(Duration::from_millis(200));
    tokio::pin!(fallback);
    let mut last = io::Error::other("No destination address");
    while !attempts.is_empty() {
        let result = tokio::select! {
            result = attempts.join_next() => result.unwrap(),
            _ = &mut fallback, if attempts.len() < 2 && !remaining.is_empty() => {
                attempts.spawn(TcpStream::connect(remaining.pop_front().unwrap()));
                continue;
            }
        };
        match result {
            Ok(Ok(stream)) => {
                attempts.abort_all();
                stream.set_nodelay(true)?;
                return Ok(stream);
            }
            Ok(Err(error)) => last = error,
            Err(error) => last = io::Error::other(error),
        }
        if let Some(address) = remaining.pop_front() {
            attempts.spawn(TcpStream::connect(address));
        }
    }
    Err(last)
}
async fn tcp(mut flow: Flow, target: Vec<u8>) -> secret_tunnel_mux::Result<()> {
    let sender = flow.sender();
    let deadline = tokio::time::Instant::now() + secret_tunnel_mux::TCP_DESTINATION_TIMEOUT;
    let dns_started = Instant::now();
    let addresses = tokio::select! {
        addresses = resolve_tcp(&target) => addresses,
        _ = sender.closed() => {
            flow.record_tcp(TcpStage::Dns, dns_started.elapsed(), Err("Flow cancelled"));
            return Ok(());
        },
    };
    let addresses = match addresses {
        Ok(addresses) => {
            flow.record_tcp(TcpStage::Dns, dns_started.elapsed(), Ok(()));
            addresses
        }
        Err(error) => {
            let error = format!("TCP resolve: {error}");
            flow.record_tcp(TcpStage::Dns, dns_started.elapsed(), Err(&error));
            flow.reject(&error);
            return Ok(());
        }
    };
    let connect_started = Instant::now();
    let result = tokio::select! {
        result = tokio::time::timeout_at(deadline, connect(addresses)) => match result {
            Ok(result) => result.map_err(|error| format!("TCP connect: {error}")),
            Err(_) => Err("TCP connect timeout".into()),
        },
        _ = sender.closed() => {
            flow.record_tcp(TcpStage::Connect, connect_started.elapsed(), Err("Flow cancelled"));
            return Ok(());
        },
    };
    let mut stream = match result {
        Ok(stream) => {
            flow.record_tcp(TcpStage::Connect, connect_started.elapsed(), Ok(()));
            stream
        }
        Err(error) => {
            flow.record_tcp(TcpStage::Connect, connect_started.elapsed(), Err(&error));
            flow.reject(&error);
            return Ok(());
        }
    };
    flow.accept()?;
    let relay_started = Instant::now();
    let result = secret_tunnel_mux::relay(&mut flow, &mut stream, |_, _| {}).await;
    if let Err(error) = &result {
        flow.record_tcp(TcpStage::Relay, relay_started.elapsed(), Err(error));
    }
    result
}

type Allowed = Arc<Mutex<HashMap<SocketAddr, Instant>>>;
type Lookup = Pin<Box<dyn std::future::Future<Output = io::Result<Vec<SocketAddr>>> + Send>>;
/// Resolves a UDP destination name; injectable so tests can model a slow DNS.
pub(crate) type Resolver = Arc<dyn Fn(String, u16) -> Lookup + Send + Sync>;

pub(crate) fn system_resolver() -> Resolver {
    Arc::new(|host, port| Box::pin(async move { resolve(PacketAddress::Domain(&host, port)).await }))
}

/// Name lookups of one association that may run at once.
const DNS_PARALLEL: usize = 4;
/// Datagrams and payload bytes that may wait for names in one association.
/// Each one also keeps its receive credit until sent or dropped.
const DNS_PENDING_PACKETS: usize = 32;
const DNS_PENDING_BYTES: usize = 64 * 1024;
/// Resolved names are reused within an association for this long.
const DNS_CACHE: Duration = Duration::from_secs(30);

/// Sockets and peer filter of one SOCKS UDP association on the exit server.
struct Relay {
    sender: secret_tunnel_mux::Sender,
    buffers: DatagramBuffers,
    allowed: Allowed,
    activity: Arc<Mutex<Instant>>,
    v4: Option<Arc<UdpSocket>>,
    v6: Option<Arc<UdpSocket>>,
    readers: JoinSet<()>,
}

impl Relay {
    /// Send `payload` to the first usable address; failures are counted.
    async fn send(&mut self, addresses: &[SocketAddr], payload: &[u8]) {
        let mut send_error = String::from("No UDP destination address");
        for &destination in addresses {
            let slot = if destination.is_ipv4() { &mut self.v4 } else { &mut self.v6 };
            if slot.is_none() {
                let bind = if destination.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
                let socket = match UdpSocket::bind(bind).await {
                    Ok(socket) => Arc::new(socket),
                    Err(error) => {
                        send_error = error.to_string();
                        continue;
                    }
                };
                let receive = socket.clone();
                let buffers = self.buffers.clone();
                let allowed = self.allowed.clone();
                let sender = self.sender.clone();
                let activity = self.activity.clone();
                self.readers.spawn(async move {
                    let mut processed=0u32;
                    loop {
                        let result=tokio::select! {packet=buffers.recv(&receive)=>packet,_=sender.closed()=>break};
                        let Ok((packet,source))=result else {break};
                        let known=allowed.lock().unwrap().get_mut(&source).is_some_and(|last|{if last.elapsed()<Duration::from_secs(120){*last=Instant::now();true}else{false}});
                        if !known{continue;}
                        *activity.lock().unwrap()=Instant::now();
                        let packet=match udp_protocol::encode_packet(source,&packet){Ok(packet)=>packet,Err(_)=>continue};
                        if sender.try_send_datagram(packet).is_err(){break;}
                        processed+=1;if processed%32==0{tokio::task::yield_now().await;}
                    }
                });
                *slot = Some(socket);
            }
            {
                let mut destinations = self.allowed.lock().unwrap();
                if !destinations.contains_key(&destination) && destinations.len() >= 256 {
                    destinations.retain(|_, last| last.elapsed() < Duration::from_secs(120));
                    if destinations.len() >= 256 {
                        send_error = "UDP destination capacity reached".into();
                        continue;
                    }
                }
                destinations.insert(destination, Instant::now());
            }
            match slot.as_ref().unwrap().send_to(payload, destination).await {
                Ok(_) => return,
                Err(error) => send_error = error.to_string(),
            }
        }
        self.sender.note_udp_failure(UdpFailure::Send, &send_error);
    }
}

async fn udp(flow: Flow, buffers: DatagramBuffers) -> secret_tunnel_mux::Result<()> {
    udp_with(flow, buffers, system_resolver()).await
}

/// Relay one association. Datagrams to an IP address go out at once; those to
/// a name wait for their own lookup in a bounded queue, so a slow resolver no
/// longer holds back unrelated datagrams of the association. Nothing queued
/// is sent once the flow has been closed or aborted.
pub(crate) async fn udp_with(
    mut flow: Flow,
    buffers: DatagramBuffers,
    resolver: Resolver,
) -> secret_tunnel_mux::Result<()> {
    flow.accept()?;
    let sender = flow.sender();
    let activity = Arc::new(Mutex::new(Instant::now()));
    let mut relay = Relay {
        sender: sender.clone(),
        buffers,
        allowed: Default::default(),
        activity: activity.clone(),
        v4: None,
        v6: None,
        readers: JoinSet::new(),
    };
    let mut cleanup = tokio::time::interval(Duration::from_secs(30));
    let mut cache: HashMap<(String, u16), (Vec<SocketAddr>, Instant)> = HashMap::new();
    let mut pending: HashMap<(String, u16), VecDeque<secret_tunnel_mux::Received>> = HashMap::new();
    let (mut pending_packets, mut pending_bytes) = (0usize, 0usize);
    let mut lookups: JoinSet<((String, u16), io::Result<Vec<SocketAddr>>)> = JoinSet::new();
    loop {
        tokio::select! {
            packet = flow.recv() => {
                let Ok(Some(packet)) = packet else { break };
                *activity.lock().unwrap() = Instant::now();
                let parsed = udp_protocol::parse_packet(&packet).map_err(|e| e.to_string())?;
                let key = match parsed.address {
                    PacketAddress::Socket(address) => {
                        relay.send(&[address], parsed.payload).await;
                        continue;
                    }
                    PacketAddress::Domain(host, port) => (host.to_owned(), port),
                };
                if let Some((addresses, _)) = cache.get(&key).filter(|(_, at)| at.elapsed() < DNS_CACHE) {
                    let addresses = addresses.clone();
                    relay.send(&addresses, parsed.payload).await;
                    continue;
                }
                let waiting = pending.contains_key(&key);
                if pending_packets >= DNS_PENDING_PACKETS
                    || pending_bytes + packet.len() > DNS_PENDING_BYTES
                    || (!waiting && lookups.len() >= DNS_PARALLEL)
                {
                    sender.note_udp_failure(UdpFailure::Dns, "UDP DNS: lookup queue full");
                    continue;
                }
                if !waiting {
                    let (host, port) = key.clone();
                    let lookup = resolver(host, port);
                    let name = key.clone();
                    lookups.spawn(async move { (name, lookup.await) });
                }
                pending_packets += 1;
                pending_bytes += packet.len();
                pending.entry(key).or_default().push_back(packet);
            }
            Some(done) = lookups.join_next(), if !lookups.is_empty() => {
                let Ok((key, result)) = done else { continue };
                let queued = pending.remove(&key).unwrap_or_default();
                pending_packets -= queued.len();
                pending_bytes -= queued.iter().map(|packet| packet.len()).sum::<usize>();
                match result {
                    Ok(addresses) => {
                        cache.insert(key, (addresses.clone(), Instant::now()));
                        for packet in queued {
                            // A delayed answer must not send after Abort/Close.
                            if sender.is_closed() {
                                break;
                            }
                            if let Ok(parsed) = udp_protocol::parse_packet(&packet) {
                                relay.send(&addresses, parsed.payload).await;
                            }
                        }
                    }
                    Err(error) => {
                        for _ in queued {
                            sender.note_udp_failure(UdpFailure::Dns, &format!("UDP DNS: {error}"));
                        }
                    }
                }
            }
            _ = cleanup.tick() => {
                cache.retain(|_, (_, at)| at.elapsed() < DNS_CACHE);
                if activity.lock().unwrap().elapsed() > Duration::from_secs(120) {
                    break;
                }
            }
        }
    }
    lookups.abort_all();
    relay.readers.abort_all();
    Ok(())
}

#[cfg(test)]
mod dns_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Agent side of one association with a resolver that answers `name`
    /// after `delay`, plus a loopback UDP echo that counts what reaches it.
    struct Bench {
        client: Endpoint,
        echo: SocketAddr,
        echoed: Arc<AtomicUsize>,
        lookups: Arc<AtomicUsize>,
    }

    async fn bench(delay: Duration) -> Bench {
        let echo = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let echo_address = echo.local_addr().unwrap();
        let echoed = Arc::new(AtomicUsize::new(0));
        let count = echoed.clone();
        tokio::spawn(async move {
            let mut buffer = [0u8; 2048];
            while let Ok((size, peer)) = echo.recv_from(&mut buffer).await {
                count.fetch_add(1, Ordering::SeqCst);
                let _ = echo.send_to(&buffer[..size], peer).await;
            }
        });
        let lookups = Arc::new(AtomicUsize::new(0));
        let counted = lookups.clone();
        let resolver: Resolver = Arc::new(move |_host, port| {
            counted.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))])
            })
        });
        let (a, b) = tokio::io::duplex(1 << 16);
        let config = Config { max_flows: 8, udp_window: Some(secret_tunnel_mux::UDP_WINDOW), ..Default::default() };
        let (client, server) = tokio::join!(
            Endpoint::start(a, Role::Client, config.clone()),
            Endpoint::start(b, Role::Server, config)
        );
        let server = server.unwrap();
        tokio::spawn(async move {
            while let Some(incoming) = server.accept().await {
                let resolver = resolver.clone();
                tokio::spawn(udp_with(incoming.flow, DatagramBuffers::default(), resolver));
            }
        });
        Bench { client: client.unwrap(), echo: echo_address, echoed, lookups }
    }

    fn to_name(name: &str, port: u16, payload: &[u8]) -> Vec<u8> {
        let mut packet = vec![0, 0, 0, 3, name.len() as u8];
        packet.extend_from_slice(name.as_bytes());
        packet.extend_from_slice(&port.to_be_bytes());
        packet.extend_from_slice(payload);
        packet
    }

    #[tokio::test]
    async fn a_slow_lookup_does_not_hold_back_datagrams_to_an_address() {
        let bench = bench(Duration::from_millis(1200)).await;
        let mut flow = bench.client.open(FlowKind::Udp, vec![]).await.unwrap();
        let sender = flow.sender();
        let started = Instant::now();
        assert!(sender.try_send_datagram(to_name("slow.test", bench.echo.port(), b"by name")).unwrap());
        assert!(sender
            .try_send_datagram(udp_protocol::encode_packet(bench.echo, b"by address").unwrap())
            .unwrap());
        let first = flow.recv().await.unwrap().unwrap();
        assert!(first.ends_with(b"by address"), "the address datagram came first");
        assert!(started.elapsed() < Duration::from_millis(300), "waited {:?}", started.elapsed());
        drop(first);
        // The named datagram follows after its own lookup, and a second one to
        // the same name reuses the cached answer instead of looking it up again.
        let second = flow.recv().await.unwrap().unwrap();
        assert!(second.ends_with(b"by name"));
        drop(second);
        assert!(sender.try_send_datagram(to_name("slow.test", bench.echo.port(), b"cached")).unwrap());
        let third = tokio::time::timeout(Duration::from_millis(300), flow.recv()).await.unwrap().unwrap().unwrap();
        assert!(third.ends_with(b"cached"));
        assert_eq!(bench.lookups.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_datagram_waiting_for_its_name_is_not_sent_after_the_flow_closes() {
        let bench = bench(Duration::from_millis(400)).await;
        let flow = bench.client.open(FlowKind::Udp, vec![]).await.unwrap();
        assert!(flow
            .sender()
            .try_send_datagram(to_name("slow.test", bench.echo.port(), b"late"))
            .unwrap());
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(flow); // Abort while the lookup is still running.
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(bench.lookups.load(Ordering::SeqCst), 1);
        assert_eq!(bench.echoed.load(Ordering::SeqCst), 0, "a datagram left after Abort");
    }

    #[tokio::test]
    async fn the_name_queue_is_bounded() {
        let bench = bench(Duration::from_secs(5)).await;
        let flow = bench.client.open(FlowKind::Udp, vec![]).await.unwrap();
        let sender = flow.sender();
        for index in 0..(DNS_PARALLEL + 2) {
            let name = format!("name{index}.test");
            assert!(sender.try_send_datagram(to_name(&name, bench.echo.port(), b"x")).unwrap());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(bench.lookups.load(Ordering::SeqCst), DNS_PARALLEL);
    }
}
