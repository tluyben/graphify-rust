//! File discovery and classification for the graphify knowledge-graph pipeline.
//!
//! Ported from the Python `detect.py` module. The key responsibilities are:
//!
//! * Classify individual files by extension (and, for `.txt` / `.md`, by
//!   content heuristics for papers).
//! * Walk a directory tree respecting `.graphifyignore` files (gitignore-style)
//!   and a fixed set of always-skipped directories and files.
//! * Count words in text-like files.
//! * Maintain an incremental manifest so only changed files are re-processed.

use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::RegexSet;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// ---------------------------------------------------------------------------
// Public extension-list constants
// (kept as const slices for backwards-compat with analyze.rs which imports them)
// ---------------------------------------------------------------------------

/// Source-code file extensions (lowercased, dot-prefixed).
pub const CODE_EXTENSIONS: &[&str] = &[
    ".py", ".ts", ".js", ".jsx", ".tsx", ".mjs", ".ejs", ".go", ".rs", ".java",
    ".cpp", ".cc", ".cxx", ".c", ".h", ".hpp", ".rb", ".swift", ".kt", ".kts",
    ".cs", ".scala", ".php", ".lua", ".toc", ".zig", ".ps1", ".ex", ".exs",
    ".m", ".mm", ".jl", ".vue", ".svelte", ".dart", ".v", ".sv", ".sql", ".r",
];

/// Academic paper / document formats.
pub const PAPER_EXTENSIONS: &[&str] = &[".pdf"];

/// Image file extensions.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg",
];

/// Document file extensions.
pub const DOC_EXTENSIONS: &[&str] = &[
    ".md", ".mdx", ".txt", ".rst", ".html", ".yaml", ".yml",
];

/// Video file extensions.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    ".mp4", ".mov", ".webm", ".mkv", ".avi", ".m4v", ".mp3", ".wav", ".m4a", ".ogg",
];

// ---------------------------------------------------------------------------
// FileType
// ---------------------------------------------------------------------------

/// Classification of a file according to its content category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileType {
    Code,
    Document,
    Paper,
    Image,
    Video,
}

impl FileType {
    /// The string key used in [`DetectResult::files`].
    pub fn as_key(self) -> &'static str {
        match self {
            FileType::Code => "code",
            FileType::Document => "document",
            FileType::Paper => "paper",
            FileType::Image => "image",
            FileType::Video => "video",
        }
    }
}

// ---------------------------------------------------------------------------
// Extension sets (lowercase, with leading dot) – backed by the public consts
// ---------------------------------------------------------------------------

fn code_extensions() -> HashSet<&'static str> {
    CODE_EXTENSIONS.iter().copied().collect()
}

fn doc_extensions() -> HashSet<&'static str> {
    DOC_EXTENSIONS.iter().copied().collect()
}

fn paper_extensions() -> HashSet<&'static str> {
    PAPER_EXTENSIONS.iter().copied().collect()
}

fn image_extensions() -> HashSet<&'static str> {
    IMAGE_EXTENSIONS.iter().copied().collect()
}

fn video_extensions() -> HashSet<&'static str> {
    VIDEO_EXTENSIONS.iter().copied().collect()
}

// ---------------------------------------------------------------------------
// Skip lists
// ---------------------------------------------------------------------------

fn skipped_dirs() -> HashSet<&'static str> {
    [
        "venv",
        ".venv",
        "env",
        ".env",
        "node_modules",
        "__pycache__",
        ".git",
        "dist",
        "build",
        "target",
        "out",
        "site-packages",
        "lib64",
        ".pytest_cache",
        ".mypy_cache",
        ".ruff_cache",
        ".tox",
        ".eggs",
        "graphify-out",
    ]
    .iter()
    .copied()
    .collect()
}

fn skipped_files() -> HashSet<&'static str> {
    [
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "Cargo.lock",
        "poetry.lock",
        "Gemfile.lock",
        "composer.lock",
        "go.sum",
        "go.work.sum",
    ]
    .iter()
    .copied()
    .collect()
}

// ---------------------------------------------------------------------------
// Sensitive-file detection (applied to file-name, not full path)
// ---------------------------------------------------------------------------

