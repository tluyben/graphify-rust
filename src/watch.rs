//! File-system watching and incremental graph rebuilding.
//!
//! Ported from Python `watch.py`. Uses simple polling (std::thread::sleep)
//! rather than a native file-system notification library.

#![allow(dead_code, unused_imports)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use crate::detect::CODE_EXTENSIONS;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The subdirectory name used for graphify outputs.
const OUT_DIR: &str = "graphify-out";

/// Extensions (lowercased, dot-prefixed) that we actively watch.
fn watched_extensions() -> HashSet<&'static str> {
    use crate::detect::{DOC_EXTENSIONS, IMAGE_EXTENSIONS, PAPER_EXTENSIONS};
    let mut set: HashSet<&'static str> = HashSet::new();
    for &e in CODE_EXTENSIONS {
        set.insert(e);
    }
    for &e in DOC_EXTENSIONS {
        set.insert(e);
    }
    for &e in PAPER_EXTENSIONS {
        set.insert(e);
    }
    for &e in IMAGE_EXTENSIONS {
        set.insert(e);
    }
    set
}

fn code_extensions() -> HashSet<&'static str> {
    CODE_EXTENSIONS.iter().copied().collect()
}

/// Derive the human-readable root label for reports.
fn report_root_label(watch_path: &Path) -> String {
    if watch_path.is_absolute() {
        watch_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| watch_path.display().to_string())
    } else if watch_path == Path::new(".") {
        std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_str().unwrap_or("").to_string()))
            .unwrap_or_else(|| ".".to_string())
    } else {
        watch_path.display().to_string()
    }
}

/// Convert source_file paths in extraction payload to be relative to `root`.
fn relativize_source_files(payload: &mut Value, root: &Path) {
    for bucket in &["nodes", "edges", "hyperedges"] {
        if let Some(items) = payload.get_mut(bucket).and_then(|v| v.as_array_mut()) {
            for item in items.iter_mut() {
                if let Some(source) = item
                    .get("source_file")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                {
                    let p = Path::new(&source);
                    if !p.is_absolute() {
                        continue;
                    }
                    if let Ok(rel) = p.canonicalize().unwrap_or_else(|_| p.to_path_buf()).strip_prefix(root) {
                        item["source_file"] = json!(rel.display().to_string());
                    }
                }
            }
        }
    }
}


// ---------------------------------------------------------------------------
// Core rebuild
// ---------------------------------------------------------------------------

