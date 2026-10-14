use std::sync::{Arc, atomic::Ordering};

use rand::{Rng, thread_rng};

use crate::snapshot::Backend;

pub fn select_backend(backends: &[Arc<Backend>]) -> Option<Arc<Backend>> {
    match backends.len() {
        0 => None,
        1 => Some(Arc::clone(&backends[0])),
        n => {
            let mut rng = thread_rng();
            let a = rng.gen_range(0..n);
            let mut b = rng.gen_range(0..n - 1);
            if b >= a {
                b += 1;
            }
            let first = &backends[a];
            let second = &backends[b];
            let first_load = first.runtime.active_connections.load(Ordering::Relaxed);
            let second_load = second.runtime.active_connections.load(Ordering::Relaxed);
            if second_load < first_load {
                Some(Arc::clone(second))
            } else {
                Some(Arc::clone(first))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::BackendRuntime;
    use std::net::SocketAddr;

    fn backend(address: &str, active: usize) -> Arc<Backend> {
        let backend = Arc::new(Backend {
            address: address.parse::<SocketAddr>().unwrap(),
            weight: 1,
            enabled: true,
            draining: false,
            runtime: Arc::new(BackendRuntime::default()),
        });
        backend
            .runtime
            .active_connections
            .store(active, Ordering::Relaxed);
        backend
    }

    #[test]
    fn empty_returns_none() {
        assert!(select_backend(&[]).is_none());
    }

    #[test]
    fn single_backend_always_selected() {
        let backends = vec![backend("127.0.0.1:8005", 7)];
        for _ in 0..10 {
            assert_eq!(
                select_backend(&backends).unwrap().address,
                backends[0].address
            );
        }
    }

    #[test]
    fn idle_backend_wins_most_picks() {
        let backends = vec![
            backend("127.0.0.1:8005", 100),
            backend("127.0.0.1:8006", 0),
            backend("127.0.0.1:8007", 100),
        ];
        let mut idle_wins = 0;
        for _ in 0..600 {
            if select_backend(&backends).unwrap().address == backends[1].address {
                idle_wins += 1;
            }
        }
        assert!(
            idle_wins > 360,
            "idle backend won {idle_wins}/600 picks, expected ~400"
        );
    }

    #[test]
    fn prefers_lighter_of_two_loads() {
        let backends = vec![backend("127.0.0.1:8005", 10), backend("127.0.0.1:8006", 0)];
        for _ in 0..50 {
            assert_eq!(
                select_backend(&backends).unwrap().address,
                backends[1].address
            );
        }
    }

    #[test]
    fn equal_loads_spread_across_backends() {
        let backends = vec![
            backend("127.0.0.1:8005", 0),
            backend("127.0.0.1:8006", 0),
            backend("127.0.0.1:8007", 0),
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
