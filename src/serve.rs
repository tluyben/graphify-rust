//! Query functions for the graphify knowledge graph.
//!
//! Ported from the Python `serve.py` module (query functions only;
//! the MCP server is omitted).

#![allow(dead_code, unused_imports)]

use std::collections::{HashMap, HashSet, VecDeque};

use unicode_normalization::UnicodeNormalization;

use crate::types::{EdgeAttrs, Graph, NodeAttrs};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const EXACT_MATCH_BONUS: f64 = 100.0;

/// Maps context names to hint words used for inferring context filters from
/// a natural-language question.
const CONTEXT_HINTS: &[(&str, &[&str])] = &[
    ("call", &["call", "calls", "called", "invoke", "invokes", "invoked"]),
    ("import", &["import", "imports", "imported", "module", "modules"]),
    ("field", &["field", "fields", "member", "members", "property", "properties"]),
    (
        "parameter_type",
        &[
            "parameter", "parameters", "param", "params", "argument", "arguments",
        ],
    ),
    ("return_type", &["return", "returns", "returned"]),
    ("generic_arg", &["generic", "generics", "template", "templates"]),
];

// ---------------------------------------------------------------------------
// Diacritics
// ---------------------------------------------------------------------------

/// Strip diacritics from `text` by NFKD decomposition and filtering combining
/// characters. Mirrors Python's `_strip_diacritics`.
fn strip_diacritics(text: &str) -> String {
    text.nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect()
}

// ---------------------------------------------------------------------------
// Context filters
// ---------------------------------------------------------------------------

fn normalize_context_filters(filters: &[&str]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut result = Vec::new();
    for &f in filters {
        let key = strip_diacritics(f).trim().to_lowercase();
        if !key.is_empty() && seen.insert(key.clone()) {
            result.push(key);
        }
    }
    result
}

fn infer_context_filters(question: &str) -> Vec<String> {
    let lowered: HashSet<String> = question
        .replace('?', " ")
        .replace(',', " ")
        .split_whitespace()
        .map(|t| strip_diacritics(t).to_lowercase())
        .collect();

    let mut inferred = Vec::new();
    for &(ctx, hints) in CONTEXT_HINTS {
        if hints.iter().any(|&h| lowered.contains(h)) {
            inferred.push(ctx.to_string());
        }
    }
    inferred
}

/// Returns `(filters, source)` where source is `Some("explicit")`,
/// `Some("heuristic")`, or `None`.
fn resolve_context_filters<'a>(
    question: &str,
    explicit_filters: Option<&[&str]>,
) -> (Vec<String>, Option<&'static str>) {
    if let Some(f) = explicit_filters {
        let normalized = normalize_context_filters(f);
        if !normalized.is_empty() {
            return (normalized, Some("explicit"));
        }
    }
    let inferred = infer_context_filters(question);
    if !inferred.is_empty() {
        return (inferred, Some("heuristic"));
    }
    (vec![], None)
}

