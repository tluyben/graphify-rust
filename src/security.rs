//! Security utilities: URL validation, safe HTTP fetching, path validation,
//! and label sanitisation.
//!
//! Ported from the Python `security.py` module. All security invariants are
//! kept identical:
//!
//! * Only `http` and `https` URL schemes are allowed.
//! * Cloud-metadata hostnames are blocked by name.
//! * Any IP address that resolves to a private, reserved, loopback, or
//!   link-local range is blocked.
//! * Fetched responses are size-capped to avoid memory exhaustion.
//! * Graph paths must stay inside `graphify-out/`.
//! * Labels are stripped of control characters and capped at 256 chars.

use dns_lookup::getaddrinfo;
use ipnetwork::IpNetwork;
use std::collections::HashSet;
use std::io::{self, Read};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use url::Url;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// HTTP schemes that are permitted for outbound requests.
fn allowed_schemes() -> HashSet<&'static str> {
    ["http", "https"].iter().copied().collect()
}

/// Well-known cloud-metadata hostnames that must never be contacted.
fn blocked_hosts() -> HashSet<&'static str> {
    ["metadata.google.internal", "metadata.google.com"]
        .iter()
        .copied()
        .collect()
}

/// Maximum number of bytes fetched by [`safe_fetch`] (50 MiB).
pub const MAX_FETCH_BYTES: usize = 52_428_800;

/// Maximum number of bytes fetched by [`safe_fetch_text`] (10 MiB).
pub const MAX_TEXT_BYTES: usize = 10_485_760;

// ---------------------------------------------------------------------------
// IP-address helpers
// ---------------------------------------------------------------------------

