use crate::{Flow, Result, TCP_CHUNK};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Clone, Copy)]
pub enum Direction {
    Upload,
    Download,
}

/// Preserve TCP half-close and release receive credit only after the local
/// socket accepts the bytes. A blocked socket never blocks the shared reader.
pub async fn relay<S, F>(flow: &mut Flow, stream: &mut S, counted: F) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: Fn(Direction, usize) + Sync,
{
    let sender = flow.sender();
    let (mut read, mut write) = tokio::io::split(stream);
    let upload = async {
        let mut buffer = vec![0; TCP_CHUNK];
        loop {
            let count = read.read(&mut buffer).await.map_err(|e| e.to_string())?;
            if count == 0 {
                sender.eof().await?;
                return Ok::<_, String>(());
            }
            sender.send(buffer[..count].to_vec()).await?;
            counted(Direction::Upload, count);
        }
    };
    let download = async {
        while let Some(packet) = flow.recv().await? {
            write.write_all(&packet).await.map_err(|e| e.to_string())?;
            counted(Direction::Download, packet.len());
            // Drop/credit happens after write_all, never upon wire receipt.
            drop(packet);
        }
        write.shutdown().await.map_err(|e| e.to_string())
    };
    // Cancellation must interrupt local socket IO too, not only Flow::recv.
    // Dropping the joined futures releases a Received packet held by write_all.
    tokio::select! {
        biased;
        _ = sender.closed() => return Err("Flow cancelled".into()),
        result = async { tokio::try_join!(upload, download) } => { result?; }
    }
    flow.finish().await
}
