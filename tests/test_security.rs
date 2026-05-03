use graphify::security::{sanitize_label, validate_graph_path, validate_url};
use std::io;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// validate_url
// ---------------------------------------------------------------------------

// Note: validate_url performs DNS lookups. http/https with public hostnames
// may pass or fail depending on network. We test scheme-rejection which is
// always deterministic (no DNS involved).

#[test]
fn test_validate_url_rejects_file() {
    let err = validate_url("file:///etc/passwd").unwrap_err();
    assert!(
        err.contains("file") || err.contains("scheme"),
        "Error should mention 'file' or 'scheme': {}",
        err
    );
}

#[test]
fn test_validate_url_rejects_ftp() {
    let err = validate_url("ftp://files.example.com/data.zip").unwrap_err();
    assert!(
        err.contains("ftp") || err.contains("scheme"),
        "Error should mention 'ftp' or 'scheme': {}",
        err
    );
}

#[test]
fn test_validate_url_rejects_data() {
    let err = validate_url("data:text/html,<script>alert(1)</script>").unwrap_err();
    assert!(
        err.contains("data") || err.contains("scheme"),
        "Error should mention 'data' or 'scheme': {}",
        err
    );
}

#[test]
fn test_validate_url_rejects_empty_scheme() {
    // A URL with no valid scheme should be rejected.
    let result = validate_url("//no-scheme.example.com");
    // Either parse error or scheme rejection
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// validate_graph_path
// ---------------------------------------------------------------------------

#[test]
fn test_validate_graph_path_allows_inside_base() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("graphify-out");
    std::fs::create_dir_all(&out).unwrap();
    let target = out.join("report.json");
    std::fs::write(&target, b"{}").unwrap();
    let result = validate_graph_path("report.json", Some(tmp.path())).unwrap();
    assert_eq!(result, target.canonicalize().unwrap());
}

#[test]
fn test_validate_graph_path_blocks_traversal() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("graphify-out");
    std::fs::create_dir_all(&out).unwrap();
    // Create the file outside graphify-out so it can be canonicalised.
    let secret = tmp.path().join("secret.txt");
    std::fs::write(&secret, b"secret").unwrap();
    let err = validate_graph_path("../secret.txt", Some(tmp.path())).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
}

#[test]
fn test_validate_graph_path_requires_base_exists() {
    let tmp = TempDir::new().unwrap();
    // graphify-out not created
    let err = validate_graph_path("graph.json", Some(tmp.path())).unwrap_err();
    // File doesn't exist → NotFound
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
}

#[test]
fn test_validate_graph_path_raises_if_file_missing() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("graphify-out")).unwrap();
    let err = validate_graph_path("nonexistent.json", Some(tmp.path())).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
}

// ---------------------------------------------------------------------------
// sanitize_label
// ---------------------------------------------------------------------------

#[test]
fn test_sanitize_label_strips_control_chars() {
    let result = sanitize_label(Some("hello\x00\x1fworld"));
    assert!(!result.contains('\x00'));
    assert!(!result.contains('\x1f'));
    assert!(result.contains("hello"));
    assert!(result.contains("world"));
}

#[test]
fn test_sanitize_label_caps_at_256() {
    let long_label: String = "a".repeat(300);
    let result = sanitize_label(Some(&long_label));
    assert!(result.chars().count() <= 256);
}

#[test]
fn test_sanitize_label_safe_passthrough() {
    assert_eq!(sanitize_label(Some("MyClass")), "MyClass");
    assert_eq!(sanitize_label(Some("extract_python")), "extract_python");
}