/// Return `true` if `filename` (just the name, no directory) is considered
/// sensitive and should be skipped regardless of extension.
fn is_sensitive_file(filename: &str) -> bool {
    let lower = filename.to_ascii_lowercase();

    // Exact-name matches (env files).
    if lower == ".env" || lower == ".envrc" {
        return true;
    }

    // Extension-based matches for credential material.
    for ext in &[".pem", ".key", ".p12", ".pfx", ".cert", ".crt"] {
        if lower.ends_with(ext) {
            return true;
        }
    }

    // Substring matches (credential/secret/passwd/password/token/private_key).
    let keywords = [
        "credential",
        "secret",
        "passwd",
        "password",
        "token",
        "private_key",
    ];
    for kw in &keywords {
        if lower.contains(kw) {
            return true;
        }
    }

    false
}

// ---------------------------------------------------------------------------
// Paper heuristic
// ---------------------------------------------------------------------------

/// Number of paper-signal patterns that must match for a document to be
/// re-classified as PAPER.
const PAPER_SIGNAL_THRESHOLD: usize = 3;

/// Bytes of file content to read for the paper heuristic.
const PAPER_SCAN_BYTES: usize = 3000;

/// Lazily-built regex set for paper signals.
fn paper_regex_set() -> &'static RegexSet {
    use std::sync::OnceLock;
    static SET: OnceLock<RegexSet> = OnceLock::new();
    SET.get_or_init(|| {
        RegexSet::new([
            r"(?i)\barxiv\b",
            r"(?i)\bdoi\s*:",
            r"(?i)\babstract\b",
            r"(?i)\bproceedings\b",
            r"(?i)\bjournal\b",
            r"(?i)\bpreprint\b",
            r"\\cite\{",
            r"\[\d+\]",
            r"(?i)eq\.\s*\d+|equation\s+\d+",
            r"\d{4}\.\d{4,5}",
            r"(?i)\bwe propose\b",
            r"(?i)\bliterature\b",
        ])
        .expect("paper regex set compilation failed")
    })
}

/// Read the first `PAPER_SCAN_BYTES` of a file (as UTF-8, lossy) and return
/// `true` if ≥ `PAPER_SIGNAL_THRESHOLD` paper-signal patterns match.
fn looks_like_paper(path: &Path) -> bool {
    let raw = match fs::read(path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let slice = if raw.len() > PAPER_SCAN_BYTES {
        &raw[..PAPER_SCAN_BYTES]
    } else {
        &raw
    };
    let text = String::from_utf8_lossy(slice);
    let matches = paper_regex_set().matches(&*text);
    matches.iter().count() >= PAPER_SIGNAL_THRESHOLD
}

// ---------------------------------------------------------------------------
// File classification
// ---------------------------------------------------------------------------

/// Classify a single file by extension and, for ambiguous document types, by
/// content heuristics.
///
/// Returns `None` for file types that should be ignored (e.g. office
/// documents, lockfiles, videos without a [`FileType`] variant — actually
/// videos do have one).
pub fn classify_file(path: &Path) -> Option<FileType> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_ascii_lowercase()))
        .unwrap_or_default();

    if paper_extensions().contains(ext.as_str()) {
        return Some(FileType::Paper);
    }
    if image_extensions().contains(ext.as_str()) {
        return Some(FileType::Image);
    }
    if video_extensions().contains(ext.as_str()) {
        return Some(FileType::Video);
    }
    if code_extensions().contains(ext.as_str()) {
        return Some(FileType::Code);
    }
    if doc_extensions().contains(ext.as_str()) {
        // Apply paper heuristic for .txt and .md files.
        if ext == ".txt" || ext == ".md" || ext == ".mdx" || ext == ".rst" {
            if looks_like_paper(path) {
                return Some(FileType::Paper);
            }
        }
        return Some(FileType::Document);
    }

    // Office and other unrecognised formats are silently ignored.
    None
}

// ---------------------------------------------------------------------------
// Word counting
// ---------------------------------------------------------------------------

/// Count whitespace-delimited words in a file.
///
/// Non-UTF-8 bytes are replaced with the Unicode replacement character.
/// Returns 0 on I/O error.
///
/// Mirrors Python's `count_words(path)`.
pub fn count_words(path: &Path) -> usize {
    let f = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return 0,
    };
    let reader = io::BufReader::new(f);
    let mut count = 0usize;
    for line in reader.lines() {
        if let Ok(l) = line {
            count += l.split_whitespace().count();
        }
    }
    count
}

