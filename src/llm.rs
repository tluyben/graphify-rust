//! LLM backend stub — semantic extraction via Claude Code subagents uses the installed skill.
//! Direct LLM API calls (claude, kimi) are not implemented in the Rust port.
#![allow(dead_code, unused_variables)]

use std::path::Path;

pub const CHARS_PER_TOKEN: usize = 4;
pub const FILE_CHAR_CAP: usize = 20_000;

pub struct ExtractionResult {
    pub nodes: Vec<serde_json::Value>,
    pub edges: Vec<serde_json::Value>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub fn extract_with_llm(
    _files: &[&Path],
    _backend: &str,
    _model: Option<&str>,
    _api_key: Option<&str>,
) -> Result<ExtractionResult, String> {
    Err("Direct LLM extraction not implemented in the Rust port. Use the Python version for LLM-backed semantic extraction.".to_string())
}

pub fn estimate_tokens(text: &str) -> usize {
    text.len().saturating_div(CHARS_PER_TOKEN).max(1)
}
