use std::{
    sync::{Arc, atomic::AtomicU64},
    time::Instant,
};

use tokio::sync::{mpsc, watch};

use crate::snapshot::Snapshot;

use super::writer::ConfigCommand;

pub struct AppState {
    pub pool: watch::Receiver<Arc<Snapshot>>,
    pub config_tx: mpsc::Sender<ConfigCommand>,
    pub rejected_connections: Arc<AtomicU64>,
    pub started_at: Instant,
}
