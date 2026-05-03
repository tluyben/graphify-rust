//! Report generation: produces a Markdown graph-report document.
//!
//! Ported from Python `report.py`.

use std::collections::HashMap;

use regex::Regex;
use serde_json::Value;

use crate::analyze::{is_concept_node, is_file_node};
use crate::types::Graph;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the `confidence` string from an edge attribute map.
fn edge_confidence(attrs: &crate::types::EdgeAttrs) -> String {
    attrs
        .get("confidence")
        .and_then(|v| v.as_str())
        .unwrap_or("EXTRACTED")
        .to_string()
}

/// Sanitise a community label so it is safe as an Obsidian wiki-link segment
/// and as a filesystem filename.
///
/// Mirrors Python `_safe_community_name`.
pub fn safe_community_name(label: &str) -> String {
    // Normalise newlines → space.
    let s = label
        .replace("\r\n", " ")
        .replace('\r', " ")
        .replace('\n', " ");

    // Strip characters that are illegal in Obsidian / Windows filenames.
    let illegal = Regex::new(r#"[\\/*?:"<>|#^\[\]]"#).expect("valid regex");
    let cleaned = illegal.replace_all(&s, "").trim().to_string();

    // Drop trailing .md / .mdx / .markdown extension (case-insensitive).
    let ext_re =
        Regex::new(r"(?i)\.(md|mdx|markdown)$").expect("valid regex");
    let cleaned = ext_re.replace(&cleaned, "").to_string();

    if cleaned.is_empty() {
        "unnamed".to_string()
    } else {
        cleaned
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Generate the full Markdown report and return it as a `String`.
///
/// Arguments mirror the Python `generate()` signature exactly.
pub fn generate(
    graph: &Graph,
    communities: &HashMap<i64, Vec<String>>,
    cohesion_scores: &HashMap<i64, f64>,
    community_labels: &HashMap<i64, String>,
    god_node_list: &[HashMap<String, Value>],
    surprise_list: &[HashMap<String, Value>],
    detection_result: &HashMap<String, Value>,
    token_cost: &HashMap<String, Value>,
    root: &str,
    suggested_questions: Option<&[HashMap<String, Value>]>,
    min_community_size: usize,
) -> String {
    // -----------------------------------------------------------------------
    // Date
    // -----------------------------------------------------------------------
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    // -----------------------------------------------------------------------
    // Edge confidence breakdown
    // -----------------------------------------------------------------------
    let edges = graph.edges_iter();
    let total = edges.len().max(1);

    let ext_count = edges
        .iter()
        .filter(|(_, _, d)| edge_confidence(d) == "EXTRACTED")
        .count();
    let inf_count = edges
        .iter()
        .filter(|(_, _, d)| edge_confidence(d) == "INFERRED")
        .count();
    let amb_count = edges
        .iter()
        .filter(|(_, _, d)| edge_confidence(d) == "AMBIGUOUS")
        .count();

    let ext_pct = (ext_count as f64 / total as f64 * 100.0).round() as u64;
    let inf_pct = (inf_count as f64 / total as f64 * 100.0).round() as u64;
    let amb_pct = (amb_count as f64 / total as f64 * 100.0).round() as u64;

    let inf_edges: Vec<_> = edges
        .iter()
        .filter(|(_, _, d)| edge_confidence(d) == "INFERRED")
        .collect();

    let inf_scores: Vec<f64> = inf_edges
        .iter()
        .map(|(_, _, d)| {
            d.get("confidence_score")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.5)
        })
        .collect();

    let inf_avg: Option<f64> = if inf_scores.is_empty() {
        None
    } else {
        let sum: f64 = inf_scores.iter().sum();
        // Round to 2 decimal places like Python's `round(x, 2)`.
        Some((sum / inf_scores.len() as f64 * 100.0).round() / 100.0)
    };

    // -----------------------------------------------------------------------
    // Build lines vector
    // -----------------------------------------------------------------------
    let mut lines: Vec<String> = Vec::new();

    lines.push(format!("# Graph Report - {}  ({})", root, today));
    lines.push(String::new());
    lines.push("## Corpus Check".to_string());

    if let Some(warning) = detection_result
        .get("warning")
        .and_then(|v| v.as_str())
    {
        lines.push(format!("- {}", warning));
    } else {
        let total_files = detection_result
            .get("total_files")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let total_words = detection_result
            .get("total_words")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        lines.push(format!(
            "- {} files · ~{} words",
            total_files,
            format_number(total_words as u64)
        ));
        lines.push(
            "- Verdict: corpus is large enough that graph structure adds value."
                .to_string(),
        );
    }

    // Non-empty communities (have at least one non-file node).
    let non_empty: HashMap<i64, &Vec<String>> = communities
        .iter()
        .filter(|(_, nodes)| nodes.iter().any(|n| !is_file_node(graph, n)))
        .map(|(&cid, nodes)| (cid, nodes))
        .collect();

    // -----------------------------------------------------------------------
    // Summary
    // -----------------------------------------------------------------------
    lines.push(String::new());
    lines.push("## Summary".to_string());

    lines.push(format!(
        "- {} nodes · {} edges · {} communities detected",
        graph.number_of_nodes(),
        graph.number_of_edges(),
        non_empty.len()
    ));

    let extraction_line = {
        let base = format!(
            "- Extraction: {}% EXTRACTED · {}% INFERRED · {}% AMBIGUOUS",
            ext_pct, inf_pct, amb_pct
        );
        if let Some(avg) = inf_avg {
            format!(
                "{} · INFERRED: {} edges (avg confidence: {})",
                base,
                inf_edges.len(),
                avg
            )
        } else {
            base
        }
    };
    lines.push(extraction_line);

    let input_cost = token_cost
        .get("input")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let output_cost = token_cost
        .get("output")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    lines.push(format!(
        "- Token cost: {} input · {} output",
        format_number(input_cost as u64),
        format_number(output_cost as u64)
    ));

    // -----------------------------------------------------------------------
    // Community Hubs
    // -----------------------------------------------------------------------
    if !non_empty.is_empty() {
        lines.push(String::new());
        lines.push("## Community Hubs (Navigation)".to_string());

        let mut cids: Vec<i64> = non_empty.keys().copied().collect();
        cids.sort_unstable();
        for cid in &cids {
            let label = community_labels
                .get(cid)
                .cloned()
                .unwrap_or_else(|| format!("Community {}", cid));
            let safe = safe_community_name(&label);
            lines.push(format!("- [[_COMMUNITY_{}|{}]]", safe, label));
        }
    }

    // -----------------------------------------------------------------------
    // God Nodes
    // -----------------------------------------------------------------------
    lines.push(String::new());
    lines.push("## God Nodes (most connected - your core abstractions)".to_string());
    for (i, node) in god_node_list.iter().enumerate() {
        let label = node
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let degree = node
            .get("degree")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        lines.push(format!("{}. `{}` - {} edges", i + 1, label, degree));
    }

    // -----------------------------------------------------------------------
    // Surprising Connections
    // -----------------------------------------------------------------------
    lines.push(String::new());
    lines.push(
        "## Surprising Connections (you probably didn't know these)".to_string(),
    );

    if !surprise_list.is_empty() {
        for s in surprise_list {
            let source = s.get("source").and_then(|v| v.as_str()).unwrap_or("?");
            let target = s.get("target").and_then(|v| v.as_str()).unwrap_or("?");
            let relation = s
                .get("relation")
                .and_then(|v| v.as_str())
                .unwrap_or("related_to");
            let note = s
                .get("note")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let conf = s
                .get("confidence")
                .and_then(|v| v.as_str())
                .unwrap_or("EXTRACTED");
            let cscore = s
                .get("confidence_score")
                .and_then(|v| v.as_f64());

            let conf_tag = if conf == "INFERRED" {
                if let Some(cs) = cscore {
                    format!("INFERRED {:.2}", cs)
                } else {
                    conf.to_string()
                }
            } else {
                conf.to_string()
            };

            let sem_tag = if relation == "semantically_similar_to" {
                " [semantically similar]"
            } else {
                ""
            };

            // Source files: expect an array of two strings.
            let empty_files = vec![
                Value::String(String::new()),
                Value::String(String::new()),
            ];
            let files_val = s.get("source_files");
            let files: Vec<&str> = files_val
                .and_then(|v| v.as_array())
                .unwrap_or(&empty_files)
                .iter()
                .map(|v| v.as_str().unwrap_or(""))
                .collect();
            let file0 = files.first().copied().unwrap_or("");
            let file1 = files.get(1).copied().unwrap_or("");

            lines.push(format!(
                "- `{}` --{}--> `{}`  [{}]{}",
                source, relation, target, conf_tag, sem_tag
            ));
            let file_line = if note.is_empty() {
                format!("  {} → {}", file0, file1)
            } else {
                format!("  {} → {}  _{}_", file0, file1, note)
            };
            lines.push(file_line);
        }
    } else {
        lines.push(
            "- None detected - all connections are within the same source files."
                .to_string(),
        );
    }

    // -----------------------------------------------------------------------
    // Hyperedges
    // -----------------------------------------------------------------------
    let hyperedges: Vec<&Value> = graph
        .graph
        .get("hyperedges")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().collect())
        .unwrap_or_default();

    if !hyperedges.is_empty() {
        lines.push(String::new());
        lines.push("## Hyperedges (group relationships)".to_string());
        for h in &hyperedges {
            let node_labels: Vec<&str> = h
                .get("nodes")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            let node_labels_str = node_labels.join(", ");

            let conf = h
                .get("confidence")
                .and_then(|v| v.as_str())
                .unwrap_or("INFERRED");
            let cscore = h.get("confidence_score").and_then(|v| v.as_f64());
            let conf_tag = if let Some(cs) = cscore {
                format!("{} {:.2}", conf, cs)
            } else {
                conf.to_string()
            };

            let hyperedge_label = h
                .get("label")
                .or_else(|| h.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            lines.push(format!(
                "- **{}** — {} [{}]",
                hyperedge_label, node_labels_str, conf_tag
            ));
        }
    }

    // -----------------------------------------------------------------------
    // Communities section
    // -----------------------------------------------------------------------
    let thin_count = communities
        .values()
        .filter(|nodes| {
            let real = nodes.iter().filter(|n| !is_file_node(graph, n)).count();
            real > 0 && real < min_community_size
        })
        .count();

    lines.push(String::new());
    lines.push(format!(
        "## Communities ({} total, {} thin omitted)",
        communities.len(),
        thin_count
    ));

    let mut comm_cids: Vec<i64> = communities.keys().copied().collect();
    comm_cids.sort_unstable();

    for cid in &comm_cids {
        let nodes = &communities[cid];
        let label = community_labels
            .get(cid)
            .cloned()
            .unwrap_or_else(|| format!("Community {}", cid));
        let score = cohesion_scores.get(cid).copied().unwrap_or(0.0);

        let real_nodes: Vec<&String> = nodes
            .iter()
            .filter(|n| !is_file_node(graph, n))
            .collect();

        if real_nodes.is_empty() || real_nodes.len() < min_community_size {
            continue;
        }

        let display: Vec<String> = real_nodes
            .iter()
            .take(8)
            .map(|n| {
                graph
                    .get_node(n)
                    .and_then(|attrs| attrs.get("label"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(n.as_str())
                    .to_string()
            })
            .collect();

        let suffix = if real_nodes.len() > 8 {
            format!(" (+{} more)", real_nodes.len() - 8)
        } else {
            String::new()
        };

        lines.push(String::new());
        lines.push(format!(
            "### Community {} - \"{}\"",
            cid, label
        ));
        lines.push(format!("Cohesion: {}", score));
        lines.push(format!(
            "Nodes ({}): {}{}",
            real_nodes.len(),
            display.join(", "),
            suffix
        ));
    }

    // -----------------------------------------------------------------------
    // Ambiguous Edges
    // -----------------------------------------------------------------------
    let ambiguous: Vec<(&str, &str, &crate::types::EdgeAttrs)> = edges
        .iter()
        .filter(|(_, _, d)| edge_confidence(d) == "AMBIGUOUS")
        .copied()
        .collect();

    if !ambiguous.is_empty() {
        lines.push(String::new());
        lines.push("## Ambiguous Edges - Review These".to_string());
        for (u, v, d) in &ambiguous {
            let ul = graph
                .get_node(u)
                .and_then(|a| a.get("label"))
                .and_then(|v| v.as_str())
                .unwrap_or(u);
            let vl = graph
                .get_node(v)
                .and_then(|a| a.get("label"))
                .and_then(|v| v.as_str())
                .unwrap_or(v);
            lines.push(format!("- `{}` → `{}`  [AMBIGUOUS]", ul, vl));
            let source_file = d
                .get("source_file")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let relation = d
                .get("relation")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            lines.push(format!(
                "  {} · relation: {}",
                source_file, relation
            ));
        }
    }

    // -----------------------------------------------------------------------
    // Knowledge Gaps
    // -----------------------------------------------------------------------
    let isolated: Vec<&str> = graph
        .nodes
        .keys()
        .filter(|n| {
            graph.degree(n) <= 1
                && !is_file_node(graph, n)
                && !is_concept_node(graph, n)
        })
        .map(|n| n.as_str())
        .collect();

    let thin_communities: HashMap<i64, &Vec<String>> = communities
        .iter()
        .filter(|(_, nodes)| {
            let real = nodes.iter().filter(|n| !is_file_node(graph, n)).count();
            real > 0 && real < 3
        })
        .map(|(&cid, nodes)| (cid, nodes))
        .collect();

    let gap_count = isolated.len() + thin_communities.len();

    if gap_count > 0 || amb_pct > 20 {
        lines.push(String::new());
        lines.push("## Knowledge Gaps".to_string());

        if !isolated.is_empty() {
            let isolated_labels: Vec<String> = isolated
                .iter()
                .take(5)
                .map(|n| {
                    graph
                        .get_node(n)
                        .and_then(|a| a.get("label"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(n)
                        .to_string()
                })
                .collect();
            let suffix = if isolated.len() > 5 {
                format!(" (+{} more)", isolated.len() - 5)
            } else {
                String::new()
            };
            lines.push(format!(
                "- **{} isolated node(s):** {}{}",
                isolated.len(),
                isolated_labels
                    .iter()
                    .map(|l| format!("`{}`", l))
                    .collect::<Vec<_>>()
                    .join(", "),
                suffix
            ));
            lines.push(
                "  These have ≤1 connection - possible missing edges or undocumented components."
                    .to_string(),
            );
        }

        if !thin_communities.is_empty() {
            lines.push(format!(
                "- **{} thin communities (<{} nodes) omitted from report** — run `graphify query` to explore isolated nodes.",
                thin_communities.len(),
                min_community_size
            ));
        }

        if amb_pct > 20 {
            lines.push(format!(
                "- **High ambiguity: {}% of edges are AMBIGUOUS.** Review the Ambiguous Edges section above.",
                amb_pct
            ));
        }
    }

    // -----------------------------------------------------------------------
    // Suggested Questions
    // -----------------------------------------------------------------------
    if let Some(questions) = suggested_questions {
        if !questions.is_empty() {
            lines.push(String::new());
            lines.push("## Suggested Questions".to_string());

            let no_signal = questions.len() == 1
                && questions[0]
                    .get("type")
                    .and_then(|v| v.as_str())
                    == Some("no_signal");

            if no_signal {
                let why = questions[0]
                    .get("why")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                lines.push(format!("_{}_", why));
            } else {
                lines.push(
                    "_Questions this graph is uniquely positioned to answer:_"
                        .to_string(),
                );
                lines.push(String::new());
                for q in questions {
                    if let Some(question) =
                        q.get("question").and_then(|v| v.as_str())
                    {
                        lines.push(format!("- **{}**", question));
                        let why =
                            q.get("why").and_then(|v| v.as_str()).unwrap_or("");
                        lines.push(format!("  _{}_", why));
                    }
                }
            }
        }
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Formatting helper
// ---------------------------------------------------------------------------

/// Format a `u64` with comma thousands separators (e.g. `1_234_567` → `"1,234,567"`).
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_community_name_strips_illegal_chars() {
        // Slash is stripped (present in the illegal-char set).
        assert_eq!(safe_community_name("Hello/World"), "HelloWorld");
        // Backslash is stripped.
        assert_eq!(safe_community_name("A\\B"), "AB");
        // Angle brackets stripped.
        assert_eq!(safe_community_name("A<B>C"), "ABC");
    }

    #[test]
    fn safe_community_name_strips_md_extension() {
        assert_eq!(safe_community_name("notes.md"), "notes");
        assert_eq!(safe_community_name("notes.MDX"), "notes");
        assert_eq!(safe_community_name("notes.markdown"), "notes");
    }

    #[test]
    fn safe_community_name_empty_becomes_unnamed() {
        assert_eq!(safe_community_name(""), "unnamed");
        assert_eq!(safe_community_name("   "), "unnamed");
        // Only illegal chars → empty → "unnamed"
        assert_eq!(safe_community_name("\\*?"), "unnamed");
    }

    #[test]
    fn safe_community_name_normalises_newlines() {
        assert_eq!(safe_community_name("a\r\nb"), "a b");
        assert_eq!(safe_community_name("a\nb"), "a b");
    }

    #[test]
    fn format_number_works() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1_000), "1,000");
        assert_eq!(format_number(1_234_567), "1,234,567");
    }

    #[test]
    fn generate_smoke_test() {
        use crate::types::{Graph, NodeAttrs};
        use serde_json::json;

        let mut g = Graph::new(false);
        let mut na: NodeAttrs = HashMap::new();
        na.insert("label".to_string(), json!("Alpha"));
        na.insert("file_type".to_string(), json!("concept"));
        na.insert("source_file".to_string(), json!(""));
        g.add_node("n1", na.clone());
        na.insert("label".to_string(), json!("Beta"));
        g.add_node("n2", na);
        g.add_edge("n1", "n2", {
            let mut ea = HashMap::new();
            ea.insert("confidence".to_string(), json!("EXTRACTED"));
            ea.insert("relation".to_string(), json!("uses"));
            ea
        });

        let communities: HashMap<i64, Vec<String>> =
            [(0, vec!["n1".to_string(), "n2".to_string()])]
                .into_iter()
                .collect();
        let cohesion: HashMap<i64, f64> = [(0, 0.8)].into_iter().collect();
        let labels: HashMap<i64, String> =
            [(0, "Core".to_string())].into_iter().collect();

        let mut det: HashMap<String, Value> = HashMap::new();
        det.insert("total_files".to_string(), json!(5));
        det.insert("total_words".to_string(), json!(1000));

        let tok: HashMap<String, Value> = {
            let mut m = HashMap::new();
            m.insert("input".to_string(), json!(500));
            m.insert("output".to_string(), json!(200));
            m
        };

        let report = generate(
            &g,
            &communities,
            &cohesion,
            &labels,
            &[],
            &[],
            &det,
            &tok,
            "test_root",
            None,
            3,
        );

        assert!(report.contains("# Graph Report - test_root"));
        assert!(report.contains("## Summary"));
        assert!(report.contains("## Communities"));
    }
}
