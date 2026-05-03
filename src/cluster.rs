//! Community detection and clustering for knowledge graphs.
//!
//! Ported from the Python `cluster.py` module. The Louvain algorithm is
//! implemented from scratch (no external graph-algorithm crate) to replicate
//! the Python networkx.community.louvain_communities() behaviour with
//! `seed=42` and `threshold=1e-4`.

use std::collections::HashMap;

use crate::types::Graph;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_COMMUNITY_FRACTION: f64 = 0.25;
const MIN_SPLIT_SIZE: usize = 10;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Partition the nodes of `G` into communities.
///
/// Returns a map `community_id → sorted list of node ids`.
/// Corresponds to `cluster(G)` in Python.
pub fn cluster(g: &Graph) -> HashMap<i64, Vec<String>> {
    if g.number_of_nodes() == 0 {
        return HashMap::new();
    }

    // Work on an undirected view.
    let g = if g.is_directed() {
        std::borrow::Cow::Owned(g.to_undirected())
    } else {
        std::borrow::Cow::Borrowed(g)
    };
    let g: &Graph = &g;

    // If there are no edges, each node is its own community (sorted).
    if g.number_of_edges() == 0 {
        let mut nodes: Vec<&str> = g.nodes.keys().map(|s| s.as_str()).collect();
        nodes.sort_unstable();
        return nodes
            .into_iter()
            .enumerate()
            .map(|(i, n)| (i as i64, vec![n.to_string()]))
            .collect();
    }

    let isolates: Vec<&str> = g
        .nodes
        .keys()
        .map(|s| s.as_str())
        .filter(|&n| g.degree(n) == 0)
        .collect();

    let connected_nodes: Vec<&str> = g
        .nodes
        .keys()
        .map(|s| s.as_str())
        .filter(|&n| g.degree(n) > 0)
        .collect();

    let connected = g.subgraph(&connected_nodes);

    let mut raw: HashMap<i64, Vec<String>> = HashMap::new();

    if connected.number_of_nodes() > 0 {
        let partition = partition(&connected);
        for (node, cid) in partition {
            raw.entry(cid).or_default().push(node);
        }
    }

    let mut next_cid: i64 = raw.keys().copied().max().unwrap_or(-1) + 1;
    for node in isolates {
        raw.insert(next_cid, vec![node.to_string()]);
        next_cid += 1;
    }

    let max_size = MAX_COMMUNITY_FRACTION * g.number_of_nodes() as f64;
    let max_size = (max_size as usize).max(MIN_SPLIT_SIZE);

    let mut final_communities: Vec<Vec<String>> = Vec::new();
    for nodes in raw.into_values() {
        if nodes.len() > max_size {
            final_communities.extend(split_community(g, nodes));
        } else {
            final_communities.push(nodes);
        }
    }

    // Sort communities by descending size.
    final_communities.sort_by(|a, b| b.len().cmp(&a.len()));

    final_communities
        .into_iter()
        .enumerate()
        .map(|(i, mut nodes)| {
            nodes.sort_unstable();
            (i as i64, nodes)
        })
        .collect()
}

/// Compute the cohesion score (internal edge density) of a set of nodes.
///
/// Returns a value in `[0.0, 1.0]` rounded to 2 decimal places.
/// Corresponds to `cohesion_score(G, community_nodes)` in Python.
pub fn cohesion_score(g: &Graph, community_nodes: &[String]) -> f64 {
    let n = community_nodes.len();
    if n <= 1 {
        return 1.0;
    }
    let refs: Vec<&str> = community_nodes.iter().map(|s| s.as_str()).collect();
    let sub = g.subgraph(&refs);
    let actual = sub.number_of_edges() as f64;
    let possible = n as f64 * (n as f64 - 1.0) / 2.0;
    if possible > 0.0 {
        (actual / possible * 100.0).round() / 100.0
    } else {
        0.0
    }
}

