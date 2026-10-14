use std::{
    collections::HashSet,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt},
    net::{TcpSocket, TcpStream},
    sync::Semaphore,
    task::JoinSet,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use crate::{
    selector,
    snapshot::{Backend, Snapshot},
};

const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

const MAX_FAILOVER_ATTEMPTS: usize = 3;

const RELAY_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

const RELAY_BUF_SIZE: usize = 8192;

const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

struct ActiveConnectionGuard {
    backend: Arc<Backend>,
}

impl ActiveConnectionGuard {
    fn new(backend: Arc<Backend>) -> Self {
        backend
            .runtime
            .active_connections
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self { backend }
    }
}
impl Drop for ActiveConnectionGuard {
    fn drop(&mut self) {
        self.backend
            .runtime
            .active_connections
            .fetch_sub(1, Ordering::Relaxed);
    }
}

pub async fn run(
    listen_address: SocketAddr,
    snapshots: tokio::sync::watch::Receiver<Arc<Snapshot>>,
    max_connections: usize,
    backlog: u32,
    shutdown: CancellationToken,
    rejected_connections: Arc<AtomicU64>,
) -> io::Result<()> {
    assert!(max_connections > 0, "max_connections must be > 0");
    assert!(backlog > 0, "listen backlog must be > 0");

    let socket = if listen_address.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    socket.set_reuseaddr(true)?;
    socket.bind(listen_address)?;
    let listener = socket.listen(backlog)?;
    tracing::info!(
        listen_address = %listen_address,
        max_connections,
        backlog,
        "proxy listening"
    );

    let semaphore = Arc::new(Semaphore::new(max_connections));
    let mut tasks = JoinSet::new();
    let mut reap_tick = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            biased;

            _ = shutdown.cancelled() => {
                tracing::info!("proxy shutdown requested, draining connections");
                break;
            }
            _ = reap_tick.tick() => {
                reap_finished(&mut tasks);
            }
            accept = listener.accept() => {
                let (client, _) = match accept {
                    Ok(v) => v,
                    Err(error) => {
                        tracing::warn!(error = %error, "accept failed");
                        continue;
                    }
                };
                let _ = client.set_nodelay(true);

                let permit = match Arc::clone(&semaphore).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        rejected_connections.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            max_connections,
                            total_rejected = rejected_connections.load(Ordering::Relaxed),
                            "rejected connection over limit"
                        );
                        drop(client);
                        continue;
                    }
                };

                let snapshot = {
                    let current = snapshots.borrow();
                    Arc::clone(&*current)
                };
                let client_address = client
                    .peer_addr()
                    .map(|address| address.to_string())
                    .unwrap_or_else(|_| "unknown".to_owned());
                tasks.spawn(async move {

                    let _permit = permit;
                    if let Err(error) = handle_client(client, snapshot).await {

                        tracing::warn!(
                            client = %client_address,
                            error = %error,
                            "connection failed"
                        );
                    }
                });

                reap_finished(&mut tasks);
            }
        }
    }

    drop(listener);
    let in_flight = tasks.len();
    tracing::info!(
        in_flight,
        drain_timeout = ?SHUTDOWN_DRAIN_TIMEOUT,
        "proxy stopped accepting, waiting for in-flight connections"
    );

    match timeout(SHUTDOWN_DRAIN_TIMEOUT, async {
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                tracing::warn!(error = %error, "connection task failed during drain");
            }
        }
    })
    .await
    {
        Ok(()) => tracing::info!("proxy drained cleanly"),
        Err(_) => {
            tracing::warn!(
                remaining = tasks.len(),
                "drain timeout exceeded, aborting remaining connections"
            );
            tasks.abort_all();
        }
    }

    Ok(())
}

fn reap_finished(tasks: &mut JoinSet<()>) {
    while let Some(result) = tasks.try_join_next() {
        if let Err(error) = result {
            tracing::warn!(error = %error, "connection task failed");
        }
    }
}

