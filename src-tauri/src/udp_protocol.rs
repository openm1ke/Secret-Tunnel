//! Versioned UDP framing shared by the desktop client and standalone agent.
use std::{
    io,
    net::{IpAddr, SocketAddr},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAGIC: &[u8; 8] = b"STUDP001";
pub const MUX_MAGIC: &[u8; 8] = b"STUDP002";
pub const MAX_FRAME: usize = 65507;

/// A four-byte association ID preserves independent UDP source ports even when
/// different applications contact the same destination. An empty packet closes
/// every server-side flow belonging to that association.
pub fn mux_packet(association: u32, packet: &[u8]) -> io::Result<Vec<u8>> {
    if association == 0 {
        return Err(invalid());
    }
    if !packet.is_empty() {
        parse_packet(packet)?;
    }
    let mut frame = Vec::with_capacity(4 + packet.len());
    frame.extend_from_slice(&association.to_be_bytes());
    frame.extend_from_slice(packet);
    Ok(frame)
}

pub async fn read_mux_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<(u32, Vec<u8>)> {
    let high = reader.read_u8().await?;
    let low = reader.read_u8().await.map_err(frame_read_error)?;
    let length = u16::from_be_bytes([high, low]) as usize;
    if !(4..=MAX_FRAME + 4).contains(&length) {
        return Err(invalid());
    }
    let mut frame = vec![0; length];
    reader
        .read_exact(&mut frame)
        .await
        .map_err(frame_read_error)?;
    let association = u32::from_be_bytes(frame[..4].try_into().map_err(|_| invalid())?);
    if association == 0 {
        return Err(invalid());
    }
    frame.drain(..4);
    if !frame.is_empty() {
        parse_packet(&frame)?;
    }
    Ok((association, frame))
}

#[allow(dead_code)] // Client writer; the agent uses its blocking stdout writer.
pub async fn write_mux_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    association: u32,
    packet: &[u8],
) -> io::Result<()> {
    if association == 0 {
        return Err(invalid());
    }
    if !packet.is_empty() {
        parse_packet(packet)?;
    }
    let mut frame = Vec::with_capacity(packet.len() + 6);
    frame.extend_from_slice(&((packet.len() + 4) as u16).to_be_bytes());
    frame.extend_from_slice(&association.to_be_bytes());
    frame.extend_from_slice(packet);
    writer.write_all(&frame).await?;
    writer.flush().await
}

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

pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    // EOF before a new frame is clean; EOF inside a frame is malformed input.
    let high = reader.read_u8().await?;
    let low = reader.read_u8().await.map_err(frame_read_error)?;
    let length = u16::from_be_bytes([high, low]) as usize;
    if !(7..=MAX_FRAME).contains(&length) {
        return Err(invalid());
    }
    let mut packet = vec![0; length];
    reader
        .read_exact(&mut packet)
        .await
        .map_err(frame_read_error)?;
    parse_packet(&packet)?;
    Ok(packet)
}

fn frame_read_error(error: io::Error) -> io::Error {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        io::Error::new(io::ErrorKind::InvalidData, "Truncated UDP frame")
    } else {
        error
    }
}

#[allow(dead_code)] // Used by the desktop client; the agent has a blocking stdout writer.
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, packet: &[u8]) -> io::Result<()> {
    parse_packet(packet)?;
    // One channel write avoids a separate encrypted SSH message for a 2-byte header.
    let mut frame = Vec::with_capacity(packet.len() + 2);
    frame.extend_from_slice(&(packet.len() as u16).to_be_bytes());
    frame.extend_from_slice(packet);
    writer.write_all(&frame).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplex_frames_preserve_associations_and_close_messages() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let packet = encode_packet("127.0.0.1:53".parse().unwrap(), b"query").unwrap();
                let (mut sender, mut reader) = tokio::io::duplex(3);
                let expected = packet.clone();
                let writer = tokio::spawn(async move {
                    write_mux_frame(&mut sender, 7, &packet).await.unwrap();
                    write_mux_frame(&mut sender, 8, &packet).await.unwrap();
                    write_mux_frame(&mut sender, 7, &[]).await.unwrap();
                });
                assert_eq!(
                    read_mux_frame(&mut reader).await.unwrap(),
                    (7, expected.clone())
                );
                assert_eq!(read_mux_frame(&mut reader).await.unwrap(), (8, expected));
                assert_eq!(read_mux_frame(&mut reader).await.unwrap(), (7, vec![]));
                writer.await.unwrap();
                assert!(mux_packet(0, &[]).is_err());
                assert!(read_mux_frame(&mut &[0, 4, 0, 0, 0, 0][..]).await.is_err());
                assert_eq!(
                    read_mux_frame(&mut &[0, 5, 0, 0][..])
                        .await
                        .unwrap_err()
                        .kind(),
                    io::ErrorKind::InvalidData
                );
            });
    }

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
    fn truncated_frames_are_not_clean_eof() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                for input in [vec![0], vec![0, 10, 0, 0, 0, 1]] {
                    let mut reader = input.as_slice();
                    assert_eq!(
                        read_frame(&mut reader).await.unwrap_err().kind(),
                        io::ErrorKind::InvalidData
                    );
                }
                let mut empty = &[][..];
                assert_eq!(
                    read_frame(&mut empty).await.unwrap_err().kind(),
                    io::ErrorKind::UnexpectedEof
                );
            });
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

    #[test]
    fn fragmented_stream_preserves_datagrams() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (mut sender, mut receiver) = tokio::io::duplex(3);
                let packet = encode_packet("127.0.0.1:1234".parse().unwrap(), b"voice").unwrap();
                let expected = packet.clone();
                let write = tokio::spawn(async move {
                    for _ in 0..3 {
                        write_frame(&mut sender, &packet).await.unwrap();
                    }
                });
                for _ in 0..3 {
                    assert_eq!(read_frame(&mut receiver).await.unwrap(), expected);
                }
                write.await.unwrap();
                assert!(read_frame(&mut receiver).await.is_err());
            });
    }
}
