use graphify::analyze::{god_nodes, surprising_connections};
use graphify::build::build_from_json;
use graphify::cluster::{cluster, score_all};
use graphify::report::generate;
use graphify::types::Graph;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

fn make_inputs() -> (
    Graph,
    HashMap<i64, Vec<String>>,
    HashMap<i64, f64>,
    HashMap<i64, String>,
    Vec<HashMap<String, Value>>,
    Vec<HashMap<String, Value>>,
    HashMap<String, Value>,
    HashMap<String, Value>,
) {
    let content = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/extraction.json"),
    )
    .unwrap();
    let extraction: Value = serde_json::from_str(&content).unwrap();

    let g = build_from_json(extraction.clone(), false);

    // cluster() returns HashMap<i64, Vec<String>>
    let communities = cluster(&g);
    let cohesion = score_all(&g, &communities);

    let labels: HashMap<i64, String> = communities
        .keys()
        .map(|&k| (k, format!("Community {}", k)))
        .collect();

    let gods = god_nodes(&g, 10);
    let surprises = surprising_connections(&g, Some(&communities), 5);

    let detection: HashMap<String, Value> = {
        let mut m = HashMap::new();
        m.insert("total_files".to_string(), serde_json::json!(4));
        m.insert("total_words".to_string(), serde_json::json!(62400));
        m.insert("needs_graph".to_string(), serde_json::json!(true));
        m.insert("warning".to_string(), Value::Null);
        m
    };

    let input_tokens = extraction["input_tokens"].clone();
    let output_tokens = extraction["output_tokens"].clone();
    let tokens: HashMap<String, Value> = {
        let mut m = HashMap::new();
        m.insert("input".to_string(), input_tokens);
        m.insert("output".to_string(), output_tokens);
        m
    };

    (g, communities, cohesion, labels, gods, surprises, detection, tokens)
}

#[test]
fn test_report_contains_header() {
    let (g, communities, cohesion, labels, gods, surprises, detection, tokens) = make_inputs();
    let report = generate(
        &g,
        &communities,
        &cohesion,
        &labels,
        &gods,
        &surprises,
        &detection,
        &tokens,
        "./project",
        None,
        3,
    );
    assert!(
        report.contains("# Graph Report"),
        "Report should contain '# Graph Report'"
    );
}

#[test]
fn test_report_contains_corpus_check() {
    let (g, communities, cohesion, labels, gods, surprises, detection, tokens) = make_inputs();
    let report = generate(
        &g,
        &communities,
        &cohesion,
        &labels,
        &gods,
        &surprises,
        &detection,
        &tokens,
        "./project",
        None,
        3,
    );
    assert!(
        report.contains("## Corpus Check"),
        "Report should contain '## Corpus Check'"
    );
}

#[test]
fn test_report_contains_god_nodes() {
    let (g, communities, cohesion, labels, gods, surprises, detection, tokens) = make_inputs();
    let report = generate(
        &g,
        &communities,
        &cohesion,
        &labels,
        &gods,
        &surprises,
        &detection,
        &tokens,
        "./project",
        None,
        3,
    );
    assert!(
        report.contains("## God Nodes"),
        "Report should contain '## God Nodes'"
    );
}

#[test]
fn test_report_contains_surprising_connections() {
    let (g, communities, cohesion, labels, gods, surprises, detection, tokens) = make_inputs();
    let report = generate(
        &g,
        &communities,
        &cohesion,
        &labels,
        &gods,
        &surprises,
        &detection,
        &tokens,
        "./project",
        None,
        3,
    );
    assert!(
        report.contains("## Surprising Connections"),
        "Report should contain '## Surprising Connections'"
    );
}

#[test]
fn test_report_contains_communities() {
    let (g, communities, cohesion, labels, gods, surprises, detection, tokens) = make_inputs();
    let report = generate(
        &g,
        &communities,
        &cohesion,
        &labels,
        &gods,
        &surprises,
        &detection,
        &tokens,
        "./project",
        None,
        3,
    );
    assert!(
        report.contains("## Communities"),
        "Report should contain '## Communities'"
    );
}

#[test]
fn test_report_contains_ambiguous_section() {
    let (g, communities, cohesion, labels, gods, surprises, detection, tokens) = make_inputs();
    let report = generate(
        &g,
        &communities,
        &cohesion,
        &labels,
        &gods,
        &surprises,
        &detection,
        &tokens,
        "./project",
        None,
        3,
    );
    assert!(
        report.contains("## Ambiguous Edges"),
        "Report should contain '## Ambiguous Edges'"
    );
}

#[test]
fn test_report_shows_token_cost() {
    let (g, communities, cohesion, labels, gods, surprises, detection, tokens) = make_inputs();
    let report = generate(
        &g,
        &communities,
        &cohesion,
        &labels,
        &gods,
        &surprises,
        &detection,
        &tokens,
        "./project",
        None,
        3,
    );
    assert!(
        report.contains("Token cost"),
        "Report should contain 'Token cost'"
    );
    // input_tokens = 1200 → should appear as "1,200"
    assert!(
        report.contains("1,200"),
        "Report should contain '1,200' for input tokens"
    );
}

#[test]
fn test_report_shows_raw_cohesion_scores() {
    let (g, communities, cohesion, labels, gods, surprises, detection, tokens) = make_inputs();
    let report = generate(
        &g,
        &communities,
        &cohesion,
        &labels,
        &gods,
        &surprises,
        &detection,
        &tokens,
        "./project",
        None,
        1, // min_community_size=1 to show all communities
    );
    assert!(
        report.contains("Cohesion:"),
        "Report should contain raw cohesion scores"
    );
    assert!(
        !report.contains('\u{2713}') && !report.contains("✓"),
        "Report should not contain checkmarks"
    );
    assert!(
        !report.contains('\u{26a0}') && !report.contains("⚠"),
        "Report should not contain warning signs"
    );
}
