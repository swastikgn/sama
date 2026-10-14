use std::sync::Arc;

use crate::snapshot::Backend;

pub fn select_backend(backends: &[Arc<Backend>], selection_number: usize) -> Option<Arc<Backend>> {
    if backends.is_empty() {
        return None;
    }

    Some(Arc::clone(&backends[selection_number % backends.len()]))
}
