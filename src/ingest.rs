//! URL ingestion: fetch tweets, arXiv papers, webpages, PDFs, and images,
//! saving them as graphify-ready markdown files.
//!
//! Ported from Python `ingest.py`.

#![allow(dead_code, unused_imports)]

use std::io;
use std::path::{Path, PathBuf};

use chrono::Utc;
use regex::Regex;

use crate::security::{safe_fetch, safe_fetch_text, validate_url, MAX_FETCH_BYTES, MAX_TEXT_BYTES};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Escape a string for embedding in a YAML double-quoted scalar.
fn yaml_str(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
        .replace('\r', " ")
}

/// Turn a URL into a safe filename stem (max 80 chars) with the given suffix.
fn safe_filename(url: &str, suffix: &str) -> String {
    let parsed = url::Url::parse(url).ok();
    let netloc = parsed
        .as_ref()
        .and_then(|u| u.host_str())
        .unwrap_or("");
    let path = parsed
        .as_ref()
        .map(|u| u.path())
        .unwrap_or("");
    let raw = format!("{netloc}{path}");

    let re_non_word = Regex::new(r"[^\w\-]").unwrap();
    let re_multi_under = Regex::new(r"_+").unwrap();

    let replaced = re_non_word.replace_all(&raw, "_");
    let cleaned = replaced.trim_matches('_');
    let collapsed = re_multi_under.replace_all(cleaned, "_");
    let stem: String = collapsed.chars().take(80).collect();
    let stem = stem.trim_matches('_');
    format!("{stem}{suffix}")
}

// ---------------------------------------------------------------------------
// URL type detection
// ---------------------------------------------------------------------------

/// Classify the URL for targeted extraction.
///
/// Returns one of: `"tweet"`, `"arxiv"`, `"github"`, `"youtube"`, `"pdf"`,
/// `"image"`, `"webpage"`.
pub fn detect_url_type(url: &str) -> &'static str {
    let lower = url.to_lowercase();
    if lower.contains("twitter.com") || lower.contains("x.com") {
        return "tweet";
    }
    if lower.contains("arxiv.org") {
        return "arxiv";
    }
    if lower.contains("github.com") {
        return "github";
    }
    if lower.contains("youtube.com") || lower.contains("youtu.be") {
        return "youtube";
    }
    let path = url::Url::parse(url)
        .ok()
        .map(|u| u.path().to_lowercase())
        .unwrap_or_default();
    if path.ends_with(".pdf") {
        return "pdf";
    }
    if path.ends_with(".png")
        || path.ends_with(".jpg")
        || path.ends_with(".jpeg")
        || path.ends_with(".webp")
        || path.ends_with(".gif")
    {
        return "image";
    }
    "webpage"
}

// ---------------------------------------------------------------------------
// HTML helpers
// ---------------------------------------------------------------------------

fn fetch_html(url: &str) -> Result<String, String> {
    safe_fetch_text(url, MAX_TEXT_BYTES, 30).map_err(|e| e.to_string())
}

/// Convert HTML to plain text / minimal markdown.
/// Strips `<script>` and `<style>` blocks first, then strips all remaining tags.
fn html_to_markdown(html: &str) -> String {
    // Strip scripts and styles.
    let re_script = Regex::new(r"(?si)<script[^>]*>.*?</script>").unwrap();
    let re_style = Regex::new(r"(?si)<style[^>]*>.*?</style>").unwrap();
    let stripped = re_script.replace_all(html, " ");
    let stripped = re_style.replace_all(&stripped, " ");

    // Strip all remaining tags.
    let re_tags = Regex::new(r"<[^>]+>").unwrap();
    let text = re_tags.replace_all(&stripped, " ");

    // Collapse whitespace.
    let re_ws = Regex::new(r"\s+").unwrap();
    let collapsed = re_ws.replace_all(text.trim(), " ");

    // Cap at 8000 chars (matches Python fallback).
    let s: String = collapsed.chars().take(8000).collect();
    s
}

