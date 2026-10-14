use std::sync::Arc;

use rand::{Rng, thread_rng};

use crate::snapshot::Backend;

pub fn select_backend(backends: &[Arc<Backend>]) -> Option<Arc<Backend>> {
    match backends.len() {
        0 => None,
        1 => Some(Arc::clone(&backends[0])),
        n => {
            let mut rng = thread_rng();
            Some(Arc::clone(&backends[rng.gen_range(0..n)]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::BackendRuntime;
    use std::net::SocketAddr;

    fn backend(address: &str) -> Arc<Backend> {
        Arc::new(Backend {
            address: address.parse::<SocketAddr>().unwrap(),
            weight: 1,
            enabled: true,
            draining: false,
            runtime: Arc::new(BackendRuntime::default()),
        })
    }

    #[test]
    fn empty_returns_none() {
        assert!(select_backend(&[]).is_none());
    }

    #[test]
    fn single_backend_always_selected() {
        let backends = vec![backend("127.0.0.1:8005")];
        for _ in 0..10 {
            assert_eq!(
                select_backend(&backends).unwrap().address,
                backends[0].address
            );
        }
    }

    #[test]
    fn spreads_evenly_over_many_picks() {
        let backends = vec![
            backend("127.0.0.1:8005"),
            backend("127.0.0.1:8006"),
            backend("127.0.0.1:8007"),
        ];
        let mut counts = [0usize; 3];
        for _ in 0..900 {
            let addr = select_backend(&backends).unwrap().address;
            let idx = backends.iter().position(|b| b.address == addr).unwrap();
            counts[idx] += 1;
        }
        for (i, count) in counts.iter().enumerate() {
            assert!(
                (200..=400).contains(count),
                "backend {i} got {count}/900 picks, expected roughly even"
            );
        }
    }
}
