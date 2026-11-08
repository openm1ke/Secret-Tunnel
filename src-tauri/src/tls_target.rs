//! Recover a TLS destination when a local fake-IP DNS resolver has replaced it.
//! Only the benchmarking range is eligible; ordinary IP destinations stay exact.
//! The original TLS records are forwarded byte-for-byte, without terminating TLS.
use std::net::IpAddr;
use tokio::io::{AsyncRead, AsyncReadExt};

const MAX_PREFACE: usize = 64 * 1024;
const MAX_RECORD: usize = 16 * 1024 + 2048;

pub fn is_fake_ip(host: &str) -> bool {
    matches!(host.parse::<IpAddr>(), Ok(IpAddr::V4(ip)) if ip.octets()[0] == 198 && matches!(ip.octets()[1], 18 | 19))
}

fn take<'a>(data: &mut &'a [u8], length: usize) -> Result<&'a [u8], String> {
    if length > data.len() {
        return Err("Truncated TLS ClientHello".into());
    }
    let (value, rest) = data.split_at(length);
    *data = rest;
    Ok(value)
}
fn u16_value(data: &mut &[u8]) -> Result<usize, String> {
    let bytes = take(data, 2)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]) as usize)
}

fn server_name(mut hello: &[u8]) -> Result<String, String> {
    take(&mut hello, 34)?; // legacy_version + random
    let session = take(&mut hello, 1)?[0] as usize;
    take(&mut hello, session)?;
    let ciphers = u16_value(&mut hello)?;
    if ciphers == 0 || ciphers % 2 != 0 {
        return Err("Invalid TLS cipher list".into());
    }
    take(&mut hello, ciphers)?;
    let compression = take(&mut hello, 1)?[0] as usize;
    take(&mut hello, compression)?;
    let extension_length = u16_value(&mut hello)?;
    let mut extensions = take(&mut hello, extension_length)?;
    let mut result = None;
    while !extensions.is_empty() {
        let kind = u16_value(&mut extensions)?;
        let length = u16_value(&mut extensions)?;
        let mut extension = take(&mut extensions, length)?;
        if kind != 0 {
            continue;
        }
        let names_length = u16_value(&mut extension)?;
        let mut names = take(&mut extension, names_length)?;
        if !extension.is_empty() {
            return Err("Invalid TLS server-name extension".into());
        }
        while !names.is_empty() {
            let name_type = take(&mut names, 1)?[0];
            let length = u16_value(&mut names)?;
            let name = take(&mut names, length)?;
            if name_type != 0 {
                continue;
            }
            if result.is_some() {
                return Err("Ambiguous TLS server name".into());
            }
            let host = std::str::from_utf8(name).map_err(|_| "Invalid TLS server name")?;
            if host.is_empty()
                || host.len() > 253
                || host.parse::<IpAddr>().is_ok()
                || host.split('.').any(|label| {
                    label.is_empty()
                        || label.len() > 63
                        || label.starts_with('-')
                        || label.ends_with('-')
                        || !label
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
            {
                return Err("Invalid TLS server name".into());
            }
            result = Some(host.to_ascii_lowercase());
        }
    }
    result.ok_or_else(|| "Local fake-IP destination has no visible TLS server name; use real DNS or a hostname-aware proxy".into())
}

pub async fn read_server_name<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<(String, Vec<u8>), String> {
    let mut original = Vec::new();
    let mut handshake = Vec::new();
    for _ in 0..16 {
        let mut header = [0u8; 5];
        reader
            .read_exact(&mut header)
            .await
            .map_err(|e| format!("Reading fake-IP TLS ClientHello: {e}"))?;
        if header[0] != 22 || header[1] != 3 {
            return Err("Local fake-IP destination requires TLS SNI; this connection needs real DNS or a hostname-aware proxy".into());
        }
        let length = u16::from_be_bytes([header[3], header[4]]) as usize;
        if length == 0 || length > MAX_RECORD || original.len() + 5 + length > MAX_PREFACE {
            return Err("Oversized fake-IP TLS ClientHello".into());
        }
        original.extend_from_slice(&header);
        let offset = original.len();
        original.resize(offset + length, 0);
        reader
            .read_exact(&mut original[offset..])
            .await
            .map_err(|e| format!("Reading fake-IP TLS ClientHello: {e}"))?;
        handshake.extend_from_slice(&original[offset..]);
        if handshake.len() >= 4 {
            if handshake[0] != 1 {
                return Err("Expected TLS ClientHello for fake-IP destination".into());
            }
            let hello_length = ((handshake[1] as usize) << 16)
                | ((handshake[2] as usize) << 8)
                | handshake[3] as usize;
            if hello_length + 4 > MAX_PREFACE {
                return Err("Oversized fake-IP TLS handshake".into());
            }
            if handshake.len() >= hello_length + 4 {
                let host = server_name(&handshake[4..4 + hello_length])?;
                return Ok((host, original));
            }
        }
    }
    Err("Too many TLS records in fake-IP ClientHello".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    fn hello(host: &str) -> Vec<u8> {
        let mut names = vec![0];
        names.extend_from_slice(&(host.len() as u16).to_be_bytes());
        names.extend_from_slice(host.as_bytes());
        let mut extension = (names.len() as u16).to_be_bytes().to_vec();
        extension.extend(names);
        let mut extensions = vec![0, 0];
        extensions.extend_from_slice(&(extension.len() as u16).to_be_bytes());
        extensions.extend(extension);
        let mut body = vec![3, 3];
        body.extend([7; 32]);
        body.extend([0, 0, 2, 0x13, 1, 1, 0]);
        body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
        body.extend(extensions);
        let mut handshake = vec![1, 0, (body.len() >> 8) as u8, body.len() as u8];
        handshake.extend(body);
        handshake
    }
    fn record(data: &[u8]) -> Vec<u8> {
        let mut result = vec![22, 3, 1];
        result.extend_from_slice(&(data.len() as u16).to_be_bytes());
        result.extend_from_slice(data);
        result
    }
    #[test]
    fn limits_recovery_to_benchmark_ips() {
        for ip in ["198.18.0.1", "198.19.255.254"] {
            assert!(is_fake_ip(ip));
        }
        for host in [
            "1.1.1.1",
            "127.0.0.1",
            "192.168.1.1",
            "198.20.0.1",
            "::1",
            "discord.com",
        ] {
            assert!(!is_fake_ip(host));
        }
    }
    #[tokio::test]
    async fn fragmented_tls_records_and_socket_reads_preserve_every_byte() {
        let message = hello("Updates.Discord.com");
        let mut bytes = record(&message[..3]);
        bytes.extend(record(&message[3..29]));
        bytes.extend(record(&message[29..]));
        let expected = bytes.clone();
        let (mut writer, mut reader) = tokio::io::duplex(32);
        tokio::spawn(async move {
            for part in bytes.chunks(7) {
                writer.write_all(part).await.unwrap();
            }
            writer.write_all(b"tail").await.unwrap();
        });
        let (host, original) = read_server_name(&mut reader).await.unwrap();
        assert_eq!(host, "updates.discord.com");
        assert_eq!(original, expected);
        let mut tail = [0; 4];
        reader.read_exact(&mut tail).await.unwrap();
        assert_eq!(&tail, b"tail");
    }
    #[tokio::test]
    async fn malformed_and_truncated_input_fails_without_panics() {
        for bytes in [
            b"GET / HTTP/1.1\r\n".to_vec(),
            vec![22, 3, 1, 255, 255],
            record(&[2, 0, 0, 0]),
            record(&[1, 0, 0, 2, 0]),
            record(&hello("bad/name")),
        ] {
            assert!(read_server_name(&mut bytes.as_slice()).await.is_err());
        }
        let valid = hello("discord.com");
        for length in 0..valid.len() {
            assert!(server_name(&valid[4..length.max(4)]).is_err());
        }
    }
}