/// Re-run AST extraction + build + cluster + report for code files.
///
/// When `force` is `true` the node-count safety check in `to_json` is bypassed.
/// Returns `true` on success, `false` on error.
pub fn rebuild_code(watch_path: &Path, follow_symlinks: bool, force: bool, merge_existing: bool) -> bool {
    let watch_root = match watch_path.canonicalize() {
        Ok(p) => p,
        Err(_) => watch_path.to_path_buf(),
    };
    let project_root = if watch_path.is_absolute() {
        watch_root.clone()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| watch_root.clone())
    };
    let report_root = report_root_label(watch_path);

    // -----------------------------------------------------------------------
    // 1. Detect code files
    // -----------------------------------------------------------------------
    let detected = crate::detect::detect(watch_path, follow_symlinks);
    let code_file_strs: Vec<String> = detected
        .files
        .get("code")
        .cloned()
        .unwrap_or_default();

    if code_file_strs.is_empty() {
        eprintln!("[graphify watch] No code files found - nothing to rebuild.");
        return false;
    }

    let code_files: Vec<PathBuf> = code_file_strs.iter().map(PathBuf::from).collect();
    let code_file_refs: Vec<&Path> = code_files.iter().map(|p| p.as_path()).collect();

    // -----------------------------------------------------------------------
    // 2. Extract AST data
    // -----------------------------------------------------------------------
    let extraction_result =
        match crate::extract::extract(&code_file_refs, Some(&watch_root)) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[graphify watch] Extraction failed: {e}");
                return false;
            }
        };

    let mut result_json = extraction_result.to_json_value();

    // -----------------------------------------------------------------------
    // 3. Merge with existing graph.json (preserve semantic nodes/edges)
    // -----------------------------------------------------------------------
    let out_dir = watch_path.join(OUT_DIR);
    let existing_graph_path = out_dir.join("graph.json");

    if merge_existing && existing_graph_path.exists() {
        if let Ok(text) = std::fs::read_to_string(&existing_graph_path) {
            if let Ok(existing) = serde_json::from_str::<Value>(&text) {
                let new_ast_ids: HashSet<String> = result_json
                    .get("nodes")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|n| n.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                let preserved_nodes: Vec<Value> = existing
                    .get("nodes")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter(|n| {
                                !n.get("id")
                                    .and_then(|v| v.as_str())
                                    .map(|id| new_ast_ids.contains(id))
                                    .unwrap_or(false)
                            })
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();

                let preserved_node_ids: HashSet<String> = preserved_nodes
                    .iter()
                    .filter_map(|n| n.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
                    .collect();

                let all_ids: HashSet<String> =
                    new_ast_ids.iter().chain(preserved_node_ids.iter()).cloned().collect();

                // "links" is the key used in node-link JSON; "edges" is used internally.
                let existing_edges_key = if existing.get("links").is_some() {
                    "links"
                } else {
                    "edges"
                };
                let preserved_edges: Vec<Value> = existing
                    .get(existing_edges_key)
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter(|e| {
                                let src = e
                                    .get("source")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                                    .unwrap_or_default();
                                let tgt = e
                                    .get("target")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                                    .unwrap_or_default();
                                all_ids.contains(&src) && all_ids.contains(&tgt)
                            })
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();

                let existing_hyperedges: Vec<Value> = existing
                    .get("hyperedges")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                // Merge: new AST nodes + preserved semantic nodes.
                let mut merged_nodes: Vec<Value> = result_json
                    .get("nodes")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                merged_nodes.extend(preserved_nodes);

                let mut merged_edges: Vec<Value> = result_json
                    .get("edges")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                merged_edges.extend(preserved_edges);

                result_json = json!({
                    "nodes": merged_nodes,
                    "edges": merged_edges,
                    "hyperedges": existing_hyperedges,
                    "input_tokens": 0,
                    "output_tokens": 0,
                });
            }
        }
    }

    // -----------------------------------------------------------------------
    // 4. Relativize source paths
    // -----------------------------------------------------------------------
    relativize_source_files(&mut result_json, &project_root);

    // -----------------------------------------------------------------------
    // 5. Build graph
    // -----------------------------------------------------------------------
    let g = crate::build::build_from_json(result_json, false);

    // -----------------------------------------------------------------------
    // 6. Cluster + analyze
    // -----------------------------------------------------------------------
    let communities: HashMap<i64, Vec<String>> = crate::cluster::cluster(&g);
    let cohesion: HashMap<i64, f64> = crate::cluster::score_all(&g, &communities);

    let gods = crate::analyze::god_nodes(&g, 10);
    let surprises = crate::analyze::surprising_connections(&g, Some(&communities), 5);

    // Build community labels.
    let community_labels: HashMap<i64, String> = communities
        .keys()
        .map(|&cid| (cid, format!("Community {cid}")))
        .collect();

    // -----------------------------------------------------------------------
    // 7. Write outputs
    // -----------------------------------------------------------------------
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("[graphify watch] Could not create output directory: {e}");
        return false;
    }

    // Write the graphify root marker.
    let _ = std::fs::write(
        out_dir.join(".graphify_root"),
        watch_root.display().to_string(),
    );

    // Build a detection_result map for the report.
    let detection_result: HashMap<String, Value> = {
        let mut m = HashMap::new();
        let files_json: HashMap<String, Value> = {
            let mut f = HashMap::new();
            f.insert("code".to_string(), json!(code_file_strs));
            f.insert("document".to_string(), json!([]));
            f.insert("paper".to_string(), json!([]));
            f.insert("image".to_string(), json!([]));
            f
        };
        m.insert("files".to_string(), json!(files_json));
        m.insert("total_files".to_string(), json!(code_file_strs.len()));
        m.insert(
            "total_words".to_string(),
            json!(detected.total_words),
        );
        m
    };

    let token_cost: HashMap<String, Value> = {
        let mut m = HashMap::new();
        m.insert("input".to_string(), json!(0));
        m.insert("output".to_string(), json!(0));
        m
    };

    // graph.json
    match crate::export::to_json(
        &g,
        &communities,
        &out_dir.join("graph.json").to_string_lossy(),
        force,
    ) {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("[graphify watch] to_json returned false (node-count check failed).");
            return false;
        }
        Err(e) => {
            eprintln!("[graphify watch] to_json failed: {e}");
            return false;
        }
    }

    // GRAPH_REPORT.md
    let report = crate::report::generate(
        &g,
        &communities,
        &cohesion,
        &community_labels,
        &gods,
        &surprises,
        &detection_result,
        &token_cost,
        &report_root,
        None, // suggested_questions
        1,
    );
    if let Err(e) = std::fs::write(out_dir.join("GRAPH_REPORT.md"), report.as_bytes()) {
        eprintln!("[graphify watch] Could not write GRAPH_REPORT.md: {e}");
    }

    // graph.html (optional — skip if graph is too large)
    let html_written = match crate::export::to_html(
        &g,
        &communities,
        &out_dir.join("graph.html").to_string_lossy(),
        Some(&community_labels),
        None,
    ) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("[graphify watch] Skipped graph.html: {e}");
            let stale = out_dir.join("graph.html");
            if stale.exists() {
                let _ = std::fs::remove_file(&stale);
            }
            false
        }
    };

    // Clear stale needs_update flag.
    let flag = out_dir.join("needs_update");
    if flag.exists() {
        let _ = std::fs::remove_file(&flag);
    }

    let products = if html_written {
        "graph.json, graph.html and GRAPH_REPORT.md"
    } else {
        "graph.json and GRAPH_REPORT.md"
    };

    eprintln!(
        "[graphify watch] Rebuilt: {} nodes, {} edges, {} communities",
        g.number_of_nodes(),
        g.number_of_edges(),
        communities.len(),
    );
    eprintln!("[graphify watch] {} updated in {}", products, out_dir.display());

    true
}