// ---------------------------------------------------------------------------
// Fetchers
// ---------------------------------------------------------------------------

/// Fetch a tweet via the oEmbed API. Returns `(markdown_content, filename)`.
fn fetch_tweet(
    url: &str,
    author: Option<&str>,
    contributor: Option<&str>,
) -> Result<(String, String), String> {
    let oembed_url = url.replace("x.com", "twitter.com");
    let quoted = urlencoding_encode(&oembed_url);
    let api_url = format!(
        "https://publish.twitter.com/oembed?url={quoted}&omit_script=true"
    );

    let (tweet_text, tweet_author) = match safe_fetch_text(&api_url, MAX_TEXT_BYTES, 30) {
        Ok(body) => {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&body) {
                let html = data
                    .get("html")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let re_tags = Regex::new(r"<[^>]+>").unwrap();
                let text = re_tags.replace_all(&html, "").trim().to_string();
                let auth = data
                    .get("author_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                (text, auth)
            } else {
                (
                    format!("Tweet at {url} (could not parse oEmbed response)"),
                    "unknown".to_string(),
                )
            }
        }
        Err(_) => (
            format!("Tweet at {url} (could not fetch content)"),
            "unknown".to_string(),
        ),
    };

    let now = Utc::now().to_rfc3339();
    let contributor_str = contributor.or(author).unwrap_or("unknown");

    let content = format!(
        "---\nsource_url: \"{}\"\ntype: tweet\nauthor: \"{}\"\ncaptured_at: {}\ncontributor: \"{}\"\n---\n\n# Tweet by @{}\n\n{}\n\nSource: {}\n",
        yaml_str(url),
        yaml_str(&tweet_author),
        now,
        yaml_str(contributor_str),
        tweet_author,
        tweet_text,
        url,
    );

    let filename = safe_filename(url, ".md");
    Ok((content, filename))
}

/// Fetch a generic webpage. Returns `(markdown_content, filename)`.
fn fetch_webpage(
    url: &str,
    author: Option<&str>,
    contributor: Option<&str>,
) -> Result<(String, String), String> {
    let html = fetch_html(url)?;

    // Extract <title>.
    let re_title = Regex::new(r"(?si)<title[^>]*>(.*?)</title>").unwrap();
    let re_ws = Regex::new(r"\s+").unwrap();
    let title = re_title
        .captures(&html)
        .map(|c| {
            re_ws
                .replace_all(c.get(1).map(|m| m.as_str()).unwrap_or(""), " ")
                .trim()
                .to_string()
        })
        .unwrap_or_else(|| url.to_string());

    let markdown = html_to_markdown(&html);
    let now = Utc::now().to_rfc3339();
    let contributor_str = contributor.or(author).unwrap_or("unknown");

    // Cap markdown at 12000 chars (matches Python).
    let markdown_capped: String = markdown.chars().take(12000).collect();

    let content = format!(
        "---\nsource_url: \"{}\"\ntype: webpage\ntitle: \"{}\"\ncaptured_at: {}\ncontributor: \"{}\"\n---\n\n# {}\n\nSource: {}\n\n---\n\n{}\n",
        yaml_str(url),
        yaml_str(&title),
        now,
        yaml_str(contributor_str),
        title,
        url,
        markdown_capped,
    );

    let filename = safe_filename(url, ".md");
    Ok((content, filename))
}

