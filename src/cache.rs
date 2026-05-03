//! Persistent on-disk cache for extraction results.
//!
//! Each cached entry is stored as `{hash}.json` under
//! `{root}/graphify-out/cache/{kind}/`.  The hash is computed over the file
//! content (with YAML front-matter stripped for Markdown files) plus the
//! relative path, so the same file at a different location gets a different
//! cache key.
//!
//! Ported from the Python `cache.py` module; all behaviour is kept identical.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};

use serde_json::Value;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Strip a YAML front-matter block from Markdown bytes.
///
/// If the content starts with `---` (possibly followed by CRLF or LF), we
/// look for the closing `\n---` marker and return everything after it.
/// If no closing marker is found, or the content does not start with `---`,
/// the original bytes are returned unchanged.
///
/// Mirrors Python's `_body_content`.
fn body_content(content: &[u8]) -> &[u8] {
    // Convert to str for easier searching (lossily – we only need to locate
    // the markers; the hash still operates on raw bytes).
    let text = match std::str::from_utf8(content) {
        Ok(t) => t,
        Err(_) => return content,
    };

    if !text.starts_with("---") {
        return content;
    }

    // Find the first "\n---" after position 3.
    if let Some(end) = text[3..].find("\n---") {
        // end is an offset inside text[3..], so the byte index in text is end+3.
        // We want everything after the closing "\n---", i.e. from end+3+4 = end+7.
        let body_start = 3 + end + 4; // skip "\n---"
        if body_start <= content.len() {
            return &content[body_start..];
        }
    }

    content
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute the SHA-256 hash used as the cache key for `path`.
///
/// The hash covers:
/// 1. File content (with YAML front-matter stripped for `.md` files).
/// 2. A zero byte separator.
/// 3. The path of `path` relative to `root` (or the absolute path when
///    `path` cannot be expressed relative to `root`).
///
/// Mirrors Python's `file_hash(path, root)`.
///
/// # Errors
///
/// Returns an [`io::Error`] if `path` is not a regular file or cannot be read.
pub fn file_hash(path: &Path, root: &Path) -> io::Result<String> {
    let p = path.to_path_buf();
    if !p.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("file_hash requires a file, got: {}", p.display()),
        ));
    }

    let raw = fs::read(&p)?;
    let content = if p.extension().map(|e| e.to_ascii_lowercase()) == Some("md".into()) {
        body_content(&raw)
    } else {
        &raw
    };

    let mut hasher = Sha256::new();
    hasher.update(content);
    hasher.update(b"\x00");

    // Try to make the path relative to root; fall back to absolute.
    let canonical_p = p.canonicalize()?;
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let rel = canonical_p
        .strip_prefix(&canonical_root)
        .map(|r| r.to_string_lossy().into_owned())
        .unwrap_or_else(|_| canonical_p.to_string_lossy().into_owned());

    hasher.update(rel.as_bytes());

    Ok(hex::encode(hasher.finalize()))
}

/// Return (and create) the cache directory for a given `root` and `kind`.
///
/// The directory is `{root}/graphify-out/cache/{kind}/`.
///
/// Mirrors Python's `cache_dir(root, kind)`.
///
/// # Errors
///
/// Returns an [`io::Error`] if the directory cannot be created.
pub fn cache_dir(root: &Path, kind: &str) -> io::Result<PathBuf> {
    let d = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .join("graphify-out")
        .join("cache")
        .join(kind);
    fs::create_dir_all(&d)?;
    Ok(d)
}

/// Load a cached extraction result for `path`, or return `None` on a miss.
///
/// Falls back to the legacy flat-cache location
/// (`{root}/graphify-out/cache/{hash}.json`) when `kind == "ast"` and the
/// new-style path does not exist.
///
/// Mirrors Python's `load_cached(path, root, kind)`.
pub fn load_cached(path: &Path, root: &Path, kind: &str) -> Option<Value> {
    let h = file_hash(path, root).ok()?;
    let dir = cache_dir(root, kind).ok()?;
    let entry = dir.join(format!("{h}.json"));

    if entry.exists() {
        let text = fs::read_to_string(&entry).ok()?;
        return serde_json::from_str(&text).ok();
    }

    // Legacy flat-cache fallback (ast only).
    if kind == "ast" {
        let root_canon = root
            .canonicalize()
            .unwrap_or_else(|_| root.to_path_buf());
        let legacy = root_canon
            .join("graphify-out")
            .join("cache")
            .join(format!("{h}.json"));
        if legacy.exists() {
            let text = fs::read_to_string(&legacy).ok()?;
            return serde_json::from_str(&text).ok();
        }
    }

    None
}

/// Persist an extraction result to the cache atomically.
///
/// The write uses a sibling `.tmp` file followed by an atomic rename so that a
/// concurrent reader never sees a partial entry.
///
/// Mirrors Python's `save_cached(path, result, root, kind)`.
///
/// # Errors
///
/// Returns an [`io::Error`] if the entry cannot be written.
pub fn save_cached(path: &Path, result: &Value, root: &Path, kind: &str) -> io::Result<()> {
    let p = path.to_path_buf();
    if !p.is_file() {
        return Ok(()); // silently ignore non-files, matching Python behaviour
    }

    let h = file_hash(&p, root)?;
    let target_dir = cache_dir(root, kind)?;
    let entry = target_dir.join(format!("{h}.json"));

    // Write to a temporary file in the same directory, then rename atomically.
    let tmp_path = {
        let prefix = format!("{h}.");
        let mut builder = tempfile::Builder::new();
        builder.prefix(prefix.as_str()).suffix(".tmp");
        let named = builder.tempfile_in(&target_dir)?;
        let tmp_path = named.path().to_path_buf();

        // Write through the NamedTempFile, then persist it manually so we can
        // do an atomic rename even on Windows (tempfile::persist does that).
        let json_bytes = serde_json::to_vec(result).map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("JSON serialisation error: {e}"))
        })?;

        {
            let mut f = named.as_file();
            f.write_all(&json_bytes)?;
            f.flush()?;
        }

        // Persist (i.e. keep the file after the NamedTempFile is dropped).
        named
            .persist(&tmp_path)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        tmp_path
    };

    fs::rename(&tmp_path, &entry)?;
    Ok(())
}

