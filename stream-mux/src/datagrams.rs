use std::{
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::{net::UdpSocket, sync::Semaphore};

/// A UDP send must not keep a flow's receive credit while its socket is
/// unwritable indefinitely, or outlive cancellation of the association.
pub async fn send_datagram(
    sender: &crate::Sender,
    socket: &UdpSocket,
    packet: &[u8],
    destination: SocketAddr,
) -> io::Result<usize> {
    send_with_timeout(
        sender,
        socket.send_to(packet, destination),
        std::time::Duration::from_secs(5),
    )
    .await
}

async fn send_with_timeout(
    sender: &crate::Sender,
    send: impl std::future::Future<Output = io::Result<usize>>,
    timeout: std::time::Duration,
) -> io::Result<usize> {
    tokio::select! {
        biased;
        _ = sender.closed() => Err(io::Error::new(io::ErrorKind::ConnectionAborted, "Flow cancelled")),
        result = tokio::time::timeout(timeout, send) => result
            .unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::TimedOut, "UDP socket send timed out"))),
    }
}

/// Idle UDP associations do not each retain a 64 KiB receive buffer. Scratch
/// buffers are shared, acquired only after readiness, and returned immediately.
#[derive(Clone)]
pub struct DatagramBuffers(Arc<Pool>);
struct Pool {
    slots: Semaphore,
    buffers: Mutex<Vec<Vec<u8>>>,
}
impl Default for DatagramBuffers {
    fn default() -> Self {
        Self(Arc::new(Pool {
            slots: Semaphore::new(16),
            buffers: Default::default(),
        }))
    }
}
impl DatagramBuffers {
    pub async fn recv(&self, socket: &UdpSocket) -> io::Result<(Vec<u8>, SocketAddr)> {
        loop {
            socket.readable().await?;
            let _permit = self
                .0
                .slots
                .acquire()
                .await
                .map_err(|_| io::Error::other("Datagram pool closed"))?;
            let mut buffer = self
                .0
                .buffers
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| vec![0; 65535]);
            let result = socket.try_recv_from(&mut buffer);
            let result = match result {
                Ok((n, peer)) => Ok((buffer[..n].to_vec(), peer)),
                Err(error) => Err(error),
            };
            self.0.buffers.lock().unwrap().push(buffer);
            match result {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                result => return result,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, Endpoint, FlowKind, Role};
    use std::time::Duration;

    #[tokio::test]
    async fn stalled_udp_sends_have_a_deadline_and_cancel_releases_held_credit() {
        tokio::time::timeout(Duration::from_secs(3), async {
            let (a, b) = tokio::io::duplex(65536);
            let (client, server) = tokio::join!(
                Endpoint::start(a, Role::Client, Config::default()),
                Endpoint::start(b, Role::Server, Config::default()),
            );
            let (client, server) = (client.unwrap(), server.unwrap());
            for receiving_client in [false, true] {
                let (opened, accepted) = tokio::join!(client.open(FlowKind::Udp, vec![]), async {
                    let flow = server.accept().await.unwrap().flow;
                    flow.accept().unwrap();
                    flow
                });
                let (sending, mut receiving, receiver) = if receiving_client {
                    (accepted, opened.unwrap(), &client)
                } else {
                    (opened.unwrap(), accepted, &server)
                };
                let sender = receiving.sender();
                let error =
                    send_with_timeout(&sender, std::future::pending(), Duration::from_millis(20))
                        .await
                        .unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                assert!(
                    !sender.is_closed(),
                    "one expired datagram does not close SSH"
                );

                sending.sender().send(vec![1; 1200]).await.unwrap();
                let packet = receiving.recv().await.unwrap().unwrap();
                assert_eq!(receiver.snapshot().receive_queue_bytes, 1200);
                let blocked = tokio::spawn(async move {
                    let result =
                        send_with_timeout(&sender, std::future::pending(), Duration::from_secs(60))
                            .await;
                    drop(packet);
                    result
                });
                drop(sending);
                let error = tokio::time::timeout(Duration::from_secs(1), blocked)
                    .await
                    .expect("cancel did not interrupt UDP socket IO")
                    .unwrap()
                    .unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
                assert_eq!(receiver.snapshot().receive_queue_bytes, 0);
                drop(receiving);
            }
            assert_eq!(client.snapshot().active, 0);
            assert_eq!(server.snapshot().active, 0);
        })
        .await
        .unwrap();
    }
}
