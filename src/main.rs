use std::{
    sync::{Arc, atomic::AtomicU64},
    time::Instant,
};

use anyhow::Result;
use clap::Parser;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{EnvFilter, fmt};

use crate::{admin::state::AppState, cli::config::Config, snapshot::Snapshot};

pub mod admin;
pub mod algorithms;
pub mod cli;
pub mod health;
pub mod proxy;
pub mod selector;
pub mod snapshot;

#[tokio::main]
async fn main() -> Result<()> {
    let args = cli::cli::Cli::parse();
    init_tracing(args.verbose);

    let config_path = args.config.clone();
    let config = Config::load(args.config).await?;
    config.validate()?;
    let admin_addr = config.admin_listen_address;
    let listen_addr = config.listen_address;
    let max_connections = config.max_connections;
    let listen_backlog = config.listen_backlog;
    let snapshot = Snapshot::from_config(&config);

    tracing::info!(
        listen_address = %listen_addr,
        admin_address = %admin_addr,
        algorithm = ?config.algorithm,
        backends = config.servers.len(),
        "starting sama"
    );

    let shutdown = CancellationToken::new();
    spawn_signal_watcher(shutdown.clone());

    let (pool_tx, pool_rx) = tokio::sync::watch::channel(Arc::new(snapshot));
    let (config_tx, config_rx) = tokio::sync::mpsc::channel(32);
    let rejected_connections = Arc::new(AtomicU64::new(0));
    let started_at = Instant::now();

    let writer_config = config.clone();
    let writer_shutdown = shutdown.clone();
    let writer_handle = tokio::spawn(admin::writer::run(
        writer_config,
        config_path,
        pool_tx,
        config_rx,
        writer_shutdown,
    ));
    let health_handle = tokio::spawn(health::run(pool_rx.clone(), shutdown.clone()));

    let app_state = Arc::new(AppState {
        pool: pool_rx.clone(),
        config_tx: config_tx.clone(),
        rejected_connections: Arc::clone(&rejected_connections),
        started_at,
    });
    let app = admin::api::router(app_state);
    let admin_listener = tokio::net::TcpListener::bind(admin_addr).await?;
    tracing::info!(address = %admin_addr, "admin listening");
    tracing::info!(
        "UI at http://{admin_addr}/ — API at http://{admin_addr}/api/v1/backends — metrics at http://{admin_addr}/metrics"
    );

    let proxy_shutdown = shutdown.clone();
    let proxy_pool = pool_rx.clone();
    let mut proxy_handle = tokio::spawn(async move {
        proxy::run(
            listen_addr,
            proxy_pool,
            max_connections,
            listen_backlog,
            proxy_shutdown,
            rejected_connections,
        )
        .await
    });
    let admin_shutdown = shutdown.clone();
    let mut admin_handle = tokio::spawn(async move {
        axum::serve(admin_listener, app)
            .with_graceful_shutdown(admin_shutdown.cancelled_owned())
            .await
    });

    enum Exited {
        Admin,
        Proxy,
    }
    let exited = tokio::select! {
        biased;
        admin_result = &mut admin_handle => {
            match admin_result {
                Ok(Ok(())) => tracing::info!("admin server exited"),
                Ok(Err(error)) => tracing::error!(error = %error, "admin server failed"),
                Err(error) => tracing::error!(error = %error, "admin task panicked"),
            }
            shutdown.cancel();
            Exited::Admin
        }
        proxy_result = &mut proxy_handle => {
            match proxy_result {
                Ok(Ok(())) => tracing::info!("proxy exited cleanly"),
                Ok(Err(error)) => tracing::error!(error = %error, "proxy failed"),
                Err(error) => tracing::error!(error = %error, "proxy task panicked"),
            }
            shutdown.cancel();
            Exited::Proxy
        }
    };

    match exited {
        Exited::Admin => {
            let _ = (&mut proxy_handle).await;
        }
        Exited::Proxy => {
            let _ = (&mut admin_handle).await;
        }
    }

    drop(config_tx);
    let _ = tokio::join!(writer_handle, health_handle);

    tracing::info!("shutdown complete");
    Ok(())
}

fn init_tracing(verbose: bool) {
    let default_level = if verbose { "debug" } else { "info" };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("sama={default_level},axum=info")));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn spawn_signal_watcher(shutdown: CancellationToken) {
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();

        #[cfg(unix)]
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut sig) => {
                    sig.recv().await;
                }
                Err(error) => {
                    tracing::warn!(error = %error, "failed to watch SIGTERM, falling back to Ctrl-C only");
                    std::future::pending::<()>().await;
                }
            }
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => tracing::info!("received Ctrl-C, starting graceful shutdown"),
            _ = terminate => tracing::info!("received SIGTERM, starting graceful shutdown"),
            _ = shutdown.cancelled() => return,
        }
        shutdown.cancel();
    });
}