// ---------------------------------------------------------------------------
// check_update
// ---------------------------------------------------------------------------

/// Check for a pending semantic-update flag and notify the user if set.
///
/// Always returns `true` (cron-safe).
pub fn check_update(watch_path: &Path) -> bool {
    let flag = watch_path.join(OUT_DIR).join("needs_update");
    if flag.exists() {
        eprintln!(
            "[graphify check-update] Pending non-code changes in {}.",
            watch_path.display()
        );
        eprintln!(
            "[graphify check-update] Run `/graphify --update` to apply semantic re-extraction."
        );
    }
    true
}

// ---------------------------------------------------------------------------
// Internal watch helpers
// ---------------------------------------------------------------------------

/// Write the needs_update flag and print a notification.
fn notify_only(watch_path: &Path) {
    let flag_dir = watch_path.join(OUT_DIR);
    let _ = std::fs::create_dir_all(&flag_dir);
    let flag = flag_dir.join("needs_update");
    let _ = std::fs::write(&flag, b"1");
    eprintln!("\n[graphify watch] New or changed files detected in {}", watch_path.display());
    eprintln!("[graphify watch] Non-code files changed - semantic re-extraction requires LLM.");
    eprintln!("[graphify watch] Run `/graphify --update` in Claude Code to update the graph.");
    eprintln!("[graphify watch] Flag written to {}", flag.display());
}

