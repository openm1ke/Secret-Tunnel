use secret_tunnel_mux::{Config, Endpoint, FlowKind, Role};
use std::{
    io,
    pin::Pin,
    process::Stdio,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf},
    net::{TcpListener, UdpSocket},
    sync::{watch, Barrier},
    task::JoinSet,
};

struct Pipes<R, W> {
    input: R,
    output: W,
}
impl<R: AsyncRead + Unpin, W: Unpin> AsyncRead for Pipes<R, W> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.input).poll_read(cx, buf)
    }
}
impl<R: Unpin, W: AsyncWrite + Unpin> AsyncWrite for Pipes<R, W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.output).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.output).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.output).poll_shutdown(cx)
    }
}
fn target(port: u16) -> Vec<u8> {
    let mut bytes = vec![0, 0, 0, 1, 127, 0, 0, 1];
    bytes.extend_from_slice(&port.to_be_bytes());
    bytes
}
fn prepare_limits() {
    #[cfg(unix)]
    unsafe {
        let mut limits = std::mem::zeroed::<libc::rlimit>();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limits) == 0 {
            limits.rlim_cur = limits.rlim_max.min(8192);
            assert_eq!(libc::setrlimit(libc::RLIMIT_NOFILE, &limits), 0);
        }
    }
}
async fn agent(max_flows: usize) -> (Endpoint, tokio::process::Child) {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_secret-tunnel-agent"))
        .arg("--stdio-v3")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let io = Pipes {
        input: child.stdout.take().unwrap(),
        output: child.stdin.take().unwrap(),
    };
    let endpoint = Endpoint::start(
        io,
        Role::Client,
        Config {
            max_flows,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    (endpoint, child)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_thousand_real_tcp_connections_share_one_agent_with_exact_payloads() {
    prepare_limits();
    tokio::time::timeout(Duration::from_secs(45),async{
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let destination=target(listener.local_addr().unwrap().port());
        let echo=tokio::spawn(async move{let mut tasks=JoinSet::new();loop{tokio::select!{
            accepted=listener.accept()=>{let(mut stream,_)=accepted.unwrap();tasks.spawn(async move{let(mut r,mut w)=stream.split();tokio::io::copy(&mut r,&mut w).await.unwrap();w.shutdown().await.unwrap();});}
            Some(result)=tasks.join_next(),if !tasks.is_empty()=>{result.unwrap();}
        }}});
        let (endpoint,mut child)=agent(2048).await;assert_eq!(endpoint.snapshot().limit,2048);
        let ready=Arc::new(Barrier::new(2049));let (start,gate)=watch::channel(false);let mut tasks=JoinSet::new();
        for index in 0u32..2048{
            let endpoint=endpoint.clone();let ready=ready.clone();let mut gate=gate.clone();let target=destination.clone();
            tasks.spawn(async move{
                let mut flow=endpoint.open(FlowKind::Tcp,target).await.unwrap();ready.wait().await;while !*gate.borrow(){gate.changed().await.unwrap();}
                let payload=[index.to_be_bytes().as_slice(),&vec![(index%251)as u8;1024]].concat();
                flow.sender().send(payload.clone()).await.unwrap();flow.sender().eof().await.unwrap();
                let mut received=Vec::new();while let Some(packet)=flow.recv().await.unwrap(){received.extend_from_slice(&packet);}
                assert_eq!(received,payload);flow.finish().await.unwrap();
            });
        }
        ready.wait().await;assert_eq!(endpoint.snapshot().active,2048);
        #[cfg(target_os="linux")]
        {
            let status=std::fs::read_to_string(format!("/proc/{}/status",child.id().unwrap())).unwrap();
            let rss=status.lines().find(|line|line.starts_with("VmRSS:")).unwrap().split_whitespace().nth(1).unwrap().parse::<usize>().unwrap();
            eprintln!("Agent RSS with 2048 live TCP connections: {rss} KiB");assert!(rss<256*1024,"Idle connection memory exceeded the agent budget");
        }
        assert!(endpoint.open(FlowKind::Tcp,destination).await.is_err());start.send(true).unwrap();
        while let Some(result)=tasks.join_next().await{result.unwrap();}
        let snapshot=endpoint.snapshot();assert_eq!(snapshot.active,0);assert_eq!(snapshot.peak,2048);assert_eq!(snapshot.protocol_errors,0);
        assert!(snapshot.receive_queue_peak<=snapshot.receive_budget);assert!(snapshot.send_queue_peak<=snapshot.send_budget);assert!(snapshot.write_batches<snapshot.frames_tx);
        endpoint.shutdown();drop(endpoint);let status=tokio::time::timeout(Duration::from_secs(5),child.wait()).await.unwrap().unwrap();assert!(status.success());echo.abort();
    }).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn udp_associations_have_distinct_sources_and_do_not_block_large_tcp_transfers() {
    prepare_limits();
    tokio::time::timeout(Duration::from_secs(30), async {
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let udp_target = target(udp.local_addr().unwrap().port());
        let udp_echo = tokio::spawn(async move {
            let mut seen = std::collections::HashSet::new();
            let mut buf = vec![0; 65535];
            loop {
                let (n, source) = udp.recv_from(&mut buf).await.unwrap();
                seen.insert(source);
                udp.send_to(&buf[..n], source).await.unwrap();
                if seen.len() == 64 {
                    return;
                }
            }
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp_target = target(listener.local_addr().unwrap().port());
        let tcp_echo = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (mut r, mut w) = socket.split();
            tokio::io::copy(&mut r, &mut w).await.unwrap();
            w.shutdown().await.unwrap();
        });
        let (endpoint, mut child) = agent(1024).await;
        let mut tasks = JoinSet::new();
        let bulk = endpoint.clone();
        tasks.spawn(async move {
            let mut flow = bulk.open(FlowKind::Tcp, tcp_target).await.unwrap();
            let sender = flow.sender();
            let sending = async {
                for index in 0..128 {
                    sender.send(vec![(index % 251) as u8; 16384]).await.unwrap();
                }
                sender.eof().await.unwrap();
            };
            let receiving = async {
                let mut count = 0usize;
                while let Some(packet) = flow.recv().await.unwrap() {
                    for byte in packet.iter() {
                        assert_eq!(*byte, ((count / 16384) % 251) as u8);
                        count += 1;
                    }
                }
                assert_eq!(count, 2 * 1024 * 1024);
            };
            tokio::join!(sending, receiving);
            flow.finish().await.unwrap();
        });
        for index in 0u32..64 {
            let endpoint = endpoint.clone();
            let target = udp_target.clone();
            tasks.spawn(async move {
                let mut flow = endpoint.open(FlowKind::Udp, vec![]).await.unwrap();
                let mut packet = target;
                packet.extend_from_slice(&index.to_be_bytes());
                assert!(flow.sender().try_send_datagram(packet.clone()).unwrap());
                let response = flow.recv().await.unwrap().unwrap();
                assert_eq!(&*response, packet.as_slice());
                drop(response);
                flow.finish().await.unwrap();
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
        udp_echo.await.unwrap();
        tcp_echo.await.unwrap();
        let snapshot = endpoint.snapshot();
        assert_eq!(snapshot.udp_drops, 0);
        assert_eq!(snapshot.protocol_errors, 0);
        endpoint.shutdown();
        drop(endpoint);
        assert!(child.wait().await.unwrap().success());
    })
    .await
    .unwrap();
}
