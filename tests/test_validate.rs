use graphify::validate::{assert_valid, validate_extraction};
use serde_json::json;

fn valid() -> serde_json::Value {
    json!({
        "nodes": [
            {"id": "n1", "label": "Foo", "file_type": "code", "source_file": "foo.py"},
            {"id": "n2", "label": "Bar", "file_type": "document", "source_file": "bar.md"}
        ],
        "edges": [
            {"source": "n1", "target": "n2", "relation": "references",
             "confidence": "EXTRACTED", "source_file": "foo.py", "weight": 1.0}
        ]
    })
}

#[test]
fn test_valid_passes() {
    assert_eq!(validate_extraction(&valid()), Vec::<String>::new());
}

#[test]
fn test_missing_nodes_key() {
    let errors = validate_extraction(&json!({"edges": []}));
    assert!(errors.iter().any(|e| e.contains("nodes")));
}

#[test]
fn test_missing_edges_key() {
    let errors = validate_extraction(&json!({"nodes": []}));
    assert!(errors.iter().any(|e| e.contains("edges")));
}

#[test]
fn test_not_a_dict() {
    let errors = validate_extraction(&json!([]));
    assert_eq!(errors.len(), 1);
}

#[test]
fn test_invalid_file_type() {
    let data = json!({
        "nodes": [{"id": "n1", "label": "X", "file_type": "video", "source_file": "x.mp4"}],
        "edges": []
    });
    let errors = validate_extraction(&data);
    assert!(errors.iter().any(|e| e.contains("file_type")));
}

#[test]
fn test_invalid_confidence() {
    let data = json!({
        "nodes": [
            {"id": "n1", "label": "A", "file_type": "code", "source_file": "a.py"},
            {"id": "n2", "label": "B", "file_type": "code", "source_file": "b.py"}
        ],
        "edges": [
            {"source": "n1", "target": "n2", "relation": "calls",
             "confidence": "CERTAIN", "source_file": "a.py"}
        ]
    });
    let errors = validate_extraction(&data);
    assert!(errors.iter().any(|e| e.contains("confidence")));
}

#[test]
fn test_dangling_edge_source() {
    let data = json!({
        "nodes": [{"id": "n1", "label": "A", "file_type": "code", "source_file": "a.py"}],
        "edges": [
            {"source": "missing_id", "target": "n1", "relation": "calls",
             "confidence": "EXTRACTED", "source_file": "a.py"}
        ]
    });
    let errors = validate_extraction(&data);
    assert!(errors.iter().any(|e| e.contains("source") && e.contains("missing_id")));
}

#[test]
fn test_dangling_edge_target() {
    let data = json!({
        "nodes": [{"id": "n1", "label": "A", "file_type": "code", "source_file": "a.py"}],
        "edges": [
            {"source": "n1", "target": "ghost", "relation": "calls",
             "confidence": "EXTRACTED", "source_file": "a.py"}
        ]
    });
    let errors = validate_extraction(&data);
    assert!(errors.iter().any(|e| e.contains("target") && e.contains("ghost")));
}

#[test]
fn test_missing_node_field() {
    let data = json!({
        "nodes": [{"id": "n1", "label": "A", "source_file": "a.py"}],
        "edges": []
    });
    let errors = validate_extraction(&data);
    assert!(errors.iter().any(|e| e.contains("file_type")));
}

#[test]
fn test_assert_valid_raises_on_errors() {
    let bad = json!({"nodes": "bad", "edges": []});
    let result = assert_valid(&bad);
    assert!(result.is_err());
    let msg = result.unwrap_err();
    assert!(msg.contains("error") || msg.contains("Error"));
}

#[test]
fn test_assert_valid_passes_silently() {
    assert!(assert_valid(&valid()).is_ok());
}
