// Token-reduction benchmark - measures how much context graphify saves vs naive full-corpus approach.
#![allow(dead_code)]
use crate::types::Graph;
use std::collections::HashSet;

const CHARS_PER_TOKEN: usize = 4;

fn estimate_tokens(text: &str) -> usize {
    (text.len() / CHARS_PER_TOKEN).max(1)
}

pub const SAMPLE_QUESTIONS: &[&str] = &[
    "how does authentication work",
    "what is the main entry point",
    "how are errors handled",
    "what connects the data layer to the api",
    "what are the core abstractions",
];

#[derive(Debug, Clone)]
pub struct QuestionResult {
    pub question: String,
    pub query_tokens: usize,
    pub reduction: f64,
}

#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    pub corpus_tokens: usize,
    pub corpus_words: usize,
    pub nodes: usize,
    pub edges: usize,
    pub avg_query_tokens: usize,
    pub reduction_ratio: f64,
    pub per_question: Vec<QuestionResult>,
    pub error: Option<String>,
}

impl Default for BenchmarkResult {
    fn default() -> Self {
        BenchmarkResult {
            corpus_tokens: 0,
            corpus_words: 0,
            nodes: 0,
            edges: 0,
            avg_query_tokens: 0,
            reduction_ratio: 0.0,
            per_question: Vec::new(),
            error: None,
        }
    }
}

