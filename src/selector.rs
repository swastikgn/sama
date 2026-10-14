use std::{
    collections::HashSet,
    net::SocketAddr,
    sync::{Arc, atomic::Ordering},
};

use crate::{
    algorithms::{
        consistent_hash, least_connections, power_of_two, random, round_robin,
        weighted_round_robin,
    },
    cli::config::Algorithm,
    snapshot::{Backend, Snapshot},
};

pub fn select_backend(
    snapshot: &Snapshot,
    client: Option<SocketAddr>,
) -> Option<Arc<Backend>> {
    select_backend_excluding(snapshot, &HashSet::new(), client)
}

pub fn select_backend_excluding(
    snapshot: &Snapshot,
    tried: &HashSet<SocketAddr>,
    client: Option<SocketAddr>,
) -> Option<Arc<Backend>> {
    let eligible: Vec<Arc<Backend>> = snapshot
        .backends
        .iter()
        .filter(|b| b.is_eligible() && !tried.contains(&b.address))
        .cloned()
        .collect();
    if eligible.is_empty() {
        return None;
    }

    match snapshot.algorithm {
        Algorithm::RoundRobin => {
            let n = snapshot.selection_cursor.fetch_add(1, Ordering::Relaxed);
            round_robin::select_backend(&eligible, n)
        }
        Algorithm::WeightedRoundRobin => weighted_round_robin::select_backend(&eligible),
        Algorithm::LeastConnections => least_connections::select_backend(&eligible),
        Algorithm::PowerOfTwoChoices => power_of_two::select_backend(&eligible),
        Algorithm::Random => random::select_backend(&eligible),
        Algorithm::ConsistentHash => {
            let key = client.map(|addr| addr.ip().to_string()).unwrap_or_default();
            consistent_hash::select_backend(&eligible, &key)
        }
    }
}
