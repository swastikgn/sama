use std::sync::Arc;

use crate::snapshot::Backend;

/// Weighted rendezvous (highest-random-weight) consistent hashing.
///
/// For each eligible backend we hash `key + backend address` into a uniform
/// value in (0, 1) and score it as `uniform^(1/weight)`. The highest score
/// wins. Properties:
/// - Same `key` (client IP) always maps to the same backend while the
///   eligible set is unchanged (stickiness).
/// - Adding/removing a backend only remaps keys that belonged to it
///   (minimal disruption), unlike `hash % N`.
/// - Weights bias the win probability proportionally (weight 3 vs 1 gets
///   roughly 75% / 25% of keys).
/// - Failover-safe: the caller filters `tried` backends out first and we
///   re-rank only the survivors, so a retry keeps the deterministic order
///   minus the failed node.
pub fn select_backend(backends: &[Arc<Backend>], key: &str) -> Option<Arc<Backend>> {
    let mut best: Option<Arc<Backend>> = None;
    let mut best_score = f64::NEG_INFINITY;

    for backend in backends {
        let score = score_for(key, &backend.address.to_string(), backend.weight);
        if score > best_score {
            best_score = score;
            best = Some(Arc::clone(backend));
        }
    }

    best
}

fn score_for(key: &str, backend_addr: &str, weight: u32) -> f64 {
    let mut buf = Vec::with_capacity(key.len() + backend_addr.len() + 1);
    buf.extend_from_slice(key.as_bytes());
    buf.push(0);
    buf.extend_from_slice(backend_addr.as_bytes());
    let hash = fnv1a64(&buf);
    // Top 53 bits -> uniform double in [0, 1).
    let uniform = ((hash >> 11) as f64) * (1.0 / ((1u64 << 53) as f64));
    let uniform = uniform.clamp(1e-9, 1.0 - 1e-9);
    let weight = f64::from(weight.max(1));
    uniform.powf(1.0 / weight)
}

fn fnv1a64(data: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET_BASIS;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    // Murmur3 fmix64 avalanche: FNV's trailing-byte differences (our backend
    // addresses share a long common prefix and differ in the last char) stay
    // correlated without this, which skewed key distribution. The mixer
    // avalanches every input bit across all 64 output bits.
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xff51afd7ed558ccd);
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xc4ceb9fe1a85ec53);
    hash ^= hash >> 33;
    hash
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

    fn three_equal() -> Vec<Arc<Backend>> {
        vec![
            backend("127.0.0.1:8005", 1),
            backend("127.0.0.1:8006", 1),
            backend("127.0.0.1:8007", 1),
        ]
    }

    #[test]
    fn empty_returns_none() {
        assert!(select_backend(&[], "10.0.0.1").is_none());
    }

    #[test]
    fn single_backend_always_selected() {
        let backends = vec![backend("127.0.0.1:8005", 1)];
        for key in ["10.0.0.1", "10.0.0.2", "192.168.1.9"] {
            assert_eq!(
                select_backend(&backends, key).unwrap().address,
                backends[0].address
            );
        }
    }

    #[test]
    fn same_key_is_sticky() {
        let backends = three_equal();
        let first = select_backend(&backends, "10.0.0.42").unwrap().address;
        for _ in 0..100 {
            assert_eq!(
                select_backend(&backends, "10.0.0.42").unwrap().address,
                first
            );
        }
    }

    #[test]
    fn distinct_keys_spread_across_backends() {
        let backends = three_equal();
        let mut counts = [0usize; 3];
        for i in 0..3000 {
            let key = format!("10.0.1.{i}");
            let addr = select_backend(&backends, &key).unwrap().address;
            let idx = backends.iter().position(|b| b.address == addr).unwrap();
            counts[idx] += 1;
        }
        for (i, count) in counts.iter().enumerate() {
            assert!(
                (700..=1300).contains(count),
                "backend {i} got {count}/3000 keys, expected roughly even"
            );
        }
    }

    #[test]
    fn removal_only_remaps_owners_keys() {
        let full = three_equal();
        let keys: Vec<String> = (0..3000).map(|i| format!("10.0.2.{i}")).collect();
        let before: Vec<_> = keys
            .iter()
            .map(|k| select_backend(&full, k).unwrap().address)
            .collect();

        let survivors = vec![Arc::clone(&full[0]), Arc::clone(&full[1])];
        let mut remapped = 0;
        for (key, prev) in keys.iter().zip(before.iter()) {
            let next = select_backend(&survivors, key).unwrap().address;
            if next != *prev {
                remapped += 1;
                // Only keys owned by the removed backend may move.
                assert_eq!(
                    *prev, full[2].address,
                    "key {key} moved from {prev} to {next} though its owner survived"
                );
            }
        }
        // Removed backend owned ~1/3 of keys; allow generous tolerance.
        assert!(
            (700..=1300).contains(&remapped),
            "remapped {remapped}/3000, expected ~1000 (only the removed backend's keys)"
        );
    }

    #[test]
    fn weights_bias_key_share() {
        let backends = vec![
            backend("127.0.0.1:8005", 3),
            backend("127.0.0.1:8006", 1),
        ];
        let mut heavy = 0;
        for i in 0..2000 {
            let key = format!("10.0.3.{i}");
            if select_backend(&backends, &key).unwrap().address == backends[0].address {
                heavy += 1;
            }
        }
        assert!(
            (1300..=1700).contains(&heavy),
            "weight-3 backend got {heavy}/2000 keys, expected ~1500"
        );
    }
}
