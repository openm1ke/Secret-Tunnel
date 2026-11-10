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

pub fn run(mode: Mode) -> io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let result = runtime.block_on(serve(mode));
    // Stdin reads run on a blocking worker. An expired handshake can leave one
    // waiting for input, so this short-lived agent exits without joining it.
    runtime.shutdown_background();
    result
}

async fn serve(mode: Mode) -> io::Result<()> {
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
async fn udp(mut flow: Flow, buffers: DatagramBuffers) -> secret_tunnel_mux::Result<()> {
    flow.accept()?;
    let sender = flow.sender();
    let allowed: Allowed = Default::default();
    let activity = Arc::new(Mutex::new(Instant::now()));
    let mut cleanup = tokio::time::interval(Duration::from_secs(30));
    let mut v4 = None;
    let mut v6 = None;
    let mut readers = JoinSet::new();
    loop {
        let packet = tokio::select! {
            packet=flow.recv()=>match packet{Ok(Some(packet))=>packet,_=>break},
            _=cleanup.tick()=>{if activity.lock().unwrap().elapsed()>Duration::from_secs(120){break;}continue;}
        };
        *activity.lock().unwrap() = Instant::now();
        let parsed = udp_protocol::parse_packet(&packet).map_err(|e| e.to_string())?;
        let addresses = match resolve(parsed.address).await {
            Ok(addresses) => addresses,
            Err(error) => {
                sender.note_udp_failure(UdpFailure::Dns, &format!("UDP DNS: {error}"));
                continue;
            }
        };
        let mut delivered = false;
        let mut send_error = String::from("No UDP destination address");
        for destination in addresses {
            let slot = if destination.is_ipv4() {
                &mut v4
            } else {
                &mut v6
            };
            if slot.is_none() {
                let bind = if destination.is_ipv4() {
                    "0.0.0.0:0"
                } else {
                    "[::]:0"
                };
                let socket = match UdpSocket::bind(bind).await {
                    Ok(socket) => Arc::new(socket),
                    Err(error) => {
                        send_error = error.to_string();
                        continue;
                    }
                };
                let receive = socket.clone();
                let buffers = buffers.clone();
                let allowed = allowed.clone();
                let sender = sender.clone();
                let activity = activity.clone();
                readers.spawn(async move {
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
                let mut destinations = allowed.lock().unwrap();
                if !destinations.contains_key(&destination) && destinations.len() >= 256 {
                    destinations.retain(|_, last| last.elapsed() < Duration::from_secs(120));
                    if destinations.len() >= 256 {
                        send_error = "UDP destination capacity reached".into();
                        continue;
                    }
                }
                destinations.insert(destination, Instant::now());
            }
            match slot
                .as_ref()
                .unwrap()
                .send_to(parsed.payload, destination)
                .await
            {
                Ok(_) => {
                    delivered = true;
                    break;
                }
                Err(error) => send_error = error.to_string(),
            }
        }
        if !delivered {
            sender.note_udp_failure(UdpFailure::Send, &send_error);
        }
        drop(packet);
    }
    readers.abort_all();
    Ok(())
}