// ---------------------------------------------------------------------------
// .graphifyignore support
// ---------------------------------------------------------------------------

/// A single `.graphifyignore` rule.
#[derive(Debug, Clone)]
#[allow(dead_code)] // `anchored` is computed and stored for documentation/future use
struct IgnoreRule {
    /// Whether this is a negation rule (starts with `!`).
    negated: bool,
    /// The compiled glob matcher.
    glob: GlobSet,
    /// The raw pattern (without leading `!`), used for anchoring logic.
    raw_pattern: String,
    /// `true` if the pattern is anchored to the root (contains `/` other than
    /// a possible leading or trailing `/`).
    anchored: bool,
}

/// A stack of ignore rules loaded from one or more `.graphifyignore` files,
/// outer-to-inner.
#[derive(Debug, Default, Clone)]
pub struct IgnoreRules {
    rules: Vec<IgnoreRule>,
}

impl IgnoreRules {
    /// Load and append rules from `ignore_file`.  Outer files should be loaded
    /// before inner files so last-match-wins ordering is respected.
    pub fn load_file(&mut self, ignore_file: &Path) {
        let content = match fs::read_to_string(ignore_file) {
            Ok(c) => c,
            Err(_) => return,
        };
        for line in content.lines() {
            self.add_pattern(line);
        }
    }

    /// Parse and add a single gitignore-style pattern line.
    pub fn add_pattern(&mut self, line: &str) {
        let line = line.trim_end();
        // Blank lines and comments.
        if line.is_empty() || line.starts_with('#') {
            return;
        }

        let negated = line.starts_with('!');
        let raw = if negated { &line[1..] } else { line };

        // A pattern is anchored when it contains `/` anywhere except possibly
        // at the end (a trailing `/` means "directory only").
        let without_trailing_slash = raw.trim_end_matches('/');
        let anchored = without_trailing_slash.contains('/');

        // Build the globset for this pattern.  We convert the gitignore
        // pattern to a globset pattern that can match full paths.
        let mut builder = GlobSetBuilder::new();

        // Strip leading `/` for anchored patterns – globset matches against
        // the full relative path, so we don't need it.
        let pat = raw.trim_start_matches('/').trim_end_matches('/');

        // Match the pattern directly (covers cases like `*.log`).
        if let Ok(g) = Glob::new(pat) {
            builder.add(g);
        }
        // Also match as a suffix so that `*.log` matches `sub/dir/foo.log`.
        if !anchored {
            let suffix_pat = format!("**/{pat}");
            if let Ok(g) = Glob::new(&suffix_pat) {
                builder.add(g);
            }
        }
        // Match as a directory prefix so that `build/` matches `build/foo`.
        {
            let dir_pat = format!("{pat}/**");
            if let Ok(g) = Glob::new(&dir_pat) {
                builder.add(g);
            }
        }

        if let Ok(glob) = builder.build() {
            self.rules.push(IgnoreRule {
                negated,
                glob,
                raw_pattern: raw.to_owned(),
                anchored,
            });
        }
    }

    /// Return `true` if `rel_path` (relative to the scan root) should be
    /// ignored.  Uses last-match-wins semantics identical to gitignore.
    pub fn is_ignored(&self, rel_path: &Path) -> bool {
        let path_str = rel_path.to_string_lossy();
        // Normalise separators to `/` for matching.
        let path_str = path_str.replace('\\', "/");
        let path_str: &str = &path_str;

        let mut ignored = false;
        for rule in &self.rules {
            if rule.glob.is_match(path_str) {
                ignored = !rule.negated;
            }
        }
        ignored
    }
}