/// Run BFS from best-matching nodes and return estimated tokens in the subgraph context.
fn query_subgraph_tokens(graph: &Graph, question: &str, depth: usize) -> usize {
    let terms: Vec<String> = question
        .split_whitespace()
        .filter(|t| t.len() > 2)
        .map(|t| t.to_lowercase())
        .collect();

    // Score each node by how many terms appear in its label
    let mut scored: Vec<(usize, &str)> = graph
        .nodes
        .iter()
        .filter_map(|(nid, data)| {
            let label = data
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            let score = terms.iter().filter(|t| label.contains(t.as_str())).count();
            if score > 0 {
                Some((score, nid.as_str()))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));

    let start_nodes: Vec<&str> = scored.iter().take(3).map(|(_, nid)| *nid).collect();
    if start_nodes.is_empty() {
        return 0;
    }

    let mut visited: HashSet<&str> = start_nodes.iter().copied().collect();
    let mut frontier: HashSet<&str> = start_nodes.iter().copied().collect();
    let mut edges_seen: Vec<(&str, &str)> = Vec::new();

    for _ in 0..depth {
        let mut next_frontier: HashSet<&str> = HashSet::new();
        for &n in &frontier {
            for neighbor in graph.neighbors(n) {
                if !visited.contains(neighbor) {
                    next_frontier.insert(neighbor);
                    edges_seen.push((n, neighbor));
                }
            }
        }
        for nid in &next_frontier {
            visited.insert(nid);
        }
        frontier = next_frontier;
    }

    let mut lines: Vec<String> = Vec::new();

    for &nid in &visited {
        if let Some(data) = graph.nodes.get(nid) {
            let label = data.get("label").and_then(|v| v.as_str()).unwrap_or(nid);
            let src = data
                .get("source_file")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let loc = data
                .get("source_location")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            lines.push(format!("NODE {} src={} loc={}", label, src, loc));
        }
    }

    for &(u, v) in &edges_seen {
        if visited.contains(u) && visited.contains(v) {
            let relation = graph
                .get_edge(u, v)
                .and_then(|attrs| attrs.get("relation"))
                .and_then(|r| r.as_str())
                .unwrap_or("");
            let u_label = graph
                .nodes
                .get(u)
                .and_then(|d| d.get("label"))
                .and_then(|v| v.as_str())
                .unwrap_or(u);
            let v_label = graph
                .nodes
                .get(v)
                .and_then(|d| d.get("label"))
                .and_then(|v2| v2.as_str())
                .unwrap_or(v);
            lines.push(format!("EDGE {} --{}--> {}", u_label, relation, v_label));
        }
    }

    estimate_tokens(&lines.join("\n"))
}

/// Measure token reduction: corpus tokens vs graphify query tokens.
///
/// * `corpus_words`: total word count from detect() output; if None, estimated from graph.
/// * `questions`: questions to benchmark; defaults to SAMPLE_QUESTIONS.
pub fn run_benchmark(
    graph: &Graph,
    corpus_words: Option<usize>,
    questions: Option<&[&str]>,
) -> BenchmarkResult {
    let n_nodes = graph.number_of_nodes();
    let n_edges = graph.number_of_edges();

    let cw = corpus_words.unwrap_or(n_nodes * 50);
    // words → tokens: 100 words ≈ 133 tokens
    let corpus_tokens = cw * 100 / 75;

    let qs: &[&str] = questions.unwrap_or(SAMPLE_QUESTIONS);

    let mut per_question: Vec<QuestionResult> = Vec::new();
    for &q in qs {
        let qt = query_subgraph_tokens(graph, q, 3);
        if qt > 0 {
            let reduction = if qt > 0 {
                (corpus_tokens as f64 / qt as f64 * 10.0).round() / 10.0
            } else {
                0.0
            };
            per_question.push(QuestionResult {
                question: q.to_string(),
                query_tokens: qt,
                reduction,
            });
        }
    }

    if per_question.is_empty() {
        return BenchmarkResult {
            error: Some(
                "No matching nodes found for sample questions. Build the graph first.".to_string(),
            ),
            nodes: n_nodes,
            edges: n_edges,
            corpus_words: cw,
            corpus_tokens,
            ..Default::default()
        };
    }

    let avg_query_tokens =
        per_question.iter().map(|p| p.query_tokens).sum::<usize>() / per_question.len();

    let reduction_ratio = if avg_query_tokens > 0 {
        (corpus_tokens as f64 / avg_query_tokens as f64 * 10.0).round() / 10.0
    } else {
        0.0
    };

    BenchmarkResult {
        corpus_tokens,
        corpus_words: cw,
        nodes: n_nodes,
        edges: n_edges,
        avg_query_tokens,
        reduction_ratio,
        per_question,
        error: None,
    }
}

/// Print a human-readable benchmark report.
pub fn print_benchmark(result: &BenchmarkResult) {
    if let Some(ref err) = result.error {
        println!("Benchmark error: {}", err);
        return;
    }
    println!("\ngraphify token reduction benchmark");
    println!("{}", "\u{2500}".repeat(50));
    println!(
        "  Corpus:          {:>12} words → ~{:>12} tokens (naive)",
        format_number(result.corpus_words),
        format_number(result.corpus_tokens)
    );
    println!(
        "  Graph:           {:>12} nodes, {:>12} edges",
        format_number(result.nodes),
        format_number(result.edges)
    );
    println!(
        "  Avg query cost:  ~{:>12} tokens",
        format_number(result.avg_query_tokens)
    );
    println!(
        "  Reduction:       {}x fewer tokens per query",
        result.reduction_ratio
    );
    println!("\n  Per question:");
    for p in &result.per_question {
        let q_trunc = if p.question.len() > 55 {
            &p.question[..55]
        } else {
            &p.question
        };
        println!("    [{}x] {}", p.reduction, q_trunc);
    }
    println!();
}

fn format_number(n: usize) -> String {
    // Insert thousands separators
    let s = n.to_string();
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && (chars.len() - i) % 3 == 0 {
            result.push(',');
        }
        result.push(*c);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Graph, NodeAttrs};
    use serde_json::Value;

    #[allow(dead_code)]
    fn make_graph_with_nodes(labels: &[(&str, &str)]) -> Graph {
        // labels: [(id, label)]
        let mut g = Graph::new(true);
        for (id, label) in labels {
            let mut attrs: NodeAttrs = std::collections::HashMap::new();
            attrs.insert("label".to_string(), Value::String(label.to_string()));
            g.add_node(id, attrs);
        }
        g
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens("aaaa"), 1); // 4 chars = 1 token
        assert_eq!(estimate_tokens("aaaabbbb"), 2); // 8 chars = 2 tokens
        assert_eq!(estimate_tokens(""), 1); // minimum is 1
    }

    #[test]
    fn test_run_benchmark_empty_graph() {
        let g = Graph::new(true);
        let result = run_benchmark(&g, None, None);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_run_benchmark_with_matching_nodes() {
        let mut g = Graph::new(true);
        let mut attrs: NodeAttrs = std::collections::HashMap::new();
        attrs.insert(
            "label".to_string(),
            Value::String("authentication module".to_string()),
        );
        g.add_node("n1", attrs.clone());
        attrs.insert(
            "label".to_string(),
            Value::String("main entry point".to_string()),
        );
        g.add_node("n2", attrs.clone());

        let result = run_benchmark(&g, Some(10000), Some(&["how does authentication work"]));
        assert!(result.error.is_none());
        assert!(result.reduction_ratio > 0.0);
        assert_eq!(result.per_question.len(), 1);
    }

    #[test]
    fn test_corpus_words_estimation() {
        let mut g = Graph::new(true);
        let mut attrs: NodeAttrs = std::collections::HashMap::new();
        attrs.insert(
            "label".to_string(),
            Value::String("authentication".to_string()),
        );
        g.add_node("n1", attrs);
        // No corpus_words provided - should use nodes * 50 = 1 * 50 = 50
        let result = run_benchmark(&g, None, Some(&["how does authentication work"]));
        let expected_corpus_words = 50usize;
        let expected_corpus_tokens = expected_corpus_words * 100 / 75;
        assert_eq!(result.corpus_words, expected_corpus_words);
        assert_eq!(result.corpus_tokens, expected_corpus_tokens);
    }

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(1000000), "1,000,000");
        assert_eq!(format_number(42), "42");
    }
}
