//! AST-based code extraction using tree-sitter.
//!
//! Extracts nodes and edges from source code files by parsing them with
//! tree-sitter grammars. Mirrors the Python `extract.py` module exactly.

use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Value};

use crate::cache::{load_cached, save_cached};

// ---------------------------------------------------------------------------
// Output type
// ---------------------------------------------------------------------------

/// Result of extracting a single file (or merged set of files).
#[derive(Debug, Clone, Default)]
pub struct ExtractionResult {
    pub nodes: Vec<Value>,
    pub edges: Vec<Value>,
    pub hyperedges: Vec<Value>,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

impl ExtractionResult {
    fn to_json(&self) -> Value {
        json!({
            "nodes": self.nodes,
            "edges": self.edges,
            "hyperedges": self.hyperedges,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
        })
    }

    fn from_json(v: &Value) -> Option<Self> {
        let obj = v.as_object()?;
        let nodes = obj.get("nodes")?.as_array()?.clone();
        let edges = obj.get("edges")?.as_array()?.clone();
        let hyperedges = obj
            .get("hyperedges")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let input_tokens = obj
            .get("input_tokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let output_tokens = obj
            .get("output_tokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        Some(ExtractionResult {
            nodes,
            edges,
            hyperedges,
            input_tokens,
            output_tokens,
        })
    }
}

// ---------------------------------------------------------------------------
// ID helpers
// ---------------------------------------------------------------------------

/// Generate a graph node ID from parts. Mirrors Python's `_make_id`.
pub fn make_id(parts: &[&str]) -> String {
    let combined: String = parts
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| p.trim_matches(|c| c == '_' || c == '.'))
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    let re = Regex::new(r"[^a-zA-Z0-9]+").unwrap();
    let cleaned = re.replace_all(&combined, "_");
    cleaned.trim_matches('_').to_lowercase()
}

/// Derive a short stem for a file path. Mirrors Python's `_file_stem`.
fn file_stem(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let parent = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if !parent.is_empty() && parent != "." {
        format!("{}.{}", parent, stem)
    } else {
        stem
    }
}

// ---------------------------------------------------------------------------
// Node / edge builders
// ---------------------------------------------------------------------------

fn make_node(id: &str, label: &str, source_file: &str, line: usize) -> Value {
    json!({
        "id": id,
        "label": label,
        "file_type": "code",
        "source_file": source_file,
        "source_location": format!("L{}", line),
    })
}

fn make_edge(
    source: &str,
    target: &str,
    relation: &str,
    source_file: &str,
    line: usize,
    confidence: &str,
    context: Option<&str>,
) -> Value {
    let mut e = json!({
        "source": source,
        "target": target,
        "relation": relation,
        "confidence": confidence,
        "source_file": source_file,
        "source_location": format!("L{}", line),
        "weight": 1.0,
    });
    if let Some(ctx) = context {
        e["context"] = json!(ctx);
    }
    e
}

// ---------------------------------------------------------------------------
// Tree-sitter helpers
// ---------------------------------------------------------------------------

/// Extract the UTF-8 text of a node from the source bytes.
fn node_text<'a>(node: &tree_sitter::Node, src: &'a [u8]) -> &'a str {
    std::str::from_utf8(&src[node.start_byte()..node.end_byte()]).unwrap_or("")
}

/// 1-based line number for a node.
fn node_line(node: &tree_sitter::Node) -> usize {
    node.start_position().row + 1
}

/// Collect direct children of `node` into a Vec (avoids cursor lifetime issues
/// when using children inside closures or complex expressions).
fn children_vec(node: tree_sitter::Node) -> Vec<tree_sitter::Node> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

