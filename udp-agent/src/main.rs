#[path = "../../src-tauri/src/udp_protocol.rs"]
mod udp_protocol;

use std::{
    collections::HashMap,
    io::{self, Write},
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use udp_protocol::{encode_packet, parse_packet, read_frame, PacketAddress, MAGIC, MAX_FRAME};

const MAX_FLOWS: usize = 1024;
const IDLE_SECONDS: u64 = 120;
struct Flow {
    socket: Arc<UdpSocket>,
    activity: Arc<AtomicU64>,
    task: JoinHandle<()>,
}
struct DnsEntry {
    ip: IpAddr,
    expires: Instant,
}

async fn resolve(
    address: PacketAddress<'_>,
    cache: &mut HashMap<String, DnsEntry>,
) -> io::Result<SocketAddr> {
    match address {
        PacketAddress::Socket(address) => Ok(address),
        PacketAddress::Domain(host, port) => {
            if let Some(entry) = cache
                .get(host)
                .filter(|entry| entry.expires > Instant::now())
            {
                return Ok(SocketAddr::new(entry.ip, port));
            }
            let address = tokio::time::timeout(
                Duration::from_secs(5),
                tokio::net::lookup_host((host, port)),
            )
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "UDP destination DNS timeout"))??
            .next()
            .ok_or_else(|| io::Error::other("No destination address"))?;
            if cache.len() >= 128 {
                if let Some(oldest) = cache
                    .iter()
                    .min_by_key(|(_, entry)| entry.expires)
                    .map(|(key, _)| key.clone())
                {
                    cache.remove(&oldest);
                }
            }
            cache.insert(
                host.to_owned(),
                DnsEntry {
                    ip: address.ip(),
                    expires: Instant::now() + Duration::from_secs(60),
                },
            );
            Ok(address)
        }
    }
}

fn expire_flows(flows: &mut HashMap<(u32, SocketAddr), Flow>, now: u64) {
    flows.retain(|_, flow| {
        let keep = now.saturating_sub(flow.activity.load(Ordering::Relaxed)) < IDLE_SECONDS
            && !flow.task.is_finished();
        if !keep {
            flow.task.abort();
        }
        keep
    });
}

