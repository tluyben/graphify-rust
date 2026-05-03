//! Wiki export: write one Markdown article per community, one per god-node,
//! and an `index.md` navigation file.
//!
//! Ported from Python `wiki.py`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::types::Graph;

// ---------------------------------------------------------------------------
// Filename helpers
// ---------------------------------------------------------------------------

/// Convert a community/node label into a safe filesystem slug.
///
/// Mirrors Python `_safe_filename`.
pub fn safe_filename(name: &str) -> String {
    // Basic character replacements.
    let s = name
        .replace('/', "-")
        .replace(' ', "_")
        .replace(':', "-");

    // Replace any remaining shell / Windows illegal characters.
    let s: String = s
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();

    // Strip leading / trailing dots and spaces.
    let s = s.trim_matches(|c| c == '.' || c == ' ').to_string();

    // Truncate to 200 chars.
    let truncated: String = s.chars().take(200).collect();
    if truncated.is_empty() {
        "unnamed".to_string()
    } else {
        truncated
    }
}

/// Return a slug that has not been used yet, appending `_2`, `_3`, … as
/// necessary.
fn unique_slug(base: &str, used: &mut HashSet<String>) -> String {
    let mut candidate = base.to_string();
    let mut n = 2usize;
    while used.contains(&candidate) {
        candidate = format!("{}_{}", base, n);
        n += 1;
    }
    used.insert(candidate.clone());
    candidate
}

// ---------------------------------------------------------------------------
// Cross-community link counting
// ---------------------------------------------------------------------------

/// Count how many times neighbours of `nodes` belong to communities other
/// than `own_cid`, grouped by community label.
///
/// Mirrors Python `_cross_community_links`.
fn cross_community_links(
    graph: &Graph,
    nodes: &[String],
    own_cid: i64,
    labels: &HashMap<i64, String>,
) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    for nid in nodes {
        for neighbor in graph.neighbors(nid) {
            if let Some(nd) = graph.get_node(neighbor) {
                if let Some(ncid_val) = nd.get("community") {
                    // community is stored as a JSON number.
                    let ncid = match ncid_val {
                        Value::Number(n) => n.as_i64().unwrap_or(own_cid),
                        _ => own_cid,
                    };
                    if ncid != own_cid {
                        let label = labels
                            .get(&ncid)
                            .cloned()
                            .unwrap_or_else(|| format!("Community {}", ncid));
                        *counts.entry(label).or_insert(0) += 1;
                    }
                }
            }
        }
    }

    // Sort descending by count.
    let mut pairs: Vec<(String, usize)> = counts.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1));
    pairs
}

// ---------------------------------------------------------------------------
// Article generators
// ---------------------------------------------------------------------------

