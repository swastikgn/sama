use crate::cli::config::{Algorithm, Config};
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering},
    },
};

pub struct BackendRuntime {
    pub healthy: AtomicBool,
    pub active_connections: AtomicUsize,
    pub total_connections: AtomicU64,
    pub bytes_in: AtomicU64,
    pub bytes_out: AtomicU64,
    pub probe_failures: AtomicUsize,
    pub probe_successes: AtomicUsize,
    pub smooth_current: AtomicI64,

    pub connect_failures: AtomicU64,
}

impl Default for BackendRuntime {
    fn default() -> Self {
        Self {
            healthy: AtomicBool::new(false),
            active_connections: AtomicUsize::new(0),
            total_connections: AtomicU64::new(0),
            bytes_in: AtomicU64::new(0),
            bytes_out: AtomicU64::new(0),
            probe_failures: AtomicUsize::new(0),
            probe_successes: AtomicUsize::new(0),
            smooth_current: AtomicI64::new(0),
            connect_failures: AtomicU64::new(0),
        }
    }
}

pub struct Backend {
    pub address: SocketAddr,
    pub weight: u32,
    pub enabled: bool,
    pub draining: bool,
    pub runtime: Arc<BackendRuntime>,
}

impl Backend {
    pub fn is_eligible(&self) -> bool {
        self.enabled && !self.draining && self.runtime.healthy.load(Ordering::Acquire)
    }
}

pub struct Snapshot {
    pub algorithm: Algorithm,
    pub backends: Vec<Arc<Backend>>,
    pub selection_cursor: AtomicUsize,
}

impl Snapshot {
    pub fn from_config(config: &Config) -> Self {
        let backends = config
            .servers
            .iter()
            .map(|s| {
                Arc::new(Backend {
                    address: s.address,
                    weight: s.weight,
                    enabled: s.enabled,
                    draining: s.draining,
                    runtime: Arc::new(BackendRuntime::default()),
                })
            })
            .collect();

        Self {
            algorithm: config.algorithm,
            backends,
            selection_cursor: AtomicUsize::new(0),
        }
    }

    pub fn eligible(&self) -> Vec<Arc<Backend>> {
        self.backends
            .iter()
            .filter(|b| b.is_eligible())
            .cloned()
            .collect()
    }
}
