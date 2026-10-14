use std::sync::{Arc, atomic::Ordering};

use crate::snapshot::Backend;

pub fn select_backend(backends: &[Arc<Backend>]) -> Option<Arc<Backend>> {
    if backends.is_empty() {
        return None;
    }

    let mut total: i64 = 0;
    let mut best: Option<(Arc<Backend>, i64)> = None;
    for backend in backends {
        let weight = i64::from(backend.weight);
        total += weight;
        let current = backend
            .runtime
            .smooth_current
            .fetch_add(weight, Ordering::Relaxed)
            + weight;
        let is_best = match &best {
            None => true,
            Some((_, best_current)) => current > *best_current,
        };
        if is_best {
            best = Some((Arc::clone(backend), current));
        }
    }
    if total == 0 {
        return None;
    }

    let (selected, _) = best?;
    selected
        .runtime
        .smooth_current
        .fetch_sub(total, Ordering::Relaxed);
    Some(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::BackendRuntime;
    use std::net::SocketAddr;

    fn backend(address: &str, weight: u32) -> Arc<Backend> {
        Arc::new(Backend {
            address: address.parse::<SocketAddr>().unwrap(),
            weight,
            enabled: true,
            draining: false,
            runtime: Arc::new(BackendRuntime::default()),
        })
    }

    fn pick_many(backends: &[Arc<Backend>], n: usize) -> Vec<SocketAddr> {
        (0..n)
            .map(|_| select_backend(backends).unwrap().address)
            .collect()
    }

    #[test]
    fn empty_returns_none() {
        assert!(select_backend(&[]).is_none());
    }

    #[test]
    fn all_zero_weights_returns_none() {
        let backends = vec![backend("127.0.0.1:8005", 0), backend("127.0.0.1:8006", 0)];
        assert!(select_backend(&backends).is_none());
    }

    #[test]
    fn single_backend_always_selected() {
        let backends = vec![backend("127.0.0.1:8005", 5)];
        for _ in 0..10 {
            assert_eq!(
                select_backend(&backends).unwrap().address,
                backends[0].address
            );
        }
    }

    #[test]
    fn equal_weights_rotate_strictly() {
        let backends = vec![
            backend("127.0.0.1:8005", 1),
            backend("127.0.0.1:8006", 1),
            backend("127.0.0.1:8007", 1),
        ];
        let picks = pick_many(&backends, 6);
        let expected = vec![
            backends[0].address,
            backends[1].address,
            backends[2].address,
            backends[0].address,
            backends[1].address,
            backends[2].address,
        ];
        assert_eq!(picks, expected);
    }

    #[test]
    fn weighted_ratio_over_full_cycle() {
        let backends = vec![
            backend("127.0.0.1:8005", 12),
            backend("127.0.0.1:8006", 1),
            backend("127.0.0.1:8007", 1),
        ];
        let picks = pick_many(&backends, 14);
        let count = |addr: SocketAddr| picks.iter().filter(|a| **a == addr).count();
        assert_eq!(count(backends[0].address), 12);
        assert_eq!(count(backends[1].address), 1);
        assert_eq!(count(backends[2].address), 1);
    }

    #[test]
    fn weighted_picks_are_spread_not_bursty() {
        let backends = vec![
            backend("127.0.0.1:8005", 12),
            backend("127.0.0.1:8006", 1),
            backend("127.0.0.1:8007", 1),
        ];
        let picks = pick_many(&backends, 14);
        let first_half_has_light_backend = picks[..7]
            .iter()
            .any(|a| *a == backends[1].address || *a == backends[2].address);
        assert!(
            first_half_has_light_backend,
            "light backends must appear in the first half: {picks:?}"
        );
    }

    #[test]
    fn state_returns_to_zero_after_full_cycle() {
        let backends = vec![
            backend("127.0.0.1:8005", 12),
            backend("127.0.0.1:8006", 1),
            backend("127.0.0.1:8007", 1),
        ];
        pick_many(&backends, 14);
        for b in &backends {
            assert_eq!(b.runtime.smooth_current.load(Ordering::Relaxed), 0);
        }
    }

    #[test]
    fn subset_selection_still_sums_to_subset_total() {
        let a = backend("127.0.0.1:8005", 3);
        let b = backend("127.0.0.1:8006", 1);
        let subset = vec![Arc::clone(&b)];
        for _ in 0..4 {
            assert_eq!(select_backend(&subset).unwrap().address, b.address);
        }
        assert_eq!(b.runtime.smooth_current.load(Ordering::Relaxed), 0);
        let _ = a;
    }
}
