use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};

use anyhow::{Context, Result};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::{
    cli::config::{Algorithm, Config, Server},
    snapshot::{Backend, BackendRuntime, Snapshot},
};

pub enum ConfigCommand {
    AddBackend {
        server: Server,
        reply: oneshot::Sender<Result<()>>,
    },
    UpdateBackend {
        address: SocketAddr,
        weight: Option<u32>,
        enabled: Option<bool>,
        draining: Option<bool>,
        reply: oneshot::Sender<Result<()>>,
    },
    RemoveBackend {
        address: SocketAddr,
        reply: oneshot::Sender<Result<()>>,
    },
    SetAlgorithm {
        algorithm: Algorithm,
        reply: oneshot::Sender<Result<()>>,
    },
}

pub async fn run(
    mut current: Config,
    toml_path: PathBuf,
    pool_tx: watch::Sender<Arc<Snapshot>>,
    mut commands: mpsc::Receiver<ConfigCommand>,
    shutdown: CancellationToken,
) {
    loop {
        let cmd = tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                tracing::info!("config writer shutting down");
                return;
            }
            cmd = commands.recv() => cmd,
        };
        let Some(cmd) = cmd else {
            tracing::info!("config channel closed, writer exiting");
            return;
        };
        match cmd {
            ConfigCommand::AddBackend { server, reply } => {
                let res = apply_and_publish(&mut current, &toml_path, &pool_tx, |candidate| {
                    if candidate
                        .servers
                        .iter()
                        .any(|s| s.address == server.address)
                    {
                        anyhow::bail!("duplicate server address: {}", server.address);
                    }
                    candidate.servers.push(server.clone());
                    Ok(())
                })
                .await;
                let _ = reply.send(res);
            }
            ConfigCommand::UpdateBackend {
                address,
                weight,
                enabled,
                draining,
                reply,
            } => {
                let res = apply_and_publish(&mut current, &toml_path, &pool_tx, |candidate| {
                    let srv = candidate
                        .servers
                        .iter_mut()
                        .find(|s| s.address == address)
                        .with_context(|| format!("no backend with address {address}"))?;
                    if let Some(w) = weight {
                        srv.weight = w;
                    }
                    if let Some(e) = enabled {
                        srv.enabled = e;
                    }
                    if let Some(d) = draining {
                        srv.draining = d;
                    }
                    Ok(())
                })
                .await;
                let _ = reply.send(res);
            }
            ConfigCommand::RemoveBackend { address, reply } => {
                let active = pool_tx
                    .borrow()
                    .backends
                    .iter()
                    .find(|b| b.address == address)
                    .map(|b| b.runtime.active_connections.load(Ordering::Relaxed))
                    .unwrap_or(0);
                if active > 0 {
                    let _ = reply.send(Err(anyhow::anyhow!(
                        "cannot delete {address}: active_connections = {active} (set draining = true first, wait for 0, then retry)"
                    )));
                    continue;
                }
                let res = apply_and_publish(&mut current, &toml_path, &pool_tx, |candidate| {
                    let before = candidate.servers.len();
                    candidate.servers.retain(|s| s.address != address);
                    if candidate.servers.len() == before {
                        anyhow::bail!("no backend with address {address}");
                    }
                    Ok(())
                })
                .await;
                let _ = reply.send(res);
            }
            ConfigCommand::SetAlgorithm { algorithm, reply } => {
                let res = apply_and_publish(&mut current, &toml_path, &pool_tx, |candidate| {
                    candidate.algorithm = algorithm;
                    Ok(())
                })
                .await;
                let _ = reply.send(res);
            }
        }
    }
}

async fn apply_and_publish(
    current: &mut Config,
    toml_path: &PathBuf,
    pool_tx: &watch::Sender<Arc<Snapshot>>,
    mutate: impl FnOnce(&mut Config) -> Result<()>,
) -> Result<()> {
    let mut candidate = current.clone();
    mutate(&mut candidate)?;
    candidate.validate()?;

    persist_toml(&candidate, toml_path).await?;

    let old = pool_tx.borrow().clone();
    let snapshot = rebuild_snapshot(&candidate, &old);
    *current = candidate;
    let _ = pool_tx.send(Arc::new(snapshot));
    Ok(())
}

fn rebuild_snapshot(candidate: &Config, old: &Snapshot) -> Snapshot {
    let runtimes: HashMap<SocketAddr, Arc<BackendRuntime>> = old
        .backends
        .iter()
        .map(|b| (b.address, Arc::clone(&b.runtime)))
        .collect();

    let backends = candidate
        .servers
        .iter()
        .map(|s| {
            let runtime = runtimes
                .get(&s.address)
                .map(Arc::clone)
                .unwrap_or_else(|| Arc::new(BackendRuntime::default()));
            Arc::new(Backend {
                address: s.address,
                weight: s.weight,
                enabled: s.enabled,
                draining: s.draining,
                runtime,
            })
        })
        .collect();

    Snapshot {
        algorithm: candidate.algorithm,
        backends,
        selection_cursor: std::sync::atomic::AtomicUsize::new(0),
    }
}

async fn persist_toml(config: &Config, toml_path: &PathBuf) -> Result<()> {
    let text = toml::to_string(config).context("failed to serialize config to TOML")?;
    let tmp_path = toml_path.with_extension("toml.tmp");
    tokio::fs::write(&tmp_path, text.as_bytes())
        .await
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;
    tokio::fs::rename(&tmp_path, toml_path)
        .await
        .with_context(|| format!("failed to replace {}", toml_path.display()))?;
    Ok(())
}
