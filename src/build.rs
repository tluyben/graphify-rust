//! Build a [`Graph`] from a parsed knowledge-graph extraction JSON object.
//!
//! Ported from the Python `build.py` module. Logic is kept identical to the
//! original so that behaviour compares correctly with the Python implementation.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use crate::types::{EdgeAttrs, Graph, NodeAttrs};
use crate::validate::validate_extraction;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Replace every run of non-alphanumeric characters with `_`, strip leading /
/// trailing underscores, then lower-case the result.
///
/// Mirrors `_normalize_id` from Python exactly.
fn normalize_id(s: &str) -> String {
    // Replace each run of non-alphanumeric chars with a single `_`.
    let mut result = String::with_capacity(s.len());
    let mut in_sep = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            in_sep = false;
            result.push(ch.to_ascii_lowercase());
        } else {
            if !in_sep {
                result.push('_');
            }
            in_sep = true;
        }
    }
    // Strip leading/trailing underscores.
    result.trim_matches('_').to_string()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build a [`Graph`] from a single parsed extraction JSON object.
///
/// Corresponds to `build_from_json(extraction, *, directed=False)` in Python.
///
/// # Arguments
/// * `extraction` – A `serde_json::Value` that must be a JSON object
///   (`Value::Object`). The function is a no-op (returns an empty graph) if
///   the value is not an object.
/// * `directed` – When `true` the result is a directed graph.
pub fn build_from_json(extraction: Value, directed: bool) -> Graph {
    // Normalise the value to an owned JSON object map.
    let mut extraction: Map<String, Value> = match extraction {
        Value::Object(m) => m,
        _ => return Graph::new(directed),
    };

    // NetworkX <= 3.1 used "links" instead of "edges" – handle legacy data.
    if !extraction.contains_key("edges") && extraction.contains_key("links") {
        let links = extraction["links"].clone();
        extraction.insert("edges".to_string(), links);
    }

    // Fix legacy nodes that use "source" instead of "source_file".
    if let Some(Value::Array(nodes)) = extraction.get_mut("nodes") {
        for node in nodes.iter_mut() {
            if let Value::Object(node_obj) = node {
                if node_obj.contains_key("source") && !node_obj.contains_key("source_file") {
                    let val = node_obj.remove("source").unwrap();
                    node_obj.insert("source_file".to_string(), val);
                    eprintln!(
                        "[graphify] Warning: node used 'source' instead of 'source_file'; \
                         field renamed automatically."
                    );
                }
            }
        }
    }

    // Validate and surface real errors (suppress dangling-edge warnings).
    let all_errors = validate_extraction(&Value::Object(extraction.clone()));
    let real_errors: Vec<&String> = all_errors
        .iter()
        .filter(|e| !e.contains("does not match any node id"))
        .collect();
    if !real_errors.is_empty() {
        eprintln!(
            "[graphify] Extraction warning ({} issues): {}",
            real_errors.len(),
            real_errors[0]
        );
    }

    let mut g = Graph::new(directed);

    // -----------------------------------------------------------------------
    // Add nodes
    // -----------------------------------------------------------------------
    if let Some(Value::Array(nodes)) = extraction.get("nodes") {
        for node in nodes {
            if let Value::Object(node_obj) = node {
                let id = match node_obj.get("id").and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let attrs: NodeAttrs = node_obj
                    .iter()
                    .filter(|(k, _)| k.as_str() != "id")
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                g.add_node(&id, attrs);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Build normalised-id lookup for fuzzy edge resolution
    // -----------------------------------------------------------------------
    let node_set: HashSet<String> = g.nodes.keys().cloned().collect();
    // Build owned strings so the borrow on g.nodes ends before add_edge calls.
    let norm_to_id: HashMap<String, String> = g
        .nodes
        .keys()
        .map(|nid| (normalize_id(nid), nid.clone()))
        .collect();

    // -----------------------------------------------------------------------
    // Add edges
    // -----------------------------------------------------------------------
    if let Some(Value::Array(edges)) = extraction.get("edges") {
        for edge in edges {
            if let Value::Object(mut edge_obj) = edge.clone() {
                // Handle legacy "from"/"to" field names.
                if !edge_obj.contains_key("source") {
                    if let Some(v) = edge_obj.remove("from") {
                        edge_obj.insert("source".to_string(), v);
                    }
                }
                if !edge_obj.contains_key("target") {
                    if let Some(v) = edge_obj.remove("to") {
                        edge_obj.insert("target".to_string(), v);
                    }
                }

                let src_raw = match edge_obj.get("source").and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let tgt_raw = match edge_obj.get("target").and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };

                // Resolve source and target against known nodes (with normalised fallback).
                let src = if node_set.contains(&src_raw) {
                    src_raw.clone()
                } else {
                    match norm_to_id.get(&normalize_id(&src_raw)) {
                        Some(resolved) => resolved.clone(),
                        None => src_raw.clone(),
                    }
                };
                let tgt = if node_set.contains(&tgt_raw) {
                    tgt_raw.clone()
                } else {
                    match norm_to_id.get(&normalize_id(&tgt_raw)) {
                        Some(resolved) => resolved.clone(),
                        None => tgt_raw.clone(),
                    }
                };

                // Skip edges whose endpoints still don't resolve to known nodes.
                if !node_set.contains(&src) || !node_set.contains(&tgt) {
                    continue;
                }

                // Build edge attrs, adding _src/_tgt fields.
                let mut attrs: EdgeAttrs = edge_obj
                    .iter()
                    .filter(|(k, _)| k.as_str() != "source" && k.as_str() != "target")
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                attrs.insert("_src".to_string(), Value::String(src.clone()));
                attrs.insert("_tgt".to_string(), Value::String(tgt.clone()));

                g.add_edge(&src, &tgt, attrs);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Store hyperedges as graph-level metadata
    // -----------------------------------------------------------------------
    if let Some(he @ Value::Array(_)) = extraction.get("hyperedges") {
        if let Value::Array(arr) = he {
            if !arr.is_empty() {
                g.graph.insert("hyperedges".to_string(), he.clone());
            }
        }
    }

    g
}

/// Build a [`Graph`] from a list of extraction JSON objects by combining them
/// and then calling [`build_from_json`].
///
/// Corresponds to `build(extractions, *, directed=False)` in Python.
pub fn build(extractions: &[Value], directed: bool) -> Graph {
    let mut combined_nodes: Vec<Value> = Vec::new();
    let mut combined_edges: Vec<Value> = Vec::new();
    let mut combined_hyperedges: Vec<Value> = Vec::new();
    let mut input_tokens: i64 = 0;
    let mut output_tokens: i64 = 0;

    for ext in extractions {
        if let Value::Object(obj) = ext {
            if let Some(Value::Array(nodes)) = obj.get("nodes") {
                combined_nodes.extend(nodes.iter().cloned());
            }
            if let Some(Value::Array(edges)) = obj.get("edges") {
                combined_edges.extend(edges.iter().cloned());
            }
            // Also accept "links" for legacy data.
            if let Some(Value::Array(links)) = obj.get("links") {
                combined_edges.extend(links.iter().cloned());
            }
            if let Some(Value::Array(he)) = obj.get("hyperedges") {
                combined_hyperedges.extend(he.iter().cloned());
            }
            input_tokens += obj
                .get("input_tokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            output_tokens += obj
                .get("output_tokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
        }
    }

    let combined = Value::Object({
        let mut m = serde_json::Map::new();
        m.insert("nodes".to_string(), Value::Array(combined_nodes));
        m.insert("edges".to_string(), Value::Array(combined_edges));
        m.insert(
            "hyperedges".to_string(),
            Value::Array(combined_hyperedges),
        );
        m.insert("input_tokens".to_string(), Value::Number(input_tokens.into()));
        m.insert(
            "output_tokens".to_string(),
            Value::Number(output_tokens.into()),
        );
        m
    });

    build_from_json(combined, directed)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_extraction() -> Value {
        json!({
            "nodes": [
                {"id": "a", "label": "A", "file_type": "code", "source_file": "a.rs"},
                {"id": "b", "label": "B", "file_type": "code", "source_file": "b.rs"}
            ],
            "edges": [
                {
                    "source": "a", "target": "b",
                    "relation": "calls", "confidence": "EXTRACTED",
                    "source_file": "a.rs"
                }
            ]
        })
    }

    #[test]
    fn build_basic_undirected() {
        let g = build_from_json(sample_extraction(), false);
        assert_eq!(g.number_of_nodes(), 2);
        assert_eq!(g.number_of_edges(), 1);
        assert!(!g.is_directed());
    }

    #[test]
    fn build_basic_directed() {
        let g = build_from_json(sample_extraction(), true);
        assert_eq!(g.number_of_nodes(), 2);
        assert_eq!(g.number_of_edges(), 1);
        assert!(g.is_directed());
    }

    #[test]
    fn links_alias_accepted() {
        let v = json!({
            "nodes": [
                {"id": "x", "label": "X", "file_type": "code", "source_file": "x.rs"},
                {"id": "y", "label": "Y", "file_type": "code", "source_file": "y.rs"}
            ],
            "links": [
                {"source": "x", "target": "y", "relation": "r",
                 "confidence": "EXTRACTED", "source_file": "x.rs"}
            ]
        });
        let g = build_from_json(v, false);
        assert_eq!(g.number_of_edges(), 1);
    }

    #[test]
    fn from_to_aliases_accepted() {
        let v = json!({
            "nodes": [
                {"id": "x", "label": "X", "file_type": "code", "source_file": "x.rs"},
                {"id": "y", "label": "Y", "file_type": "code", "source_file": "y.rs"}
            ],
            "edges": [
                {"from": "x", "to": "y", "relation": "r",
                 "confidence": "EXTRACTED", "source_file": "x.rs"}
            ]
        });
        let g = build_from_json(v, false);
        assert_eq!(g.number_of_edges(), 1);
    }

    #[test]
    fn dangling_edge_skipped() {
        let v = json!({
            "nodes": [
                {"id": "a", "label": "A", "file_type": "code", "source_file": "a.rs"}
            ],
            "edges": [
                {"source": "a", "target": "ghost", "relation": "r",
                 "confidence": "EXTRACTED", "source_file": "a.rs"}
            ]
        });
        let g = build_from_json(v, false);
        assert_eq!(g.number_of_edges(), 0);
    }

    #[test]
    fn normalised_id_resolution() {
        // "A Node" normalises to "a_node"; if the edge uses the un-normalised
        // form the builder should still resolve it.
        let v = json!({
            "nodes": [
                {"id": "A Node", "label": "A Node", "file_type": "code", "source_file": "a.rs"},
                {"id": "b", "label": "B", "file_type": "code", "source_file": "b.rs"}
            ],
            "edges": [
                {"source": "a_node", "target": "b", "relation": "r",
                 "confidence": "EXTRACTED", "source_file": "a.rs"}
            ]
        });
        let g = build_from_json(v, false);
        assert_eq!(g.number_of_edges(), 1);
    }

    #[test]
    fn build_combines_extractions() {
        let e1 = json!({
            "nodes": [{"id": "a", "label": "A", "file_type": "code", "source_file": "a.rs"}],
            "edges": [],
            "input_tokens": 10, "output_tokens": 5
        });
        let e2 = json!({
            "nodes": [{"id": "b", "label": "B", "file_type": "code", "source_file": "b.rs"}],
            "edges": [],
            "input_tokens": 20, "output_tokens": 8
        });
        let g = build(&[e1, e2], false);
        assert_eq!(g.number_of_nodes(), 2);
    }

    #[test]
    fn hyperedges_stored_in_graph_metadata() {
        let v = json!({
            "nodes": [
                {"id": "a", "label": "A", "file_type": "code", "source_file": "a.rs"}
            ],
            "edges": [],
            "hyperedges": [["a", "b", "c"]]
        });
        let g = build_from_json(v, false);
        assert!(g.graph.contains_key("hyperedges"));
    }

    #[test]
    fn edge_has_src_tgt_fields() {
        let g = build_from_json(sample_extraction(), false);
        let edge = g.get_edge("a", "b").unwrap();
        assert_eq!(edge.get("_src").and_then(|v| v.as_str()), Some("a"));
        assert_eq!(edge.get("_tgt").and_then(|v| v.as_str()), Some("b"));
    }

    #[test]
    fn normalize_id_examples() {
        assert_eq!(normalize_id("Hello World!"), "hello_world");
        assert_eq!(normalize_id("__foo__bar__"), "foo_bar");
        assert_eq!(normalize_id("camelCase"), "camelcase");
        assert_eq!(normalize_id("a.b.c"), "a_b_c");
    }
}