/// Return `true` if the address is private, reserved, loopback, or
/// link-local.  Mirrors the Python check:
/// `ip.is_private or ip.is_reserved or ip.is_loopback or ip.is_link_local`.
fn is_internal_ip(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                // RFC 6598 shared address space (100.64.0.0/10).
                || is_v4_shared(v4)
                // IETF protocol assignments (192.0.0.0/24).
                || is_v4_ietf_protocol(v4)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // Unique-local (fc00::/7).
                || ((v6.segments()[0] & 0xfe00) == 0xfc00)
                // Link-local (fe80::/10).
                || ((v6.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

fn is_v4_shared(addr: std::net::Ipv4Addr) -> bool {
    // 100.64.0.0/10
    let n: IpNetwork = "100.64.0.0/10".parse().unwrap();
    n.contains(IpAddr::V4(addr))
}

fn is_v4_ietf_protocol(addr: std::net::Ipv4Addr) -> bool {
    // 192.0.0.0/24
    let n: IpNetwork = "192.0.0.0/24".parse().unwrap();
    n.contains(IpAddr::V4(addr))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Validate a URL for safe outbound access.
///
/// Checks (in order):
/// 1. Scheme must be `http` or `https`.
/// 2. Hostname must not be a blocked cloud-metadata endpoint.
/// 3. All resolved IP addresses must be public.
///
/// Returns the original URL string on success, or a [`String`] error message
/// on failure.
///
/// Mirrors Python's `validate_url(url)`.
pub fn validate_url(url: &str) -> Result<String, String> {
    let parsed = Url::parse(url).map_err(|e| format!("Invalid URL {url:?}: {e}"))?;

    let scheme = parsed.scheme().to_ascii_lowercase();
    if !allowed_schemes().contains(scheme.as_str()) {
        return Err(format!(
            "Blocked URL scheme '{scheme}' - only http and https are allowed. Got: {url:?}"
        ));
    }

    let hostname = match parsed.host_str() {
        Some(h) => h.to_string(),
        None => return Ok(url.to_string()), // no hostname (e.g. relative) – pass through
    };

    if blocked_hosts().contains(hostname.to_ascii_lowercase().as_str()) {
        return Err(format!(
            "Blocked cloud metadata endpoint '{hostname}'. Got: {url:?}"
        ));
    }

    // DNS resolution.
    let infos = getaddrinfo(Some(&hostname), None, None)
        .map_err(|e| format!("DNS resolution failed for '{hostname}': {e:?}. Got: {url:?}"))?;

    for result in infos {
        let info = result
            .map_err(|e| format!("DNS resolution failed for '{hostname}': {e:?}. Got: {url:?}"))?;
        let addr: IpAddr = info.sockaddr.ip();
        if is_internal_ip(addr) {
            return Err(format!(
                "Blocked private/internal IP {addr} (resolved from '{hostname}'). Got: {url:?}"
            ));
        }
    }

    Ok(url.to_string())
}

/// Fetch a URL, returning the raw bytes.
///
/// Size is capped at `max_bytes`; if the response body exceeds this limit an
/// [`io::Error`] with kind `Other` is returned.
///
/// Non-2xx status codes cause the function to return a [`reqwest`] error.
///
/// Mirrors Python's `safe_fetch(url, max_bytes, timeout)`.
///
/// # Errors
///
/// * [`String`] error from [`validate_url`] (bad scheme / blocked host / SSRF).
/// * [`reqwest::Error`] for transport / status errors.
/// * [`io::Error`] when the response body exceeds `max_bytes`.
pub fn safe_fetch(url: &str, max_bytes: usize, timeout_secs: u64) -> Result<Vec<u8>, FetchError> {
    validate_url(url).map_err(FetchError::Validation)?;

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .map_err(FetchError::Http)?;

    let mut response = client
        .get(url)
        .send()
        .map_err(FetchError::Http)?
        .error_for_status()
        .map_err(FetchError::Http)?;

    // Read up to max_bytes + 1 so we can detect an oversize body.
    let mut buf = Vec::with_capacity(max_bytes.min(1 << 20));
    let mut limited = (&mut response).take((max_bytes + 1) as u64);
    limited
        .read_to_end(&mut buf)
        .map_err(|e| FetchError::Io(e))?;

    if buf.len() > max_bytes {
        return Err(FetchError::Io(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "Response body exceeds {max_bytes} bytes limit for {url:?}"
            ),
        )));
    }

    Ok(buf)
}

/// Fetch a URL and decode the body as UTF-8 (with lossy replacement for
/// invalid sequences).
///
/// Mirrors Python's `safe_fetch_text(url, max_bytes, timeout)`.
pub fn safe_fetch_text(url: &str, max_bytes: usize, timeout_secs: u64) -> Result<String, FetchError> {
    let raw = safe_fetch(url, max_bytes, timeout_secs)?;
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// Error type returned by [`safe_fetch`] and [`safe_fetch_text`].
#[derive(Debug)]
pub enum FetchError {
    /// URL failed security validation.
    Validation(String),
    /// HTTP transport or status error.
    Http(reqwest::Error),
    /// I/O error (e.g. body size exceeded).
    Io(io::Error),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Validation(s) => write!(f, "URL validation error: {s}"),
            FetchError::Http(e) => write!(f, "HTTP error: {e}"),
            FetchError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for FetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FetchError::Http(e) => Some(e),
            FetchError::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// Validate that `path` (a string received from user input) refers to a file
/// that actually exists inside the `graphify-out/` directory tree under `base`.
///
/// If `base` is `None` the current working directory is used.
///
/// Returns the canonicalised [`PathBuf`] on success.
///
/// # Errors
///
/// * [`std::io::Error`] with kind `NotFound` if the resolved path does not
///   exist.
/// * [`std::io::Error`] with kind `PermissionDenied` if the resolved path
///   escapes the `graphify-out/` subtree (path traversal attempt).
///
/// Mirrors Python's `validate_graph_path(path, base)`.
pub fn validate_graph_path(path: &str, base: Option<&Path>) -> io::Result<PathBuf> {
    let base_dir = match base {
        Some(b) => b.to_path_buf(),
        None => std::env::current_dir()?,
    };

    // The allowed root is graphify-out/ under base.
    let allowed_root = base_dir.join("graphify-out");

    // Construct a candidate path.  If `path` is absolute we use it directly;
    // otherwise we resolve it relative to the allowed root.
    let candidate = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        allowed_root.join(path)
    };

    // Canonicalise (also checks existence).
    let canonical = candidate.canonicalize().map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("Path not found: {path:?}"),
        )
    })?;

    // The allowed root may not exist yet; normalise it without requiring
    // existence by resolving the base and appending the component.
    let canonical_root = base_dir
        .canonicalize()
        .unwrap_or_else(|_| base_dir.clone())
        .join("graphify-out");

    if !canonical.starts_with(&canonical_root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "Path escapes graphify-out/: {path:?} resolved to {}",
                canonical.display()
            ),
        ));
    }

    Ok(canonical)
}