/// Return `true` if any of the given paths has a non-code extension.
fn has_non_code(changed_paths: &[PathBuf]) -> bool {
    let code_exts = code_extensions();
    changed_paths.iter().any(|p| {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))
            .unwrap_or_default();
        !code_exts.contains(ext.as_str())
    })
}

/// Collect modification times of all watched files under `root`.
fn collect_mtimes(root: &Path) -> HashMap<PathBuf, SystemTime> {
    let watched = watched_extensions();
    let out_dir_name = OUT_DIR;

    let mut map = HashMap::new();

    let walker = walkdir::WalkDir::new(root).follow_links(false);
    for entry in walker.into_iter().filter_map(|e| e.ok()) {
        let path = entry.path().to_path_buf();

        // Skip hidden components.
        if path.components().any(|c| {
            c.as_os_str()
                .to_str()
                .map(|s| s.starts_with('.'))
                .unwrap_or(false)
        }) {
            continue;
        }

        // Skip graphify-out directory.
        if path.components().any(|c| c.as_os_str() == out_dir_name) {
            continue;
        }

        if !entry.file_type().is_file() {
            continue;
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))
            .unwrap_or_default();

        if !watched.contains(ext.as_str()) {
            continue;
        }

        if let Ok(meta) = std::fs::metadata(&path) {
            if let Ok(mtime) = meta.modified() {
                map.insert(path, mtime);
            }
        }
    }

    map
}

// ---------------------------------------------------------------------------
// Public watch loop (polling)
// ---------------------------------------------------------------------------

/// Watch `watch_path` for new or modified files and auto-update the graph.
///
/// * Code-only changes trigger an immediate AST rebuild (no LLM required).
/// * Non-code file changes write a `needs_update` flag and notify the user.
///
/// `debounce_secs` is the time to wait after the last change before triggering
/// the rebuild (avoids spurious rebuilds during rapid multi-file saves).
pub fn watch(watch_path: &Path, debounce_secs: f64) -> std::io::Result<()> {
    eprintln!(
        "[graphify watch] Watching {} - press Ctrl+C to stop",
        watch_path.canonicalize().unwrap_or_else(|_| watch_path.to_path_buf()).display()
    );
    eprintln!(
        "[graphify watch] Code changes rebuild graph automatically. \
         Doc/image changes require /graphify --update."
    );
    eprintln!("[graphify watch] Debounce: {debounce_secs}s (polling)");

    let debounce = Duration::from_secs_f64(debounce_secs.max(0.1));
    let poll_interval = Duration::from_millis(500);

    // Baseline snapshot.
    let mut baseline = collect_mtimes(watch_path);
    let mut last_change: Option<Instant> = None;
    let mut changed: HashSet<PathBuf> = HashSet::new();

    loop {
        std::thread::sleep(poll_interval);

        // Check if we should fire the rebuild.
        if let Some(t) = last_change {
            if t.elapsed() >= debounce && !changed.is_empty() {
                let batch: Vec<PathBuf> = changed.drain().collect();
                last_change = None;
                eprintln!("\n[graphify watch] {} file(s) changed", batch.len());
                if has_non_code(&batch) {
                    notify_only(watch_path);
                } else {
                    rebuild_code(watch_path, false, false, true);
                }
                // Refresh baseline after rebuild.
                baseline = collect_mtimes(watch_path);
                continue;
            }
        }

        // Diff current state against baseline.
        let current = collect_mtimes(watch_path);
        for (path, mtime) in &current {
            let is_new_or_changed = baseline
                .get(path)
                .map(|&old_mtime| mtime > &old_mtime)
                .unwrap_or(true);

            if is_new_or_changed {
                changed.insert(path.clone());
                last_change = Some(Instant::now());
            }
        }
        // Also catch deleted files — no need to act on deletions specially,
        // the rebuild will simply not see them.
        baseline = current;
    }
}

// ---------------------------------------------------------------------------
// Extension helpers
// ---------------------------------------------------------------------------

