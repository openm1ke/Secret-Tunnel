//! SOCKS5 UDP packet addresses and payloads used inside STMUX003 flows.
use std::{
    io,
    net::{IpAddr, SocketAddr},
};
pub const MAX_FRAME: usize = 65507;

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid or unsupported UDP frame",
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketAddress<'a> {
    Socket(SocketAddr),
    Domain(&'a str, u16),
}

pub struct ParsedPacket<'a> {
    #[allow(dead_code)]
    // The desktop relay needs only payload length; the server uses the destination.
    pub address: PacketAddress<'a>,
    pub payload: &'a [u8],
}

pub fn parse_packet(packet: &[u8]) -> io::Result<ParsedPacket<'_>> {
    if packet.len() > MAX_FRAME || packet.get(..3) != Some(&[0, 0, 0]) {
        return Err(invalid());
    }
    let (ip, domain, offset) = match packet.get(3) {
        Some(1) if packet.len() >= 10 => (
            Some(IpAddr::V4(
                [packet[4], packet[5], packet[6], packet[7]].into(),
            )),
            None,
            8,
        ),
        Some(4) if packet.len() >= 22 => {
            let octets: [u8; 16] = packet[4..20].try_into().map_err(|_| invalid())?;
            (Some(IpAddr::V6(octets.into())), None, 20)
        }
        Some(3) if packet.len() >= 7 => {
            let length = packet[4] as usize;
            if length == 0 || packet.len() < 7 + length {
                return Err(invalid());
            }
            (
                None,
                Some(std::str::from_utf8(&packet[5..5 + length]).map_err(|_| invalid())?),
                5 + length,
            )
        }
        _ => return Err(invalid()),
    };
    let port = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
    if port == 0 {
        return Err(invalid());
    }
    let address = match ip {
        Some(ip) => PacketAddress::Socket(SocketAddr::new(ip, port)),
        None => PacketAddress::Domain(domain.ok_or_else(invalid)?, port),
    };
    Ok(ParsedPacket {
        address,
        payload: &packet[offset + 2..],
    })
}

#[cfg(test)]
pub fn decode_packet(packet: &[u8]) -> io::Result<(String, u16, &[u8])> {
    let parsed = parse_packet(packet)?;
    let (host, port) = match parsed.address {
        PacketAddress::Socket(address) => (address.ip().to_string(), address.port()),
        PacketAddress::Domain(host, port) => (host.to_owned(), port),
    };
    Ok((host, port, parsed.payload))
}

pub fn encode_packet(address: SocketAddr, payload: &[u8]) -> io::Result<Vec<u8>> {
    let mut packet = vec![0, 0, 0];
    match address.ip() {
        IpAddr::V4(ip) => {
            packet.push(1);
            packet.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            packet.push(4);
            packet.extend_from_slice(&ip.octets());
        }
    }
    packet.extend_from_slice(&address.port().to_be_bytes());
    packet.extend_from_slice(payload);
    if packet.len() > MAX_FRAME {
        return Err(invalid());
    }
    Ok(packet)
}

#[allow(dead_code)] // Client target builder; server parses the same representation.
pub fn encode_target(host: &str, port: u16) -> io::Result<Vec<u8>> {
    if port == 0 {
        return Err(invalid());
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return encode_packet(SocketAddr::new(ip, port), &[]);
    }
    if host.is_empty() || host.len() > 255 {
        return Err(invalid());
    }
    let mut packet = vec![0, 0, 0, 3, host.len() as u8];
    packet.extend_from_slice(host.as_bytes());
    packet.extend_from_slice(&port.to_be_bytes());
    Ok(packet)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn domain_and_ip_parser_preserve_payload() {
        let packet = b"\0\0\0\x03\x0bexample.com\x01\xbbvoice";
        let parsed = parse_packet(packet).unwrap();
        assert_eq!(parsed.address, PacketAddress::Domain("example.com", 443));
        assert_eq!(parsed.payload, b"voice");
        let packet = encode_packet("[::1]:53".parse().unwrap(), b"dns").unwrap();
        assert_eq!(
            parse_packet(&packet).unwrap().address,
            PacketAddress::Socket("[::1]:53".parse().unwrap())
        );
    }

    #[test]
    fn packet_roundtrip() {
        for address in ["127.0.0.1:1234", "[::1]:4321"] {
            let address: SocketAddr = address.parse().unwrap();
            for payload in [b"".as_slice(), b"voice"] {
                let packet = encode_packet(address, payload).unwrap();
                let (host, port, data) = decode_packet(&packet).unwrap();
                assert_eq!(host, address.ip().to_string());
                assert_eq!(port, address.port());
                assert_eq!(data, payload);
            }
        }
    }
    #[test]
    fn rejects_fragmentation_and_truncation() {
        let packet = encode_packet("127.0.0.1:1234".parse().unwrap(), b"x").unwrap();
        for size in 0..10 {
            assert!(decode_packet(&packet[..size]).is_err());
        }
        let mut fragmented = packet;
        fragmented[2] = 1;
        assert!(decode_packet(&fragmented).is_err());
        assert!(encode_packet("[::1]:1234".parse().unwrap(), &vec![0; MAX_FRAME]).is_err());
    }
}