/// Build a filtered adjacency view: keep all nodes but only edges whose
/// `context` attribute is in `filters`.  Returns the set of edges to use.
/// Rather than building a new Graph, we return a filtered edge predicate
/// captured as a closure in caller code — but since we need it in multiple
/// places, we collect filtered neighbors into a helper function.
fn filtered_neighbors<'a>(
    g: &'a Graph,
    node: &str,
    filters: &HashSet<String>,
) -> Vec<&'a str> {
    if filters.is_empty() {
        return g.neighbors(node);
    }
    match g.adj.get(node) {
        None => vec![],
        Some(nbrs) => nbrs
            .iter()
            .filter(|(_, attrs)| {
                attrs
                    .get("context")
                    .and_then(|v| v.as_str())
                    .map(|ctx| filters.contains(ctx))
                    .unwrap_or(false)
            })
            .map(|(n, _)| n.as_str())
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Node scoring
// ---------------------------------------------------------------------------

/// Score nodes by how well they match `terms`.
/// Returns a sorted list of `(score, node_id)` in descending order.
pub fn score_nodes(g: &Graph, terms: &[&str]) -> Vec<(f64, String)> {
    let norm_terms: Vec<String> = terms
        .iter()
        .map(|t| strip_diacritics(t).to_lowercase())
        .collect();

    let mut scored: Vec<(f64, String)> = Vec::new();

    for (nid, data) in &g.nodes {
        let norm_label = data
            .get("norm_label")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                strip_diacritics(
                    data.get("label")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                )
                .to_lowercase()
            });

        let source = data
            .get("source_file")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();

        let label_hits: f64 = norm_terms
            .iter()
            .filter(|t| norm_label.contains(t.as_str()))
            .count() as f64;
        let source_hits: f64 = norm_terms
            .iter()
            .filter(|t| source.contains(t.as_str()))
            .count() as f64
            * 0.5;

        let mut score = label_hits + source_hits;

        // Exact-match bonus: a term equals the full label (strip trailing `()`).
        let stripped_label = norm_label.trim_end_matches("()");
        if norm_terms
            .iter()
            .any(|t| t == &norm_label || t == stripped_label)
        {
            score += EXACT_MATCH_BONUS;
        }

        if score > 0.0 {
            scored.push((score, nid.clone()));
        }
    }

    // Sort descending by score.
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

// ---------------------------------------------------------------------------
// BFS / DFS traversal
// ---------------------------------------------------------------------------

/// Breadth-first traversal from `start_nodes` up to `depth` hops.
///
/// Returns `(visited_node_ids, edges_discovered)`.
pub fn bfs(
    g: &Graph,
    start_nodes: &[&str],
    depth: usize,
) -> (HashSet<String>, Vec<(String, String)>) {
    bfs_filtered(g, start_nodes, depth, &HashSet::new())
}

fn bfs_filtered(
    g: &Graph,
    start_nodes: &[&str],
    depth: usize,
    filters: &HashSet<String>,
) -> (HashSet<String>, Vec<(String, String)>) {
    let mut visited: HashSet<String> = start_nodes.iter().map(|&s| s.to_string()).collect();
    let mut frontier: HashSet<String> = visited.clone();
    let mut edges_seen: Vec<(String, String)> = Vec::new();

    for _ in 0..depth {
        let mut next_frontier: HashSet<String> = HashSet::new();
        for n in &frontier {
            let neighbors = filtered_neighbors(g, n, filters);
            for neighbor in neighbors {
                if !visited.contains(neighbor) {
                    next_frontier.insert(neighbor.to_string());
                    edges_seen.push((n.clone(), neighbor.to_string()));
                }
            }
        }
        visited.extend(next_frontier.iter().cloned());
        frontier = next_frontier;
    }

    (visited, edges_seen)
}

/// Depth-first traversal from `start_nodes` up to `depth` hops.
///
/// Returns `(visited_node_ids, edges_discovered)`.
pub fn dfs(
    g: &Graph,
    start_nodes: &[&str],
    depth: usize,
) -> (HashSet<String>, Vec<(String, String)>) {
    dfs_filtered(g, start_nodes, depth, &HashSet::new())
}

fn dfs_filtered(
    g: &Graph,
    start_nodes: &[&str],
    depth: usize,
    filters: &HashSet<String>,
) -> (HashSet<String>, Vec<(String, String)>) {
    let mut visited: HashSet<String> = HashSet::new();
    let mut edges_seen: Vec<(String, String)> = Vec::new();

    // Stack of (node, depth); reversed so first start_node is processed first.
    let mut stack: Vec<(String, usize)> = start_nodes
        .iter()
        .rev()
        .map(|&s| (s.to_string(), 0))
        .collect();

    while let Some((node, d)) = stack.pop() {
        if visited.contains(&node) || d > depth {
            continue;
        }
        visited.insert(node.clone());
        let neighbors = filtered_neighbors(g, &node, filters);
        for neighbor in neighbors {
            if !visited.contains(neighbor) {
                stack.push((neighbor.to_string(), d + 1));
                edges_seen.push((node.clone(), neighbor.to_string()));
            }
        }
    }

    (visited, edges_seen)
}

