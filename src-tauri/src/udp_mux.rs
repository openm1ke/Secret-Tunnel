//! One SSH exec channel carries independent UDP associations. The legacy agent
//! remains usable, but cannot avoid the SSH server's per-connection session cap.
use super::{udp_protocol, SharedSession};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::{mpsc, watch};

type Logs = Arc<Mutex<Vec<String>>>;
type Routes = Arc<Mutex<HashMap<u32, mpsc::Sender<Vec<u8>>>>>;

#[derive(Default)]
pub(super) struct UdpService(tokio::sync::Mutex<ServiceState>);
#[derive(Default)]
enum ServiceState {
    #[default]
    Unknown,
    Legacy,
    Ready(Arc<Multiplexer>),
}

impl UdpService {
    pub async fn associate(
        &self,
        session: &SharedSession,
        logs: &Logs,
    ) -> Result<Association, String> {
        let mut state = self.0.lock().await;
        if let ServiceState::Ready(mux) = &*state {
            if !*mux.closed.borrow() {
                return mux.associate();
            }
        }
        let multiplex = !matches!(*state, ServiceState::Legacy);
        let (stream, multiplex) = tokio::time::timeout(Duration::from_secs(10), async {
            let channel = session
                .channel_open_session()
                .await
                .map_err(|e| e.to_string())?;
            let command = if multiplex {
                "exec \"$HOME/.local/bin/secret-tunnel-agent\" --multiplex"
            } else {
                "exec \"$HOME/.local/bin/secret-tunnel-agent\""
            };
            channel
                .exec(true, command)
                .await
                .map_err(|e| e.to_string())?;
            let mut stream = channel.into_stream();
            let mut magic = [0; 8];
            stream
                .read_exact(&mut magic)
                .await
                .map_err(|e| format!("UDP agent startup: {e}"))?;
            if &magic != udp_protocol::MAGIC && &magic != udp_protocol::MUX_MAGIC {
                return Err("Incompatible UDP agent protocol".to_owned());
            }
            Ok((stream, &magic == udp_protocol::MUX_MAGIC))
        })
        .await
        .map_err(|_| "UDP agent startup timed out; check the server agent".to_owned())??;
        let mux = Multiplexer::start(stream, multiplex, logs.clone());
        if multiplex {
            crate_log(
                logs,
                "UDP: multiplexed agent ready; independent flows share one SSH channel",
            );
            *state = ServiceState::Ready(mux.clone());
        } else {
            if !matches!(*state, ServiceState::Legacy) {
                crate_log(
                    logs,
                    "UDP: legacy server agent; update it to remove the parallel-session limit",
                );
            }
            *state = ServiceState::Legacy;
        }
        mux.associate()
    }
}

fn crate_log(logs: &Logs, text: impl Into<String>) {
    super::super::log_line(logs, text);
}