async fn handle_client(mut client: TcpStream, snapshot: Arc<Snapshot>) -> io::Result<()> {
    let client_peer = client.peer_addr().ok();
    let client_address = client_peer
        .map(|address| address.to_string())
        .unwrap_or_else(|| "unknown".to_owned());

    let mut tried: HashSet<SocketAddr> = HashSet::with_capacity(MAX_FAILOVER_ATTEMPTS);
    let mut last_error: Option<io::Error> = None;

    for attempt in 1..=MAX_FAILOVER_ATTEMPTS {
        let Some(backend) = selector::select_backend_excluding(&snapshot, &tried, client_peer)
        else {
            break;
        };
        tried.insert(backend.address);

        let active_connection = ActiveConnectionGuard::new(Arc::clone(&backend));

        tracing::debug!(
            client = %client_address,
            backend = %active_connection.backend.address,
            attempt,
            max_attempts = MAX_FAILOVER_ATTEMPTS,
            "backend selected"
        );

        let mut upstream = match timeout(
            UPSTREAM_CONNECT_TIMEOUT,
            TcpStream::connect(active_connection.backend.address),
        )
        .await
        {
            Ok(Ok(upstream)) => {
                let _ = upstream.set_nodelay(true);
                upstream
            }
            Ok(Err(error)) => {
                active_connection
                    .backend
                    .runtime
                    .connect_failures
                    .fetch_add(1, Ordering::Relaxed);

                tracing::warn!(
                    client = %client_address,
                    backend = %active_connection.backend.address,
                    attempt,
                    error = %error,
                    "upstream connect failed"
                );
                last_error = Some(error);
                drop(active_connection);
                continue;
            }
            Err(_) => {
                active_connection
                    .backend
                    .runtime
                    .connect_failures
                    .fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    client = %client_address,
                    backend = %active_connection.backend.address,
                    attempt,
                    timeout = ?UPSTREAM_CONNECT_TIMEOUT,
                    "upstream connect timed out"
                );
                last_error = Some(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "upstream connect timed out",
                ));
                drop(active_connection);
                continue;
            }
        };

        active_connection
            .backend
            .runtime
            .total_connections
            .fetch_add(1, Ordering::Relaxed);

        let (client_to_backend, backend_to_client, relay_result) =
            relay_bidirectional(&mut client, &mut upstream, RELAY_IDLE_TIMEOUT).await;
        active_connection
            .backend
            .runtime
            .bytes_in
            .fetch_add(client_to_backend, Ordering::Relaxed);
        active_connection
            .backend
            .runtime
            .bytes_out
            .fetch_add(backend_to_client, Ordering::Relaxed);

        match relay_result {
            Ok(()) => {
                tracing::debug!(
                    client = %client_address,
                    backend = %active_connection.backend.address,
                    bytes_client_to_backend = client_to_backend,
                    bytes_backend_to_client = backend_to_client,
                    "relay finished"
                );
            }
            Err(error) => {
                tracing::warn!(
                    client = %client_address,
                    backend = %active_connection.backend.address,
                    bytes_client_to_backend = client_to_backend,
                    bytes_backend_to_client = backend_to_client,
                    error = %error,
                    "relay failed"
                );
                return Err(error);
            }
        }

        return Ok(());
    }

    if tried.is_empty() {
        tracing::debug!(
            client = %client_address,
            "no eligible backend"
        );
        return Ok(());
    }

    tracing::warn!(
        client = %client_address,
        tried = ?tried,
        "all upstreams failed"
    );
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::ConnectionRefused, "all upstreams failed")
    }))
}