/// Walk from `scan_root` upward to find a VCS root (`.git`, `.hg`, `.svn`).
/// Returns the VCS root path, or `None` if not found.
fn find_vcs_root(scan_root: &Path) -> Option<PathBuf> {
    let mut current = scan_root.to_path_buf();
    loop {
        for marker in &[".git", ".hg", ".svn"] {
            if current.join(marker).exists() {
                return Some(current.clone());
            }
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Load `.graphifyignore` files from VCS root down to `scan_root`, returning
/// the merged ignore rules (outer files first so inner files win).
///
/// Mirrors the Python implementation's outer→inner loading order.
pub fn load_graphifyignore(scan_root: &Path) -> IgnoreRules {
    let mut rules = IgnoreRules::default();

    let vcs_root = find_vcs_root(scan_root);
    let stop_at = vcs_root.as_deref().unwrap_or(scan_root);

    // Build a list of directories from stop_at down to scan_root.
    let mut ancestors: Vec<PathBuf> = Vec::new();
    {
        let canonical_scan = scan_root
            .canonicalize()
            .unwrap_or_else(|_| scan_root.to_path_buf());
        let canonical_stop = stop_at
            .canonicalize()
            .unwrap_or_else(|_| stop_at.to_path_buf());

        let mut current = canonical_scan.clone();
        loop {
            ancestors.push(current.clone());
            if current == canonical_stop {
                break;
            }
            if !current.pop() {
                break;
            }
        }
    }
    // Reverse so outer (higher) directories come first.
    ancestors.reverse();

    for dir in &ancestors {
        let ignore_file = dir.join(".graphifyignore");
        if ignore_file.is_file() {
            rules.load_file(&ignore_file);
        }
    }

    rules
}

// ---------------------------------------------------------------------------
// Manifest (for incremental detection)
// ---------------------------------------------------------------------------

/// An entry in the file manifest, recording the hash and classification of a
/// previously-seen file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// SHA-256 hex digest of the file at the time it was last processed.
    pub hash: String,
    /// The file's classification.
    pub file_type: String,
}

/// Load a manifest JSON file (maps relative-path strings → [`ManifestEntry`]).
///
/// Returns an empty map on any I/O or parse error.
///
/// Mirrors Python's `load_manifest(path)`.
pub fn load_manifest(path: &Path) -> HashMap<String, ManifestEntry> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return HashMap::new(),
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Write a manifest JSON file mapping relative-path strings to lists of paths
/// (compatible with the Python side which stores `{kind: [paths…]}`).
///
/// Mirrors Python's `save_manifest(files, path)`.
pub fn save_manifest(files: &HashMap<String, Vec<String>>, path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(files)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    fs::write(path, json)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// DetectResult
// ---------------------------------------------------------------------------

/// Threshold (in words) above which a warning is emitted about corpus size.
const CORPUS_WARN_THRESHOLD: usize = 50_000;

/// The result returned by [`detect`] and [`detect_incremental`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetectResult {
    /// Files grouped by [`FileType`] key (`"code"`, `"document"`, …).
    pub files: HashMap<String, Vec<String>>,
    /// Total number of files discovered (across all categories).
    pub total_files: usize,
    /// Total word count across all text-like files.
    pub total_words: usize,
    /// `true` when the corpus is large enough to warrant graph extraction.
    pub needs_graph: bool,
    /// Optional warning (e.g. when the corpus exceeds the word threshold).
    pub warning: Option<String>,
    /// Number of sensitive files that were skipped.
    pub skipped_sensitive: usize,
    /// The raw `.graphifyignore` patterns that were active during this scan.
    pub graphifyignore_patterns: Vec<String>,
}

// ---------------------------------------------------------------------------
// Core detection logic
// ---------------------------------------------------------------------------

/// Discover all processable files under `root`.
///
/// `follow_symlinks` controls whether symbolic links to directories are
/// followed during the walk.
///
/// Mirrors Python's `detect(root, follow_symlinks)`.
pub fn detect(root: &Path, follow_symlinks: bool) -> DetectResult {
    let root = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf());

    let ignore_rules = load_graphifyignore(&root);

    // Collect the raw patterns for the result.
    let graphifyignore_patterns: Vec<String> = ignore_rules
        .rules
        .iter()
        .map(|r| {
            if r.negated {
                format!("!{}", r.raw_pattern)
            } else {
                r.raw_pattern.clone()
            }
        })
        .collect();

    let skip_dirs = skipped_dirs();
    let skip_files = skipped_files();

    let mut files: HashMap<String, Vec<String>> = {
        let mut m = HashMap::new();
        for key in &["code", "document", "paper", "image", "video"] {
            m.insert(key.to_string(), Vec::new());
        }
        m
    };
    let mut total_words = 0usize;
    let mut skipped_sensitive = 0usize;

    let walker = WalkDir::new(&root)
        .follow_links(follow_symlinks)
        .sort_by_file_name();

    let mut it = walker.into_iter();
    loop {
        let entry = match it.next() {
            None => break,
            Some(Err(_)) => continue,
            Some(Ok(e)) => e,
        };

        let entry_path = entry.path();
        let file_name = entry
            .file_name()
            .to_string_lossy()
            .into_owned();

        if entry.file_type().is_dir() {
            // Skip the root dir itself (depth == 0) through to its children.
            if entry.depth() == 0 {
                continue;
            }
            // Skip hidden directories (leading dot).
            if file_name.starts_with('.') && file_name != ".graphifyignore" {
                it.skip_current_dir();
                continue;
            }
            // Skip well-known noise directories.
            if skip_dirs.contains(file_name.as_str()) {
                it.skip_current_dir();
                continue;
            }
            // Skip directories ignored by .graphifyignore.
            if let Ok(rel) = entry_path.strip_prefix(&root) {
                if ignore_rules.is_ignored(rel) {
                    it.skip_current_dir();
                    continue;
                }
            }
            continue;
        }

        if !entry.file_type().is_file() {
            continue;
        }

        // For files: check sensitivity FIRST (counts toward skipped_sensitive),
        // then hidden-file check (silent skip), then lockfiles/ignore rules.
        if is_sensitive_file(&file_name) {
            skipped_sensitive += 1;
            continue;
        }

        // Skip hidden files (leading dot, excluding .graphifyignore itself).
        if entry.depth() > 0
            && file_name.starts_with('.')
            && file_name != ".graphifyignore"
        {
            continue;
        }

        // Skip lockfiles and other always-skipped files.
        if skip_files.contains(file_name.as_str()) {
            continue;
        }

        // Check .graphifyignore rules.
        if let Ok(rel) = entry_path.strip_prefix(&root) {
            if ignore_rules.is_ignored(rel) {
                continue;
            }
        }

        // Classify the file.
        let ft = match classify_file(entry_path) {
            Some(ft) => ft,
            None => continue,
        };

        let path_str = entry_path.to_string_lossy().into_owned();

        // Count words for text-like files.
        match ft {
            FileType::Code | FileType::Document | FileType::Paper => {
                total_words += count_words(entry_path);
            }
            _ => {}
        }

        files
            .entry(ft.as_key().to_string())
            .or_default()
            .push(path_str);
    }

    let total_files: usize = files.values().map(|v| v.len()).sum();
    let needs_graph = total_files > 0;

    let warning = if total_words > CORPUS_WARN_THRESHOLD {
        Some(format!(
            "Large corpus detected ({total_words} words). \
             Graph extraction may be slow or expensive."
        ))
    } else {
        None
    };

    DetectResult {
        files,
        total_files,
        total_words,
        needs_graph,
        warning,
        skipped_sensitive,
        graphifyignore_patterns,
    }
}

// ---------------------------------------------------------------------------
// Incremental detection
// ---------------------------------------------------------------------------

/// Hash a file using SHA-256 (raw content, no path component).
///
/// Returns an empty string on I/O error.
fn quick_hash(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(_) => return String::new(),
    };
    let mut h = Sha256::new();
    h.update(&data);
    hex::encode(h.finalize())
}

