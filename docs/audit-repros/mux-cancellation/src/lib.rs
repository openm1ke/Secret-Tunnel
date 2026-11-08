#[cfg(test)]
mod tests {
    use secret_tunnel_mux::{Config,Endpoint,FlowKind,Role};
    use std::time::Duration;
    #[tokio::test]
    async fn aborted_flow_must_cancel_relay_blocked_in_local_socket_write() {
        let (a,b)=tokio::io::duplex(65536);
        let (client,server)=tokio::join!(Endpoint::start(a,Role::Client,Config::default()),Endpoint::start(b,Role::Server,Config::default()));
        let (client,server)=(client.unwrap(),server.unwrap());
        let open={let client=client.clone();tokio::spawn(async move{client.open(FlowKind::Tcp,vec![]).await.unwrap()})};
        let mut incoming=server.accept().await.unwrap().flow;
        incoming.accept().unwrap();
        let flow=open.await.unwrap();
        // The destination remains connected, but stops consuming input.
        let (mut destination,_not_reading)=tokio::io::duplex(1);
        let mut relay=tokio::spawn(async move{secret_tunnel_mux::relay(&mut incoming,&mut destination,|_,_|{}).await});
        flow.sender().send(vec![7;16384]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(flow);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let ended=tokio::time::timeout(Duration::from_millis(500),&mut relay).await;
        let reclaimed=ended.is_ok();
        relay.abort();client.shutdown();server.shutdown();
        assert!(reclaimed,"confirmed: active=0 but cancelled relay still owns its blocked destination socket");
    }
    #[tokio::test]
    async fn late_cancellation_observer_must_not_wait_for_another_event() {
        let (a,b)=tokio::io::duplex(65536);
        let (client,server)=tokio::join!(Endpoint::start(a,Role::Client,Config::default()),Endpoint::start(b,Role::Server,Config::default()));
        let (client,server)=(client.unwrap(),server.unwrap());
        let open={let client=client.clone();tokio::spawn(async move{client.open(FlowKind::Tcp,vec![]).await.unwrap()})};
        let incoming=server.accept().await.unwrap().flow;
        incoming.accept().unwrap();
        let flow=open.await.unwrap();
        let sender=incoming.sender();
        drop(incoming);
        assert!(sender.is_closed());
        let notified=tokio::time::timeout(Duration::from_millis(100),sender.closed()).await.is_ok();
        drop(flow);client.shutdown();server.shutdown();
        assert!(notified,"confirmed: is_closed=true but a late closed() subscriber misses the cancellation");
    }
}
