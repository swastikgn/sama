use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use tokio::{net::TcpStream, task::JoinSet, time};
use tokio_util::sync::CancellationToken;

use crate::snapshot::{Backend, Snapshot};

const PROBE_INTERVAL: Duration = Duration::from_secs(2);
const PROBE_TIMEOUT: Duration = Duration::from_millis(600);
const FALL: usize = 3;
const RISE: usize = 2;
const MAX_PARALLEL_PROBES: usize = 32;

const PROBE_JITTER_MAX: Duration = Duration::from_millis(200);

pub async fn run(
    mut snapshots: tokio::sync::watch::Receiver<Arc<Snapshot>>,
    shutdown: CancellationToken,
) {
    let mut ticker = time::interval(PROBE_INTERVAL);
    ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    tracing::info!(interval = ?PROBE_INTERVAL, fall = FALL, rise = RISE, "health checker started");

    loop {
        tokio::select! {
            biased;

            _ = shutdown.cancelled() => {
                tracing::info!("health checker shutting down");
                return;
            }
            _ = ticker.tick() => {}
            changed = snapshots.changed() => {
                if changed.is_err() {
                    tracing::warn!("snapshot channel closed, health checker exiting");
                    return;
                }
            }
        }

        let jitter = probe_jitter();
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                tracing::info!("health checker shutting down");
                return;
            }
            _ = time::sleep(jitter) => {}
        }

        let snapshot = {
            let current = snapshots.borrow_and_update();
            Arc::clone(&*current)
        };

        probe_backends(&snapshot).await;
    }
}

async fn probe_backends(snapshot: &Snapshot) {
    for batch in snapshot.backends.chunks(MAX_PARALLEL_PROBES) {
        let mut probes = JoinSet::new();

        for backend in batch {
            let backend = Arc::clone(backend);
            probes.spawn(async move {
                let succeeded = probe(backend.address).await;
                (backend, succeeded)
            });
        }

        while let Some(result) = probes.join_next().await {
            match result {
                Ok((backend, succeeded)) => apply_result(&backend, succeeded),
                Err(error) => tracing::warn!(error = %error, "probe task failed"),
            }
        }
    }
}

fn probe_jitter() -> Duration {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    Duration::from_nanos(u64::from(nanos) % PROBE_JITTER_MAX.as_nanos() as u64)
}

async fn probe(address: std::net::SocketAddr) -> bool {
    match time::timeout(PROBE_TIMEOUT, TcpStream::connect(address)).await {
        Ok(Ok(stream)) => {
            drop(stream);
            true
        }
        Ok(Err(_)) | Err(_) => false,
    }
}

fn apply_result(backend: &Backend, succeeded: bool) {
    let runtime = &backend.runtime;
    let healthy = runtime.healthy.load(Ordering::Acquire);

    if succeeded {
        runtime.probe_failures.store(0, Ordering::Relaxed);

        if healthy {
            runtime.probe_successes.store(0, Ordering::Relaxed);
            return;
        }

        let successes = runtime
            .probe_successes
            .load(Ordering::Relaxed)
            .saturating_add(1);

        if successes >= RISE {
            runtime.probe_successes.store(0, Ordering::Relaxed);
            runtime.healthy.store(true, Ordering::Release);
            tracing::info!(backend = %backend.address, "backend became healthy");
        } else {
            runtime.probe_successes.store(successes, Ordering::Relaxed);
        }

        return;
    }

    runtime.probe_successes.store(0, Ordering::Relaxed);

    if !healthy {
        runtime.probe_failures.store(0, Ordering::Relaxed);
        return;
    }

    let failures = runtime
        .probe_failures
        .load(Ordering::Relaxed)
        .saturating_add(1);

    if failures >= FALL {
        runtime.probe_failures.store(0, Ordering::Relaxed);
        runtime.healthy.store(false, Ordering::Release);
        tracing::warn!(backend = %backend.address, "backend became unhealthy");
    } else {
        runtime.probe_failures.store(failures, Ordering::Relaxed);
    }
}
