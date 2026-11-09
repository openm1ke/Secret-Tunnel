use crate::{FlowKind, FlowWindow};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAGIC: &[u8; 8] = b"STMUX003";
pub const MAX_PAYLOAD: usize = 65507;
pub const TCP_CHUNK: usize = 16384;
pub const HEADER: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    OpenTcp = 1,
    OpenUdp = 2,
    Accepted = 3,
    Rejected = 4,
    Data = 5,
    Eof = 6,
    Close = 7,
    Credit = 8,
    Abort = 9,
    Stats = 10,
    ProbeTcp = 11,
}
impl TryFrom<u8> for Kind {
    type Error = io::Error;
    fn try_from(value: u8) -> io::Result<Self> {
        Ok(match value {
            1 => Self::OpenTcp,
            2 => Self::OpenUdp,
            3 => Self::Accepted,
            4 => Self::Rejected,
            5 => Self::Data,
            6 => Self::Eof,
            7 => Self::Close,
            8 => Self::Credit,
            9 => Self::Abort,
            10 => Self::Stats,
            11 => Self::ProbeTcp,
            _ => return Err(invalid("Unknown frame type")),
        })
    }
}

#[derive(Debug)]
pub struct Frame {
    pub kind: Kind,
    pub id: u32,
    pub data: Vec<u8>,
}

pub fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

impl Frame {
    pub fn encode(&self, output: &mut Vec<u8>) -> io::Result<()> {
        if (self.id == 0) != (self.kind == Kind::Stats) || self.data.len() > MAX_PAYLOAD {
            return Err(invalid("Invalid frame size or ID"));
        }
        output.extend_from_slice(&((self.data.len() + 5) as u16).to_be_bytes());
        output.push(self.kind as u8);
        output.extend_from_slice(&self.id.to_be_bytes());
        output.extend_from_slice(&self.data);
        Ok(())
    }
}

pub async fn read<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Frame> {
    let mut header = [0; HEADER];
    reader.read_exact(&mut header[..1]).await?;
    reader
        .read_exact(&mut header[1..])
        .await
        .map_err(|_| invalid("Truncated frame header"))?;
    let length = u16::from_be_bytes([header[0], header[1]]) as usize;
    if !(5..=MAX_PAYLOAD + 5).contains(&length) {
        return Err(invalid("Invalid frame length"));
    }
    let kind = Kind::try_from(header[2])?;
    let id = u32::from_be_bytes(header[3..7].try_into().unwrap());
    if (id == 0) != (kind == Kind::Stats) {
        return Err(invalid("Reserved flow ID"));
    }
    let mut data = vec![0; length - 5];
    reader
        .read_exact(&mut data)
        .await
        .map_err(|_| invalid("Truncated frame payload"))?;
    Ok(Frame { kind, id, data })
}

#[derive(Debug)]
pub(crate) struct Limits {
    pub(crate) flows: usize,
    pub(crate) tcp: FlowWindow,
    pub(crate) udp: Option<FlowWindow>,
}

impl Limits {
    pub(crate) fn window(&self, kind: FlowKind) -> FlowWindow {
        match kind {
            FlowKind::Tcp => self.tcp,
            FlowKind::Udp => self.udp.unwrap_or(self.tcp),
        }
    }
}

pub(crate) async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    server: bool,
    local: Limits,
) -> io::Result<Limits> {
    let mut hello = Vec::with_capacity(28);
    hello.extend_from_slice(MAGIC);
    hello.extend_from_slice(&(local.flows as u32).to_be_bytes());
    for window in std::iter::once(local.tcp).chain(local.udp) {
        hello.extend_from_slice(&(window.bytes as u32).to_be_bytes());
        hello.extend_from_slice(&(window.slots as u32).to_be_bytes());
    }
    if server {
        stream.write_all(&hello).await?;
        stream.flush().await?;
    }
    let mut peer = [0; 28];
    stream.read_exact(&mut peer[..8]).await?;
    if &peer[..8] != MAGIC {
        return Err(invalid("Server agent needs STMUX003; update the agent"));
    }
    stream.read_exact(&mut peer[8..hello.len()]).await?;
    let read = |offset| u32::from_be_bytes(peer[offset..offset + 4].try_into().unwrap()) as usize;
    let read_window = |offset| FlowWindow {
        bytes: read(offset),
        slots: read(offset + 4),
    };
    let remote = Limits {
        flows: read(8),
        tcp: read_window(12),
        udp: local.udp.map(|_| read_window(20)),
    };
    if !(1..=8192).contains(&remote.flows)
        || !remote.tcp.valid()
        || remote.udp.is_some_and(|window| !window.valid())
    {
        return Err(invalid("Invalid peer resource limits"));
    }
    if !server {
        stream.write_all(&hello).await?;
        stream.flush().await?;
    }
    Ok(remote)
}