// ---------------------------------------------------------------------------
// Subgraph rendering
// ---------------------------------------------------------------------------

/// Sanitise a label for text output (strip control characters, cap length).
fn sanitize_label(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    if cleaned.len() > 256 {
        cleaned[..256].to_string()
    } else {
        cleaned
    }
}

/// Render a subgraph as text, truncated to approximately `token_budget` tokens
/// (using ~3 chars/token).
///
/// `seeds` are nodes that should appear first (i.e. the exact-match nodes).
pub fn subgraph_to_text(
    g: &Graph,
    nodes: &HashSet<String>,
    edges: &[(String, String)],
    token_budget: usize,
    seeds: Option<&[&str]>,
) -> String {
    let char_budget = token_budget * 3;
    let mut lines: Vec<String> = Vec::new();

    let _seed_set: HashSet<&str> = seeds.unwrap_or(&[]).iter().copied().collect();

    // Seeds first (in given order), then remaining nodes sorted by degree desc.
    let mut ordered: Vec<String> = seeds
        .unwrap_or(&[])
        .iter()
        .filter(|&&s| nodes.contains(s))
        .map(|&s| s.to_string())
        .collect();

    let seed_ids: HashSet<String> = ordered.iter().cloned().collect();

    let mut rest: Vec<String> = nodes
        .iter()
        .filter(|n| !seed_ids.contains(*n))
        .cloned()
        .collect();
    rest.sort_by(|a, b| g.degree(b).cmp(&g.degree(a)));
    ordered.extend(rest);

    for nid in &ordered {
        let d = match g.nodes.get(nid) {
            Some(d) => d,
            None => continue,
        };
        let label = sanitize_label(
            d.get("label")
                .and_then(|v| v.as_str())
                .unwrap_or(nid),
        );
        let source_file = d
            .get("source_file")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let source_location = d
            .get("source_location")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let community = d
            .get("community")
            .map(|v| v.to_string())
            .unwrap_or_default();
        lines.push(format!(
            "NODE {label} [src={source_file} loc={source_location} community={community}]"
        ));
    }

    let node_set: HashSet<&str> = ordered.iter().map(|s| s.as_str()).collect();

    for (u, v) in edges {
        if !node_set.contains(u.as_str()) || !node_set.contains(v.as_str()) {
            continue;
        }
        let edge_attrs = g.adj.get(u.as_str()).and_then(|m| m.get(v.as_str()));
        let (relation, confidence, context) = if let Some(attrs) = edge_attrs {
            let rel = attrs
                .get("relation")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let conf = attrs
                .get("confidence")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let ctx = attrs
                .get("context")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (rel, conf, ctx)
        } else {
            (String::new(), String::new(), None)
        };

        let context_suffix = context
            .as_deref()
            .map(|c| format!(" context={c}"))
            .unwrap_or_default();

        let u_label = sanitize_label(
            g.nodes
                .get(u.as_str())
                .and_then(|d| d.get("label"))
                .and_then(|v| v.as_str())
                .unwrap_or(u),
        );
        let v_label = sanitize_label(
            g.nodes
                .get(v.as_str())
                .and_then(|d| d.get("label"))
                .and_then(|v| v.as_str())
                .unwrap_or(v),
        );

        lines.push(format!(
            "EDGE {u_label} --{relation} [{confidence}{context_suffix}]--> {v_label}"
        ));
    }

    let mut output = lines.join("\n");
    if output.len() > char_budget {
        output.truncate(char_budget);
        output.push_str(&format!(
            "\n... (truncated to ~{token_budget} token budget)"
        ));
    }
    output
}