/// Fetch an arXiv abstract page. Returns `(markdown_content, filename)`.
fn fetch_arxiv(
    url: &str,
    author: Option<&str>,
    contributor: Option<&str>,
) -> Result<(String, String), String> {
    let re_id = Regex::new(r"(\d{4}\.\d{4,5})").unwrap();
    let arxiv_id = re_id.captures(url).map(|c| c[1].to_string());

    let (title, abstract_text, paper_authors) = if let Some(ref id) = arxiv_id {
        let api_url = format!("https://export.arxiv.org/abs/{id}");
        match fetch_html(&api_url) {
            Ok(html) => {
                let re_abstract =
                    Regex::new(r#"(?si)class="abstract[^"]*"[^>]*>(.*?)</blockquote>"#).unwrap();
                let re_title =
                    Regex::new(r#"(?si)class="title[^"]*"[^>]*>(.*?)</h1>"#).unwrap();
                let re_authors =
                    Regex::new(r#"(?si)class="authors"[^>]*>(.*?)</div>"#).unwrap();
                let re_tags = Regex::new(r"<[^>]+>").unwrap();
                let re_ws = Regex::new(r"\s+").unwrap();

                let abstract_text = re_abstract
                    .captures(&html)
                    .map(|c| {
                        re_tags
                            .replace_all(c.get(1).map(|m| m.as_str()).unwrap_or(""), "")
                            .trim()
                            .to_string()
                    })
                    .unwrap_or_default();

                let title = re_title
                    .captures(&html)
                    .map(|c| {
                        re_ws
                            .replace_all(
                                &re_tags
                                    .replace_all(c.get(1).map(|m| m.as_str()).unwrap_or(""), " "),
                                " ",
                            )
                            .trim()
                            .to_string()
                    })
                    .unwrap_or_else(|| id.clone());

                let paper_authors = re_authors
                    .captures(&html)
                    .map(|c| {
                        re_tags
                            .replace_all(c.get(1).map(|m| m.as_str()).unwrap_or(""), "")
                            .trim()
                            .to_string()
                    })
                    .unwrap_or_default();

                (title, abstract_text, paper_authors)
            }
            Err(_) => (id.clone(), String::new(), String::new()),
        }
    } else {
        // No arXiv ID found — fall back to generic webpage fetch.
        return fetch_webpage(url, author, contributor);
    };

    let id_str = arxiv_id.as_deref().unwrap_or("");
    let now = Utc::now().to_rfc3339();
    let contributor_str = contributor.or(author).unwrap_or("unknown");

    let content = format!(
        "---\nsource_url: \"{}\"\narxiv_id: \"{}\"\ntype: paper\ntitle: \"{}\"\npaper_authors: \"{}\"\ncaptured_at: {}\ncontributor: \"{}\"\n---\n\n# {}\n\n**Authors:** {}\n**arXiv:** {}\n\n## Abstract\n\n{}\n\nSource: {}\n",
        yaml_str(url),
        yaml_str(id_str),
        yaml_str(&title),
        yaml_str(&paper_authors),
        now,
        yaml_str(contributor_str),
        title,
        paper_authors,
        id_str,
        abstract_text,
        url,
    );

    let filename = if arxiv_id.is_some() {
        format!("arxiv_{}.md", id_str.replace('.', "_"))
    } else {
        safe_filename(url, ".md")
    };

    Ok((content, filename))
}

/// Download a binary file (PDF, image) directly to `target_dir`.
fn download_binary(url: &str, suffix: &str, target_dir: &Path) -> Result<PathBuf, String> {
    let filename = safe_filename(url, suffix);
    let out_path = target_dir.join(&filename);
    let bytes = safe_fetch(url, MAX_FETCH_BYTES, 60).map_err(|e| e.to_string())?;
    std::fs::write(&out_path, &bytes).map_err(|e| e.to_string())?;
    Ok(out_path)
}

// ---------------------------------------------------------------------------
// URL-encode helper (avoids adding `percent-encoding` dependency)
// ---------------------------------------------------------------------------

fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~' => out.push(b as char),
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Fetch a URL and save it into `target_dir` as a graphify-ready file.
///
/// Returns the path of the saved file.
pub fn ingest(
    url: &str,
    target_dir: &Path,
    author: Option<&str>,
    contributor: Option<&str>,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(target_dir)
        .map_err(|e| format!("ingest: could not create target_dir: {e}"))?;

    let url_type = detect_url_type(url);

    validate_url(url).map_err(|e| format!("ingest: {e}"))?;

    match url_type {
        "pdf" => {
            let out = download_binary(url, ".pdf", target_dir)
                .map_err(|e| format!("ingest: failed to fetch {url:?}: {e}"))?;
            eprintln!("Downloaded PDF: {}", out.file_name().unwrap_or_default().to_string_lossy());
            return Ok(out);
        }
        "image" => {
            let suffix = url::Url::parse(url)
                .ok()
                .map(|u| {
                    Path::new(u.path())
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| format!(".{e}"))
                        .unwrap_or_else(|| ".jpg".to_string())
                })
                .unwrap_or_else(|| ".jpg".to_string());
            let out = download_binary(url, &suffix, target_dir)
                .map_err(|e| format!("ingest: failed to fetch {url:?}: {e}"))?;
            eprintln!("Downloaded image: {}", out.file_name().unwrap_or_default().to_string_lossy());
            return Ok(out);
        }
        "youtube" => {
            // YouTube transcription requires an optional dependency; return a stub.
            return Err(format!(
                "ingest: YouTube ingestion not supported in the Rust port (url={url:?})"
            ));
        }
        _ => {}
    }

    let (content, filename) = match url_type {
        "tweet" => fetch_tweet(url, author, contributor),
        "arxiv" => fetch_arxiv(url, author, contributor),
        _ => fetch_webpage(url, author, contributor),
    }
    .map_err(|e| format!("ingest: failed to fetch {url:?}: {e}"))?;

    // Avoid overwriting: append counter if needed.
    let mut out_path = target_dir.join(&filename);
    let mut counter = 1usize;
    while out_path.exists() && counter < 1000 {
        let stem = Path::new(&filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        out_path = target_dir.join(format!("{stem}_{counter}.md"));
        counter += 1;
    }

    std::fs::write(&out_path, content.as_bytes())
        .map_err(|e| format!("ingest: failed to write {out_path:?}: {e}"))?;

    eprintln!(
        "Saved {url_type}: {}",
        out_path.file_name().unwrap_or_default().to_string_lossy()
    );

    Ok(out_path)
}

/// Save a Q&A result as a markdown file so it gets extracted into the graph on
/// the next `--update` run.
///
/// Files are stored in `memory_dir` with YAML frontmatter that graphify's
/// extractor reads as node metadata.
pub fn save_query_result(
    question: &str,
    answer: &str,
    memory_dir: &Path,
    query_type: &str,
    source_nodes: Option<&[String]>,
) -> io::Result<PathBuf> {
    std::fs::create_dir_all(memory_dir)?;

    let now = Utc::now();
    let re_non_word = Regex::new(r"[^\w]").unwrap();
    let slug: String = re_non_word
        .replace_all(&question.to_lowercase(), "_")
        .chars()
        .take(50)
        .collect::<String>()
        .trim_matches('_')
        .to_string();

    let filename = format!("query_{}_{}.md", now.format("%Y%m%d_%H%M%S"), slug);

    let mut frontmatter_lines = vec![
        "---".to_string(),
        format!("type: \"{}\"", query_type),
        format!("date: \"{}\"", now.to_rfc3339()),
        format!("question: \"{}\"", yaml_str(question)),
        "contributor: \"graphify\"".to_string(),
    ];

    if let Some(nodes) = source_nodes {
        if !nodes.is_empty() {
            let nodes_str: Vec<String> = nodes
                .iter()
                .take(10)
                .map(|n| format!("\"{}\"", n))
                .collect();
            frontmatter_lines.push(format!("source_nodes: [{}]", nodes_str.join(", ")));
        }
    }

    frontmatter_lines.push("---".to_string());

    let mut body_lines = vec![
        String::new(),
        format!("# Q: {question}"),
        String::new(),
        "## Answer".to_string(),
        String::new(),
        answer.to_string(),
    ];

    if let Some(nodes) = source_nodes {
        if !nodes.is_empty() {
            body_lines.push(String::new());
            body_lines.push("## Source Nodes".to_string());
            body_lines.push(String::new());
            for n in nodes {
                body_lines.push(format!("- {n}"));
            }
        }
    }

    let all_lines: Vec<String> = frontmatter_lines.into_iter().chain(body_lines).collect();
    let content = all_lines.join("\n");

    let out_path = memory_dir.join(&filename);
    std::fs::write(&out_path, content.as_bytes())?;
    Ok(out_path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_detect_url_type_tweet() {
        assert_eq!(detect_url_type("https://twitter.com/user/status/12345"), "tweet");
        assert_eq!(detect_url_type("https://x.com/user/status/12345"), "tweet");
    }

    #[test]
    fn test_detect_url_type_arxiv() {
        assert_eq!(detect_url_type("https://arxiv.org/abs/2301.00001"), "arxiv");
    }

    #[test]
    fn test_detect_url_type_github() {
        assert_eq!(detect_url_type("https://github.com/user/repo"), "github");
    }

    #[test]
    fn test_detect_url_type_pdf() {
        assert_eq!(detect_url_type("https://example.com/paper.pdf"), "pdf");
    }

    #[test]
    fn test_detect_url_type_image() {
        assert_eq!(detect_url_type("https://example.com/photo.jpg"), "image");
        assert_eq!(detect_url_type("https://example.com/image.png"), "image");
    }

    #[test]
    fn test_detect_url_type_webpage() {
        assert_eq!(detect_url_type("https://example.com/blog/post"), "webpage");
    }

    #[test]
    fn test_safe_filename() {
        let name = safe_filename("https://example.com/some/path", ".md");
        assert!(name.ends_with(".md"));
        assert!(!name.contains('/'));
        assert!(name.len() <= 83); // 80 stem + ".md"
    }

    #[test]
    fn test_yaml_str_escaping() {
        assert_eq!(yaml_str("hello \"world\""), "hello \\\"world\\\"");
        assert_eq!(yaml_str("line1\nline2"), "line1 line2");
    }

    #[test]
    fn test_save_query_result() {
        let dir = TempDir::new().unwrap();
        let out = save_query_result(
            "What is Rust?",
            "Rust is a systems programming language.",
            dir.path(),
            "query",
            Some(&["rust_lang".to_string(), "memory_safety".to_string()]),
        )
        .unwrap();
        assert!(out.exists());
        let content = std::fs::read_to_string(&out).unwrap();
        assert!(content.contains("# Q: What is Rust?"));
        assert!(content.contains("## Answer"));
        assert!(content.contains("## Source Nodes"));
        assert!(content.contains("- rust_lang"));
    }

    #[test]
    fn test_save_query_result_no_nodes() {
        let dir = TempDir::new().unwrap();
        let out = save_query_result(
            "Simple question",
            "Simple answer.",
            dir.path(),
            "query",
            None,
        )
        .unwrap();
        assert!(out.exists());
        let content = std::fs::read_to_string(&out).unwrap();
        assert!(!content.contains("Source Nodes"));
    }

    #[test]
    fn test_html_to_markdown_strips_scripts() {
        let html = r#"<html><script>alert(1)</script><body>Hello <b>world</b></body></html>"#;
        let md = html_to_markdown(html);
        assert!(!md.contains("alert"));
        assert!(md.contains("Hello"));
    }

    #[test]
    fn test_urlencoding_encode() {
        let encoded = urlencoding_encode("https://example.com/path?q=hello world");
        assert!(encoded.contains("%3A")); // ':'
        assert!(encoded.contains("%20")); // ' '
        assert!(!encoded.contains(' '));
    }
}
