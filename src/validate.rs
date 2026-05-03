//! Validation of knowledge-graph extraction JSON.
//!
//! Ported from the Python `validate.py` module. All validation rules are kept
//! identical to the original so that error messages compare byte-for-byte with
//! the Python output (important for cross-language test compatibility).

use serde_json::Value;
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// All recognised values for a node's `file_type` field.
fn valid_file_types() -> HashSet<&'static str> {
    ["code", "document", "paper", "image", "rationale", "concept"]
        .iter()
        .copied()
        .collect()
}

/// All recognised values for an edge's `confidence` field.
fn valid_confidences() -> HashSet<&'static str> {
    ["EXTRACTED", "INFERRED", "AMBIGUOUS"]
        .iter()
        .copied()
        .collect()
}

/// Fields that every node object must contain.
fn required_node_fields() -> &'static [&'static str] {
    &["id", "label", "file_type", "source_file"]
}

/// Fields that every edge object must contain.
fn required_edge_fields() -> &'static [&'static str] {
    &["source", "target", "relation", "confidence", "source_file"]
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Validate a parsed extraction object and return a list of human-readable
/// error strings (empty means the data is valid).
///
/// Mirrors `validate_extraction(data)` from the Python implementation exactly,
/// including error-message wording and ordering.
pub fn validate_extraction(data: &Value) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();

    // Top-level must be a JSON object.
    let obj = match data.as_object() {
        Some(o) => o,
        None => return vec!["Extraction must be a JSON object".to_string()],
    };

    // -----------------------------------------------------------------------
    // Validate nodes
    // -----------------------------------------------------------------------
    let valid_ft = valid_file_types();
    let required_nf = required_node_fields();

    match obj.get("nodes") {
        None => errors.push("Missing required key 'nodes'".to_string()),
        Some(nodes_val) => match nodes_val.as_array() {
            None => errors.push("'nodes' must be a list".to_string()),
            Some(nodes) => {
                for (i, node) in nodes.iter().enumerate() {
                    match node.as_object() {
                        None => {
                            errors.push(format!("Node {i} must be an object"));
                            continue;
                        }
                        Some(node_obj) => {
                            // Derive the id for error messages (mirrors Python's node.get('id', '?')).
                            let id_repr = node_obj
                                .get("id")
                                .and_then(|v| v.as_str())
                                .map(|s| format!("'{s}'"))
                                .unwrap_or_else(|| "'?'".to_string());

                            for field in required_nf {
                                if !node_obj.contains_key(*field) {
                                    errors.push(format!(
                                        "Node {i} (id={id_repr}) missing required field '{field}'"
                                    ));
                                }
                            }

                            if let Some(ft_val) = node_obj.get("file_type") {
                                if let Some(ft) = ft_val.as_str() {
                                    if !valid_ft.contains(ft) {
                                        // Build sorted list for the error message.
                                        let mut sorted: Vec<&str> =
                                            valid_ft.iter().copied().collect();
                                        sorted.sort_unstable();
                                        errors.push(format!(
                                            "Node {i} (id={id_repr}) has invalid file_type \
                                             '{ft}' - must be one of {sorted:?}"
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
    }

    // -----------------------------------------------------------------------
    // Validate edges
    // -----------------------------------------------------------------------
    let valid_conf = valid_confidences();
    let required_ef = required_edge_fields();

    // Accept either "edges" or "links" (Python: data.get("edges") if "edges" in data else data.get("links")).
    let edge_list_val: Option<&Value> = if obj.contains_key("edges") {
        obj.get("edges")
    } else {
        obj.get("links")
    };

    match edge_list_val {
        None => errors.push("Missing required key 'edges'".to_string()),
        Some(edges_val) => match edges_val.as_array() {
            None => errors.push("'edges' must be a list".to_string()),
            Some(edges) => {
                // Build the set of known node ids for referential-integrity checks.
                let node_ids: HashSet<String> = obj
                    .get("nodes")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|n| n.as_object())
                            .filter_map(|n| n.get("id"))
                            .filter_map(|v| v.as_str())
                            .map(|s| s.to_owned())
                            .collect()
                    })
                    .unwrap_or_default();

                for (i, edge) in edges.iter().enumerate() {
                    match edge.as_object() {
                        None => {
                            errors.push(format!("Edge {i} must be an object"));
                            continue;
                        }
                        Some(edge_obj) => {
                            for field in required_ef {
                                if !edge_obj.contains_key(*field) {
                                    errors.push(format!(
                                        "Edge {i} missing required field '{field}'"
                                    ));
                                }
                            }

                            if let Some(conf_val) = edge_obj.get("confidence") {
                                if let Some(conf) = conf_val.as_str() {
                                    if !valid_conf.contains(conf) {
                                        let mut sorted: Vec<&str> =
                                            valid_conf.iter().copied().collect();
                                        sorted.sort_unstable();
                                        errors.push(format!(
                                            "Edge {i} has invalid confidence '{conf}' \
                                             - must be one of {sorted:?}"
                                        ));
                                    }
                                }
                            }

                            // Referential integrity – only checked when node_ids is non-empty.
                            if !node_ids.is_empty() {
                                if let Some(src_val) = edge_obj.get("source") {
                                    if let Some(src) = src_val.as_str() {
                                        if !node_ids.contains(src) {
                                            errors.push(format!(
                                                "Edge {i} source '{src}' does not match any node id"
                                            ));
                                        }
                                    }
                                }
                                if let Some(tgt_val) = edge_obj.get("target") {
                                    if let Some(tgt) = tgt_val.as_str() {
                                        if !node_ids.contains(tgt) {
                                            errors.push(format!(
                                                "Edge {i} target '{tgt}' does not match any node id"
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
    }

    errors
}

/// Assert that an extraction object is valid, raising a descriptive error if not.
///
/// Mirrors `assert_valid(data)` from the Python implementation.
pub fn assert_valid(data: &Value) -> Result<(), String> {
    let errors = validate_extraction(data);
    if errors.is_empty() {
        Ok(())
    } else {
        let bullet_list = errors
            .iter()
            .map(|e| format!("  \u{2022} {e}"))
            .collect::<Vec<_>>()
            .join("\n");
        Err(format!(
            "Extraction JSON has {} error(s):\n{}",
            errors.len(),
            bullet_list
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn minimal_valid() -> Value {
        json!({
            "nodes": [
                {
                    "id": "n1",
                    "label": "Main",
                    "file_type": "code",
                    "source_file": "src/main.rs"
                }
            ],
            "edges": [
                {
                    "source": "n1",
                    "target": "n1",
                    "relation": "self",
                    "confidence": "EXTRACTED",
                    "source_file": "src/main.rs"
                }
            ]
        })
    }

    #[test]
    fn valid_extraction_produces_no_errors() {
        assert!(validate_extraction(&minimal_valid()).is_empty());
    }

    #[test]
    fn missing_nodes_key() {
        let v = json!({"edges": []});
        let errs = validate_extraction(&v);
        assert!(errs.iter().any(|e| e.contains("Missing required key 'nodes'")));
    }

    #[test]
    fn missing_edges_key() {
        let v = json!({"nodes": []});
        let errs = validate_extraction(&v);
        assert!(errs.iter().any(|e| e.contains("Missing required key 'edges'")));
    }

    #[test]
    fn links_accepted_as_edges() {
        let v = json!({"nodes": [], "links": []});
        let errs = validate_extraction(&v);
        assert!(!errs.iter().any(|e| e.contains("edges")));
    }

    #[test]
    fn invalid_file_type() {
        let mut v = minimal_valid();
        v["nodes"][0]["file_type"] = json!("video");
        let errs = validate_extraction(&v);
        assert!(errs.iter().any(|e| e.contains("invalid file_type")));
    }

    #[test]
    fn invalid_confidence() {
        let mut v = minimal_valid();
        v["edges"][0]["confidence"] = json!("GUESSED");
        let errs = validate_extraction(&v);
        assert!(errs.iter().any(|e| e.contains("invalid confidence")));
    }

    #[test]
    fn dangling_edge_source() {
        let mut v = minimal_valid();
        v["edges"][0]["source"] = json!("nonexistent");
        let errs = validate_extraction(&v);
        assert!(errs.iter().any(|e| e.contains("does not match any node id")));
    }

    #[test]
    fn not_an_object_returns_single_error() {
        let errs = validate_extraction(&json!([1, 2, 3]));
        assert_eq!(errs, vec!["Extraction must be a JSON object"]);
    }

    #[test]
    fn assert_valid_ok() {
        assert!(assert_valid(&minimal_valid()).is_ok());
    }

    #[test]
    fn assert_valid_err_contains_bullet() {
        let v = json!({"nodes": [], "edges": [{"source": "x", "target": "y", "relation": "r", "confidence": "BAD", "source_file": "f"}]});
        let err = assert_valid(&v).unwrap_err();
        assert!(err.contains('\u{2022}'));
    }
}
