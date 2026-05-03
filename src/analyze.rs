//! Graph analysis: god nodes, surprising connections, graph diff.
//!
//! Ported from the Python `analyze.py` module. Logic is kept identical to the
//! original so that outputs compare correctly with the Python implementation.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;

use crate::detect::{CODE_EXTENSIONS, IMAGE_EXTENSIONS, PAPER_EXTENSIONS};
use crate::types::Graph;

// ---------------------------------------------------------------------------
// Language-family table (mirrors _LANG_FAMILY in Python)
// ---------------------------------------------------------------------------

fn lang_family(ext: &str) -> Option<&'static str> {
    match ext {
        ".py" | ".pyw" => Some("python"),
        ".js" | ".jsx" | ".mjs" | ".ejs" | ".ts" | ".tsx" | ".vue" | ".svelte" => Some("js"),
        ".go" => Some("go"),
        ".rs" => Some("rust"),
        ".java" | ".kt" | ".kts" | ".scala" => Some("jvm"),
        ".c" | ".h" | ".cpp" | ".cc" | ".cxx" | ".hpp" => Some("c"),
        ".rb" => Some("ruby"),
        ".swift" => Some("swift"),
        ".cs" => Some("dotnet"),
        ".php" => Some("php"),
        ".r" => Some("r"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// File-classification helpers
// ---------------------------------------------------------------------------

/// Return the lowercased dot-prefixed extension of `path`, or `""`.
fn file_ext(path: &str) -> String {
    match path.rsplit('.').next() {
        Some(ext) if path.contains('.') => format!(".{}", ext.to_lowercase()),
        _ => String::new(),
    }
}

/// Classify a file path as `"code"`, `"paper"`, `"image"`, or `"doc"`.
///
/// Mirrors `_file_category(path)` in Python.
pub fn file_category(path: &str) -> &'static str {
    let ext = file_ext(path);
    let ext = ext.as_str();
    if CODE_EXTENSIONS.contains(&ext) {
        return "code";
    }
    if PAPER_EXTENSIONS.contains(&ext) {
        return "paper";
    }
    if IMAGE_EXTENSIONS.contains(&ext) {
        return "image";
    }
    "doc"
}

/// Return the first path component (top-level directory) of `path`.
///
/// Mirrors `_top_level_dir(path)` in Python.
fn top_level_dir(path: &str) -> &str {
    match path.find('/') {
        Some(pos) => &path[..pos],
        None => path,
    }
}

/// Return `true` when `src_a` and `src_b` belong to different language
/// families.
///
/// Mirrors `_cross_language(src_a, src_b)` in Python.
fn cross_language(src_a: &str, src_b: &str) -> bool {
    let ext_a = file_ext(src_a);
    let ext_b = file_ext(src_b);
    let fam_a = lang_family(&ext_a);
    let fam_b = lang_family(&ext_b);
    match (fam_a, fam_b) {
        (Some(a), Some(b)) => a != b,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Node classification helpers
// ---------------------------------------------------------------------------

/// Return `true` when `node_id` looks like a file node rather than a concept.
///
/// Mirrors `_is_file_node(G, node_id)` in Python.
pub fn is_file_node(g: &Graph, node_id: &str) -> bool {
    let attrs = match g.get_node(node_id) {
        Some(a) => a,
        None => return false,
    };
    let label = match attrs.get("label").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };
    let source_file = attrs
        .get("source_file")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !source_file.is_empty() {
        // Compare label to the filename part of source_file.
        let filename = source_file.rsplit('/').next().unwrap_or(source_file);
        if label == filename {
            return true;
        }
    }
    if label.starts_with('.') && label.ends_with("()") {
        return true;
    }
    if label.ends_with("()") && g.degree(node_id) <= 1 {
        return true;
    }
    false
}

/// Return `true` when `node_id` is a concept node (no source file or no
/// file extension in the source path).
///
/// Mirrors `_is_concept_node(G, node_id)` in Python.
pub fn is_concept_node(g: &Graph, node_id: &str) -> bool {
    let attrs = match g.get_node(node_id) {
        Some(a) => a,
        None => return true,
    };
    let source = attrs
        .get("source_file")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if source.is_empty() {
        return true;
    }
    // If the last path component has no dot, treat as concept.
    let basename = source.rsplit('/').next().unwrap_or(source);
    !basename.contains('.')
}

// ---------------------------------------------------------------------------
// Community helpers
// ---------------------------------------------------------------------------

fn node_community_map(communities: &HashMap<i64, Vec<String>>) -> HashMap<&str, i64> {
    communities
        .iter()
        .flat_map(|(&cid, nodes)| nodes.iter().map(move |n| (n.as_str(), cid)))
        .collect()
}

// ---------------------------------------------------------------------------
// Public API – God nodes
// ---------------------------------------------------------------------------

/// Return the top-`top_n` highest-degree nodes that are neither file nodes
/// nor concept nodes.
///
/// Each entry: `{"id": …, "label": …, "degree": …}`.
/// Corresponds to `god_nodes(G, top_n=10)` in Python.
pub fn god_nodes(g: &Graph, top_n: usize) -> Vec<HashMap<String, Value>> {
    let mut degree_pairs: Vec<(&str, usize)> = g
        .nodes
        .keys()
        .map(|s| (s.as_str(), g.degree(s)))
        .collect();
    degree_pairs.sort_by(|a, b| b.1.cmp(&a.1));

    let mut result = Vec::new();
    for (node_id, deg) in degree_pairs {
        if is_file_node(g, node_id) || is_concept_node(g, node_id) {
            continue;
        }
        let label = g
            .get_node(node_id)
            .and_then(|a| a.get("label"))
            .and_then(|v| v.as_str())
            .unwrap_or(node_id)
            .to_string();
        let mut entry = HashMap::new();
        entry.insert("id".to_string(), Value::String(node_id.to_string()));
        entry.insert("label".to_string(), Value::String(label));
        entry.insert("degree".to_string(), Value::Number(deg.into()));
        result.push(entry);
        if result.len() >= top_n {
            break;
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Public API – Surprising connections
// ---------------------------------------------------------------------------

/// Return the most surprising edges in `G`, optionally using community
/// information.
///
/// Corresponds to `surprising_connections(G, communities=None, top_n=5)`
/// in Python.
pub fn surprising_connections(
    g: &Graph,
    communities: Option<&HashMap<i64, Vec<String>>>,
    top_n: usize,
) -> Vec<HashMap<String, Value>> {
    let source_files: HashSet<&str> = g
        .nodes
        .values()
        .filter_map(|attrs| attrs.get("source_file")?.as_str())
        .filter(|s| !s.is_empty())
        .collect();

    let empty_communities: HashMap<i64, Vec<String>> = HashMap::new();
    let communities_ref = communities.unwrap_or(&empty_communities);

    if source_files.len() > 1 {
        cross_file_surprises(g, communities_ref, top_n)
    } else {
        cross_community_surprises(g, communities_ref, top_n)
    }
}

// ---------------------------------------------------------------------------
// Public API – Graph diff
// ---------------------------------------------------------------------------

/// Compare two graph versions and return added / removed nodes and edges.
///
/// Corresponds to `graph_diff(G_old, G_new)` in Python.
pub fn graph_diff(g_old: &Graph, g_new: &Graph) -> HashMap<String, Value> {
    let old_nodes: HashSet<&str> = g_old.nodes.keys().map(|s| s.as_str()).collect();
    let new_nodes: HashSet<&str> = g_new.nodes.keys().map(|s| s.as_str()).collect();

    let added_node_ids: HashSet<&str> = new_nodes.difference(&old_nodes).copied().collect();
    let removed_node_ids: HashSet<&str> = old_nodes.difference(&new_nodes).copied().collect();

    let new_nodes_list: Vec<Value> = added_node_ids
        .iter()
        .map(|&n| {
            let label = g_new
                .get_node(n)
                .and_then(|a| a.get("label"))
                .and_then(|v| v.as_str())
                .unwrap_or(n);
            serde_json::json!({"id": n, "label": label})
        })
        .collect();

    let removed_nodes_list: Vec<Value> = removed_node_ids
        .iter()
        .map(|&n| {
            let label = g_old
                .get_node(n)
                .and_then(|a| a.get("label"))
                .and_then(|v| v.as_str())
                .unwrap_or(n);
            serde_json::json!({"id": n, "label": label})
        })
        .collect();

    // Build edge key sets.
    let old_edge_keys: HashSet<(String, String, String)> = g_old
        .edges_iter()
        .into_iter()
        .map(|(u, v, data)| edge_key(g_old, u, v, data))
        .collect();
    let new_edge_keys: HashSet<(String, String, String)> = g_new
        .edges_iter()
        .into_iter()
        .map(|(u, v, data)| edge_key(g_new, u, v, data))
        .collect();

    let added_edge_keys: HashSet<&(String, String, String)> =
        new_edge_keys.difference(&old_edge_keys).collect();
    let removed_edge_keys: HashSet<&(String, String, String)> =
        old_edge_keys.difference(&new_edge_keys).collect();

    let new_edges_list: Vec<Value> = g_new
        .edges_iter()
        .into_iter()
        .filter(|(u, v, data)| added_edge_keys.contains(&edge_key(g_new, u, v, data)))
        .map(|(u, v, data): (&str, &str, &crate::types::EdgeAttrs)| {
            let relation = data
                .get("relation")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let confidence = data
                .get("confidence")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            serde_json::json!({
                "source": u,
                "target": v,
                "relation": relation,
                "confidence": confidence,
            })
        })
        .collect();

    let removed_edges_list: Vec<Value> = g_old
        .edges_iter()
        .into_iter()
        .filter(|(u, v, data)| removed_edge_keys.contains(&edge_key(g_old, u, v, data)))
        .map(|(u, v, data): (&str, &str, &crate::types::EdgeAttrs)| {
            let relation = data
                .get("relation")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let confidence = data
                .get("confidence")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            serde_json::json!({
                "source": u,
                "target": v,
                "relation": relation,
                "confidence": confidence,
            })
        })
        .collect();

    // Build summary string.
    let mut parts: Vec<String> = Vec::new();
    if !new_nodes_list.is_empty() {
        let nn = new_nodes_list.len();
        parts.push(format!("{} new node{}", nn, if nn != 1 { "s" } else { "" }));
    }
    if !new_edges_list.is_empty() {
        let ne = new_edges_list.len();
        parts.push(format!("{} new edge{}", ne, if ne != 1 { "s" } else { "" }));
    }
    if !removed_nodes_list.is_empty() {
        let rn = removed_nodes_list.len();
        parts.push(format!(
            "{} node{} removed",
            rn,
            if rn != 1 { "s" } else { "" }
        ));
    }
    if !removed_edges_list.is_empty() {
        let re = removed_edges_list.len();
        parts.push(format!(
            "{} edge{} removed",
            re,
            if re != 1 { "s" } else { "" }
        ));
    }
    let summary = if parts.is_empty() {
        "no changes".to_string()
    } else {
        parts.join(", ")
    };

    let mut result = HashMap::new();
    result.insert("new_nodes".to_string(), Value::Array(new_nodes_list));
    result.insert("removed_nodes".to_string(), Value::Array(removed_nodes_list));
    result.insert("new_edges".to_string(), Value::Array(new_edges_list));
    result.insert("removed_edges".to_string(), Value::Array(removed_edges_list));
    result.insert("summary".to_string(), Value::String(summary));
    result
}

// ---------------------------------------------------------------------------
// Internal: surprise scoring
// ---------------------------------------------------------------------------

/// Compute a surprise score for a cross-file edge.
///
/// Returns `(score, reasons)`.
/// Mirrors `_surprise_score(…)` in Python.
fn surprise_score(
    g: &Graph,
    u: &str,
    v: &str,
    data: &crate::types::EdgeAttrs,
    node_community: &HashMap<&str, i64>,
    u_source: &str,
    v_source: &str,
) -> (i64, Vec<String>) {
    let mut score: i64 = 0;
    let mut reasons: Vec<String> = Vec::new();

    let conf = data
        .get("confidence")
        .and_then(|v| v.as_str())
        .unwrap_or("EXTRACTED");
    let relation = data
        .get("relation")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut conf_bonus: i64 = match conf {
        "AMBIGUOUS" => 3,
        "INFERRED" => 2,
        "EXTRACTED" => 1,
        _ => 1,
    };

    // Cross-language inferred "calls" edges are not surprising.
    if conf == "INFERRED" && relation == "calls" && cross_language(u_source, v_source) {
        conf_bonus = 0;
    }
    score += conf_bonus;

    if matches!(conf, "AMBIGUOUS" | "INFERRED") {
        reasons.push(format!(
            "{} connection - not explicitly stated in source",
            conf.to_lowercase()
        ));
    }

    let cat_u = file_category(u_source);
    let cat_v = file_category(v_source);
    if cat_u != cat_v {
        score += 2;
        reasons.push(format!("crosses file types ({cat_u} ↔ {cat_v})"));
    }

    if top_level_dir(u_source) != top_level_dir(v_source) {
        score += 2;
        reasons.push("connects across different repos/directories".to_string());
    }

    let cid_u = node_community.get(u).copied();
    let cid_v = node_community.get(v).copied();
    if let (Some(cu), Some(cv)) = (cid_u, cid_v) {
        if cu != cv {
            score += 1;
            reasons.push("bridges separate communities".to_string());
        }
    }

    if relation == "semantically_similar_to" {
        score = (score as f64 * 1.5) as i64;
        reasons.push("semantically similar concepts with no structural link".to_string());
    }

    let deg_u = g.degree(u);
    let deg_v = g.degree(v);
    if deg_u.min(deg_v) <= 2 && deg_u.max(deg_v) >= 5 {
        score += 1;
        let (peripheral_label, hub_label) = if deg_u <= 2 {
            (
                g.get_node(u)
                    .and_then(|a| a.get("label"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(u),
                g.get_node(v)
                    .and_then(|a| a.get("label"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(v),
            )
        } else {
            (
                g.get_node(v)
                    .and_then(|a| a.get("label"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(v),
                g.get_node(u)
                    .and_then(|a| a.get("label"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(u),
            )
        };
        reasons.push(format!(
            "peripheral node `{peripheral_label}` unexpectedly reaches hub `{hub_label}`"
        ));
    }

    (score, reasons)
}

// ---------------------------------------------------------------------------
// Internal: cross-file surprises
// ---------------------------------------------------------------------------

fn cross_file_surprises(
    g: &Graph,
    communities: &HashMap<i64, Vec<String>>,
    top_n: usize,
) -> Vec<HashMap<String, Value>> {
    let node_community = node_community_map(communities);

    // We need all edges including direction info; use edges_iter() which
    // respects directionality.
    let mut candidates: Vec<(i64, HashMap<String, Value>)> = Vec::new();

    for (u, v, data) in g.edges_iter() {
        let relation = data.get("relation").and_then(|v| v.as_str()).unwrap_or("");
        if matches!(
            relation,
            "imports" | "imports_from" | "contains" | "method"
        ) {
            continue;
        }
        if is_concept_node(g, u) || is_concept_node(g, v) {
            continue;
        }
        if is_file_node(g, u) || is_file_node(g, v) {
            continue;
        }
        let u_source = g
            .get_node(u)
            .and_then(|a| a.get("source_file"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let v_source = g
            .get_node(v)
            .and_then(|a| a.get("source_file"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if u_source.is_empty() || v_source.is_empty() || u_source == v_source {
            continue;
        }

        let (score, reasons) =
            surprise_score(g, u, v, data, &node_community, u_source, v_source);

        // Determine the canonical source/target using _src/_tgt fields.
        let src_id = data
            .get("_src")
            .and_then(|v| v.as_str())
            .filter(|&s| g.has_node(s))
            .unwrap_or(u);
        let tgt_id = data
            .get("_tgt")
            .and_then(|v| v.as_str())
            .filter(|&s| g.has_node(s))
            .unwrap_or(v);

        let src_label = g
            .get_node(src_id)
            .and_then(|a| a.get("label"))
            .and_then(|v| v.as_str())
            .unwrap_or(src_id);
        let tgt_label = g
            .get_node(tgt_id)
            .and_then(|a| a.get("label"))
            .and_then(|v| v.as_str())
            .unwrap_or(tgt_id);
        let src_file = g
            .get_node(src_id)
            .and_then(|a| a.get("source_file"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let tgt_file = g
            .get_node(tgt_id)
            .and_then(|a| a.get("source_file"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let confidence = data
            .get("confidence")
            .and_then(|v| v.as_str())
            .unwrap_or("EXTRACTED");
        let why = if reasons.is_empty() {
            "cross-file semantic connection".to_string()
        } else {
            reasons.join("; ")
        };

        let mut entry = HashMap::new();
        entry.insert("source".to_string(), Value::String(src_label.to_string()));
        entry.insert("target".to_string(), Value::String(tgt_label.to_string()));
        entry.insert(
            "source_files".to_string(),
            Value::Array(vec![
                Value::String(src_file.to_string()),
                Value::String(tgt_file.to_string()),
            ]),
        );
        entry.insert(
            "confidence".to_string(),
            Value::String(confidence.to_string()),
        );
        entry.insert("relation".to_string(), Value::String(relation.to_string()));
        entry.insert("why".to_string(), Value::String(why));

        candidates.push((score, entry));
    }

    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    let results: Vec<HashMap<String, Value>> =
        candidates.into_iter().map(|(_, entry)| entry).collect();

    if !results.is_empty() {
        results.into_iter().take(top_n).collect()
    } else {
        cross_community_surprises(g, communities, top_n)
    }
}

// ---------------------------------------------------------------------------
// Internal: cross-community surprises
// ---------------------------------------------------------------------------

fn cross_community_surprises(
    g: &Graph,
    communities: &HashMap<i64, Vec<String>>,
    top_n: usize,
) -> Vec<HashMap<String, Value>> {
    if communities.is_empty() {
        // Fall back to edge betweenness centrality.
        if g.number_of_edges() == 0 || g.number_of_nodes() > 5000 {
            return Vec::new();
        }
        let betweenness = edge_betweenness_centrality(g);
        let mut top_edges: Vec<((String, String), f64)> = betweenness.into_iter().collect();
        top_edges.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        top_edges.truncate(top_n);

        return top_edges
            .into_iter()
            .map(|((u, v), score)| {
                let u_label = g
                    .get_node(&u)
                    .and_then(|a| a.get("label"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(u.as_str());
                let v_label = g
                    .get_node(&v)
                    .and_then(|a| a.get("label"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(v.as_str());
                let data = g.get_edge(&u, &v).cloned().unwrap_or_default();
                let confidence = data
                    .get("confidence")
                    .and_then(|v| v.as_str())
                    .unwrap_or("EXTRACTED");
                let relation = data
                    .get("relation")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let u_file = g
                    .get_node(&u)
                    .and_then(|a| a.get("source_file"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let v_file = g
                    .get_node(&v)
                    .and_then(|a| a.get("source_file"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let mut entry = HashMap::new();
                entry.insert(
                    "source".to_string(),
                    Value::String(u_label.to_string()),
                );
                entry.insert(
                    "target".to_string(),
                    Value::String(v_label.to_string()),
                );
                entry.insert(
                    "source_files".to_string(),
                    Value::Array(vec![
                        Value::String(u_file.to_string()),
                        Value::String(v_file.to_string()),
                    ]),
                );
                entry.insert(
                    "confidence".to_string(),
                    Value::String(confidence.to_string()),
                );
                entry.insert(
                    "relation".to_string(),
                    Value::String(relation.to_string()),
                );
                entry.insert(
                    "note".to_string(),
                    Value::String(format!(
                        "Bridges graph structure (betweenness={score:.3})"
                    )),
                );
                entry
            })
            .collect();
    }

    let node_community = node_community_map(communities);

    // Confidence ordering: AMBIGUOUS < INFERRED < EXTRACTED (lower = more surprising).
    let conf_order = |s: &str| -> i64 {
        match s {
            "AMBIGUOUS" => 0,
            "INFERRED" => 1,
            "EXTRACTED" => 2,
            _ => 3,
        }
    };

    let mut surprises: Vec<(i64, (i64, i64), HashMap<String, Value>)> = Vec::new();

    for (u, v, data) in g.edges_iter() {
        let cid_u = match node_community.get(u) {
            Some(&c) => c,
            None => continue,
        };
        let cid_v = match node_community.get(v) {
            Some(&c) => c,
            None => continue,
        };
        if cid_u == cid_v {
            continue;
        }
        if is_file_node(g, u) || is_file_node(g, v) {
            continue;
        }
        let relation = data.get("relation").and_then(|v| v.as_str()).unwrap_or("");
        if matches!(
            relation,
            "imports" | "imports_from" | "contains" | "method"
        ) {
            continue;
        }
        let confidence = data
            .get("confidence")
            .and_then(|v| v.as_str())
            .unwrap_or("EXTRACTED");

        // Resolve _src/_tgt.
        let src_id = data
            .get("_src")
            .and_then(|v| v.as_str())
            .filter(|&s| g.has_node(s))
            .unwrap_or(u);
        let tgt_id = data
            .get("_tgt")
            .and_then(|v| v.as_str())
            .filter(|&s| g.has_node(s))
            .unwrap_or(v);

        let src_label = g
            .get_node(src_id)
            .and_then(|a| a.get("label"))
            .and_then(|v| v.as_str())
            .unwrap_or(src_id);
        let tgt_label = g
            .get_node(tgt_id)
            .and_then(|a| a.get("label"))
            .and_then(|v| v.as_str())
            .unwrap_or(tgt_id);
        let src_file = g
            .get_node(src_id)
            .and_then(|a| a.get("source_file"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let tgt_file = g
            .get_node(tgt_id)
            .and_then(|a| a.get("source_file"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let pair = (cid_u.min(cid_v), cid_u.max(cid_v));

        let mut entry = HashMap::new();
        entry.insert("source".to_string(), Value::String(src_label.to_string()));
        entry.insert("target".to_string(), Value::String(tgt_label.to_string()));
        entry.insert(
            "source_files".to_string(),
            Value::Array(vec![
                Value::String(src_file.to_string()),
                Value::String(tgt_file.to_string()),
            ]),
        );
        entry.insert(
            "confidence".to_string(),
            Value::String(confidence.to_string()),
        );
        entry.insert("relation".to_string(), Value::String(relation.to_string()));
        entry.insert(
            "note".to_string(),
            Value::String(format!(
                "Bridges community {cid_u} → community {cid_v}"
            )),
        );

        surprises.push((conf_order(confidence), pair, entry));
    }

    // Sort by confidence order (ascending = most surprising first).
    surprises.sort_by_key(|(ord, _, _)| *ord);

    // Deduplicate by community pair, keeping the first (most surprising) entry.
    let mut seen_pairs: HashSet<(i64, i64)> = HashSet::new();
    let mut deduped: Vec<HashMap<String, Value>> = Vec::new();
    for (_, pair, entry) in surprises {
        if seen_pairs.insert(pair) {
            deduped.push(entry);
        }
    }

    deduped.into_iter().take(top_n).collect()
}

// ---------------------------------------------------------------------------
// Edge betweenness centrality – Brandes algorithm
// ---------------------------------------------------------------------------

/// Compute edge betweenness centrality for all edges in `G` using the
/// Brandes algorithm (BFS-based, unweighted).
///
/// Returns a map `(u, v) → score` where `(u, v)` is in the canonical order
/// returned by [`Graph::edges_iter`] (i.e. for undirected graphs the pair
/// with the smaller string first).
pub fn edge_betweenness_centrality(g: &Graph) -> HashMap<(String, String), f64> {
    let nodes: Vec<&str> = g.nodes.keys().map(|s| s.as_str()).collect();
    let n = nodes.len();
    if n == 0 {
        return HashMap::new();
    }

    let mut centrality: HashMap<(String, String), f64> = HashMap::new();

    // Initialise all edges to 0.0.
    for (u, v, _) in g.edges_iter() {
        let key = if u <= v {
            (u.to_string(), v.to_string())
        } else {
            (v.to_string(), u.to_string())
        };
        centrality.entry(key).or_insert(0.0);
    }

    for &s in &nodes {
        // BFS from source s.
        let mut stack: Vec<&str> = Vec::with_capacity(n);
        // pred[w] = list of predecessors of w on shortest paths from s.
        let mut pred: HashMap<&str, Vec<&str>> = nodes.iter().map(|&v| (v, Vec::new())).collect();
        // sigma[v] = number of shortest paths from s to v.
        let mut sigma: HashMap<&str, f64> = nodes.iter().map(|&v| (v, 0.0)).collect();
        *sigma.get_mut(s).unwrap() = 1.0;
        // dist[v] = distance from s to v (−1 = unvisited).
        let mut dist: HashMap<&str, i64> = nodes.iter().map(|&v| (v, -1)).collect();
        *dist.get_mut(s).unwrap() = 0;

        let mut queue: VecDeque<&str> = VecDeque::new();
        queue.push_back(s);

        while let Some(v) = queue.pop_front() {
            stack.push(v);
            let dv = dist[v];
            for &w in &g.neighbors(v) {
                // First visit to w?
                if dist[w] < 0 {
                    queue.push_back(w);
                    *dist.get_mut(w).unwrap() = dv + 1;
                }
                // Is this a shortest path to w via v?
                if dist[w] == dv + 1 {
                    *sigma.get_mut(w).unwrap() += sigma[v];
                    pred.get_mut(w).unwrap().push(v);
                }
            }
        }

        // Accumulate dependencies.
        let mut delta: HashMap<&str, f64> = nodes.iter().map(|&v| (v, 0.0)).collect();

        while let Some(w) = stack.pop() {
            for &v in &pred[w] {
                let coeff = (sigma[v] / sigma[w]) * (1.0 + delta[w]);
                *delta.get_mut(v).unwrap() += coeff;

                // The edge (v, w) carries this fraction.
                let key = if v <= w {
                    (v.to_string(), w.to_string())
                } else {
                    (w.to_string(), v.to_string())
                };
                if let Some(score) = centrality.get_mut(&key) {
                    *score += coeff;
                }
            }
        }
    }

    // Normalise: for undirected graphs divide by 2 (each source counted
    // both directions).
    let scale = if g.is_directed() { 1.0 } else { 2.0 };
    for v in centrality.values_mut() {
        *v /= scale;
    }

    centrality
}

// ---------------------------------------------------------------------------
// Internal: edge key for graph_diff
// ---------------------------------------------------------------------------

fn edge_key(g: &Graph, u: &str, v: &str, data: &crate::types::EdgeAttrs) -> (String, String, String) {
    let relation = data
        .get("relation")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if g.is_directed() {
        (u.to_string(), v.to_string(), relation)
    } else {
        let (a, b) = if u <= v { (u, v) } else { (v, u) };
        (a.to_string(), b.to_string(), relation)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Graph;

    fn make_graph_with_sources() -> Graph {
        let mut g = Graph::new(false);
        let mut a_attrs = crate::types::NodeAttrs::new();
        a_attrs.insert("label".to_string(), Value::String("Alpha".to_string()));
        a_attrs.insert(
            "source_file".to_string(),
            Value::String("src/a.rs".to_string()),
        );
        let mut b_attrs = crate::types::NodeAttrs::new();
        b_attrs.insert("label".to_string(), Value::String("Beta".to_string()));
        b_attrs.insert(
            "source_file".to_string(),
            Value::String("lib/b.py".to_string()),
        );
        g.add_node("a", a_attrs);
        g.add_node("b", b_attrs);
        let mut edge_attrs = crate::types::EdgeAttrs::new();
        edge_attrs.insert(
            "relation".to_string(),
            Value::String("calls".to_string()),
        );
        edge_attrs.insert(
            "confidence".to_string(),
            Value::String("AMBIGUOUS".to_string()),
        );
        edge_attrs.insert("_src".to_string(), Value::String("a".to_string()));
        edge_attrs.insert("_tgt".to_string(), Value::String("b".to_string()));
        g.add_edge("a", "b", edge_attrs);
        g
    }

    #[test]
    fn file_category_code() {
        assert_eq!(file_category("src/main.rs"), "code");
        assert_eq!(file_category("app.py"), "code");
    }

    #[test]
    fn file_category_paper() {
        assert_eq!(file_category("paper.pdf"), "paper");
    }

    #[test]
    fn file_category_image() {
        assert_eq!(file_category("logo.png"), "image");
    }

    #[test]
    fn file_category_doc() {
        assert_eq!(file_category("README.md"), "doc");
    }

    #[test]
    fn cross_language_rs_py() {
        assert!(cross_language("main.rs", "script.py"));
    }

    #[test]
    fn cross_language_same() {
        assert!(!cross_language("a.rs", "b.rs"));
    }

    #[test]
    fn god_nodes_basic() {
        let g = make_graph_with_sources();
        // a and b both have source_file set and no label == filename, so
        // they should pass the file/concept filters.
        let gn = god_nodes(&g, 10);
        // At most 2 nodes.
        assert!(gn.len() <= 2);
    }

    #[test]
    fn surprising_connections_multi_source() {
        let g = make_graph_with_sources();
        let sc = surprising_connections(&g, None, 5);
        // Should return the cross-file edge.
        assert!(!sc.is_empty());
    }

    #[test]
    fn graph_diff_no_change() {
        let g = make_graph_with_sources();
        let diff = graph_diff(&g, &g);
        assert_eq!(
            diff["summary"].as_str().unwrap(),
            "no changes"
        );
    }

    #[test]
    fn graph_diff_added_node() {
        let g1 = make_graph_with_sources();
        let mut g2 = g1.clone();
        g2.add_node("c", Default::default());
        let diff = graph_diff(&g1, &g2);
        let new_nodes = diff["new_nodes"].as_array().unwrap();
        assert_eq!(new_nodes.len(), 1);
        assert!(diff["summary"].as_str().unwrap().contains("1 new node"));
    }

    #[test]
    fn graph_diff_removed_node() {
        let g1 = make_graph_with_sources();
        let mut g2 = Graph::new(false);
        let mut a_attrs = crate::types::NodeAttrs::new();
        a_attrs.insert("label".to_string(), Value::String("Alpha".to_string()));
        g2.add_node("a", a_attrs);
        let diff = graph_diff(&g1, &g2);
        let removed = diff["removed_nodes"].as_array().unwrap();
        assert_eq!(removed.len(), 1);
    }

    #[test]
    fn edge_betweenness_triangle() {
        let mut g = Graph::new(false);
        for n in &["a", "b", "c"] {
            g.add_node(n, Default::default());
        }
        g.add_edge("a", "b", Default::default());
        g.add_edge("b", "c", Default::default());
        g.add_edge("a", "c", Default::default());
        let bw = edge_betweenness_centrality(&g);
        // All edges in a triangle should have equal (non-zero) betweenness.
        assert_eq!(bw.len(), 3);
        for &v in bw.values() {
            assert!(v >= 0.0);
        }
    }

    #[test]
    fn edge_betweenness_path() {
        // Path a-b-c: the middle edge has higher betweenness.
        let mut g = Graph::new(false);
        for n in &["a", "b", "c"] {
            g.add_node(n, Default::default());
        }
        g.add_edge("a", "b", Default::default());
        g.add_edge("b", "c", Default::default());
        let bw = edge_betweenness_centrality(&g);
        assert_eq!(bw.len(), 2);
        let ab = bw
            .get(&("a".to_string(), "b".to_string()))
            .copied()
            .unwrap_or(0.0);
        let bc = bw
            .get(&("b".to_string(), "c".to_string()))
            .copied()
            .unwrap_or(0.0);
        // Both bridge edges should have betweenness >= 1.0.
        assert!(ab >= 1.0, "ab={ab}");
        assert!(bc >= 1.0, "bc={bc}");
    }
}