/// Render a community article as a Markdown string.
///
/// Mirrors Python `_community_article`.
fn community_article(
    graph: &Graph,
    cid: i64,
    nodes: &[String],
    label: &str,
    labels: &HashMap<i64, String>,
    cohesion: Option<f64>,
) -> String {
    // Top 25 nodes by degree, descending.
    let mut top_nodes: Vec<&String> = nodes.iter().collect();
    top_nodes.sort_by(|a, b| graph.degree(b).cmp(&graph.degree(a)));
    top_nodes.truncate(25);

    let cross = cross_community_links(graph, nodes, cid, labels);

    // Count edge confidences across all members.
    let mut conf_counts: HashMap<&str, usize> = HashMap::new();
    for nid in nodes {
        for neighbor in graph.neighbors(nid) {
            if let Some(ed) = graph.get_edge(nid, neighbor) {
                let conf = ed
                    .get("confidence")
                    .and_then(|v| v.as_str())
                    .unwrap_or("EXTRACTED");
                *conf_counts.entry(conf).or_insert(0) += 1;
            }
        }
    }
    let total_edges = conf_counts.values().sum::<usize>().max(1);

    // Unique non-empty source files.
    let mut sources: Vec<String> = nodes
        .iter()
        .filter_map(|n| {
            graph
                .get_node(n)
                .and_then(|a| a.get("source_file"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    sources.sort();

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("# {}", label));
    lines.push(String::new());

    // Meta line.
    let mut meta_parts = vec![format!("{} nodes", nodes.len())];
    if let Some(coh) = cohesion {
        meta_parts.push(format!("cohesion {:.2}", coh));
    }
    lines.push(format!("> {}", meta_parts.join(" · ")));
    lines.push(String::new());
    lines.push("## Key Concepts".to_string());
    lines.push(String::new());

    for nid in &top_nodes {
        let d = graph.get_node(nid);
        let node_label = d
            .and_then(|a| a.get("label"))
            .and_then(|v| v.as_str())
            .unwrap_or(nid.as_str());
        let src = d
            .and_then(|a| a.get("source_file"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let degree = graph.degree(nid);
        let src_str = if src.is_empty() {
            String::new()
        } else {
            format!(" — `{}`", src)
        };
        lines.push(format!(
            "- **{}** ({} connections){}",
            node_label, degree, src_str
        ));
    }

    let remaining = nodes.len().saturating_sub(top_nodes.len());
    if remaining > 0 {
        lines.push(format!(
            "- *... and {} more nodes in this community*",
            remaining
        ));
    }
    lines.push(String::new());

    lines.push("## Relationships".to_string());
    lines.push(String::new());
    if !cross.is_empty() {
        for (other_label, count) in cross.iter().take(12) {
            lines.push(format!(
                "- [[{}]] ({} shared connections)",
                other_label, count
            ));
        }
    } else {
        lines.push("- No strong cross-community connections detected".to_string());
    }
    lines.push(String::new());

    if !sources.is_empty() {
        lines.push("## Source Files".to_string());
        lines.push(String::new());
        for src in sources.iter().take(20) {
            lines.push(format!("- `{}`", src));
        }
        lines.push(String::new());
    }

    lines.push("## Audit Trail".to_string());
    lines.push(String::new());
    for conf in &["EXTRACTED", "INFERRED", "AMBIGUOUS"] {
        let n = *conf_counts.get(conf).unwrap_or(&0);
        let pct = (n as f64 / total_edges as f64 * 100.0).round() as usize;
        lines.push(format!("- {}: {} ({}%)", conf, n, pct));
    }
    lines.push(String::new());

    lines.push("---".to_string());
    lines.push(String::new());
    lines.push(
        "*Part of the graphify knowledge wiki. See [[index]] to navigate.*"
            .to_string(),
    );

    lines.join("\n")
}

/// Render a god-node article as a Markdown string.
///
/// Mirrors Python `_god_node_article`.
fn god_node_article(
    graph: &Graph,
    nid: &str,
    labels: &HashMap<i64, String>,
) -> String {
    let d = graph.get_node(nid);
    let node_label = d
        .and_then(|a| a.get("label"))
        .and_then(|v| v.as_str())
        .unwrap_or(nid);
    let src = d
        .and_then(|a| a.get("source_file"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let cid_opt: Option<i64> = d
        .and_then(|a| a.get("community"))
        .and_then(|v| v.as_i64());
    let community_name = cid_opt.map(|cid| {
        labels
            .get(&cid)
            .cloned()
            .unwrap_or_else(|| format!("Community {}", cid))
    });

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("# {}", node_label));
    lines.push(String::new());
    lines.push(format!(
        "> God node · {} connections · `{}`",
        graph.degree(nid),
        src
    ));
    lines.push(String::new());

    if let Some(ref cname) = community_name {
        lines.push(format!("**Community:** [[{}]]", cname));
        lines.push(String::new());
    }

    // Group neighbours by relation type.  Sort neighbours by degree desc.
    let mut by_relation: HashMap<String, Vec<String>> = HashMap::new();
    let mut neighbors: Vec<&str> = graph.neighbors(nid);
    neighbors.sort_by(|a, b| graph.degree(b).cmp(&graph.degree(a)));

    for neighbor in &neighbors {
        let nd = graph.get_node(neighbor);
        let ed = graph.get_edge(nid, neighbor);
        let rel = ed
            .and_then(|e| e.get("relation"))
            .and_then(|v| v.as_str())
            .unwrap_or("related")
            .to_string();
        let neighbor_label = nd
            .and_then(|a| a.get("label"))
            .and_then(|v| v.as_str())
            .unwrap_or(neighbor);
        let conf = ed
            .and_then(|e| e.get("confidence"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let conf_str = if conf.is_empty() {
            String::new()
        } else {
            format!(" `{}`", conf)
        };
        by_relation
            .entry(rel)
            .or_default()
            .push(format!("[[{}]]{}", neighbor_label, conf_str));
    }

    lines.push("## Connections by Relation".to_string());
    lines.push(String::new());

    // Sort relations alphabetically for deterministic output.
    let mut rel_keys: Vec<&String> = by_relation.keys().collect();
    rel_keys.sort();

    for rel in rel_keys {
        let targets = &by_relation[rel];
        lines.push(format!("### {}", rel));
        for t in targets.iter().take(20) {
            lines.push(format!("- {}", t));
        }
        lines.push(String::new());
    }

    lines.push("---".to_string());
    lines.push(String::new());
    lines.push(
        "*Part of the graphify knowledge wiki. See [[index]] to navigate.*"
            .to_string(),
    );

    lines.join("\n")
}

/// Render the `index.md` navigation article.
///
/// Mirrors Python `_index_md`.
fn index_md(
    communities: &HashMap<i64, Vec<String>>,
    labels: &HashMap<i64, String>,
    god_nodes_data: &[HashMap<String, Value>],
    total_nodes: usize,
    total_edges: usize,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("# Knowledge Graph Index".to_string());
    lines.push(String::new());
    lines.push(
        "> Auto-generated by graphify. Start here — read community articles \
         for context, then drill into god nodes for detail."
            .to_string(),
    );
    lines.push(String::new());
    lines.push(format!(
        "**{} nodes · {} edges · {} communities**",
        total_nodes,
        total_edges,
        communities.len()
    ));
    lines.push(String::new());
    lines.push("---".to_string());
    lines.push(String::new());
    lines.push("## Communities".to_string());
    lines.push("(sorted by size, largest first)".to_string());
    lines.push(String::new());

    // Sort communities by descending member count.
    let mut sorted_cids: Vec<i64> = communities.keys().copied().collect();
    sorted_cids.sort_by(|a, b| {
        let la = communities[a].len();
        let lb = communities[b].len();
        lb.cmp(&la)
    });

    for cid in &sorted_cids {
        let nodes = &communities[cid];
        let label = labels
            .get(cid)
            .cloned()
            .unwrap_or_else(|| format!("Community {}", cid));
        lines.push(format!("- [[{}]] — {} nodes", label, nodes.len()));
    }
    lines.push(String::new());

    if !god_nodes_data.is_empty() {
        lines.push("## God Nodes".to_string());
        lines.push(
            "(most connected concepts — the load-bearing abstractions)".to_string(),
        );
        lines.push(String::new());
        for node in god_nodes_data {
            let label = node
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let degree = node.get("degree").and_then(|v| v.as_i64()).unwrap_or(0);
            lines.push(format!("- [[{}]] — {} connections", label, degree));
        }
        lines.push(String::new());
    }

    lines.push("---".to_string());
    lines.push(String::new());
    lines.push(
        "*Generated by [graphify](https://github.com/safishamsi/graphify)*"
            .to_string(),
    );

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Write a complete wiki to `output_dir`.
///
/// Creates the directory (and parents) if it does not exist.  Deletes any
/// existing `*.md` files in the directory before writing new articles.
///
/// Returns the total number of article files written (excluding `index.md`).
///
/// Mirrors Python `to_wiki`.
pub fn to_wiki(
    graph: &Graph,
    communities: &HashMap<i64, Vec<String>>,
    output_dir: &str,
    community_labels: Option<&HashMap<i64, String>>,
    cohesion: Option<&HashMap<i64, f64>>,
    god_nodes_data: Option<&[HashMap<String, Value>]>,
) -> Result<usize, Box<dyn std::error::Error>> {
    let out = Path::new(output_dir);
    fs::create_dir_all(out)?;

    // Remove old .md files.
    for entry in fs::read_dir(out)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            fs::remove_file(&path)?;
        }
    }

    // Build a default labels map if none provided.
    let owned_labels: HashMap<i64, String>;
    let labels: &HashMap<i64, String> = match community_labels {
        Some(m) => m,
        None => {
            owned_labels = communities
                .keys()
                .map(|&cid| (cid, format!("Community {}", cid)))
                .collect();
            &owned_labels
        }
    };

    let empty_cohesion: HashMap<i64, f64> = HashMap::new();
    let cohesion = cohesion.unwrap_or(&empty_cohesion);

    let empty_gods: Vec<HashMap<String, Value>> = Vec::new();
    let god_nodes_data = god_nodes_data.unwrap_or(&empty_gods);

    let mut used_slugs: HashSet<String> = HashSet::new();
    let mut count = 0usize;

    // -----------------------------------------------------------------------
    // Community articles.
    // -----------------------------------------------------------------------
    // Process communities in a stable order (sorted by cid).
    let mut cids: Vec<i64> = communities.keys().copied().collect();
    cids.sort_unstable();

    for cid in &cids {
        let nodes = &communities[cid];
        let label = labels
            .get(cid)
            .cloned()
            .unwrap_or_else(|| format!("Community {}", cid));
        let cohesion_val = cohesion.get(cid).copied();
        let article = community_article(graph, *cid, nodes, &label, labels, cohesion_val);
        let slug = unique_slug(&safe_filename(&label), &mut used_slugs);
        let path = out.join(format!("{}.md", slug));
        fs::write(&path, article)?;
        count += 1;
    }

    // -----------------------------------------------------------------------
    // God-node articles.
    // -----------------------------------------------------------------------
    for node_data in god_nodes_data {
        let nid = match node_data.get("id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => continue,
        };
        if !graph.has_node(nid) {
            continue;
        }
        let article = god_node_article(graph, nid, labels);
        let node_label = node_data
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or(nid);
        let slug = unique_slug(&safe_filename(node_label), &mut used_slugs);
        let path = out.join(format!("{}.md", slug));
        fs::write(&path, article)?;
        count += 1;
    }

    // -----------------------------------------------------------------------
    // Index article.
    // -----------------------------------------------------------------------
    let index = index_md(
        communities,
        labels,
        god_nodes_data,
        graph.number_of_nodes(),
        graph.number_of_edges(),
    );
    fs::write(out.join("index.md"), index)?;

    Ok(count)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Graph, NodeAttrs};
    use serde_json::json;

    fn make_graph() -> (Graph, HashMap<i64, Vec<String>>, HashMap<i64, String>) {
        let mut g = Graph::new(false);
        let mut na: NodeAttrs = HashMap::new();
        na.insert("label".to_string(), json!("Alpha"));
        na.insert("file_type".to_string(), json!("concept"));
        na.insert("source_file".to_string(), json!("a.md"));
        na.insert("community".to_string(), json!(0));
        g.add_node("n1", na.clone());

        na.insert("label".to_string(), json!("Beta"));
        na.insert("community".to_string(), json!(0));
        g.add_node("n2", na.clone());

        na.insert("label".to_string(), json!("Gamma"));
        na.insert("community".to_string(), json!(1));
        g.add_node("n3", na);

        let mut ea = HashMap::new();
        ea.insert("relation".to_string(), json!("uses"));
        ea.insert("confidence".to_string(), json!("EXTRACTED"));
        g.add_edge("n1", "n2", ea.clone());
        g.add_edge("n1", "n3", ea);

        let communities: HashMap<i64, Vec<String>> = [
            (0, vec!["n1".to_string(), "n2".to_string()]),
            (1, vec!["n3".to_string()]),
        ]
        .into_iter()
        .collect();

        let labels: HashMap<i64, String> = [
            (0, "Core".to_string()),
            (1, "Peripheral".to_string()),
        ]
        .into_iter()
        .collect();

        (g, communities, labels)
    }

    #[test]
    fn safe_filename_basic() {
        assert_eq!(safe_filename("Hello World"), "Hello_World");
        assert_eq!(safe_filename("A/B"), "A-B");
        assert_eq!(safe_filename("A:B"), "A-B");
        assert_eq!(safe_filename(""), "unnamed");
    }

    #[test]
    fn safe_filename_strips_dots() {
        assert_eq!(safe_filename("...hidden..."), "hidden");
    }

    #[test]
    fn safe_filename_truncates() {
        let long = "a".repeat(300);
        assert_eq!(safe_filename(&long).len(), 200);
    }

    #[test]
    fn unique_slug_deduplicates() {
        let mut used = HashSet::new();
        let s1 = unique_slug("foo", &mut used);
        let s2 = unique_slug("foo", &mut used);
        let s3 = unique_slug("foo", &mut used);
        assert_eq!(s1, "foo");
        assert_eq!(s2, "foo_2");
        assert_eq!(s3, "foo_3");
    }

    #[test]
    fn community_article_contains_header() {
        let (g, communities, labels) = make_graph();
        let nodes = &communities[&0];
        let art = community_article(&g, 0, nodes, "Core", &labels, Some(0.75));
        assert!(art.starts_with("# Core"));
        assert!(art.contains("## Key Concepts"));
        assert!(art.contains("## Relationships"));
        assert!(art.contains("## Audit Trail"));
        assert!(art.contains("cohesion 0.75"));
    }

    #[test]
    fn god_node_article_contains_header() {
        let (g, _, labels) = make_graph();
        let art = god_node_article(&g, "n1", &labels);
        assert!(art.starts_with("# Alpha"));
        assert!(art.contains("God node"));
        assert!(art.contains("## Connections by Relation"));
    }

    #[test]
    fn index_md_contains_sections() {
        let (g, communities, labels) = make_graph();
        let idx = index_md(&communities, &labels, &[], g.number_of_nodes(), g.number_of_edges());
        assert!(idx.contains("# Knowledge Graph Index"));
        assert!(idx.contains("## Communities"));
        assert!(idx.contains("[[Core]]"));
    }

    #[test]
    fn to_wiki_writes_files() {
        let (g, communities, labels) = make_graph();
        let tmp_dir = std::env::temp_dir()
            .join("graphify_wiki_test")
            .to_string_lossy()
            .to_string();
        let count = to_wiki(
            &g,
            &communities,
            &tmp_dir,
            Some(&labels),
            None,
            None,
        )
        .unwrap();
        // 2 communities → 2 articles + index.md written separately.
        assert_eq!(count, 2);
        let index_path = Path::new(&tmp_dir).join("index.md");
        assert!(index_path.exists());
        let index_content = fs::read_to_string(index_path).unwrap();
        assert!(index_content.contains("# Knowledge Graph Index"));
    }

    #[test]
    fn to_wiki_clears_old_md_files() {
        let tmp_dir = std::env::temp_dir()
            .join("graphify_wiki_clear_test")
            .to_string_lossy()
            .to_string();
        fs::create_dir_all(&tmp_dir).unwrap();
        let stale = Path::new(&tmp_dir).join("stale_article.md");
        fs::write(&stale, "old content").unwrap();

        let (g, communities, labels) = make_graph();
        to_wiki(&g, &communities, &tmp_dir, Some(&labels), None, None).unwrap();

        // Stale file should be gone.
        assert!(!stale.exists());
    }
}