/// Perform an incremental file discovery, using `manifest_path` to skip files
/// that have not changed since the last run.
///
/// The manifest maps relative path strings to [`ManifestEntry`] structs.
/// Files present in the manifest with a matching hash are included in the
/// result without re-classifying; changed or new files are classified
/// normally.
///
/// Mirrors Python's `detect_incremental(root, manifest_path)`.
pub fn detect_incremental(root: &Path, manifest_path: &Path) -> DetectResult {
    let manifest = load_manifest(manifest_path);
    let root = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf());

    let ignore_rules = load_graphifyignore(&root);
    let graphifyignore_patterns: Vec<String> = ignore_rules
        .rules
        .iter()
        .map(|r| {
            if r.negated {
                format!("!{}", r.raw_pattern)
            } else {
                r.raw_pattern.clone()
            }
        })
        .collect();

    let skip_dirs = skipped_dirs();
    let skip_files = skipped_files();

    let mut files: HashMap<String, Vec<String>> = {
        let mut m = HashMap::new();
        for key in &["code", "document", "paper", "image", "video"] {
            m.insert(key.to_string(), Vec::new());
        }
        m
    };
    let mut total_words = 0usize;
    let mut skipped_sensitive = 0usize;

    let walker = WalkDir::new(&root)
        .follow_links(false)
        .sort_by_file_name();

    let mut it = walker.into_iter();
    loop {
        let entry = match it.next() {
            None => break,
            Some(Err(_)) => continue,
            Some(Ok(e)) => e,
        };

        let entry_path = entry.path();
        let file_name = entry
            .file_name()
            .to_string_lossy()
            .into_owned();

        if entry.file_type().is_dir() {
            if entry.depth() == 0 {
                continue;
            }
            if file_name.starts_with('.') && file_name != ".graphifyignore" {
                it.skip_current_dir();
                continue;
            }
            if skip_dirs.contains(file_name.as_str()) {
                it.skip_current_dir();
                continue;
            }
            if let Ok(rel) = entry_path.strip_prefix(&root) {
                if ignore_rules.is_ignored(rel) {
                    it.skip_current_dir();
                    continue;
                }
            }
            continue;
        }

        if !entry.file_type().is_file() {
            continue;
        }

        // Sensitivity check before hidden-file check (so .env counts as skipped_sensitive).
        if is_sensitive_file(&file_name) {
            skipped_sensitive += 1;
            continue;
        }

        if entry.depth() > 0
            && file_name.starts_with('.')
            && file_name != ".graphifyignore"
        {
            continue;
        }

        if skip_files.contains(file_name.as_str()) {
            continue;
        }

        if let Ok(rel) = entry_path.strip_prefix(&root) {
            if ignore_rules.is_ignored(rel) {
                continue;
            }
        }

        // Check manifest for an unchanged file.
        let rel_key = entry_path
            .strip_prefix(&root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| entry_path.to_string_lossy().replace('\\', "/"));

        let path_str = entry_path.to_string_lossy().into_owned();

        if let Some(manifest_entry) = manifest.get(&rel_key) {
            let current_hash = quick_hash(entry_path);
            if current_hash == manifest_entry.hash && !current_hash.is_empty() {
                // File unchanged – use the cached classification.
                let ft_key = &manifest_entry.file_type;
                files.entry(ft_key.clone()).or_default().push(path_str);
                // Still count words for text files.
                match ft_key.as_str() {
                    "code" | "document" | "paper" => {
                        total_words += count_words(entry_path);
                    }
                    _ => {}
                }
                continue;
            }
        }

        // New or changed file – classify normally.
        let ft = match classify_file(entry_path) {
            Some(ft) => ft,
            None => continue,
        };

        match ft {
            FileType::Code | FileType::Document | FileType::Paper => {
                total_words += count_words(entry_path);
            }
            _ => {}
        }

        files
            .entry(ft.as_key().to_string())
            .or_default()
            .push(path_str);
    }

    let total_files: usize = files.values().map(|v| v.len()).sum();
    let needs_graph = total_files > 0;
    let warning = if total_words > CORPUS_WARN_THRESHOLD {
        Some(format!(
            "Large corpus detected ({total_words} words). \
             Graph extraction may be slow or expensive."
        ))
    } else {
        None
    };

    DetectResult {
        files,
        total_files,
        total_words,
        needs_graph,
        warning,
        skipped_sensitive,
        graphifyignore_patterns,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_file(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, content).unwrap();
        p
    }

    // ------------------------------------------------------------------
    // classify_file
    // ------------------------------------------------------------------

    #[test]
    fn classify_rust_file() {
        let tmp = TempDir::new().unwrap();
        let p = make_file(tmp.path(), "main.rs", b"fn main() {}");
        assert_eq!(classify_file(&p), Some(FileType::Code));
    }

    #[test]
    fn classify_python_file() {
        let tmp = TempDir::new().unwrap();
        let p = make_file(tmp.path(), "script.py", b"print('hi')");
        assert_eq!(classify_file(&p), Some(FileType::Code));
    }

    #[test]
    fn classify_markdown_document() {
        let tmp = TempDir::new().unwrap();
        let p = make_file(tmp.path(), "README.md", b"# Hello\nThis is a readme.");
        assert_eq!(classify_file(&p), Some(FileType::Document));
    }

    #[test]
    fn classify_pdf_as_paper() {
        let tmp = TempDir::new().unwrap();
        let p = make_file(tmp.path(), "paper.pdf", b"%PDF-1.4 binary content");
        assert_eq!(classify_file(&p), Some(FileType::Paper));
    }

    #[test]
    fn classify_png_as_image() {
        let tmp = TempDir::new().unwrap();
        let p = make_file(tmp.path(), "logo.png", b"\x89PNG\r\n\x1a\n");
        assert_eq!(classify_file(&p), Some(FileType::Image));
    }

    #[test]
    fn classify_mp4_as_video() {
        let tmp = TempDir::new().unwrap();
        let p = make_file(tmp.path(), "clip.mp4", b"\x00\x00\x00\x18ftyp");
        assert_eq!(classify_file(&p), Some(FileType::Video));
    }

    #[test]
    fn classify_unknown_extension_returns_none() {
        let tmp = TempDir::new().unwrap();
        let p = make_file(tmp.path(), "data.xyz", b"random");
        assert_eq!(classify_file(&p), None);
    }

    #[test]
    fn txt_file_with_paper_signals_classified_as_paper() {
        let tmp = TempDir::new().unwrap();
        let content = b"Abstract\n\
                        This paper appears in Proceedings of the Journal of AI.\n\
                        ArXiv preprint 2310.12345. We propose a new method.\n\
                        See [1] for literature review. doi: 10.1234/xyz";
        let p = make_file(tmp.path(), "paper.txt", content);
        assert_eq!(classify_file(&p), Some(FileType::Paper));
    }

    #[test]
    fn txt_file_without_paper_signals_classified_as_document() {
        let tmp = TempDir::new().unwrap();
        let p = make_file(tmp.path(), "notes.txt", b"Just some notes about the project.");
        assert_eq!(classify_file(&p), Some(FileType::Document));
    }

    // ------------------------------------------------------------------
    // count_words
    // ------------------------------------------------------------------

    #[test]
    fn count_words_basic() {
        let tmp = TempDir::new().unwrap();
        let p = make_file(tmp.path(), "f.txt", b"one two three\nfour five");
        assert_eq!(count_words(&p), 5);
    }

    #[test]
    fn count_words_empty_file() {
        let tmp = TempDir::new().unwrap();
        let p = make_file(tmp.path(), "empty.txt", b"");
        assert_eq!(count_words(&p), 0);
    }

    // ------------------------------------------------------------------
    // is_sensitive_file
    // ------------------------------------------------------------------

    #[test]
    fn sensitive_env_files() {
        assert!(is_sensitive_file(".env"));
        assert!(is_sensitive_file(".envrc"));
    }

    #[test]
    fn sensitive_key_files() {
        assert!(is_sensitive_file("server.pem"));
        assert!(is_sensitive_file("id_rsa.key"));
        assert!(is_sensitive_file("cert.crt"));
    }

    #[test]
    fn sensitive_keyword_files() {
        assert!(is_sensitive_file("aws_credentials.json"));
        assert!(is_sensitive_file("secrets.yaml"));
        assert!(is_sensitive_file("db_password.txt"));
        assert!(is_sensitive_file("auth_token.env"));
        assert!(is_sensitive_file("private_key.pem"));
    }

    #[test]
    fn non_sensitive_file() {
        assert!(!is_sensitive_file("main.rs"));
        assert!(!is_sensitive_file("README.md"));
    }

    // ------------------------------------------------------------------
    // IgnoreRules
    // ------------------------------------------------------------------

    #[test]
    fn ignore_rules_star_glob() {
        let mut rules = IgnoreRules::default();
        rules.add_pattern("*.log");
        assert!(rules.is_ignored(Path::new("app.log")));
        assert!(rules.is_ignored(Path::new("sub/dir/app.log")));
        assert!(!rules.is_ignored(Path::new("main.rs")));
    }

    #[test]
    fn ignore_rules_negation() {
        let mut rules = IgnoreRules::default();
        rules.add_pattern("*.log");
        rules.add_pattern("!important.log");
        assert!(!rules.is_ignored(Path::new("important.log")));
        assert!(rules.is_ignored(Path::new("debug.log")));
    }

    #[test]
    fn ignore_rules_blank_and_comment_lines_skipped() {
        let mut rules = IgnoreRules::default();
        rules.add_pattern("# this is a comment");
        rules.add_pattern("");
        rules.add_pattern("   ");
        // No rules should have been added.
        assert!(!rules.is_ignored(Path::new("anything.rs")));
    }

    // ------------------------------------------------------------------
    // detect
    // ------------------------------------------------------------------

    #[test]
    fn detect_finds_code_and_documents() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "main.rs", b"fn main() {}");
        make_file(tmp.path(), "README.md", b"# Hello");
        let result = detect(tmp.path(), false);
        assert_eq!(result.files["code"].len(), 1);
        assert_eq!(result.files["document"].len(), 1);
        assert_eq!(result.total_files, 2);
        assert!(result.needs_graph);
    }

    #[test]
    fn detect_skips_node_modules() {
        let tmp = TempDir::new().unwrap();
        let nm = tmp.path().join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        make_file(&nm, "index.js", b"module.exports = {};");
        make_file(tmp.path(), "app.js", b"const x = 1;");
        let result = detect(tmp.path(), false);
        // Only app.js should be found, not node_modules/index.js.
        assert_eq!(result.files["code"].len(), 1);
        assert!(result.files["code"][0].ends_with("app.js"));
    }

    #[test]
    fn detect_skips_lockfiles() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "Cargo.lock", b"[metadata]\n");
        make_file(tmp.path(), "main.rs", b"fn main() {}");
        let result = detect(tmp.path(), false);
        assert_eq!(result.files["code"].len(), 1);
    }

    #[test]
    fn detect_skips_sensitive_files() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), ".env", b"SECRET=abc");
        make_file(tmp.path(), "main.rs", b"fn main() {}");
        let result = detect(tmp.path(), false);
        assert_eq!(result.skipped_sensitive, 1);
        assert_eq!(result.files["code"].len(), 1);
    }

    #[test]
    fn detect_respects_graphifyignore() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), ".graphifyignore", b"*.log\n");
        make_file(tmp.path(), "debug.log", b"error\n");
        make_file(tmp.path(), "main.rs", b"fn main() {}");
        // .log files have no recognised extension so they'd be None anyway,
        // but this still exercises the ignore-rule path.
        let result = detect(tmp.path(), false);
        assert_eq!(result.files["code"].len(), 1);
    }

    #[test]
    fn detect_incremental_uses_manifest_for_unchanged_files() {
        let tmp = TempDir::new().unwrap();
        let src = make_file(tmp.path(), "main.rs", b"fn main() {}");

        // Run a full detect first.
        let result1 = detect(tmp.path(), false);
        assert_eq!(result1.total_files, 1);

        // Build a manifest from the result.
        let rel = "main.rs";
        let h = quick_hash(&src);
        let mut manifest: HashMap<String, ManifestEntry> = HashMap::new();
        manifest.insert(
            rel.to_string(),
            ManifestEntry {
                hash: h,
                file_type: "code".to_string(),
            },
        );
        let manifest_path = tmp.path().join("manifest.json");
        fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap()).unwrap();

        let result2 = detect_incremental(tmp.path(), &manifest_path);
        assert_eq!(result2.total_files, 1);
        assert_eq!(result2.files["code"].len(), 1);
    }

    // ------------------------------------------------------------------
    // save_manifest / load_manifest
    // ------------------------------------------------------------------

    #[test]
    fn manifest_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("manifest.json");
        let mut files: HashMap<String, Vec<String>> = HashMap::new();
        files.insert("code".to_string(), vec!["src/main.rs".to_string()]);
        save_manifest(&files, &path).unwrap();
        // load_manifest expects ManifestEntry format; this just checks that
        // save writes valid JSON.
        assert!(path.exists());
        let text = fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(v["code"].is_array());
    }
}