// ---------------------------------------------------------------------------
// Main query entry point
// ---------------------------------------------------------------------------

/// Search the graph using `question` and return a text summary of the
/// subgraph most relevant to that question.
///
/// Mirrors Python's `_query_graph_text`.
pub fn query_graph_text(
    g: &Graph,
    question: &str,
    mode: &str,
    depth: usize,
    token_budget: usize,
    context_filters: Option<&[&str]>,
) -> String {
    let terms: Vec<&str> = question
        .split_whitespace()
        .filter(|t| t.len() > 2)
        .map(|t| t)
        .collect();
    let term_strs: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();
    let term_refs: Vec<&str> = term_strs.iter().map(|s| s.as_str()).collect();

    let scored = score_nodes(g, &term_refs);
    let start_nodes: Vec<&str> = scored.iter().take(3).map(|(_, id)| id.as_str()).collect();

    if start_nodes.is_empty() {
        return "No matching nodes found.".to_string();
    }

    let (resolved_filters, filter_source) = resolve_context_filters(question, context_filters);
    let filter_set: HashSet<String> = resolved_filters.iter().cloned().collect();

    let (nodes, edges) = if mode == "dfs" {
        dfs_filtered(g, &start_nodes, depth, &filter_set)
    } else {
        bfs_filtered(g, &start_nodes, depth, &filter_set)
    };

    // Build header.
    let start_labels: Vec<String> = start_nodes
        .iter()
        .map(|&n| {
            g.nodes
                .get(n)
                .and_then(|d| d.get("label"))
                .and_then(|v| v.as_str())
                .unwrap_or(n)
                .to_string()
        })
        .collect();

    let mut header_parts = vec![
        format!("Traversal: {} depth={}", mode.to_uppercase(), depth),
        format!("Start: {:?}", start_labels),
    ];

    if !resolved_filters.is_empty() {
        header_parts.push(format!(
            "Context: {} ({})",
            resolved_filters.join(", "),
            filter_source.unwrap_or("unknown")
        ));
    }

    header_parts.push(format!("{} nodes found", nodes.len()));

    let header = header_parts.join(" | ") + "\n\n";

    let seed_strings: Vec<&str> = start_nodes.to_vec();
    header + &subgraph_to_text(g, &nodes, &edges, token_budget, Some(&seed_strings))
}

// ---------------------------------------------------------------------------
// Node lookup
// ---------------------------------------------------------------------------