impl crate::extract::ExtractionResult {
    /// Serialise to a `serde_json::Value` (mirrors `to_json` on the Python side).
    pub fn to_json_value(&self) -> Value {
        json!({
            "nodes": self.nodes,
            "edges": self.edges,
            "hyperedges": self.hyperedges,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_report_root_label_dot() {
        let label = report_root_label(Path::new("."));
        // Should be the name of the current directory, not empty.
        assert!(!label.is_empty());
    }

    #[test]
    fn test_report_root_label_absolute() {
        let label = report_root_label(Path::new("/home/user/myproject"));
        assert_eq!(label, "myproject");
    }

    #[test]
    fn test_report_root_label_relative() {
        let label = report_root_label(Path::new("foo/bar"));
        assert_eq!(label, "foo/bar");
    }

    #[test]
    fn test_check_update_no_flag() {
        let dir = TempDir::new().unwrap();
        // Should return true even when no flag file exists.
        assert!(check_update(dir.path()));
    }

    #[test]
    fn test_check_update_with_flag() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join(OUT_DIR);
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("needs_update"), b"1").unwrap();
        assert!(check_update(dir.path()));
    }

    #[test]
    fn test_has_non_code_all_code() {
        let paths = vec![
            PathBuf::from("main.rs"),
            PathBuf::from("lib.rs"),
        ];
        assert!(!has_non_code(&paths));
    }

    #[test]
    fn test_has_non_code_with_doc() {
        let paths = vec![
            PathBuf::from("main.rs"),
            PathBuf::from("README.md"),
        ];
        assert!(has_non_code(&paths));
    }

    #[test]
    fn test_collect_mtimes_empty_dir() {
        let dir = TempDir::new().unwrap();
        let mtimes = collect_mtimes(dir.path());
        // No watched files in an empty dir.
        assert!(mtimes.is_empty());
    }

    #[test]
    fn test_collect_mtimes_with_rs_file() {
        // Use a named subdirectory under the system temp dir so that the path
        // does not contain hidden components (TempDir often creates paths like
        // /tmp/.tmpXXXX which get filtered out by the leading-dot check).
        let tmp = std::env::temp_dir().join("graphify_test_collect_mtimes");
        std::fs::create_dir_all(&tmp).unwrap();
        let rs_file = tmp.join("test.rs");
        std::fs::write(&rs_file, b"fn main() {}").unwrap();
        let mtimes = collect_mtimes(&tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(mtimes.contains_key(&rs_file));
    }

    #[test]
    fn test_collect_mtimes_skips_graphify_out() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join(OUT_DIR);
        std::fs::create_dir_all(&out).unwrap();
        let rs_in_out = out.join("generated.rs");
        std::fs::write(&rs_in_out, b"fn foo() {}").unwrap();
        let mtimes = collect_mtimes(dir.path());
        // The file inside graphify-out should not be watched.
        assert!(!mtimes.contains_key(&rs_in_out));
    }

    #[test]
    fn test_relativize_source_files() {
        let dir = TempDir::new().unwrap();
        let abs_path = dir.path().join("src").join("main.rs");
        std::fs::create_dir_all(abs_path.parent().unwrap()).unwrap();
        std::fs::write(&abs_path, b"").unwrap();

        let mut payload = json!({
            "nodes": [{"id": "foo", "source_file": abs_path.to_str().unwrap()}],
            "edges": [],
            "hyperedges": [],
        });

        relativize_source_files(&mut payload, dir.path());

        let source = payload["nodes"][0]["source_file"]
            .as_str()
            .unwrap_or("");
        // Should now be relative.
        assert!(!Path::new(source).is_absolute(), "Expected relative path, got: {source}");
    }

    #[test]
    fn test_communities_are_i64() {
        // cluster() returns HashMap<i64, Vec<String>>.
        // Ensure we can store it in the right type.
        let map: HashMap<i64, Vec<String>> = HashMap::new();
        assert!(map.is_empty());
    }
}