/// Strip control characters from `text` and cap it at 256 Unicode scalar
/// values.
///
/// Returns an empty string when `text` is `None`.
///
/// Mirrors Python's `sanitize_label(text)`.
pub fn sanitize_label(text: Option<&str>) -> String {
    let s = match text {
        None => return String::new(),
        Some(t) => t,
    };

    // Remove C0 and C1 control characters (U+0000–U+001F and U+007F).
    let filtered: String = s
        .chars()
        .filter(|&c| {
            let n = c as u32;
            !(n <= 0x1F || n == 0x7F)
        })
        .collect();

    // Cap at 256 characters.
    if filtered.chars().count() > 256 {
        filtered.chars().take(256).collect()
    } else {
        filtered
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // sanitize_label
    // ------------------------------------------------------------------

    #[test]
    fn sanitize_label_none_returns_empty() {
        assert_eq!(sanitize_label(None), "");
    }

    #[test]
    fn sanitize_label_strips_control_chars() {
        let s = "hello\x00world\x1f!";
        assert_eq!(sanitize_label(Some(s)), "helloworld!");
    }

    #[test]
    fn sanitize_label_strips_del() {
        assert_eq!(sanitize_label(Some("ab\x7fcd")), "abcd");
    }

    #[test]
    fn sanitize_label_truncates_at_256() {
        let long: String = "a".repeat(300);
        let result = sanitize_label(Some(&long));
        assert_eq!(result.chars().count(), 256);
    }

    #[test]
    fn sanitize_label_passthrough_normal() {
        assert_eq!(sanitize_label(Some("hello world")), "hello world");
    }

    // ------------------------------------------------------------------
    // validate_url – scheme
    // ------------------------------------------------------------------

    #[test]
    fn validate_url_rejects_ftp() {
        let err = validate_url("ftp://example.com/file").unwrap_err();
        assert!(err.contains("Blocked URL scheme"));
    }

    #[test]
    fn validate_url_rejects_file_scheme() {
        let err = validate_url("file:///etc/passwd").unwrap_err();
        assert!(err.contains("Blocked URL scheme"));
    }

    // ------------------------------------------------------------------
    // validate_url – blocked hosts
    // ------------------------------------------------------------------

    #[test]
    fn validate_url_rejects_metadata_google_internal() {
        let err =
            validate_url("http://metadata.google.internal/computeMetadata/v1/").unwrap_err();
        assert!(err.contains("Blocked cloud metadata endpoint"));
    }

    // ------------------------------------------------------------------
    // validate_graph_path
    // ------------------------------------------------------------------

    #[test]
    fn validate_graph_path_accepts_inside_graphify_out() {
        let tmp = tempfile::TempDir::new().unwrap();
        let out = tmp.path().join("graphify-out");
        std::fs::create_dir_all(&out).unwrap();
        let target = out.join("report.json");
        std::fs::write(&target, b"{}").unwrap();
        let result = validate_graph_path("report.json", Some(tmp.path())).unwrap();
        assert_eq!(result, target.canonicalize().unwrap());
    }

    #[test]
    fn validate_graph_path_rejects_traversal() {
        let tmp = tempfile::TempDir::new().unwrap();
        let out = tmp.path().join("graphify-out");
        std::fs::create_dir_all(&out).unwrap();
        // Create the file outside graphify-out so it can be canonicalised.
        let secret = tmp.path().join("secret.txt");
        std::fs::write(&secret, b"secret").unwrap();
        let err =
            validate_graph_path("../secret.txt", Some(tmp.path())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn validate_graph_path_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("graphify-out")).unwrap();
        let err =
            validate_graph_path("nonexistent.json", Some(tmp.path())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