struct Multiplexer {
    routes: Routes,
    outgoing: mpsc::Sender<(u32, Vec<u8>)>,
    closed: watch::Receiver<bool>,
    next: AtomicU32,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Multiplexer {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl Multiplexer {
    fn start<S>(stream: S, multiplex: bool, logs: Logs) -> Arc<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let routes: Routes = Default::default();
        let read_routes = routes.clone();
        let (outgoing, mut packets) = mpsc::channel::<(u32, Vec<u8>)>(64);
        let (closed_tx, closed) = watch::channel(false);
        let read_closed = closed_tx.clone();
        let mut read_stop = closed.clone();
        let mut write_stop = closed.clone();
        let read_logs = logs.clone();
        let reader = tokio::spawn(async move {
            loop {
                let reading = async {
                    if multiplex {
                        udp_protocol::read_mux_frame(&mut reader).await
                    } else {
                        udp_protocol::read_frame(&mut reader).await.map(|p| (1, p))
                    }
                };
                let packet =
                    tokio::select! { packet = reading => packet, _ = read_stop.changed() => break };
                match packet {
                    Ok((id, packet)) => {
                        if let Some(route) = read_routes.lock().unwrap().get(&id) {
                            // A slow application may drop UDP, but never block other flows.
                            let _ = route.try_send(packet);
                        }
                    }
                    Err(error) => {
                        crate_log(&read_logs, format!("UDP agent receive: {error}"));
                        break;
                    }
                }
            }
            let _ = read_closed.send(true);
        });
        let writer = tokio::spawn(async move {
            loop {
                let packet = tokio::select! { packet = packets.recv() => packet, _ = write_stop.changed() => break };
                let Some((id, packet)) = packet else { break };
                if !multiplex && packet.is_empty() {
                    break;
                }
                let result = tokio::time::timeout(Duration::from_secs(5), async {
                    if multiplex {
                        udp_protocol::write_mux_frame(&mut writer, id, &packet).await
                    } else {
                        udp_protocol::write_frame(&mut writer, &packet).await
                    }
                })
                .await;
                match result {
                    Ok(Ok(())) => (),
                    error => {
                        crate_log(&logs, format!("UDP agent send: {error:?}"));
                        break;
                    }
                }
            }
            let _ = closed_tx.send(true);
        });
        Arc::new(Self {
            routes,
            outgoing,
            closed,
            next: AtomicU32::new(1),
            tasks: vec![reader, writer],
        })
    }
    fn associate(self: &Arc<Self>) -> Result<Association, String> {
        if *self.closed.borrow() {
            return Err("UDP agent channel closed".into());
        }
        let (sender, incoming) = mpsc::channel(16);
        let mut routes = self.routes.lock().unwrap();
        let id = loop {
            let id = self.next.fetch_add(1, Ordering::Relaxed);
            if id != 0 && !routes.contains_key(&id) {
                break id;
            }
        };
        routes.insert(id, sender);
        Ok(Association {
            id,
            mux: self.clone(),
            incoming,
            closed: self.closed.clone(),
        })
    }
}

pub(super) struct Association {
    id: u32,
    mux: Arc<Multiplexer>,
    pub incoming: mpsc::Receiver<Vec<u8>>,
    pub closed: watch::Receiver<bool>,
}
impl Association {
    pub async fn send(&self, packet: Vec<u8>) -> Result<(), String> {
        tokio::time::timeout(
            Duration::from_secs(5),
            self.mux.outgoing.send((self.id, packet)),
        )
        .await
        .map_err(|_| "UDP send queue timed out".to_owned())?
        .map_err(|_| "UDP agent channel closed".to_owned())
    }
}
impl Drop for Association {
    fn drop(&mut self) {
        self.mux.routes.lock().unwrap().remove(&self.id);
        // Close is best-effort under overload; the agent also expires idle flows.
        let _ = self.mux.outgoing.try_send((self.id, Vec::new()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn routes_same_destination_separately_and_propagates_agent_failure() {
        let (client, mut agent) = tokio::io::duplex(4096);
        let mux = Multiplexer::start(client, true, Default::default());
        let mut first = mux.associate().unwrap();
        let mut second = mux.associate().unwrap();
        let packet =
            udp_protocol::encode_packet("127.0.0.1:53".parse().unwrap(), b"query").unwrap();
        first.send(packet.clone()).await.unwrap();
        second.send(packet.clone()).await.unwrap();
        assert_eq!(
            udp_protocol::read_mux_frame(&mut agent).await.unwrap(),
            (first.id, packet.clone())
        );
        assert_eq!(
            udp_protocol::read_mux_frame(&mut agent).await.unwrap(),
            (second.id, packet.clone())
        );
        udp_protocol::write_mux_frame(&mut agent, second.id, &packet)
            .await
            .unwrap();
        assert_eq!(second.incoming.recv().await.unwrap(), packet);
        assert!(first.incoming.try_recv().is_err());
        let id = first.id;
        drop(first);
        assert_eq!(
            udp_protocol::read_mux_frame(&mut agent).await.unwrap(),
            (id, vec![])
        );
        assert_eq!(mux.routes.lock().unwrap().len(), 1);
        udp_protocol::write_mux_frame(&mut agent, id, &packet)
            .await
            .unwrap();
        udp_protocol::write_mux_frame(&mut agent, second.id, &packet)
            .await
            .unwrap();
        assert_eq!(second.incoming.recv().await.unwrap(), packet);
        drop(agent);
        tokio::time::timeout(Duration::from_secs(1), second.closed.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(*second.closed.borrow());
        assert!(mux.associate().is_err());
    }
}