/// Return node IDs whose label or ID contains `label` (diacritic-insensitive).
///
/// Mirrors Python's `_find_node`.
pub fn find_node(g: &Graph, label: &str) -> Vec<String> {
    let term = strip_diacritics(label).to_lowercase();
    g.nodes
        .iter()
        .filter(|(nid, d)| {
            let norm_label = d
                .get("norm_label")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    strip_diacritics(
                        d.get("label")
                            .and_then(|v| v.as_str())
                            .unwrap_or(""),
                    )
                    .to_lowercase()
                });
            norm_label.contains(&term) || nid.to_lowercase() == term
        })
        .map(|(nid, _)| nid.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Communities
// ---------------------------------------------------------------------------

/// Reconstruct the community map from `community` attributes stored on nodes.
///
/// Mirrors Python's `_communities_from_graph`.
pub fn communities_from_graph(g: &Graph) -> HashMap<i64, Vec<String>> {
    let mut communities: HashMap<i64, Vec<String>> = HashMap::new();
    for (nid, data) in &g.nodes {
        if let Some(cid) = data
            .get("community")
            .and_then(|v| v.as_i64())
        {
            communities.entry(cid).or_default().push(nid.clone());
        }
    }
    communities
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Graph;
    use std::collections::HashMap;

    fn make_test_graph() -> Graph {
        let mut g = Graph::new(false);

        let mut n1: HashMap<String, serde_json::Value> = HashMap::new();
        n1.insert("label".into(), serde_json::json!("FooBar"));
        n1.insert("source_file".into(), serde_json::json!("src/foo.rs"));
        n1.insert("community".into(), serde_json::json!(0));
        g.add_node("foo_bar", n1);

        let mut n2: HashMap<String, serde_json::Value> = HashMap::new();
        n2.insert("label".into(), serde_json::json!("BazQux"));
        n2.insert("source_file".into(), serde_json::json!("src/baz.rs"));
        n2.insert("community".into(), serde_json::json!(1));
        g.add_node("baz_qux", n2);

        let mut e: HashMap<String, serde_json::Value> = HashMap::new();
        e.insert("relation".into(), serde_json::json!("calls"));
        e.insert("confidence".into(), serde_json::json!("EXTRACTED"));
        e.insert("context".into(), serde_json::json!("call"));
        g.add_edge("foo_bar", "baz_qux", e);

        g
    }

    #[test]
    fn test_strip_diacritics() {
        assert_eq!(strip_diacritics("café"), "cafe");
        assert_eq!(strip_diacritics("naïve"), "naive");
        assert_eq!(strip_diacritics("hello"), "hello");
    }

    #[test]
    fn test_score_nodes_basic() {
        let g = make_test_graph();
        let scored = score_nodes(&g, &["foo"]);
        assert!(!scored.is_empty());
        assert_eq!(scored[0].1, "foo_bar");
    }

    #[test]
    fn test_score_nodes_exact_match() {
        let g = make_test_graph();
        let scored = score_nodes(&g, &["foobar"]);
        // "foobar" won't exactly match "FooBar" after normalisation since label is "FooBar"
        // but norm_label computed on the fly is "foobar"
        assert!(!scored.is_empty());
        assert!(scored[0].0 >= EXACT_MATCH_BONUS);
    }

    #[test]
    fn test_bfs() {
        let g = make_test_graph();
        let (visited, edges) = bfs(&g, &["foo_bar"], 1);
        assert!(visited.contains("foo_bar"));
        assert!(visited.contains("baz_qux"));
        assert!(!edges.is_empty());
    }

    #[test]
    fn test_dfs() {
        let g = make_test_graph();
        let (visited, _edges) = dfs(&g, &["foo_bar"], 2);
        assert!(visited.contains("foo_bar"));
        assert!(visited.contains("baz_qux"));
    }

    #[test]
    fn test_find_node() {
        let g = make_test_graph();
        let results = find_node(&g, "foo");
        assert!(results.contains(&"foo_bar".to_string()));
    }

    #[test]
    fn test_communities_from_graph() {
        let g = make_test_graph();
        let comms = communities_from_graph(&g);
        assert!(comms.contains_key(&0));
        assert!(comms.contains_key(&1));
    }

    #[test]
    fn test_query_graph_text() {
        let g = make_test_graph();
        let result = query_graph_text(&g, "foo bar", "bfs", 2, 500, None);
        assert!(result.contains("Traversal: BFS"));
    }

    #[test]
    fn test_subgraph_to_text_empty() {
        let g = make_test_graph();
        let nodes = HashSet::new();
        let edges = vec![];
        let text = subgraph_to_text(&g, &nodes, &edges, 100, None);
        assert_eq!(text, "");
    }

    #[test]
    fn test_infer_context_filters() {
        let filters = infer_context_filters("What does this function call?");
        assert!(filters.contains(&"call".to_string()));
    }

    #[test]
    fn test_context_filter_edges() {
        let g = make_test_graph();
        let (visited, _) = bfs_filtered(
            &g,
            &["foo_bar"],
            1,
            &std::collections::HashSet::from(["call".to_string()]),
        );
        // Edge has context=call so baz_qux should be reachable.
        assert!(visited.contains("baz_qux"));
    }
}
