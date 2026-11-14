//! Small, independent SOCKS probes. These exercise the proxy, not OS interception.
use crate::udp_protocol::{self, PacketAddress};
use serde::Serialize;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TIMEOUT: Duration = Duration::from_secs(3);
const TCP4: SocketAddr = SocketAddr::new(
    std::net::IpAddr::V4(std::net::Ipv4Addr::new(1, 1, 1, 1)),
    443,
);
const TCP6: SocketAddr = SocketAddr::new(
    std::net::IpAddr::V6(std::net::Ipv6Addr::new(
        0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111,
    )),
    443,
);
const DNS: SocketAddr = SocketAddr::new(TCP4.ip(), 53);
const QUESTION: &[u8] = b"\x0acloudflare\x03com\x00\x00\x01\x00\x01";

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Outcome {
    Passed { ms: u64 },
    Failed { detail: String },
    Unsupported,
}

impl Outcome {
    fn from_result(started: Instant, result: io::Result<()>) -> Self {
        match result {
            Ok(()) => Self::Passed {
                ms: started.elapsed().as_millis() as u64,
            },
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                Self::Failed {
                    detail: "Нет ответа за 3 с".into(),
                }
            }
            Err(error) => Self::Failed {
                detail: error.to_string(),
            },
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct Report {
    tcp: Outcome,
    udp: Outcome,
    dns: Outcome,
    ipv6: Outcome,
    checked_at: u64,
}

#[derive(Default, Clone, Debug, Serialize)]
pub struct Snapshot {
    running: bool,
    report: Option<Report>,
}

#[derive(Default)]
pub struct Checks(Mutex<Snapshot>);

impl Checks {
    pub fn snapshot(&self) -> Snapshot {
        self.0.lock().unwrap().clone()
    }

    pub fn start(self: &Arc<Self>, proxy: SocketAddr, supports_udp: bool) -> io::Result<bool> {
        if !proxy.ip().is_loopback() {
            return Err(invalid("Проверка доступна только для локального прокси"));
        }
        let mut snapshot = self.0.lock().unwrap();
        if snapshot.running {
            return Ok(false);
        }
        let checks = Arc::clone(self);
        let worker = std::thread::Builder::new().name("connection-checks".into());
        worker.spawn(move || {
            let (tcp, ipv6, (udp, dns)) = std::thread::scope(|scope| {
                let tcp = scope.spawn(|| check_tcp(proxy, TCP4));
                let ipv6 = scope.spawn(|| check_tcp(proxy, TCP6));
                let datagrams = if supports_udp {
                    check_dns(proxy, DNS)
                } else {
                    (Outcome::Unsupported, Outcome::Unsupported)
                };
                (tcp.join().unwrap(), ipv6.join().unwrap(), datagrams)
            });
            *checks.0.lock().unwrap() = Snapshot {
                running: false,
                report: Some(Report {
                    tcp,
                    udp,
                    dns,
                    ipv6,
                    checked_at: epoch().as_secs(),
                }),
            };
        })?;
        *snapshot = Snapshot {
            running: true,
            report: None,
        };
        Ok(true)
    }
}

fn epoch() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|time| !time.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Истекло время проверки"))
}

fn read_exact(stream: &mut TcpStream, mut data: &mut [u8], deadline: Instant) -> io::Result<()> {
    while !data.is_empty() {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        let count = stream.read(data)?;
        if count == 0 {
            return Err(invalid("Локальный прокси закрыл соединение"));
        }
        data = &mut data[count..];
    }
    Ok(())
}

fn socks_request(
    proxy: SocketAddr,
    command: u8,
    target: SocketAddr,
    deadline: Instant,
) -> io::Result<(TcpStream, SocketAddr)> {
    let mut stream = TcpStream::connect_timeout(&proxy, remaining(deadline)?)?;
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    stream.write_all(&[5, 1, 0])?;
    let mut method = [0; 2];
    read_exact(&mut stream, &mut method, deadline)?;
    if method != [5, 0] {
        return Err(invalid("Локальный прокси отклонил SOCKS5"));
    }
    let mut request = udp_protocol::encode_packet(target, &[])?;
    request[..3].copy_from_slice(&[5, command, 0]);
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    stream.write_all(&request)?;
    let mut head = [0; 4];
    read_exact(&mut stream, &mut head, deadline)?;
    if head[0] != 5 || head[2] != 0 {
        return Err(invalid("Некорректный ответ SOCKS5"));
    }
    if head[1] != 0 {
        return Err(io::Error::other(format!(
            "Прокси отклонил запрос (код SOCKS5 {})",
            head[1]
        )));
    }
    let mut ip = proxy.ip();
    match head[3] {
        1 => {
            let mut bytes = [0; 4];
            read_exact(&mut stream, &mut bytes, deadline)?;
            ip = bytes.into();
        }
        4 => {
            let mut bytes = [0; 16];
            read_exact(&mut stream, &mut bytes, deadline)?;
            ip = bytes.into();
        }
        3 if command == 1 => {
            let mut length = [0; 1];
            read_exact(&mut stream, &mut length, deadline)?;
            read_exact(&mut stream, &mut [0; 255][..length[0] as usize], deadline)?;
        }
        _ => return Err(invalid("Неожиданный адрес в ответе SOCKS5")),
    }
    let mut port = [0; 2];
    read_exact(&mut stream, &mut port, deadline)?;
    Ok((
        stream,
        SocketAddr::new(
            if ip.is_unspecified() { proxy.ip() } else { ip },
            u16::from_be_bytes(port),
        ),
    ))
}

fn check_tcp(proxy: SocketAddr, target: SocketAddr) -> Outcome {
    let started = Instant::now();
    Outcome::from_result(
        started,
        socks_request(proxy, 1, target, started + TIMEOUT).map(drop),
    )
}

fn check_dns(proxy: SocketAddr, resolver: SocketAddr) -> (Outcome, Outcome) {
    let started = Instant::now();
    match dns_reply(proxy, resolver, started + TIMEOUT) {
        Ok(has_answer) => {
            let udp = Outcome::from_result(started, Ok(()));
            let dns = if has_answer {
                udp.clone()
            } else {
                Outcome::Failed {
                    detail: "Резолвер ответил, но не вернул IPv4-адрес".into(),
                }
            };
            (udp, dns)
        }
        Err(error) => {
            let failure = Outcome::from_result(started, Err(error));
            (failure.clone(), failure)
        }
    }
}

fn dns_reply(proxy: SocketAddr, resolver: SocketAddr, deadline: Instant) -> io::Result<bool> {
    let unspecified = SocketAddr::new(
        if proxy.is_ipv4() {
            "0.0.0.0".parse().unwrap()
        } else {
            "::".parse().unwrap()
        },
        0,
    );
    let (_control, relay) = socks_request(proxy, 3, unspecified, deadline)?;
    if !relay.ip().is_loopback() || relay.port() == 0 {
        return Err(invalid("Прокси вернул некорректный адрес UDP"));
    }
    let socket = UdpSocket::bind(SocketAddr::new(relay.ip(), 0))?;
    socket.connect(relay)?;
    let id = (epoch().subsec_nanos() as u16).to_be_bytes();
    let mut query = Vec::from(id);
    query.extend_from_slice(&[1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    query.extend_from_slice(QUESTION);
    socket.set_write_timeout(Some(remaining(deadline)?))?;
    socket.send(&udp_protocol::encode_packet(resolver, &query)?)?;
    let mut buffer = [0; 4096];
    loop {
        socket.set_read_timeout(Some(remaining(deadline)?))?;
        let size = socket.recv(&mut buffer)?;
        let Ok(packet) = udp_protocol::parse_packet(&buffer[..size]) else {
            continue;
        };
        let reply = packet.payload;
        if packet.address != PacketAddress::Socket(resolver)
            || reply.get(..2) != Some(&id)
            || reply.get(2).is_none_or(|flags| flags & 0xf8 != 0x80)
            || reply.get(4..6) != Some(&[0, 1])
            || !reply
                .get(12..12 + QUESTION.len())
                .is_some_and(|question| question.eq_ignore_ascii_case(QUESTION))
        {
            continue;
        }
        return Ok(reply[2] & 2 == 0 && reply[3] & 15 == 0 && has_ipv4_answer(reply));
    }
}

// Skip each answer's name (uncompressed or a compression pointer). The probe
// asks an A-only question and does not need to resolve the pointer itself.
fn has_ipv4_answer(reply: &[u8]) -> bool {
    let mut offset = 12 + QUESTION.len();
    for _ in 0..u16::from_be_bytes([reply[6], reply[7]]) {
        loop {
            match reply.get(offset).copied() {
                Some(0) => {
                    offset += 1;
                    break;
                }
                Some(byte) if byte & 0xc0 == 0xc0 => {
                    offset += 2;
                    break;
                }
                Some(byte) if byte < 64 => offset += 1 + byte as usize,
                _ => return false,
            }
        }
        let Some(record) = reply.get(offset..offset + 10) else {
            return false;
        };
        let size = u16::from_be_bytes([record[8], record[9]]) as usize;
        offset += 10;
        if offset + size > reply.len() {
            return false;
        }
        if record[..4] == [0, 1, 0, 1] && size == 4 {
            return true;
        }
        offset += size;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn udp_delivery_does_not_mean_dns_resolution_succeeded() {
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = proxy.local_addr().unwrap();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let (mut control, _) = proxy.accept().unwrap();
                control.set_read_timeout(Some(TIMEOUT)).unwrap();
                let mut greeting = [0; 3];
                control.read_exact(&mut greeting).unwrap();
                assert_eq!(greeting, [5, 1, 0]);
                control.write_all(&[5, 0]).unwrap();
                let mut request = [0; 10];
                control.read_exact(&mut request).unwrap();
                assert_eq!(request, [5, 3, 0, 1, 0, 0, 0, 0, 0, 0]);
                let relay = UdpSocket::bind("127.0.0.1:0").unwrap();
                relay.set_read_timeout(Some(TIMEOUT)).unwrap();
                let mut response =
                    udp_protocol::encode_packet(relay.local_addr().unwrap(), &[]).unwrap();
                response[0] = 5;
                control.write_all(&response).unwrap();
                let mut query = [0; 512];
                let (size, sender) = relay.recv_from(&mut query).unwrap();
                // A legitimate NXDOMAIN traverses UDP, but provides no address.
                query[12] = 0x81;
                query[13] = 0x83;
                query[10] ^= 1;
                relay.send_to(&query[..size], sender).unwrap(); // Ignore a different ID.
                query[10] ^= 1;
                relay.send_to(&query[..size], sender).unwrap();
            });
            let (udp, dns) = check_dns(addr, DNS);
            assert!(matches!(udp, Outcome::Passed { .. }));
            assert!(matches!(dns, Outcome::Failed { .. }));
        });
    }

    #[test]
    fn a_truncated_dns_answer_cannot_pass() {
        let mut reply = vec![0, 0, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0];
        reply.extend_from_slice(QUESTION);
        reply.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 0, 0, 4, 1, 2, 3]);
        assert!(!has_ipv4_answer(&reply));
        reply.push(4);
        assert!(has_ipv4_answer(&reply));
    }
}