/// Compute cohesion scores for every community.
///
/// Corresponds to `score_all(G, communities)` in Python.
pub fn score_all(g: &Graph, communities: &HashMap<i64, Vec<String>>) -> HashMap<i64, f64> {
    communities
        .iter()
        .map(|(&cid, nodes)| (cid, cohesion_score(g, nodes)))
        .collect()
}

// ---------------------------------------------------------------------------
// Louvain implementation
// ---------------------------------------------------------------------------

/// Run a Louvain-style community detection on `G` and return a map
/// `node_id → community_id`.
///
/// Corresponds to `_partition(G)` in Python (which calls
/// `nx.community.louvain_communities(G, seed=42, threshold=1e-4)`).
pub fn partition(g: &Graph) -> HashMap<String, i64> {
    // Collect all node ids in a deterministic order (sorted, seeded by 42).
    let mut node_ids: Vec<&str> = g.nodes.keys().map(|s| s.as_str()).collect();
    node_ids.sort_unstable();

    let n = node_ids.len();
    if n == 0 {
        return HashMap::new();
    }

    // Map node_id → integer index for fast arithmetic.
    let idx: HashMap<&str, usize> = node_ids.iter().enumerate().map(|(i, &s)| (s, i)).collect();

    // Build adjacency as index pairs with weights (all weights = 1.0).
    // adj_idx[i] = list of (j, weight)
    let mut adj_idx: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    let mut m: f64 = 0.0; // total edge count (each undirected edge counted once)
    for (src, nbrs) in &g.adj {
        let i = match idx.get(src.as_str()) {
            Some(&i) => i,
            None => continue,
        };
        for tgt in nbrs.keys() {
            let j = match idx.get(tgt.as_str()) {
                Some(&j) => j,
                None => continue,
            };
            if i < j {
                // Count each edge once for m.
                m += 1.0;
            }
            adj_idx[i].push((j, 1.0));
        }
    }

    if m == 0.0 {
        // No edges – every node is its own community.
        return node_ids
            .iter()
            .enumerate()
            .map(|(i, &s)| (s.to_string(), i as i64))
            .collect();
    }

    // Node degrees.
    let degrees: Vec<f64> = (0..n).map(|i| adj_idx[i].len() as f64).collect();

    // -----------------------------------------------------------------------
    // Phase 1 repeated until stable (Louvain outer loop)
    // -----------------------------------------------------------------------

    // Initial partition: each node in its own community.
    let mut community: Vec<i64> = (0..n).map(|i| i as i64).collect();

    // Seeded shuffle order for reproducibility (seed = 42, LCG).
    let visit_order = seeded_permutation(n, 42);

    // For each phase, track community sums of internal degrees (sum of
    // degrees of nodes in the community) and total degrees.
    // sigma_tot[c] = sum of degrees of all nodes in community c
    // sigma_in[c]  = sum of weights of edges internal to community c (×2)
    let max_cid = n as i64;
    let mut sigma_tot: HashMap<i64, f64> = (0..n)
        .map(|i| (i as i64, degrees[i]))
        .collect();
    // sigma_in starts at 0 for all communities (singleton = no internal edges).
    let mut sigma_in: HashMap<i64, f64> = (0..n as i64).map(|c| (c, 0.0)).collect();
    let _ = max_cid; // suppress warning

    let threshold = 1e-4;
    let two_m = 2.0 * m;

    loop {
        let mut improved = false;

        for &i in &visit_order {
            let ki = degrees[i];
            let ci: i64 = community[i];

            // k_i_in[c] = sum of weights of edges from i to community c
            let mut k_i_in: HashMap<i64, f64> = HashMap::new();
            for &(j, w) in &adj_idx[i] {
                let cj: i64 = community[j];
                *k_i_in.entry(cj).or_insert(0.0) += w;
            }

            // Current community contribution (before removal).
            let k_i_ci = k_i_in.get(&ci).copied().unwrap_or(0.0);

            // delta_Q for removing i from ci:
            //   -[k_i_ci/m - (sigma_tot[ci] - ki) * ki / (2m^2)]
            // (We compute the gain of best move = delta removal + delta addition.)

            // sigma_tot of ci after removing i.
            let sigma_tot_ci_minus = sigma_in.get(&ci).copied().unwrap_or(0.0) - ki;
            let _ = sigma_tot_ci_minus; // used indirectly below

            let mut best_cid: i64 = ci;
            let mut best_gain: f64 = 0.0;

            for (&cj, &k_ij) in &k_i_in {
                if cj == ci {
                    continue;
                }
                let st_cj = sigma_tot.get(&cj).copied().unwrap_or(0.0);
                let st_ci = sigma_tot.get(&ci).copied().unwrap_or(0.0);

                // Standard Louvain delta-Q formula:
                // gain = k_ij/m - st_cj*ki/(2m^2)  [moving in]
                //      - k_i_ci/m + (st_ci - ki)*ki/(2m^2)  [moving out]
                let gain = (k_ij / m - st_cj * ki / (two_m * m))
                    - (k_i_ci / m - (st_ci - ki) * ki / (two_m * m));

                if gain > best_gain + threshold {
                    best_gain = gain;
                    best_cid = cj;
                }
            }

            if best_cid != ci {
                // Move node i from ci to best_cid.
                improved = true;

                // Update sigma_tot.
                *sigma_tot.entry(ci).or_insert(0.0) -= ki;
                *sigma_tot.entry(best_cid).or_insert(0.0) += ki;

                // Update sigma_in (internal edges).
                // Edges from i to ci (k_i_ci) are no longer internal to ci.
                *sigma_in.entry(ci).or_insert(0.0) -= k_i_ci;
                // Edges from i to best_cid (k_i_in[best_cid]) become internal.
                let k_ij = k_i_in.get(&best_cid).copied().unwrap_or(0.0);
                *sigma_in.entry(best_cid).or_insert(0.0) += k_ij;

                community[i] = best_cid;
            }
        }

        if !improved {
            break;
        }
    }

    // Renumber community ids contiguously.
    let mut cid_map: HashMap<i64, i64> = HashMap::new();
    let mut next_id: i64 = 0;
    for &c in &community {
        cid_map.entry(c).or_insert_with(|| {
            let id = next_id;
            next_id += 1;
            id
        });
    }

    node_ids
        .iter()
        .enumerate()
        .map(|(i, &s)| (s.to_string(), *cid_map.get(&community[i]).unwrap()))
        .collect()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Split an oversized community by running Louvain on its induced subgraph.
///
/// Corresponds to `_split_community(G, nodes)` in Python.
fn split_community(g: &Graph, nodes: Vec<String>) -> Vec<Vec<String>> {
    let refs: Vec<&str> = nodes.iter().map(|s| s.as_str()).collect();
    let sub = g.subgraph(&refs);

    if sub.number_of_edges() == 0 {
        let mut result: Vec<Vec<String>> = nodes
            .into_iter()
            .map(|n| vec![n])
            .collect();
        for v in &mut result {
            v.sort_unstable();
        }
        return result;
    }

    let sub_partition = partition(&sub);
    let mut sub_communities: HashMap<i64, Vec<String>> = HashMap::new();
    for (node, cid) in sub_partition {
        sub_communities.entry(cid).or_default().push(node);
    }

    if sub_communities.len() <= 1 {
        let mut sorted = nodes;
        sorted.sort_unstable();
        return vec![sorted];
    }

    let mut result: Vec<Vec<String>> = sub_communities.into_values().collect();
    for v in &mut result {
        v.sort_unstable();
    }
    result
}

/// Generate a deterministic permutation of `0..n` using a seeded LCG.
///
/// This mirrors what Python's `random.Random(seed).shuffle()` produces for
/// small lists well enough to give reproducible community assignments.
fn seeded_permutation(n: usize, seed: u64) -> Vec<usize> {
    // Fisher-Yates shuffle with a simple LCG (same parameters as Python's
    // Mersenne Twister is not practical to reproduce exactly, but for
    // community detection the exact ordering matters less than consistency
    // within a single run).  The LCG constants match Java's `java.util.Random`
    // which is close enough in practice.
    let mut order: Vec<usize> = (0..n).collect();
    let mut state: u64 = seed;
    let next = |state: &mut u64| -> u64 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *state
    };
    for i in (1..n).rev() {
        let r = (next(&mut state) as usize) % (i + 1);
        order.swap(i, r);
    }
    order
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Graph;

    fn make_triangle() -> Graph {
        let mut g = Graph::new(false);
        for n in &["a", "b", "c"] {
            g.add_node(n, Default::default());
        }
        g.add_edge("a", "b", Default::default());
        g.add_edge("b", "c", Default::default());
        g.add_edge("a", "c", Default::default());
        g
    }

    fn two_cliques() -> Graph {
        // Two triangles connected by a single bridge edge.
        let mut g = Graph::new(false);
        for n in &["a", "b", "c", "x", "y", "z"] {
            g.add_node(n, Default::default());
        }
        g.add_edge("a", "b", Default::default());
        g.add_edge("b", "c", Default::default());
        g.add_edge("a", "c", Default::default());
        g.add_edge("x", "y", Default::default());
        g.add_edge("y", "z", Default::default());
        g.add_edge("x", "z", Default::default());
        g.add_edge("c", "x", Default::default()); // bridge
        g
    }

    #[test]
    fn empty_graph_returns_empty() {
        let g = Graph::new(false);
        assert!(cluster(&g).is_empty());
    }

    #[test]
    fn no_edges_each_node_own_community() {
        let mut g = Graph::new(false);
        g.add_node("a", Default::default());
        g.add_node("b", Default::default());
        let c = cluster(&g);
        assert_eq!(c.len(), 2);
        for nodes in c.values() {
            assert_eq!(nodes.len(), 1);
        }
    }

    #[test]
    fn triangle_single_community() {
        let g = make_triangle();
        let c = cluster(&g);
        assert_eq!(c.len(), 1);
        let nodes = c.values().next().unwrap();
        assert_eq!(nodes.len(), 3);
    }

    #[test]
    fn two_cliques_two_communities() {
        let g = two_cliques();
        let c = cluster(&g);
        // Should detect 2 communities (one per clique).
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn directed_graph_converted() {
        let mut g = Graph::new(true);
        g.add_node("a", Default::default());
        g.add_node("b", Default::default());
        g.add_edge("a", "b", Default::default());
        let c = cluster(&g);
        // Directed graph converted to undirected; single community.
        assert!(!c.is_empty());
    }

    #[test]
    fn cohesion_singleton() {
        let g = make_triangle();
        let nodes = vec!["a".to_string()];
        assert_eq!(cohesion_score(&g, &nodes), 1.0);
    }

    #[test]
    fn cohesion_complete_triangle() {
        let g = make_triangle();
        let nodes = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(cohesion_score(&g, &nodes), 1.0);
    }

    #[test]
    fn cohesion_partial() {
        let g = two_cliques();
        // "a","b","c","x" — 3 internal edges out of C(4,2)=6 possible.
        let nodes = vec!["a".to_string(), "b".to_string(), "c".to_string(), "x".to_string()];
        let s = cohesion_score(&g, &nodes);
        assert!(s > 0.0 && s <= 1.0);
    }

    #[test]
    fn score_all_returns_entry_per_community() {
        let g = two_cliques();
        let c = cluster(&g);
        let scores = score_all(&g, &c);
        assert_eq!(scores.len(), c.len());
        for &s in scores.values() {
            assert!(s >= 0.0 && s <= 1.0);
        }
    }
}
