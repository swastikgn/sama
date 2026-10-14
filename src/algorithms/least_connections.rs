use std::sync::{Arc, atomic::Ordering};

use crate::snapshot::Backend;

pub fn select_backend(backends: &[Arc<Backend>]) -> Option<Arc<Backend>> {
    let mut best: Option<Arc<Backend>> = None;
    let mut best_count = usize::MAX;

    for backend in backends {
        let count = backend.runtime.active_connections.load(Ordering::Relaxed);
        if count < best_count {
            best_count = count;
            best = Some(Arc::clone(backend));
        }
    }

    best
}
