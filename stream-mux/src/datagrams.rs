use std::{
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::{net::UdpSocket, sync::Semaphore};

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