/// Recursively collect all descendant nodes matching `kind`.
fn collect_nodes_of_kind<'tree>(
    node: tree_sitter::Node<'tree>,
    kind: &str,
    out: &mut Vec<tree_sitter::Node<'tree>>,
) {
    if node.kind() == kind {
        out.push(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_nodes_of_kind(child, kind, out);
    }
}

/// Collect all descendants matching any of the given kinds.
fn collect_nodes_of_kinds<'tree>(
    node: tree_sitter::Node<'tree>,
    kinds: &[&str],
    out: &mut Vec<tree_sitter::Node<'tree>>,
) {
    if kinds.contains(&node.kind()) {
        out.push(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_nodes_of_kinds(child, kinds, out);
    }
}

// ---------------------------------------------------------------------------
// Supported file extensions
// ---------------------------------------------------------------------------

const SUPPORTED_EXTENSIONS: &[&str] = &[
    ".py", ".ts", ".js", ".jsx", ".tsx", ".mjs", ".go", ".rs", ".java", ".c", ".cpp", ".cc",
    ".cxx", ".h", ".hpp", ".rb", ".cs", ".kt", ".kts", ".scala", ".php", ".swift", ".lua",
    ".toc", ".zig", ".ps1", ".ex", ".exs", ".m", ".mm", ".sql",
];

// ---------------------------------------------------------------------------
// Public: collect_files
// ---------------------------------------------------------------------------

/// Collect all supported source files under `dir`. Mirrors Python's
/// `collect_files`.
pub fn collect_files(dir: &Path, follow_symlinks: bool) -> Vec<PathBuf> {
    let mut result = Vec::new();
    collect_files_inner(dir, follow_symlinks, &mut result);
    result
}

fn collect_files_inner(dir: &Path, follow_symlinks: bool, out: &mut Vec<PathBuf>) {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in read.flatten() {
        let path = entry.path();
        // Skip hidden directories/files: check only the entry name (last component).
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if name.starts_with('.') {
            continue;
        }
        let meta = if follow_symlinks {
            std::fs::metadata(&path)
        } else {
            std::fs::symlink_metadata(&path)
        };
        let meta = match meta {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() {
            collect_files_inner(&path, follow_symlinks, out);
        } else if meta.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!(".{}", e.to_lowercase()))
                .unwrap_or_default();
            if SUPPORTED_EXTENSIONS.contains(&ext.as_str()) {
                out.push(path);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public: extract_file dispatcher
// ---------------------------------------------------------------------------

/// Extract nodes/edges from a single file, dispatching by extension.
pub fn extract_file(path: &Path) -> Result<ExtractionResult, Box<dyn Error>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "py" => extract_python(path),
        "ts" | "tsx" => extract_typescript(path),
        "js" | "jsx" | "mjs" => extract_javascript(path),
        "go" => extract_go(path),
        "rs" => extract_rust_file(path),
        "java" => extract_java(path),
        "c" | "h" => extract_c(path),
        "cpp" | "cc" | "cxx" | "hpp" => extract_cpp(path),
        "rb" => extract_ruby(path),
        "cs" => extract_csharp(path),
        _ => Ok(ExtractionResult::default()),
    }
}

// ---------------------------------------------------------------------------
// Public: extract (multi-file, with cache)
// ---------------------------------------------------------------------------

/// Extract from multiple files, merging results. Uses cache when `cache_root`
/// is provided. Performs a cross-file call-resolution second pass.
pub fn extract(files: &[&Path], cache_root: Option<&Path>) -> Result<ExtractionResult, Box<dyn Error>> {
    // Per-file results (including intra-file call stubs).
    let mut per_file: Vec<ExtractionResult> = Vec::new();

    for &path in files {
        let result = if let Some(root) = cache_root {
            if let Some(cached) = load_cached(path, root, "ast") {
                ExtractionResult::from_json(&cached).unwrap_or_default()
            } else {
                let r = extract_file(path)?;
                let _ = save_cached(path, &r.to_json(), root, "ast");
                r
            }
        } else {
            extract_file(path)?
        };
        per_file.push(result);
    }

    // Merge: deduplicate nodes by id (last wins), collect all edges.
    let mut node_map: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
    let mut all_edges: Vec<Value> = Vec::new();
    let mut all_hyperedges: Vec<Value> = Vec::new();

    for r in &per_file {
        for n in &r.nodes {
            if let Some(id) = n.get("id").and_then(|v| v.as_str()) {
                node_map.insert(id.to_string(), n.clone());
            }
        }
        all_edges.extend(r.edges.iter().cloned());
        all_hyperedges.extend(r.hyperedges.iter().cloned());
    }

    // Build global label -> [node_id] map for cross-file call resolution.
    let mut label_to_ids: HashMap<String, Vec<String>> = HashMap::new();
    for (id, node) in &node_map {
        if let Some(label) = node.get("label").and_then(|v| v.as_str()) {
            // Normalise: strip trailing "()" that we append to function labels.
            let key = label.trim_end_matches("()").to_string();
            label_to_ids.entry(key).or_default().push(id.clone());
        }
    }

    // Cross-file calls: look at edges tagged with context="call_stub" and
    // resolve the target. Within-file calls are already EXTRACTED; cross-file
    // become INFERRED.
    let mut cross_edges: Vec<Value> = Vec::new();
    // Re-collect stubs that were emitted with context="call_stub" sentinel.
    let mut retained_edges: Vec<Value> = Vec::new();
    for edge in all_edges {
        if edge.get("context").and_then(|v| v.as_str()) == Some("call_stub") {
            // This is a stub: try to resolve cross-file.
            let callee_label = match edge.get("target").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let caller_id = match edge.get("source").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let source_file = edge
                .get("source_file")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let source_location = edge
                .get("source_location")
                .and_then(|v| v.as_str())
                .unwrap_or("L1")
                .to_string();

            if let Some(ids) = label_to_ids.get(&callee_label) {
                // Filter to ids that are NOT in the same file as the caller.
                let caller_file = node_map
                    .get(&caller_id)
                    .and_then(|n| n.get("source_file"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let cross: Vec<&String> = ids
                    .iter()
                    .filter(|id| {
                        node_map
                            .get(*id)
                            .and_then(|n| n.get("source_file"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            != caller_file
                    })
                    .collect();
                if cross.len() == 1 {
                    // Unique cross-file match → INFERRED.
                    cross_edges.push(json!({
                        "source": caller_id,
                        "target": cross[0],
                        "relation": "calls",
                        "confidence": "INFERRED",
                        "source_file": source_file,
                        "source_location": source_location,
                        "weight": 1.0,
                        "context": "call",
                    }));
                }
                // If same-file ids exist uniquely those were already emitted as EXTRACTED.
                let same: Vec<&String> = ids
                    .iter()
                    .filter(|id| {
                        node_map
                            .get(*id)
                            .and_then(|n| n.get("source_file"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            == caller_file
                    })
                    .collect();
                if same.len() == 1 {
                    // Re-emit the within-file edge as EXTRACTED (it was a stub).
                    retained_edges.push(json!({
                        "source": caller_id,
                        "target": same[0],
                        "relation": "calls",
                        "confidence": "EXTRACTED",
                        "source_file": source_file,
                        "source_location": source_location,
                        "weight": 1.0,
                        "context": "call",
                    }));
                }
            }
            // Drop unresolved stubs.
        } else {
            retained_edges.push(edge);
        }
    }
    retained_edges.extend(cross_edges);

    Ok(ExtractionResult {
        nodes: node_map.into_values().collect(),
        edges: retained_edges,
        hyperedges: all_hyperedges,
        input_tokens: 0,
        output_tokens: 0,
    })
}

// ---------------------------------------------------------------------------
// Python extractor
// ---------------------------------------------------------------------------

pub fn extract_python(path: &Path) -> Result<ExtractionResult, Box<dyn Error>> {
    let src = std::fs::read(path)?;
    let source_file = path.to_string_lossy().into_owned();
    let stem = file_stem(path);
    let file_id = make_id(&[&source_file]);
    let file_label = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let lang: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang)?;
    let tree = parser.parse(&src, None).ok_or("parse failed")?;
    let root = tree.root_node();

    let mut nodes: Vec<Value> = Vec::new();
    let mut edges: Vec<Value> = Vec::new();

    // File node.
    nodes.push(make_node(&file_id, &file_label, &source_file, 1));

    // Map: node_id -> label (for call resolution).
    // We collect (caller_id, call_label, line) for second pass.
    let mut fn_bodies: Vec<(String, tree_sitter::Node)> = Vec::new();
    // label -> vec<id> within this file.
    let mut local_label_to_ids: HashMap<String, Vec<String>> = HashMap::new();

    // Walk top-level children.
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        extract_python_top_level(
            child,
            &src,
            &source_file,
            &stem,
            &file_id,
            None,
            &mut nodes,
            &mut edges,
            &mut fn_bodies,
            &mut local_label_to_ids,
        );
    }

    // Second pass: call graph.
    for (caller_id, body_node) in &fn_bodies {
        let mut calls: Vec<tree_sitter::Node> = Vec::new();
        collect_nodes_of_kind(*body_node, "call", &mut calls);
        for call in calls {
            // The function being called is the first child of call.
            let ch = children_vec(call);
            if let Some(func_node) = ch.into_iter().next() {
                let callee_label = extract_python_call_name(func_node, &src);
                if callee_label.is_empty() {
                    continue;
                }
                let line = node_line(&call);
                // Emit as stub; extract() will resolve.
                edges.push(json!({
                    "source": caller_id,
                    "target": callee_label,
                    "relation": "calls",
                    "confidence": "EXTRACTED",
                    "source_file": source_file,
                    "source_location": format!("L{}", line),
                    "weight": 1.0,
                    "context": "call_stub",
                }));
            }
        }
    }

    Ok(ExtractionResult {
        nodes,
        edges,
        ..Default::default()
    })
}

fn extract_python_call_name(node: tree_sitter::Node, src: &[u8]) -> String {
    match node.kind() {
        "identifier" => node_text(&node, src).to_string(),
        "attribute" => {
            // attribute = object "." attribute_name
            // We want just the method name (last segment).
            let mut c = node.walk();
            let children: Vec<_> = node.children(&mut c).collect();
            // Last child that is identifier or attribute.
            for ch in children.iter().rev() {
                if ch.kind() == "identifier" {
                    return node_text(ch, src).to_string();
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_python_top_level<'a>(
    node: tree_sitter::Node<'a>,
    src: &[u8],
    source_file: &str,
    stem: &str,
    parent_id: &str,
    class_id: Option<&str>,
    nodes: &mut Vec<Value>,
    edges: &mut Vec<Value>,
    fn_bodies: &mut Vec<(String, tree_sitter::Node<'a>)>,
    local_label_to_ids: &mut HashMap<String, Vec<String>>,
) {
    match node.kind() {
        "import_statement" => {
            // import foo, bar
            let mut c = node.walk();
            for child in node.children(&mut c) {
                if child.kind() == "dotted_name" || child.kind() == "aliased_import" {
                    let name = if child.kind() == "aliased_import" {
                        // aliased_import: dotted_name "as" identifier
                        children_vec(child)
                            .into_iter()
                            .find(|n| n.kind() == "dotted_name")
                            .map(|n| node_text(&n, src).to_string())
                            .unwrap_or_default()
                    } else {
                        node_text(&child, src).to_string()
                    };
                    if name.is_empty() {
                        continue;
                    }
                    let target_id = make_id(&[&name]);
                    edges.push(make_edge(
                        parent_id,
                        &target_id,
                        "imports",
                        source_file,
                        node_line(&node),
                        "EXTRACTED",
                        None,
                    ));
                }
            }
        }
        "import_from_statement" => {
            // from foo import bar, baz
            let mut c = node.walk();
            let children: Vec<_> = node.children(&mut c).collect();
            // Module is the first dotted_name after "from".
            let module = children
                .iter()
                .find(|n| n.kind() == "dotted_name" || n.kind() == "relative_import")
                .map(|n| node_text(n, src).to_string())
                .unwrap_or_default();
            if !module.is_empty() {
                let target_id = make_id(&[&module]);
                edges.push(make_edge(
                    parent_id,
                    &target_id,
                    "imports_from",
                    source_file,
                    node_line(&node),
                    "EXTRACTED",
                    None,
                ));
            }
        }
        "class_definition" => {
            extract_python_class(
                node,
                src,
                source_file,
                stem,
                parent_id,
                nodes,
                edges,
                fn_bodies,
                local_label_to_ids,
            );
        }
        "function_definition" => {
            extract_python_function(
                node,
                src,
                source_file,
                stem,
                parent_id,
                class_id,
                nodes,
                edges,
                fn_bodies,
                local_label_to_ids,
            );
        }
        "decorated_definition" => {
            // Look for the inner class or function definition.
            let mut c = node.walk();
            for child in node.children(&mut c) {
                if child.kind() == "class_definition" || child.kind() == "function_definition" {
                    extract_python_top_level(
                        child,
                        src,
                        source_file,
                        stem,
                        parent_id,
                        class_id,
                        nodes,
                        edges,
                        fn_bodies,
                        local_label_to_ids,
                    );
                }
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_python_class<'a>(
    node: tree_sitter::Node<'a>,
    src: &[u8],
    source_file: &str,
    stem: &str,
    parent_id: &str,
    nodes: &mut Vec<Value>,
    edges: &mut Vec<Value>,
    fn_bodies: &mut Vec<(String, tree_sitter::Node<'a>)>,
    local_label_to_ids: &mut HashMap<String, Vec<String>>,
) {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(&n, src).to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return;
    }
    let class_id = make_id(&[stem, &name]);
    nodes.push(make_node(&class_id, &name, source_file, node_line(&node)));
    local_label_to_ids
        .entry(name.clone())
        .or_default()
        .push(class_id.clone());

    // contains edge: file -> class
    edges.push(make_edge(
        parent_id,
        &class_id,
        "contains",
        source_file,
        node_line(&node),
        "EXTRACTED",
        None,
    ));

    // Inheritance: argument_list holds base classes.
    if let Some(args) = node.child_by_field_name("superclasses") {
        let mut c = args.walk();
        for base in args.children(&mut c) {
            if base.kind() == "identifier" || base.kind() == "dotted_name" {
                let base_name = node_text(&base, src).to_string();
                let base_id = make_id(&[&base_name]);
                edges.push(make_edge(
                    &class_id,
                    &base_id,
                    "inherits",
                    source_file,
                    node_line(&base),
                    "EXTRACTED",
                    None,
                ));
            }
        }
    }

    // Walk class body for methods.
    if let Some(body) = node.child_by_field_name("body") {
        let mut c = body.walk();
        for child in body.children(&mut c) {
            match child.kind() {
                "function_definition" => {
                    extract_python_function(
                        child,
                        src,
                        source_file,
                        stem,
                        &class_id,
                        Some(&class_id),
                        nodes,
                        edges,
                        fn_bodies,
                        local_label_to_ids,
                    );
                }
                "decorated_definition" => {
                    let mut cc = child.walk();
                    for inner in child.children(&mut cc) {
                        if inner.kind() == "function_definition" {
                            extract_python_function(
                                inner,
                                src,
                                source_file,
                                stem,
                                &class_id,
                                Some(&class_id),
                                nodes,
                                edges,
                                fn_bodies,
                                local_label_to_ids,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_python_function<'a>(
    node: tree_sitter::Node<'a>,
    src: &[u8],
    source_file: &str,
    stem: &str,
    parent_id: &str,
    class_id: Option<&str>,
    nodes: &mut Vec<Value>,
    edges: &mut Vec<Value>,
    fn_bodies: &mut Vec<(String, tree_sitter::Node<'a>)>,
    local_label_to_ids: &mut HashMap<String, Vec<String>>,
) {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(&n, src).to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return;
    }
    let fn_id = make_id(&[stem, &name]);
    let label = format!("{}()", name);
    nodes.push(make_node(&fn_id, &label, source_file, node_line(&node)));
    local_label_to_ids
        .entry(name.clone())
        .or_default()
        .push(fn_id.clone());

    let relation = if class_id.is_some() { "method" } else { "contains" };
    edges.push(make_edge(
        parent_id,
        &fn_id,
        relation,
        source_file,
        node_line(&node),
        "EXTRACTED",
        None,
    ));

    // Collect body for call analysis.
    if let Some(body) = node.child_by_field_name("body") {
        fn_bodies.push((fn_id, body));
    }
}

// ---------------------------------------------------------------------------
// TypeScript extractor
// ---------------------------------------------------------------------------

pub fn extract_typescript(path: &Path) -> Result<ExtractionResult, Box<dyn Error>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let lang: tree_sitter::Language = if ext == "tsx" {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    } else {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    };
    extract_js_like(path, lang)
}

// ---------------------------------------------------------------------------
// JavaScript extractor
// ---------------------------------------------------------------------------

pub fn extract_javascript(path: &Path) -> Result<ExtractionResult, Box<dyn Error>> {
    let lang: tree_sitter::Language = tree_sitter_javascript::LANGUAGE.into();
    extract_js_like(path, lang)
}

/// Shared JS/TS extraction logic.
fn extract_js_like(
    path: &Path,
    lang: tree_sitter::Language,
) -> Result<ExtractionResult, Box<dyn Error>> {
    let src = std::fs::read(path)?;
    let source_file = path.to_string_lossy().into_owned();
    let stem = file_stem(path);
    let file_id = make_id(&[&source_file]);
    let file_label = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang)?;
    let tree = parser.parse(&src, None).ok_or("parse failed")?;
    let root = tree.root_node();

    let mut nodes: Vec<Value> = Vec::new();
    let mut edges: Vec<Value> = Vec::new();
    let mut fn_bodies: Vec<(String, tree_sitter::Node)> = Vec::new();

    nodes.push(make_node(&file_id, &file_label, &source_file, 1));

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        extract_js_statement(
            child,
            &src,
            &source_file,
            &stem,
            &file_id,
            None,
            &mut nodes,
            &mut edges,
            &mut fn_bodies,
        );
    }

    // Call graph second pass.
    for (caller_id, body_node) in &fn_bodies {
        let mut calls: Vec<tree_sitter::Node> = Vec::new();
        collect_nodes_of_kind(*body_node, "call_expression", &mut calls);
        for call in calls {
            let callee = extract_js_call_name(call, &src);
            if callee.is_empty() {
                continue;
            }
            edges.push(json!({
                "source": caller_id,
                "target": callee,
                "relation": "calls",
                "confidence": "EXTRACTED",
                "source_file": source_file,
                "source_location": format!("L{}", node_line(&call)),
                "weight": 1.0,
                "context": "call_stub",
            }));
        }
    }

    Ok(ExtractionResult {
        nodes,
        edges,
        ..Default::default()
    })
}

fn extract_js_call_name(call: tree_sitter::Node, src: &[u8]) -> String {
    // call_expression: function arguments
    let mut c = call.walk();
    if let Some(func) = call.children(&mut c).next() {
        match func.kind() {
            "identifier" => return node_text(&func, src).to_string(),
            "member_expression" => {
                // object "." property
                let mut mc = func.walk();
                for ch in func.children(&mut mc) {
                    if ch.kind() == "property_identifier" {
                        return node_text(&ch, src).to_string();
                    }
                }
            }
            _ => {}
        }
    }
    String::new()
}

#[allow(clippy::too_many_arguments)]
fn extract_js_statement<'a>(
    node: tree_sitter::Node<'a>,
    src: &[u8],
    source_file: &str,
    stem: &str,
    parent_id: &str,
    class_id: Option<&str>,
    nodes: &mut Vec<Value>,
    edges: &mut Vec<Value>,
    fn_bodies: &mut Vec<(String, tree_sitter::Node<'a>)>,
) {
    match node.kind() {
        "import_statement" => {
            // import X from '...' or import { X } from '...'
            let module = {
                let ch = children_vec(node);
                ch.into_iter()
                    .find(|n| n.kind() == "string")
                    .map(|n| {
                        // String node: extract the string_fragment child.
                        children_vec(n)
                            .into_iter()
                            .find(|c| c.kind() == "string_fragment")
                            .map(|c| node_text(&c, src).to_string())
                            .unwrap_or_else(|| {
                                // Fallback: strip quotes from raw text.
                                node_text(&n, src)
                                    .trim_matches(|c| c == '\'' || c == '"')
                                    .to_string()
                            })
                    })
                    .unwrap_or_default()
            };
            if !module.is_empty() {
                let target_id = make_id(&[&module]);
                edges.push(make_edge(
                    parent_id,
                    &target_id,
                    "imports",
                    source_file,
                    node_line(&node),
                    "EXTRACTED",
                    None,
                ));
            }
        }
        "function_declaration" => {
            extract_js_function(
                node, src, source_file, stem, parent_id, nodes, edges, fn_bodies,
            );
        }
        "class_declaration" => {
            extract_js_class(
                node, src, source_file, stem, parent_id, nodes, edges, fn_bodies,
            );
        }
        "lexical_declaration" | "variable_declaration" => {
            // const/let/var x = () => {} or function() {}
            let mut c = node.walk();
            for child in node.children(&mut c) {
                if child.kind() == "variable_declarator" {
                    extract_js_variable_declarator(
                        child, src, source_file, stem, parent_id, nodes, edges, fn_bodies,
                    );
                }
            }
        }
        "export_statement" => {
            // export function ... / export class ... / export default class ...
            let mut c = node.walk();
            for child in node.children(&mut c) {
                extract_js_statement(
                    child,
                    src,
                    source_file,
                    stem,
                    parent_id,
                    class_id,
                    nodes,
                    edges,
                    fn_bodies,
                );
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_js_function<'a>(
    node: tree_sitter::Node<'a>,
    src: &[u8],
    source_file: &str,
    stem: &str,
    parent_id: &str,
    nodes: &mut Vec<Value>,
    edges: &mut Vec<Value>,
    fn_bodies: &mut Vec<(String, tree_sitter::Node<'a>)>,
) {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(&n, src).to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return;
    }
    let fn_id = make_id(&[stem, &name]);
    let label = format!("{}()", name);
    nodes.push(make_node(&fn_id, &label, source_file, node_line(&node)));
    edges.push(make_edge(
        parent_id,
        &fn_id,
        "contains",
        source_file,
        node_line(&node),
        "EXTRACTED",
        None,
    ));
    if let Some(body) = node.child_by_field_name("body") {
        fn_bodies.push((fn_id, body));
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_js_class<'a>(
    node: tree_sitter::Node<'a>,
    src: &[u8],
    source_file: &str,
    stem: &str,
    parent_id: &str,
    nodes: &mut Vec<Value>,
    edges: &mut Vec<Value>,
    fn_bodies: &mut Vec<(String, tree_sitter::Node<'a>)>,
) {
    // Name field is "name" for class_declaration, might be type_identifier.
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(&n, src).to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return;
    }
    let class_id = make_id(&[stem, &name]);
    nodes.push(make_node(&class_id, &name, source_file, node_line(&node)));
    edges.push(make_edge(
        parent_id,
        &class_id,
        "contains",
        source_file,
        node_line(&node),
        "EXTRACTED",
        None,
    ));

    // Heritage: extends X.
    if let Some(heritage) = node.child_by_field_name("heritage") {
        // class_heritage → extends_clause → identifier/member_expression
        let mut hc = heritage.walk();
        for hchild in heritage.children(&mut hc) {
            if hchild.kind() == "extends_clause" {
                let mut ec = hchild.walk();
                for base in hchild.children(&mut ec) {
                    if base.kind() == "identifier" {
                        let base_id = make_id(&[node_text(&base, src)]);
                        edges.push(make_edge(
                            &class_id,
                            &base_id,
                            "inherits",
                            source_file,
                            node_line(&base),
                            "EXTRACTED",
                            None,
                        ));
                    }
                }
            }
        }
    }

    // Methods in class body.
    if let Some(body) = node.child_by_field_name("body") {
        let mut bc = body.walk();
        for member in body.children(&mut bc) {
            if member.kind() == "method_definition" {
                let mname = member
                    .child_by_field_name("name")
                    .map(|n| node_text(&n, src).to_string())
                    .unwrap_or_default();
                if mname.is_empty() {
                    continue;
                }
                let method_id = make_id(&[stem, &name, &mname]);
                let label = format!("{}()", mname);
                nodes.push(make_node(
                    &method_id,
                    &label,
                    source_file,
                    node_line(&member),
                ));
                edges.push(make_edge(
                    &class_id,
                    &method_id,
                    "method",
                    source_file,
                    node_line(&member),
                    "EXTRACTED",
                    None,
                ));
                if let Some(body) = member.child_by_field_name("body") {
                    fn_bodies.push((method_id, body));
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_js_variable_declarator<'a>(
    node: tree_sitter::Node<'a>,
    src: &[u8],
    source_file: &str,
    stem: &str,
    parent_id: &str,
    nodes: &mut Vec<Value>,
    edges: &mut Vec<Value>,
    fn_bodies: &mut Vec<(String, tree_sitter::Node<'a>)>,
) {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(&n, src).to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return;
    }
    // Check if value is arrow_function or function_expression.
    let value = node.child_by_field_name("value");
    if let Some(v) = value {
        if v.kind() == "arrow_function" || v.kind() == "function_expression" {
            let fn_id = make_id(&[stem, &name]);
            let label = format!("{}()", name);
            nodes.push(make_node(&fn_id, &label, source_file, node_line(&node)));
            edges.push(make_edge(
                parent_id,
                &fn_id,
                "contains",
                source_file,
                node_line(&node),
                "EXTRACTED",
                None,
            ));
            // Body: for arrow functions it may be statement_block or an expr.
            if let Some(body) = v.child_by_field_name("body") {
                if body.kind() == "statement_block" {
                    fn_bodies.push((fn_id, body));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Go extractor
// ---------------------------------------------------------------------------

/// Extract the receiver type name from a Go method's receiver parameter list.
fn go_receiver_type(recv: tree_sitter::Node, src: &[u8]) -> String {
    let pd_opt = children_vec(recv)
        .into_iter()
        .find(|n| n.kind() == "parameter_declaration");
    let pd = match pd_opt {
        Some(p) => p,
        None => return String::new(),
    };
    let type_node_opt = children_vec(pd).into_iter().find(|n| {
        n.kind() == "type_identifier" || n.kind() == "pointer_type"
    });
    let type_node = match type_node_opt {
        Some(t) => t,
        None => return String::new(),
    };
    if type_node.kind() == "pointer_type" {
        children_vec(type_node)
            .into_iter()
            .find(|c| c.kind() == "type_identifier")
            .map(|c| node_text(&c, src).to_string())
            .unwrap_or_else(|| node_text(&type_node, src).to_string())
    } else {
        node_text(&type_node, src).to_string()
    }
}

pub fn extract_go(path: &Path) -> Result<ExtractionResult, Box<dyn Error>> {
    let src_vec = std::fs::read(path)?;
    let src: &[u8] = &src_vec;
    let source_file = path.to_string_lossy().into_owned();
    let stem = file_stem(path);
    let file_id = make_id(&[&source_file]);
    let file_label = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let lang: tree_sitter::Language = tree_sitter_go::LANGUAGE.into();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang)?;
    let tree = parser.parse(&src, None).ok_or("parse failed")?;
    let root = tree.root_node();

    let mut nodes: Vec<Value> = Vec::new();
    let mut edges: Vec<Value> = Vec::new();
    let mut fn_bodies: Vec<(String, tree_sitter::Node)> = Vec::new();

    nodes.push(make_node(&file_id, &file_label, &source_file, 1));

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "import_declaration" => {
                // import "pkg" or import ( "pkg1" "pkg2" )
                let mut calls: Vec<tree_sitter::Node> = Vec::new();
                collect_nodes_of_kind(child, "import_spec", &mut calls);
                for spec in calls {
                    // path is a string literal
                    let mut sc = spec.walk();
                    let path_node = spec
                        .children(&mut sc)
                        .find(|n| n.kind() == "interpreted_string_literal");
                    if let Some(pn) = path_node {
                        // Extract inner string_fragment.
                        let mut pc = pn.walk();
                        let raw = pn
                            .children(&mut pc)
                            .find(|n| n.kind() == "interpreted_string_literal_content")
                            .map(|n| node_text(&n, src).to_string())
                            .unwrap_or_else(|| {
                                node_text(&pn, src)
                                    .trim_matches('"')
                                    .to_string()
                            });
                        if !raw.is_empty() {
                            let target_id = make_id(&[&raw]);
                            edges.push(make_edge(
                                &file_id,
                                &target_id,
                                "imports",
                                &source_file,
                                node_line(&child),
                                "EXTRACTED",
                                None,
                            ));
                        }
                    }
                }
            }
            "function_declaration" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| node_text(&n, src).to_string())
                    .unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                let fn_id = make_id(&[&stem, &name]);
                nodes.push(make_node(&fn_id, &format!("{}()", name), &source_file, node_line(&child)));
                edges.push(make_edge(&file_id, &fn_id, "contains", &source_file, node_line(&child), "EXTRACTED", None));
                if let Some(body) = child.child_by_field_name("body") {
                    fn_bodies.push((fn_id, body));
                }
            }
            "method_declaration" => {
                // receiver name body
                let name = child
                    .child_by_field_name("name")
                    .map(|n| node_text(&n, src).to_string())
                    .unwrap_or_default();
                // Receiver type.
                let recv_type = child
                    .child_by_field_name("receiver")
                    .map(|recv| go_receiver_type(recv, src))
                    .unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                let method_id = make_id(&[&stem, &recv_type, &name]);
                nodes.push(make_node(&method_id, &format!("{}()", name), &source_file, node_line(&child)));
                let parent = if recv_type.is_empty() {
                    file_id.clone()
                } else {
                    make_id(&[&stem, &recv_type])
                };
                edges.push(make_edge(&parent, &method_id, "method", &source_file, node_line(&child), "EXTRACTED", None));
                if let Some(body) = child.child_by_field_name("body") {
                    fn_bodies.push((method_id, body));
                }
            }
            "type_declaration" => {
                // type Foo struct {} / type Foo interface {}
                let mut specs: Vec<tree_sitter::Node> = Vec::new();
                collect_nodes_of_kind(child, "type_spec", &mut specs);
                for spec in specs {
                    let name = spec
                        .child_by_field_name("name")
                        .map(|n| node_text(&n, src).to_string())
                        .unwrap_or_default();
                    if name.is_empty() {
                        continue;
                    }
                    let type_id = make_id(&[&stem, &name]);
                    nodes.push(make_node(&type_id, &name, &source_file, node_line(&spec)));
                    edges.push(make_edge(&file_id, &type_id, "contains", &source_file, node_line(&spec), "EXTRACTED", None));
                }
            }
            _ => {}
        }
    }

    // Call graph.
    for (caller_id, body_node) in &fn_bodies {
        let mut calls: Vec<tree_sitter::Node> = Vec::new();
        collect_nodes_of_kind(*body_node, "call_expression", &mut calls);
        for call in calls {
            let callee = extract_go_call_name(call, &src);
            if callee.is_empty() {
                continue;
            }
            edges.push(json!({
                "source": caller_id,
                "target": callee,
                "relation": "calls",
                "confidence": "EXTRACTED",
                "source_file": source_file,
                "source_location": format!("L{}", node_line(&call)),
                "weight": 1.0,
                "context": "call_stub",
            }));
        }
    }

    Ok(ExtractionResult {
        nodes,
        edges,
        ..Default::default()
    })
}

fn extract_go_call_name(call: tree_sitter::Node, src: &[u8]) -> String {
    let mut c = call.walk();
    if let Some(func) = call.children(&mut c).next() {
        match func.kind() {
            "identifier" => return node_text(&func, src).to_string(),
            "selector_expression" => {
                let mut sc = func.walk();
                for ch in func.children(&mut sc) {
                    if ch.kind() == "field_identifier" {
                        return node_text(&ch, src).to_string();
                    }
                }
            }
            _ => {}
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Rust extractor
// ---------------------------------------------------------------------------

pub fn extract_rust_file(path: &Path) -> Result<ExtractionResult, Box<dyn Error>> {
    let src_vec = std::fs::read(path)?;
    let src: &[u8] = &src_vec;
    let source_file = path.to_string_lossy().into_owned();
    let stem = file_stem(path);
    let file_id = make_id(&[&source_file]);
    let file_label = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang)?;
    let tree = parser.parse(&src, None).ok_or("parse failed")?;
    let root = tree.root_node();

    let mut nodes: Vec<Value> = Vec::new();
    let mut edges: Vec<Value> = Vec::new();
    let mut fn_bodies: Vec<(String, tree_sitter::Node)> = Vec::new();

    nodes.push(make_node(&file_id, &file_label, &source_file, 1));

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        extract_rust_item(
            child,
            &src,
            &source_file,
            &stem,
            &file_id,
            None,
            &mut nodes,
            &mut edges,
            &mut fn_bodies,
        );
    }

    // Call graph.
    for (caller_id, body_node) in &fn_bodies {
        let mut calls: Vec<tree_sitter::Node> = Vec::new();
        collect_nodes_of_kind(*body_node, "call_expression", &mut calls);
        for call in calls {
            let callee = extract_rust_call_name(call, &src);
            if callee.is_empty() {
                continue;
            }
            edges.push(json!({
                "source": caller_id,
                "target": callee,
                "relation": "calls",
                "confidence": "EXTRACTED",
                "source_file": source_file,
                "source_location": format!("L{}", node_line(&call)),
                "weight": 1.0,
                "context": "call_stub",
            }));
        }
    }

    Ok(ExtractionResult {
        nodes,
        edges,
        ..Default::default()
    })
}

fn extract_rust_call_name(call: tree_sitter::Node, src: &[u8]) -> String {
    let mut c = call.walk();
    if let Some(func) = call.children(&mut c).next() {
        match func.kind() {
            "identifier" => return node_text(&func, src).to_string(),
            "field_expression" => {
                let mut fc = func.walk();
                for ch in func.children(&mut fc) {
                    if ch.kind() == "field_identifier" {
                        return node_text(&ch, src).to_string();
                    }
                }
            }
            "scoped_identifier" => {
                // Last identifier segment.
                let mut sc = func.walk();
                let mut last = String::new();
                for ch in func.children(&mut sc) {
                    if ch.kind() == "identifier" {
                        last = node_text(&ch, src).to_string();
                    }
                }
                return last;
            }
            _ => {}
        }
    }
    String::new()
}

#[allow(clippy::too_many_arguments)]
fn extract_rust_item<'a>(
    node: tree_sitter::Node<'a>,
    src: &[u8],
    source_file: &str,
    stem: &str,
    parent_id: &str,
    impl_type: Option<&str>,
    nodes: &mut Vec<Value>,
    edges: &mut Vec<Value>,
    fn_bodies: &mut Vec<(String, tree_sitter::Node<'a>)>,
) {
    match node.kind() {
        "use_declaration" => {
            // use std::io;
            let text = node_text(&node, src);
            // Strip "use " and ";"
            let module = text
                .trim_start_matches("use ")
                .trim_end_matches(';')
                .trim()
                .to_string();
            if !module.is_empty() {
                let target_id = make_id(&[&module]);
                edges.push(make_edge(
                    parent_id,
                    &target_id,
                    "imports",
                    source_file,
                    node_line(&node),
                    "EXTRACTED",
                    None,
                ));
            }
        }
        "mod_item" => {
            let name = node
                .child_by_field_name("name")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            if !name.is_empty() {
                let mod_id = make_id(&[stem, &name]);
                nodes.push(make_node(&mod_id, &name, source_file, node_line(&node)));
                edges.push(make_edge(
                    parent_id,
                    &mod_id,
                    "contains",
                    source_file,
                    node_line(&node),
                    "EXTRACTED",
                    None,
                ));
                // If inline module, walk its body.
                if let Some(body) = node.child_by_field_name("body") {
                    let mut bc = body.walk();
                    for child in body.children(&mut bc) {
                        extract_rust_item(
                            child,
                            src,
                            source_file,
                            &make_id(&[stem, &name]),
                            &mod_id,
                            None,
                            nodes,
                            edges,
                            fn_bodies,
                        );
                    }
                }
            }
        }
        "function_item" => {
            let name = node
                .child_by_field_name("name")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            if name.is_empty() {
                return;
            }
            let fn_id = if let Some(t) = impl_type {
                make_id(&[stem, t, &name])
            } else {
                make_id(&[stem, &name])
            };
            nodes.push(make_node(&fn_id, &format!("{}()", name), source_file, node_line(&node)));
            let rel = if impl_type.is_some() { "method" } else { "contains" };
            edges.push(make_edge(parent_id, &fn_id, rel, source_file, node_line(&node), "EXTRACTED", None));
            if let Some(body) = node.child_by_field_name("body") {
                fn_bodies.push((fn_id, body));
            }
        }
        "function_signature_item" => {
            // Trait function signature (no body).
            let name = node
                .child_by_field_name("name")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            if name.is_empty() {
                return;
            }
            let fn_id = if let Some(t) = impl_type {
                make_id(&[stem, t, &name])
            } else {
                make_id(&[stem, &name])
            };
            nodes.push(make_node(&fn_id, &format!("{}()", name), source_file, node_line(&node)));
            edges.push(make_edge(parent_id, &fn_id, "method", source_file, node_line(&node), "EXTRACTED", None));
        }
        "struct_item" => {
            let name = node
                .child_by_field_name("name")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            if name.is_empty() {
                return;
            }
            let struct_id = make_id(&[stem, &name]);
            nodes.push(make_node(&struct_id, &name, source_file, node_line(&node)));
            edges.push(make_edge(parent_id, &struct_id, "contains", source_file, node_line(&node), "EXTRACTED", None));
        }
        "enum_item" => {
            let name = node
                .child_by_field_name("name")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            if name.is_empty() {
                return;
            }
            let enum_id = make_id(&[stem, &name]);
            nodes.push(make_node(&enum_id, &name, source_file, node_line(&node)));
            edges.push(make_edge(parent_id, &enum_id, "contains", source_file, node_line(&node), "EXTRACTED", None));
        }
        "trait_item" => {
            let name = node
                .child_by_field_name("name")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            if name.is_empty() {
                return;
            }
            let trait_id = make_id(&[stem, &name]);
            nodes.push(make_node(&trait_id, &name, source_file, node_line(&node)));
            edges.push(make_edge(parent_id, &trait_id, "contains", source_file, node_line(&node), "EXTRACTED", None));
            // Walk trait body for signatures.
            if let Some(body) = node.child_by_field_name("body") {
                let mut bc = body.walk();
                for child in body.children(&mut bc) {
                    extract_rust_item(
                        child,
                        src,
                        source_file,
                        stem,
                        &trait_id,
                        Some(&name),
                        nodes,
                        edges,
                        fn_bodies,
                    );
                }
            }
        }
        "impl_item" => {
            // impl TypeName { ... } or impl Trait for TypeName { ... }
            let type_name = node
                .child_by_field_name("type")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            let impl_id = if type_name.is_empty() {
                parent_id.to_string()
            } else {
                make_id(&[stem, &type_name])
            };
            if let Some(body) = node.child_by_field_name("body") {
                let mut bc = body.walk();
                for child in body.children(&mut bc) {
                    extract_rust_item(
                        child,
                        src,
                        source_file,
                        stem,
                        &impl_id,
                        if type_name.is_empty() {
                            None
                        } else {
                            Some(&type_name)
                        },
                        nodes,
                        edges,
                        fn_bodies,
                    );
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Java extractor
// ---------------------------------------------------------------------------

pub fn extract_java(path: &Path) -> Result<ExtractionResult, Box<dyn Error>> {
    let src_vec = std::fs::read(path)?;
    let src: &[u8] = &src_vec;
    let source_file = path.to_string_lossy().into_owned();
    let stem = file_stem(path);
    let file_id = make_id(&[&source_file]);
    let file_label = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let lang: tree_sitter::Language = tree_sitter_java::LANGUAGE.into();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang)?;
    let tree = parser.parse(&src, None).ok_or("parse failed")?;
    let root = tree.root_node();

    let mut nodes: Vec<Value> = Vec::new();
    let mut edges: Vec<Value> = Vec::new();
    let mut fn_bodies: Vec<(String, tree_sitter::Node)> = Vec::new();

    nodes.push(make_node(&file_id, &file_label, &source_file, 1));

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "import_declaration" => {
                // import java.util.List;
                let mut ic = child.walk();
                let module = child
                    .children(&mut ic)
                    .find(|n| n.kind() == "scoped_identifier" || n.kind() == "identifier")
                    .map(|n| node_text(&n, src).to_string())
                    .unwrap_or_default();
                if !module.is_empty() {
                    let target_id = make_id(&[&module]);
                    edges.push(make_edge(&file_id, &target_id, "imports", &source_file, node_line(&child), "EXTRACTED", None));
                }
            }
            "class_declaration" | "interface_declaration" | "enum_declaration" => {
                extract_java_class(child, &src, &source_file, &stem, &file_id, &mut nodes, &mut edges, &mut fn_bodies);
            }
            _ => {}
        }
    }

    // Call graph.
    for (caller_id, body_node) in &fn_bodies {
        let mut calls: Vec<tree_sitter::Node> = Vec::new();
        collect_nodes_of_kind(*body_node, "method_invocation", &mut calls);
        for call in calls {
            let callee = call
                .child_by_field_name("name")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            if callee.is_empty() {
                continue;
            }
            edges.push(json!({
                "source": caller_id,
                "target": callee,
                "relation": "calls",
                "confidence": "EXTRACTED",
                "source_file": source_file,
                "source_location": format!("L{}", node_line(&call)),
                "weight": 1.0,
                "context": "call_stub",
            }));
        }
    }

    Ok(ExtractionResult {
        nodes,
        edges,
        ..Default::default()
    })
}

#[allow(clippy::too_many_arguments)]
fn extract_java_class<'a>(
    node: tree_sitter::Node<'a>,
    src: &[u8],
    source_file: &str,
    stem: &str,
    parent_id: &str,
    nodes: &mut Vec<Value>,
    edges: &mut Vec<Value>,
    fn_bodies: &mut Vec<(String, tree_sitter::Node<'a>)>,
) {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(&n, src).to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return;
    }
    let class_id = make_id(&[stem, &name]);
    nodes.push(make_node(&class_id, &name, source_file, node_line(&node)));
    edges.push(make_edge(parent_id, &class_id, "contains", source_file, node_line(&node), "EXTRACTED", None));

    // Superclass.
    if let Some(superclass) = node.child_by_field_name("superclass") {
        let base_opt = children_vec(superclass)
            .into_iter()
            .find(|n| n.kind() == "type_identifier");
        if let Some(base) = base_opt {
            let base_id = make_id(&[node_text(&base, src)]);
            edges.push(make_edge(&class_id, &base_id, "inherits", source_file, node_line(&base), "EXTRACTED", None));
        }
    }

    // Interfaces.
    if let Some(interfaces) = node.child_by_field_name("interfaces") {
        let mut ic = interfaces.walk();
        for iface in interfaces.children(&mut ic) {
            if iface.kind() == "type_list" {
                let mut tc = iface.walk();
                for ti in iface.children(&mut tc) {
                    if ti.kind() == "type_identifier" {
                        let iface_id = make_id(&[node_text(&ti, src)]);
                        edges.push(make_edge(&class_id, &iface_id, "implements", source_file, node_line(&ti), "EXTRACTED", None));
                    }
                }
            }
        }
    }

    // Methods.
    if let Some(body) = node.child_by_field_name("body") {
        let mut bc = body.walk();
        for member in body.children(&mut bc) {
            if member.kind() == "method_declaration" || member.kind() == "constructor_declaration" {
                let mname = member
                    .child_by_field_name("name")
                    .map(|n| node_text(&n, src).to_string())
                    .unwrap_or_default();
                if mname.is_empty() {
                    continue;
                }
                let method_id = make_id(&[stem, &name, &mname]);
                nodes.push(make_node(&method_id, &format!("{}()", mname), source_file, node_line(&member)));
                edges.push(make_edge(&class_id, &method_id, "method", source_file, node_line(&member), "EXTRACTED", None));
                if let Some(body) = member.child_by_field_name("body") {
                    fn_bodies.push((method_id, body));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// C extractor
// ---------------------------------------------------------------------------

pub fn extract_c(path: &Path) -> Result<ExtractionResult, Box<dyn Error>> {
    let lang: tree_sitter::Language = tree_sitter_c::LANGUAGE.into();
    extract_c_like(path, lang)
}

// ---------------------------------------------------------------------------
// C++ extractor
// ---------------------------------------------------------------------------

pub fn extract_cpp(path: &Path) -> Result<ExtractionResult, Box<dyn Error>> {
    let lang: tree_sitter::Language = tree_sitter_cpp::LANGUAGE.into();
    extract_c_like(path, lang)
}

/// Shared C/C++ extraction.
fn extract_c_like(
    path: &Path,
    lang: tree_sitter::Language,
) -> Result<ExtractionResult, Box<dyn Error>> {
    let src_vec = std::fs::read(path)?;
    let src: &[u8] = &src_vec;
    let source_file = path.to_string_lossy().into_owned();
    let stem = file_stem(path);
    let file_id = make_id(&[&source_file]);
    let file_label = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang)?;
    let tree = parser.parse(&src, None).ok_or("parse failed")?;
    let root = tree.root_node();

    let mut nodes: Vec<Value> = Vec::new();
    let mut edges: Vec<Value> = Vec::new();
    let mut fn_bodies: Vec<(String, tree_sitter::Node)> = Vec::new();

    nodes.push(make_node(&file_id, &file_label, &source_file, 1));

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "preproc_include" => {
                // #include <foo.h> or #include "foo.h"
                let mut ic = child.walk();
                let header = child
                    .children(&mut ic)
                    .find(|n| {
                        n.kind() == "system_lib_string" || n.kind() == "string_literal"
                    })
                    .map(|n| {
                        node_text(&n, src)
                            .trim_matches(|c| c == '<' || c == '>' || c == '"')
                            .to_string()
                    })
                    .unwrap_or_default();
                if !header.is_empty() {
                    let target_id = make_id(&[&header]);
                    edges.push(make_edge(&file_id, &target_id, "imports", &source_file, node_line(&child), "EXTRACTED", None));
                }
            }
            "function_definition" => {
                extract_c_function(child, &src, &source_file, &stem, &file_id, None, &mut nodes, &mut edges, &mut fn_bodies);
            }
            "class_specifier" => {
                // C++ class
                extract_cpp_class(child, &src, &source_file, &stem, &file_id, &mut nodes, &mut edges, &mut fn_bodies);
            }
            "struct_specifier" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| node_text(&n, src).to_string())
                    .unwrap_or_default();
                if !name.is_empty() {
                    let struct_id = make_id(&[&stem, &name]);
                    nodes.push(make_node(&struct_id, &name, &source_file, node_line(&child)));
                    edges.push(make_edge(&file_id, &struct_id, "contains", &source_file, node_line(&child), "EXTRACTED", None));
                }
            }
            _ => {}
        }
    }

    // Call graph.
    for (caller_id, body_node) in &fn_bodies {
        let mut calls: Vec<tree_sitter::Node> = Vec::new();
        collect_nodes_of_kind(*body_node, "call_expression", &mut calls);
        for call in calls {
            let mut cc = call.walk();
            let callee = call
                .children(&mut cc)
                .next()
                .filter(|n| n.kind() == "identifier")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            if callee.is_empty() {
                continue;
            }
            edges.push(json!({
                "source": caller_id,
                "target": callee,
                "relation": "calls",
                "confidence": "EXTRACTED",
                "source_file": source_file,
                "source_location": format!("L{}", node_line(&call)),
                "weight": 1.0,
                "context": "call_stub",
            }));
        }
    }

    Ok(ExtractionResult {
        nodes,
        edges,
        ..Default::default()
    })
}

#[allow(clippy::too_many_arguments)]
fn extract_c_function<'a>(
    node: tree_sitter::Node<'a>,
    src: &[u8],
    source_file: &str,
    stem: &str,
    parent_id: &str,
    class_name: Option<&str>,
    nodes: &mut Vec<Value>,
    edges: &mut Vec<Value>,
    fn_bodies: &mut Vec<(String, tree_sitter::Node<'a>)>,
) {
    // function_definition: type declarator body
    // declarator is function_declarator: name params
    let declarator = node.child_by_field_name("declarator");
    let name = declarator
        .and_then(|d| {
            if d.kind() == "function_declarator" {
                d.child_by_field_name("declarator")
            } else {
                // C++ method inside class: declarator may be field_identifier
                d.child_by_field_name("declarator")
                    .or_else(|| Some(d))
            }
        })
        .map(|n| node_text(&n, src).to_string())
        .unwrap_or_default();
    let name = name.trim().to_string();
    if name.is_empty() || name.contains(' ') {
        return;
    }
    let fn_id = if let Some(cls) = class_name {
        make_id(&[stem, cls, &name])
    } else {
        make_id(&[stem, &name])
    };
    let rel = if class_name.is_some() { "method" } else { "contains" };
    nodes.push(make_node(&fn_id, &format!("{}()", name), source_file, node_line(&node)));
    edges.push(make_edge(parent_id, &fn_id, rel, source_file, node_line(&node), "EXTRACTED", None));
    if let Some(body) = node.child_by_field_name("body") {
        fn_bodies.push((fn_id, body));
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_cpp_class<'a>(
    node: tree_sitter::Node<'a>,
    src: &[u8],
    source_file: &str,
    stem: &str,
    parent_id: &str,
    nodes: &mut Vec<Value>,
    edges: &mut Vec<Value>,
    fn_bodies: &mut Vec<(String, tree_sitter::Node<'a>)>,
) {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(&n, src).to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return;
    }
    let class_id = make_id(&[stem, &name]);
    nodes.push(make_node(&class_id, &name, source_file, node_line(&node)));
    edges.push(make_edge(parent_id, &class_id, "contains", source_file, node_line(&node), "EXTRACTED", None));

    // Base classes.
    if let Some(bases) = node.child_by_field_name("base_clause") {
        let mut bc = bases.walk();
        for base in bases.children(&mut bc) {
            if base.kind() == "type_identifier" {
                let base_id = make_id(&[node_text(&base, src)]);
                edges.push(make_edge(&class_id, &base_id, "inherits", source_file, node_line(&base), "EXTRACTED", None));
            }
        }
    }

    // Members.
    if let Some(body) = node.child_by_field_name("body") {
        let mut bc = body.walk();
        for member in body.children(&mut bc) {
            if member.kind() == "function_definition" {
                // Field identifier (C++ method name can be field_identifier).
                let declarator = member.child_by_field_name("declarator");
                let mname = declarator
                    .and_then(|d| {
                        if d.kind() == "function_declarator" {
                            d.child_by_field_name("declarator")
                        } else {
                            None
                        }
                    })
                    .map(|n| node_text(&n, src).to_string())
                    .unwrap_or_default();
                if mname.is_empty() {
                    continue;
                }
                let method_id = make_id(&[stem, &name, &mname]);
                nodes.push(make_node(&method_id, &format!("{}()", mname), source_file, node_line(&member)));
                edges.push(make_edge(&class_id, &method_id, "method", source_file, node_line(&member), "EXTRACTED", None));
                if let Some(mbody) = member.child_by_field_name("body") {
                    fn_bodies.push((method_id, mbody));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Ruby extractor
// ---------------------------------------------------------------------------

pub fn extract_ruby(path: &Path) -> Result<ExtractionResult, Box<dyn Error>> {
    let src_vec = std::fs::read(path)?;
    let src: &[u8] = &src_vec;
    let source_file = path.to_string_lossy().into_owned();
    let stem = file_stem(path);
    let file_id = make_id(&[&source_file]);
    let file_label = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let lang: tree_sitter::Language = tree_sitter_ruby::LANGUAGE.into();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang)?;
    let tree = parser.parse(&src, None).ok_or("parse failed")?;
    let root = tree.root_node();

    let mut nodes: Vec<Value> = Vec::new();
    let mut edges: Vec<Value> = Vec::new();
    let mut fn_bodies: Vec<(String, tree_sitter::Node)> = Vec::new();

    nodes.push(make_node(&file_id, &file_label, &source_file, 1));

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        extract_ruby_statement(
            child,
            &src,
            &source_file,
            &stem,
            &file_id,
            None,
            &mut nodes,
            &mut edges,
            &mut fn_bodies,
        );
    }

    // Call graph.
    for (caller_id, body_node) in &fn_bodies {
        let mut calls: Vec<tree_sitter::Node> = Vec::new();
        collect_nodes_of_kind(*body_node, "call", &mut calls);
        for call in calls {
            let callee = call
                .child_by_field_name("method")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_else(|| {
                    // First child as fallback.
                    children_vec(call)
                        .into_iter()
                        .find(|n| n.kind() == "identifier")
                        .map(|n| node_text(&n, src).to_string())
                        .unwrap_or_default()
                });
            if callee.is_empty() {
                continue;
            }
            edges.push(json!({
                "source": caller_id,
                "target": callee,
                "relation": "calls",
                "confidence": "EXTRACTED",
                "source_file": source_file,
                "source_location": format!("L{}", node_line(&call)),
                "weight": 1.0,
                "context": "call_stub",
            }));
        }
    }

    Ok(ExtractionResult {
        nodes,
        edges,
        ..Default::default()
    })
}

#[allow(clippy::too_many_arguments)]
fn extract_ruby_statement<'a>(
    node: tree_sitter::Node<'a>,
    src: &[u8],
    source_file: &str,
    stem: &str,
    parent_id: &str,
    class_id: Option<&str>,
    nodes: &mut Vec<Value>,
    edges: &mut Vec<Value>,
    fn_bodies: &mut Vec<(String, tree_sitter::Node<'a>)>,
) {
    match node.kind() {
        "call" => {
            // require 'foo'
            let method = children_vec(node)
                .into_iter()
                .find(|n| n.kind() == "identifier")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            if method == "require" || method == "require_relative" {
                // argument is a string
                let arg = children_vec(node)
                    .into_iter()
                    .find(|n| n.kind() == "argument_list")
                    .and_then(|al| {
                        children_vec(al)
                            .into_iter()
                            .find(|n| n.kind() == "string")
                            .map(|s| {
                                let content = children_vec(s)
                                    .into_iter()
                                    .find(|c| c.kind() == "string_content")
                                    .map(|c| node_text(&c, src).to_string());
                                content.unwrap_or_else(|| {
                                    node_text(&s, src).trim_matches(|c| c == '\'' || c == '"').to_string()
                                })
                            })
                    })
                    .unwrap_or_default();
                if !arg.is_empty() {
                    let target_id = make_id(&[&arg]);
                    edges.push(make_edge(parent_id, &target_id, "imports", source_file, node_line(&node), "EXTRACTED", None));
                }
            }
        }
        "class" => {
            let name = node
                .child_by_field_name("name")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            if name.is_empty() {
                return;
            }
            let cid = make_id(&[stem, &name]);
            nodes.push(make_node(&cid, &name, source_file, node_line(&node)));
            edges.push(make_edge(parent_id, &cid, "contains", source_file, node_line(&node), "EXTRACTED", None));

            // Superclass.
            if let Some(sup) = node.child_by_field_name("superclass") {
                let base_opt = children_vec(sup).into_iter().find(|n| n.kind() == "constant");
                if let Some(base) = base_opt {
                    let base_id = make_id(&[node_text(&base, src)]);
                    edges.push(make_edge(&cid, &base_id, "inherits", source_file, node_line(&base), "EXTRACTED", None));
                }
            }

            // Body.
            if let Some(body) = node.child_by_field_name("body") {
                let mut bc = body.walk();
                for child in body.children(&mut bc) {
                    extract_ruby_statement(child, src, source_file, stem, &cid, Some(&cid), nodes, edges, fn_bodies);
                }
            }
        }
        "method" => {
            let name = node
                .child_by_field_name("name")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            if name.is_empty() {
                return;
            }
            let fn_id = if let Some(cls) = class_id {
                make_id(&[stem, cls, &name])
            } else {
                make_id(&[stem, &name])
            };
            let rel = if class_id.is_some() { "method" } else { "contains" };
            nodes.push(make_node(&fn_id, &format!("{}()", name), source_file, node_line(&node)));
            edges.push(make_edge(parent_id, &fn_id, rel, source_file, node_line(&node), "EXTRACTED", None));
            if let Some(body) = node.child_by_field_name("body") {
                fn_bodies.push((fn_id, body));
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// C# extractor
// ---------------------------------------------------------------------------

pub fn extract_csharp(path: &Path) -> Result<ExtractionResult, Box<dyn Error>> {
    let src_vec = std::fs::read(path)?;
    let src: &[u8] = &src_vec;
    let source_file = path.to_string_lossy().into_owned();
    let stem = file_stem(path);
    let file_id = make_id(&[&source_file]);
    let file_label = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let lang: tree_sitter::Language = tree_sitter_c_sharp::LANGUAGE.into();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang)?;
    let tree = parser.parse(&src, None).ok_or("parse failed")?;
    let root = tree.root_node();

    let mut nodes: Vec<Value> = Vec::new();
    let mut edges: Vec<Value> = Vec::new();
    let mut fn_bodies: Vec<(String, tree_sitter::Node)> = Vec::new();

    nodes.push(make_node(&file_id, &file_label, &source_file, 1));

    // Walk top-level (may be wrapped in namespace_declaration).
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        extract_csharp_member(child, &src, &source_file, &stem, &file_id, None, &mut nodes, &mut edges, &mut fn_bodies);
    }

    // Call graph.
    for (caller_id, body_node) in &fn_bodies {
        let mut calls: Vec<tree_sitter::Node> = Vec::new();
        collect_nodes_of_kind(*body_node, "invocation_expression", &mut calls);
        for call in calls {
            // invocation_expression: expression argument_list
            let mut cc = call.walk();
            let callee = call
                .children(&mut cc)
                .next()
                .map(|expr| {
                    // Could be identifier or member_access_expression.
                    match expr.kind() {
                        "identifier" => node_text(&expr, src).to_string(),
                        "member_access_expression" => {
                            let mut mc = expr.walk();
                            expr.children(&mut mc)
                                .filter(|n| n.kind() == "identifier")
                                .last()
                                .map(|n| node_text(&n, src).to_string())
                                .unwrap_or_default()
                        }
                        _ => String::new(),
                    }
                })
                .unwrap_or_default();
            if callee.is_empty() {
                continue;
            }
            edges.push(json!({
                "source": caller_id,
                "target": callee,
                "relation": "calls",
                "confidence": "EXTRACTED",
                "source_file": source_file,
                "source_location": format!("L{}", node_line(&call)),
                "weight": 1.0,
                "context": "call_stub",
            }));
        }
    }

    Ok(ExtractionResult {
        nodes,
        edges,
        ..Default::default()
    })
}

#[allow(clippy::too_many_arguments)]
fn extract_csharp_member<'a>(
    node: tree_sitter::Node<'a>,
    src: &[u8],
    source_file: &str,
    stem: &str,
    parent_id: &str,
    class_name: Option<&str>,
    nodes: &mut Vec<Value>,
    edges: &mut Vec<Value>,
    fn_bodies: &mut Vec<(String, tree_sitter::Node<'a>)>,
) {
    match node.kind() {
        "using_directive" => {
            // using System.Collections;
            let text = node_text(&node, src);
            let module = text
                .trim_start_matches("using ")
                .trim_end_matches(';')
                .trim()
                .to_string();
            if !module.is_empty() {
                let target_id = make_id(&[&module]);
                edges.push(make_edge(parent_id, &target_id, "imports", source_file, node_line(&node), "EXTRACTED", None));
            }
        }
        "namespace_declaration" => {
            let name = node
                .child_by_field_name("name")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            let ns_parent = if name.is_empty() {
                parent_id.to_string()
            } else {
                let ns_id = make_id(&[stem, &name]);
                nodes.push(make_node(&ns_id, &name, source_file, node_line(&node)));
                edges.push(make_edge(parent_id, &ns_id, "contains", source_file, node_line(&node), "EXTRACTED", None));
                ns_id
            };
            if let Some(body) = node.child_by_field_name("body") {
                let mut bc = body.walk();
                for child in body.children(&mut bc) {
                    extract_csharp_member(child, src, source_file, stem, &ns_parent, None, nodes, edges, fn_bodies);
                }
            }
        }
        "class_declaration" | "interface_declaration" | "struct_declaration" => {
            let name = node
                .child_by_field_name("name")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            if name.is_empty() {
                return;
            }
            let class_id = make_id(&[stem, &name]);
            nodes.push(make_node(&class_id, &name, source_file, node_line(&node)));
            edges.push(make_edge(parent_id, &class_id, "contains", source_file, node_line(&node), "EXTRACTED", None));

            // Base types.
            if let Some(bases) = node.child_by_field_name("bases") {
                let mut bc = bases.walk();
                for base in bases.children(&mut bc) {
                    if base.kind() == "identifier" || base.kind() == "qualified_name" {
                        let base_id = make_id(&[node_text(&base, src)]);
                        edges.push(make_edge(&class_id, &base_id, "inherits", source_file, node_line(&base), "EXTRACTED", None));
                    }
                }
            }

            // Members.
            if let Some(body) = node.child_by_field_name("body") {
                let mut bc = body.walk();
                for child in body.children(&mut bc) {
                    extract_csharp_member(child, src, source_file, stem, &class_id, Some(&name), nodes, edges, fn_bodies);
                }
            }
        }
        "method_declaration" | "constructor_declaration" => {
            let name = node
                .child_by_field_name("name")
                .map(|n| node_text(&n, src).to_string())
                .unwrap_or_default();
            if name.is_empty() {
                return;
            }
            let method_id = if let Some(cls) = class_name {
                make_id(&[stem, cls, &name])
            } else {
                make_id(&[stem, &name])
            };
            let rel = if class_name.is_some() { "method" } else { "contains" };
            nodes.push(make_node(&method_id, &format!("{}()", name), source_file, node_line(&node)));
            edges.push(make_edge(parent_id, &method_id, rel, source_file, node_line(&node), "EXTRACTED", None));
            if let Some(body) = node.child_by_field_name("body") {
                fn_bodies.push((method_id, body));
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp(ext: &str, content: &[u8]) -> NamedTempFile {
        let mut f = tempfile::Builder::new()
            .suffix(&format!(".{}", ext))
            .tempfile()
            .unwrap();
        f.write_all(content).unwrap();
        f
    }

    #[test]
    fn make_id_basic() {
        assert_eq!(make_id(&["foo", "bar"]), "foo_bar");
        assert_eq!(make_id(&["Hello World", "Rust"]), "hello_world_rust");
        assert_eq!(make_id(&["__foo__", "bar"]), "foo_bar");
        assert_eq!(make_id(&["", "baz"]), "baz");
    }

    #[test]
    fn make_id_special_chars() {
        assert_eq!(make_id(&["a.b.c"]), "a_b_c");
        assert_eq!(make_id(&["foo/bar"]), "foo_bar");
    }

    #[test]
    fn file_stem_basic() {
        let p = Path::new("src/foo.py");
        assert_eq!(file_stem(p), "src.foo");
    }

    #[test]
    fn file_stem_no_parent() {
        let p = Path::new("foo.py");
        assert_eq!(file_stem(p), "foo");
    }

    #[test]
    fn extract_python_basic() {
        let f = write_temp(
            "py",
            b"import os\nfrom sys import path\n\nclass Foo(Bar):\n    def method(self):\n        pass\n\ndef func():\n    Foo()\n",
        );
        let result = extract_python(f.path()).unwrap();
        let ids: Vec<_> = result
            .nodes
            .iter()
            .filter_map(|n| n.get("id").and_then(|v| v.as_str()))
            .collect();
        // Should have file, class, method, func nodes.
        assert!(ids.iter().any(|id| id.contains("foo")), "missing class node");
        assert!(ids.iter().any(|id| id.contains("method")), "missing method node");
        assert!(ids.iter().any(|id| id.contains("func")), "missing func node");

        let relations: Vec<_> = result
            .edges
            .iter()
            .filter_map(|e| e.get("relation").and_then(|v| v.as_str()))
            .collect();
        assert!(relations.contains(&"imports"), "missing imports edge");
        assert!(relations.contains(&"imports_from"), "missing imports_from edge");
        assert!(relations.contains(&"contains"), "missing contains edge");
        assert!(relations.contains(&"method"), "missing method edge");
        assert!(relations.contains(&"inherits"), "missing inherits edge");
    }

    #[test]
    fn extract_typescript_basic() {
        let f = write_temp(
            "ts",
            b"import { foo } from './foo';\n\nclass MyClass extends Base {\n  myMethod() { bar(); }\n}\n\nconst fn = () => baz();\n",
        );
        let result = extract_typescript(f.path()).unwrap();
        let labels: Vec<_> = result
            .nodes
            .iter()
            .filter_map(|n| n.get("label").and_then(|v| v.as_str()))
            .collect();
        assert!(labels.iter().any(|l| *l == "MyClass"), "missing class");
        assert!(labels.iter().any(|l| l.contains("myMethod")), "missing method");
    }

    #[test]
    fn extract_go_basic() {
        let f = write_temp(
            "go",
            b"package main\nimport \"fmt\"\n\ntype Server struct{}\n\nfunc New() *Server { return &Server{} }\n\nfunc (s *Server) Run() { fmt.Println(\"running\") }\n",
        );
        let result = extract_go(f.path()).unwrap();
        let labels: Vec<_> = result
            .nodes
            .iter()
            .filter_map(|n| n.get("label").and_then(|v| v.as_str()))
            .collect();
        assert!(labels.iter().any(|l| *l == "Server"), "missing struct");
        assert!(labels.iter().any(|l| l.contains("New")), "missing func");
        assert!(labels.iter().any(|l| l.contains("Run")), "missing method");
    }

    #[test]
    fn extract_rust_basic() {
        let f = write_temp(
            "rs",
            b"use std::io;\n\npub struct Foo;\n\nimpl Foo {\n    pub fn bar(&self) {}\n}\n\npub fn standalone() {}\n",
        );
        let result = extract_rust_file(f.path()).unwrap();
        let labels: Vec<_> = result
            .nodes
            .iter()
            .filter_map(|n| n.get("label").and_then(|v| v.as_str()))
            .collect();
        assert!(labels.iter().any(|l| *l == "Foo"), "missing struct");
        assert!(labels.iter().any(|l| l.contains("bar")), "missing method");
        assert!(labels.iter().any(|l| l.contains("standalone")), "missing fn");
    }

    #[test]
    fn extract_java_basic() {
        let f = write_temp(
            "java",
            b"import java.util.List;\n\npublic class MyClass extends Base {\n    public void doWork() { helper(); }\n}\n",
        );
        let result = extract_java(f.path()).unwrap();
        let labels: Vec<_> = result
            .nodes
            .iter()
            .filter_map(|n| n.get("label").and_then(|v| v.as_str()))
            .collect();
        assert!(labels.iter().any(|l| *l == "MyClass"), "missing class");
        assert!(labels.iter().any(|l| l.contains("doWork")), "missing method");
    }

    #[test]
    fn extract_c_basic() {
        let f = write_temp(
            "c",
            b"#include <stdio.h>\n\nvoid hello(int x) { printf(\"%d\", x); }\n",
        );
        let result = extract_c(f.path()).unwrap();
        let labels: Vec<_> = result
            .nodes
            .iter()
            .filter_map(|n| n.get("label").and_then(|v| v.as_str()))
            .collect();
        assert!(labels.iter().any(|l| l.contains("hello")), "missing func");
    }

    #[test]
    fn extract_ruby_basic() {
        let f = write_temp(
            "rb",
            b"require 'json'\n\nclass Foo < Bar\n  def method\n    bar()\n  end\nend\n",
        );
        let result = extract_ruby(f.path()).unwrap();
        let labels: Vec<_> = result
            .nodes
            .iter()
            .filter_map(|n| n.get("label").and_then(|v| v.as_str()))
            .collect();
        assert!(labels.iter().any(|l| *l == "Foo"), "missing class");
        assert!(labels.iter().any(|l| l.contains("method")), "missing method");
    }

    #[test]
    fn extract_csharp_basic() {
        let f = write_temp(
            "cs",
            b"using System;\n\nnamespace NS {\n    class MyClass {\n        public void Work() { Foo(); }\n    }\n}\n",
        );
        let result = extract_csharp(f.path()).unwrap();
        let labels: Vec<_> = result
            .nodes
            .iter()
            .filter_map(|n| n.get("label").and_then(|v| v.as_str()))
            .collect();
        assert!(labels.iter().any(|l| *l == "MyClass"), "missing class");
        assert!(labels.iter().any(|l| l.contains("Work")), "missing method");
    }

    #[test]
    fn collect_files_finds_supported() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("main.py"), b"").unwrap();
        std::fs::write(dir.path().join("app.ts"), b"").unwrap();
        std::fs::write(dir.path().join("readme.md"), b"").unwrap();
        let files = collect_files(dir.path(), false);
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn collect_files_skips_hidden() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".hidden")).unwrap();
        std::fs::write(dir.path().join(".hidden/main.py"), b"").unwrap();
        std::fs::write(dir.path().join("visible.py"), b"").unwrap();
        let files = collect_files(dir.path(), false);
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn extract_multi_file_merge() {
        let f1 = write_temp("py", b"def foo(): pass\n");
        let f2 = write_temp("py", b"def bar(): pass\n");
        let paths: Vec<&Path> = vec![f1.path(), f2.path()];
        let result = extract(&paths, None).unwrap();
        // Should have nodes from both files.
        assert!(result.nodes.len() >= 2);
    }
}
