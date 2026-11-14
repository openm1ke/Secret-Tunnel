//! Counts bytes crossing the local SOCKS TCP boundary, including SOCKS headers.
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Default)]
struct Shared {
    stopped: AtomicBool,
    rx: AtomicU64,
    tx: AtomicU64,
    connections: Mutex<HashMap<u64, (TcpStream, TcpStream)>>,
}

pub struct TrafficRelay {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl TrafficRelay {
    pub fn start(listener: TcpListener, upstream: SocketAddr) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        let shared = Arc::new(Shared::default());
        let state = shared.clone();
        let worker = thread::Builder::new()
            .name("socks-traffic-relay".into())
            .spawn(move || {
                let mut next_id = 0;
                while !state.stopped.load(Ordering::Acquire) {
                    let (client, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                            continue;
                        }
                        Err(_) => break,
                    };
                    // Winsock inherits the listener's nonblocking mode on accept.
                    if client.set_nonblocking(false).is_err() {
                        continue;
                    }
                    let remote = match TcpStream::connect_timeout(&upstream, Duration::from_secs(2))
                    {
                        Ok(remote) => remote,
                        Err(_) => continue,
                    };
                    let _ = client.set_nodelay(true);
                    let _ = remote.set_nodelay(true);
                    let pair = match (client.try_clone(), remote.try_clone()) {
                        (Ok(client), Ok(remote)) => (client, remote),
                        _ => continue,
                    };
                    let id = next_id;
                    next_id += 1;
                    {
                        let mut connections = state.connections.lock().unwrap();
                        if state.stopped.load(Ordering::Acquire) {
                            break;
                        }
                        if connections.len() >= 256 {
                            continue;
                        }
                        connections.insert(id, pair);
                    }
                    let state = state.clone();
                    let cleanup_state = state.clone();
                    if thread::Builder::new()
                        .name("socks-connection".into())
                        .spawn(move || {
                            let client_write = client.try_clone();
                            let remote_read = remote.try_clone();
                            if let (Ok(mut client_write), Ok(mut remote_read)) =
                                (client_write, remote_read)
                            {
                                thread::scope(|scope| {
                                    scope.spawn(|| {
                                        if copy_counted(
                                            &mut remote_read,
                                            &mut client_write,
                                            &state.rx,
                                        )
                                        .is_err()
                                        {
                                            let _ = client_write.shutdown(Shutdown::Both);
                                            let _ = remote_read.shutdown(Shutdown::Both);
                                        }
                                    });
                                    let mut client = client;
                                    let mut remote = remote;
                                    if copy_counted(&mut client, &mut remote, &state.tx).is_err() {
                                        let _ = client.shutdown(Shutdown::Both);
                                        let _ = remote.shutdown(Shutdown::Both);
                                    }
                                });
                            }
                            state.connections.lock().unwrap().remove(&id);
                        })
                        .is_err()
                    {
                        cleanup_state.connections.lock().unwrap().remove(&id);
                    }
                }
            })?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    pub fn totals(&self) -> (u64, u64) {
        (
            self.shared.rx.load(Ordering::Relaxed),
            self.shared.tx.load(Ordering::Relaxed),
        )
    }
}

impl Drop for TrafficRelay {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        for (client, remote) in self.shared.connections.lock().unwrap().values() {
            let _ = client.shutdown(Shutdown::Both);
            let _ = remote.shutdown(Shutdown::Both);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn copy_counted(
    reader: &mut TcpStream,
    writer: &mut TcpStream,
    total: &AtomicU64,
) -> io::Result<()> {
    let mut buffer = [0; 32768];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            writer.shutdown(Shutdown::Write)?;
            return Ok(());
        }
        let mut offset = 0;
        while offset < count {
            let written = writer.write(&buffer[offset..count])?;
            if written == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            total.fetch_add(written as u64, Ordering::Relaxed);
            offset += written;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts_both_directions_and_preserves_half_close() {
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = upstream.local_addr().unwrap();
        let echo = thread::spawn(move || {
            let (mut stream, _) = upstream.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut data = Vec::new();
            stream.read_to_end(&mut data).unwrap();
            stream.write_all(&data).unwrap();
        });
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let local = listener.local_addr().unwrap();
        let relay = TrafficRelay::start(listener, address).unwrap();
        let mut client = TcpStream::connect(local).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let payload = vec![42; 256 * 1024];
        client.write_all(&payload).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let mut result = Vec::new();
        client.read_to_end(&mut result).unwrap();
        assert_eq!(result, payload);
        assert_eq!(relay.totals(), (payload.len() as u64, payload.len() as u64));
        echo.join().unwrap();
    }

    #[test]
    fn stop_closes_live_connections() {
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let local = listener.local_addr().unwrap();
        let relay = TrafficRelay::start(listener, upstream.local_addr().unwrap()).unwrap();
        let mut client = TcpStream::connect(local).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (_remote, _) = upstream.accept().unwrap();
        drop(relay);
        assert_eq!(client.read(&mut [0; 1]).unwrap(), 0);
    }

    #[test]
    fn supports_idle_gaps_between_requests() {
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let local = listener.local_addr().unwrap();
        let relay = TrafficRelay::start(listener, upstream.local_addr().unwrap()).unwrap();
        let echo = thread::spawn(move || {
            let (mut stream, _) = upstream.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            for _ in 0..3 {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                thread::sleep(Duration::from_millis(25));
                stream.write_all(&byte).unwrap();
            }
        });
        let mut client = TcpStream::connect(local).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        for value in 1..=3 {
            thread::sleep(Duration::from_millis(25));
            client.write_all(&[value]).unwrap();
            let mut byte = [0];
            client.read_exact(&mut byte).unwrap();
            assert_eq!(byte, [value]);
        }
        // The last byte can reach the client before the relay counts it.
        // The downstream half-close occurs after all counter updates.
        client.shutdown(Shutdown::Write).unwrap();
        assert_eq!(client.read(&mut [0; 1]).unwrap(), 0);
        echo.join().unwrap();
        assert_eq!(relay.totals(), (3, 3));
    }
}