async fn relay_bidirectional(
    client: &mut TcpStream,
    upstream: &mut TcpStream,
    idle_timeout: Duration,
) -> (u64, u64, io::Result<()>) {
    let (mut client_read, mut client_write) = client.split();
    let (mut upstream_read, mut upstream_write) = upstream.split();

    let mut client_to_backend: u64 = 0;
    let mut backend_to_client: u64 = 0;
    let mut client_to_backend_done = false;
    let mut backend_to_client_done = false;
    let mut buf_c2u = [0u8; RELAY_BUF_SIZE];
    let mut buf_u2c = [0u8; RELAY_BUF_SIZE];
    let mut deadline = tokio::time::Instant::now() + idle_timeout;

    loop {
        if client_to_backend_done && backend_to_client_done {
            return (client_to_backend, backend_to_client, Ok(()));
        }
        tokio::select! {
            biased;

            read_res = async {
                if client_to_backend_done {
                    std::future::pending::<io::Result<usize>>().await
                } else {
                    client_read.read(&mut buf_c2u).await
                }
            } => {
                match read_res {
                    Ok(0) => {
                        client_to_backend_done = true;
                        let _ = upstream_write.shutdown().await;
                    }
                    Ok(n) => {
                        if let Err(error) = upstream_write.write_all(&buf_c2u[..n]).await {
                            return (client_to_backend, backend_to_client, Err(error));
                        }
                        client_to_backend += n as u64;
                        deadline = tokio::time::Instant::now() + idle_timeout;
                    }
                    Err(error) => {
                        return (client_to_backend, backend_to_client, Err(error));
                    }
                }
            }

            read_res = async {
                if backend_to_client_done {
                    std::future::pending::<io::Result<usize>>().await
                } else {
                    upstream_read.read(&mut buf_u2c).await
                }
            } => {
                match read_res {
                    Ok(0) => {
                        backend_to_client_done = true;
                        let _ = client_write.shutdown().await;
                    }
                    Ok(n) => {
                        if let Err(error) = client_write.write_all(&buf_u2c[..n]).await {
                            return (client_to_backend, backend_to_client, Err(error));
                        }
                        backend_to_client += n as u64;
                        deadline = tokio::time::Instant::now() + idle_timeout;
                    }
                    Err(error) => {
                        return (client_to_backend, backend_to_client, Err(error));
                    }
                }
            }

            _ = tokio::time::sleep_until(deadline) => {
                let error = io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("relay idle for {idle_timeout:?}"),
                );
                return (client_to_backend, backend_to_client, Err(error));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    async fn loopback_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (dial, accept) = tokio::join!(TcpStream::connect(addr), listener.accept());
        (dial.unwrap(), accept.unwrap().0)
    }

    #[tokio::test]
    async fn relay_counts_bytes_on_clean_close() {
        let (mut a_end, mut a_peer) = loopback_pair().await;
        let (mut b_end, mut b_peer) = loopback_pair().await;

        let relay_task = tokio::spawn(async move {
            relay_bidirectional(&mut a_end, &mut b_end, Duration::from_secs(5)).await
        });

        a_peer.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 16];
        b_peer.read_exact(&mut buf[..5]).await.unwrap();
        assert_eq!(&buf[..5], b"hello");
        drop(a_peer);
        drop(b_peer);

        let (c2u, u2c, result) = relay_task.await.unwrap();
        assert!(result.is_ok());
        assert_eq!(c2u, 5);
        assert_eq!(u2c, 0);
    }

    #[tokio::test]
    async fn relay_idle_timeout_fires_but_keeps_partial_bytes() {
        let (mut a_end, _a_peer) = loopback_pair().await;
        let (mut b_end, _b_peer) = loopback_pair().await;

        let (c2u, u2c, result) =
            relay_bidirectional(&mut a_end, &mut b_end, Duration::from_millis(100)).await;
        assert_eq!(c2u, 0);
        assert_eq!(u2c, 0);
        let error = result.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn relay_one_way_silence_does_not_trigger_idle() {
        let (mut a_end, mut a_peer) = loopback_pair().await;
        let (mut b_end, mut b_peer) = loopback_pair().await;

        let relay_task = tokio::spawn(async move {
            relay_bidirectional(&mut a_end, &mut b_end, Duration::from_millis(300)).await
        });

        a_peer.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        b_peer.read_exact(&mut buf).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        b_peer.write_all(b"pong").await.unwrap();
        a_peer.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");
        drop(a_peer);
        drop(b_peer);

        let (c2u, u2c, result) = relay_task.await.unwrap();
        assert!(
            result.is_ok(),
            "one-way silence is not idleness: {result:?}"
        );
        assert_eq!(c2u, 4);
        assert_eq!(u2c, 4);
    }
}