fn write_packet(writer: &mut impl Write, packet: &[u8]) -> io::Result<()> {
    if packet.len() > MAX_FRAME + 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Oversized UDP response",
        ));
    }
    writer.write_all(&(packet.len() as u16).to_be_bytes())?;
    writer.write_all(packet)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let multiplex = arguments == ["--multiplex"];
    if !arguments.is_empty() && !multiplex {
        return Err(io::Error::other("Usage: secret-tunnel-agent [--multiplex]"));
    }
    let (responses, mut outgoing) = mpsc::channel::<Vec<u8>>(64);
    let (writer_error, mut writer_failed) = oneshot::channel();
    // Dedicated blocking stdout writer avoids a blocking-pool dispatch for every
    // tiny header/body write. Drain only already-queued responses: no batching delay.
    std::thread::Builder::new()
        .name("udp-ssh-writer".into())
        .spawn(move || {
            let result = (|| -> io::Result<()> {
                let stdout = io::stdout();
                let mut writer = io::BufWriter::with_capacity(128 * 1024, stdout.lock());
                writer.write_all(if multiplex { udp_protocol::MUX_MAGIC } else { MAGIC })?;
                writer.flush()?;
                while let Some(packet) = outgoing.blocking_recv() {
                    write_packet(&mut writer, &packet)?;
                    for _ in 1..16 {
                        match outgoing.try_recv() {
                            Ok(packet) => write_packet(&mut writer, &packet)?,
                            Err(_) => break,
                        }
                    }
                    writer.flush()?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                let _ = writer_error.send(error.to_string());
            }
        })?;
    let (requests, mut incoming) = mpsc::channel(16);
    let reader = tokio::spawn(async move {
        let mut input = tokio::io::stdin();
        loop {
            let packet = if multiplex { udp_protocol::read_mux_frame(&mut input).await }
                else { read_frame(&mut input).await.map(|packet| (0, packet)) };
            let failed = packet.is_err();
            if requests.send(packet).await.is_err() || failed {
                break;
            }
        }
    });
    let mut flows: HashMap<(u32, SocketAddr), Flow> = HashMap::new();
    let mut dns = HashMap::new();
    let started = Instant::now();
    let mut cleanup = tokio::time::interval(Duration::from_secs(30));
    let mut flow_drops = 0u64;
    let result = loop {
        tokio::select! {
            packet=incoming.recv()=>{
                let (association, packet)=match packet {
                    Some(Ok(packet))=>packet,
                    Some(Err(error)) if error.kind()==io::ErrorKind::UnexpectedEof=>break Ok(()),
                    Some(Err(error))=>break Err(error),
                    None=>break Err(io::Error::other("UDP request reader stopped")),
                };
                if packet.is_empty() {
                    flows.retain(|(id,_),flow| {
                        if *id==association {flow.task.abort();false} else {true}
                    });
                    continue;
                }
                let packet=parse_packet(&packet)?;
                let destination=match resolve(packet.address,&mut dns).await {
                    Ok(destination)=>destination,
                    Err(error)=>{eprintln!("UDP destination: {error}");continue;}
                };
                let key=(association,destination);
                let now=started.elapsed().as_secs();
                if flows.get(&key).is_some_and(|flow|flow.task.is_finished()) {flows.remove(&key);}
                if !flows.contains_key(&key) {
                    if flows.len()>=MAX_FLOWS {expire_flows(&mut flows,now);}
                    if flows.len()>=MAX_FLOWS {
                        flow_drops+=1;if flow_drops.is_power_of_two(){eprintln!("UDP flow limit reached; dropped {flow_drops} new-flow packets");}
                        continue;
                    }
                    let bind=if destination.is_ipv4(){"0.0.0.0:0"}else{"[::]:0"};
                    let socket=match UdpSocket::bind(bind).await {Ok(socket)=>socket,Err(error)=>{eprintln!("UDP bind: {error}");continue;}};
                    if let Err(error)=socket.connect(destination).await {eprintln!("UDP connect: {error}");continue;}
                    let socket=Arc::new(socket);
                    let activity=Arc::new(AtomicU64::new(now));
                    let task_socket=socket.clone();let task_activity=activity.clone();let responses=responses.clone();
                    let task=tokio::spawn(async move {
                        let mut buffer=vec![0;65535];
                        loop {
                            let count=match task_socket.recv(&mut buffer).await {Ok(count)=>count,Err(error)=>{eprintln!("UDP receive: {error}");break;}};
                            task_activity.store(started.elapsed().as_secs(),Ordering::Relaxed);
                            let packet=match encode_packet(destination,&buffer[..count]){Ok(packet)=>packet,Err(error)=>{eprintln!("UDP response: {error}");continue;}};
                            let packet=if multiplex {match udp_protocol::mux_packet(association,&packet){Ok(packet)=>packet,Err(error)=>{eprintln!("UDP multiplex response: {error}");continue;}}}else{packet};
                            if responses.send(packet).await.is_err(){break;}
                        }
                    });
                    flows.insert(key,Flow{socket,activity,task});
                }
                let flow=&flows[&key];
                flow.activity.store(now,Ordering::Relaxed);
                if let Err(error)=flow.socket.send(packet.payload).await {
                    eprintln!("UDP send: {error}");if let Some(flow)=flows.remove(&key){flow.task.abort();}
                }
            }
            error=&mut writer_failed=>break Err(io::Error::other(error.unwrap_or_else(|_|"UDP writer stopped".into()))),
            _=cleanup.tick()=>expire_flows(&mut flows,started.elapsed().as_secs()),
        }
    };
    reader.abort();
    for (_, flow) in flows {
        flow.task.abort();
    }
    result
}

#[cfg(test)]
mod agent_tests {
    use super::*;
    #[test]
    fn cached_dns_preserves_requested_port_and_literal_ips_bypass_cache() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut cache = HashMap::new();
                cache.insert(
                    "cached.invalid".into(),
                    DnsEntry {
                        ip: "127.0.0.1".parse().unwrap(),
                        expires: Instant::now() + Duration::from_secs(10),
                    },
                );
                assert_eq!(
                    resolve(PacketAddress::Domain("cached.invalid", 4567), &mut cache)
                        .await
                        .unwrap(),
                    "127.0.0.1:4567".parse().unwrap()
                );
                let address = "[::1]:1234".parse().unwrap();
                assert_eq!(
                    resolve(PacketAddress::Socket(address), &mut cache)
                        .await
                        .unwrap(),
                    address
                );
                assert_eq!(cache.len(), 1);
            });
    }
    #[test]
    fn idle_flows_are_reclaimed_without_interrupting_active_flows() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut flows = HashMap::new();
                for (port, activity) in [(1111, 0), (2222, 110)] {
                    flows.insert(
                        (0,SocketAddr::from(([127, 0, 0, 1], port))),
                        Flow {
                            socket: Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap()),
                            activity: Arc::new(AtomicU64::new(activity)),
                            task: tokio::spawn(std::future::pending()),
                        },
                    );
                }
                expire_flows(&mut flows, 125);
                assert_eq!(flows.len(), 1);
                assert!(flows.contains_key(&(0,"127.0.0.1:2222".parse().unwrap())));
                for (_, flow) in flows {
                    flow.task.abort();
                }
            });
    }
}