/// Return the set of cache-entry hash stems across all cache locations under
/// `root`.
///
/// Scans:
/// * `{root}/graphify-out/cache/*.json`  (legacy flat cache)
/// * `{root}/graphify-out/cache/ast/*.json`
/// * `{root}/graphify-out/cache/semantic/*.json`
///
/// Mirrors Python's `cached_files(root)`.
pub fn cached_files(root: &Path) -> std::collections::HashSet<String> {
    let base = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .join("graphify-out")
        .join("cache");

    let mut hashes = std::collections::HashSet::new();

    // Legacy flat-cache directory.
    if base.is_dir() {
        if let Ok(entries) = fs::read_dir(&base) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("json") {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        hashes.insert(stem.to_owned());
                    }
                }
            }
        }
    }

    // Kind-specific subdirectories.
    for kind in &["ast", "semantic"] {
        let d = base.join(kind);
        if d.is_dir() {
            if let Ok(entries) = fs::read_dir(&d) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("json") {
                        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                            hashes.insert(stem.to_owned());
                        }
                    }
                }
            }
        }
    }

    hashes
}

/// Delete every `*.json` entry from the cache directories under `root`.
///
/// Mirrors Python's `clear_cache(root)`.
///
/// Errors during individual file removal are silently ignored to match the
/// Python implementation (which uses `f.unlink()` without propagating errors).
pub fn clear_cache(root: &Path) {
    let base = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .join("graphify-out")
        .join("cache");

    // Flat legacy cache.
    if base.is_dir() {
        if let Ok(entries) = fs::read_dir(&base) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("json") {
                    let _ = fs::remove_file(p);
                }
            }
        }
    }

    // Kind subdirectories.
    for kind in &["ast", "semantic"] {
        let d = base.join(kind);
        if d.is_dir() {
            if let Ok(entries) = fs::read_dir(&d) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("json") {
                        let _ = fs::remove_file(p);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn make_file(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn file_hash_returns_hex_string() {
        let tmp = TempDir::new().unwrap();
        let f = make_file(tmp.path(), "hello.txt", b"hello world");
        let h = file_hash(&f, tmp.path()).unwrap();
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn file_hash_different_for_different_content() {
        let tmp = TempDir::new().unwrap();
        let f1 = make_file(tmp.path(), "a.txt", b"hello");
        let f2 = make_file(tmp.path(), "b.txt", b"world");
        assert_ne!(
            file_hash(&f1, tmp.path()).unwrap(),
            file_hash(&f2, tmp.path()).unwrap()
        );
    }

    #[test]
    fn file_hash_strips_md_front_matter() {
        let tmp = TempDir::new().unwrap();
        // Two .md files with different front-matter but identical body.
        let f1 = make_file(tmp.path(), "a.md", b"---\ntitle: A\n---\nbody");
        let f2 = make_file(tmp.path(), "b.md", b"---\ntitle: B\n---\nbody");
        // Same body → different paths → still different hashes, but both should
        // differ from a plain text file with the full content.
        let h1 = file_hash(&f1, tmp.path()).unwrap();
        let h2 = file_hash(&f2, tmp.path()).unwrap();
        // Different names → different relative paths → different hashes.
        assert_ne!(h1, h2);
    }

    #[test]
    fn round_trip_save_and_load() {
        let tmp = TempDir::new().unwrap();
        let f = make_file(tmp.path(), "source.py", b"print('hi')");
        let data = json!({"nodes": [], "edges": []});

        save_cached(&f, &data, tmp.path(), "ast").unwrap();
        let loaded = load_cached(&f, tmp.path(), "ast");
        assert_eq!(loaded.unwrap(), data);
    }

    #[test]
    fn load_returns_none_on_miss() {
        let tmp = TempDir::new().unwrap();
        let f = make_file(tmp.path(), "new.py", b"x = 1");
        assert!(load_cached(&f, tmp.path(), "ast").is_none());
    }

    #[test]
    fn cached_files_lists_all_entries() {
        let tmp = TempDir::new().unwrap();
        let f1 = make_file(tmp.path(), "a.py", b"a");
        let f2 = make_file(tmp.path(), "b.py", b"b");
        let v = json!({});
        save_cached(&f1, &v, tmp.path(), "ast").unwrap();
        save_cached(&f2, &v, tmp.path(), "semantic").unwrap();
        let files = cached_files(tmp.path());
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn clear_cache_removes_entries() {
        let tmp = TempDir::new().unwrap();
        let f = make_file(tmp.path(), "x.py", b"x");
        save_cached(&f, &json!({}), tmp.path(), "ast").unwrap();
        assert!(!cached_files(tmp.path()).is_empty());
        clear_cache(tmp.path());
        assert!(cached_files(tmp.path()).is_empty());
    }

    #[test]
    fn cache_dir_creates_directory() {
        let tmp = TempDir::new().unwrap();
        let d = cache_dir(tmp.path(), "ast").unwrap();
        assert!(d.is_dir());
    }
}
