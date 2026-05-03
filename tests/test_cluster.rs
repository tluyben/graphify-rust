use graphify::build::build_from_json;
use graphify::cluster::{cluster, cohesion_score, score_all};
use graphify::types::Graph;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

fn make_graph() -> Graph {
    let content = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/extraction.json"),
    )
    .unwrap();
    let extraction: Value = serde_json::from_str(&content).unwrap();
    build_from_json(extraction, false)
}

#[test]
fn test_cluster_returns_dict() {
    let g = make_graph();
    let communities = cluster(&g);
    // Returns a non-empty HashMap when there are nodes
    assert!(!communities.is_empty());
}

#[test]
fn test_cluster_covers_all_nodes() {
    let g = make_graph();
    let communities = cluster(&g);
    let all_nodes_in_communities: std::collections::HashSet<String> = communities
        .values()
        .flat_map(|nodes| nodes.iter().cloned())
        .collect();
    let graph_nodes: std::collections::HashSet<String> =
        g.nodes.keys().cloned().collect();
    assert_eq!(all_nodes_in_communities, graph_nodes);
}

#[test]
fn test_cohesion_score_complete_graph() {
    use indexmap::IndexMap;

    let mut g = Graph {
        nodes: IndexMap::new(),
        adj: HashMap::new(),
        directed: false,
        graph: HashMap::new(),
    };
    let nodes = ["0", "1", "2", "3"];
    for n in &nodes {
        g.nodes.insert(n.to_string(), HashMap::new());
        g.adj.insert(n.to_string(), HashMap::new());
    }
    // Add all edges for complete graph
    for i in 0..4 {
        for j in 0..4 {
            if i != j {
                let u = nodes[i].to_string();
                let v = nodes[j].to_string();
                g.adj.entry(u.clone()).or_default().insert(v.clone(), HashMap::new());
            }
        }
    }
    let node_list: Vec<String> = nodes.iter().map(|s| s.to_string()).collect();
    let score = cohesion_score(&g, &node_list);
    assert!(
        (score - 1.0).abs() < 1e-9,
        "Expected 1.0, got {}",
        score
    );
}

#[test]
fn test_cohesion_score_single_node() {
    let mut g = Graph::new(false);
    g.add_node("a", HashMap::new());
    let nodes = vec!["a".to_string()];
    let score = cohesion_score(&g, &nodes);
    assert_eq!(score, 1.0);
}

#[test]
fn test_cohesion_score_disconnected() {
    let mut g = Graph::new(false);
    g.add_node("a", HashMap::new());
    g.add_node("b", HashMap::new());
    g.add_node("c", HashMap::new());
    let nodes = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let score = cohesion_score(&g, &nodes);
    assert_eq!(score, 0.0);
}

#[test]
fn test_cohesion_score_range() {
    let g = make_graph();
    let communities = cluster(&g);
    for (_cid, nodes) in &communities {
        let score = cohesion_score(&g, nodes);
        assert!(
            score >= 0.0 && score <= 1.0,
            "Cohesion score {} out of range",
            score
        );
    }
}

#[test]
fn test_score_all_keys_match_communities() {
    let g = make_graph();
    let communities = cluster(&g);
    let scores = score_all(&g, &communities);
    let community_keys: std::collections::HashSet<i64> = communities.keys().copied().collect();
    let score_keys: std::collections::HashSet<i64> = scores.keys().copied().collect();
    assert_eq!(score_keys, community_keys);
}
