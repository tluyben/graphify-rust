// Video transcription using yt-dlp and faster-whisper (called as external processes).
#![allow(dead_code)]
use std::path::{Path, PathBuf};
use std::process::Command;

pub const VIDEO_EXTENSIONS: &[&str] = &[
    ".mp4", ".mov", ".webm", ".mkv", ".avi", ".m4v", ".mp3", ".wav", ".m4a", ".ogg",
];

const URL_PREFIXES: &[&str] = &["http://", "https://", "www."];
const DEFAULT_MODEL: &str = "base";
const TRANSCRIPTS_DIR: &str = "graphify-out/transcripts";
const FALLBACK_PROMPT: &str = "Use proper punctuation and paragraph breaks.";

fn model_name() -> String {
    std::env::var("GRAPHIFY_WHISPER_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string())
}

/// Return true if the string looks like a URL rather than a file path.
pub fn is_url(path: &str) -> bool {
    URL_PREFIXES.iter().any(|prefix| path.starts_with(prefix))
}

/// Download audio-only stream from a URL using yt-dlp.
///
/// Returns the path to the downloaded audio file.
/// Uses cached file if already downloaded.
pub fn download_audio(url: &str, target_dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(target_dir)?;

    // Compute a stable name based on URL hash (SHA-1, first 12 hex chars)
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let hash_bytes = hasher.finalize();
    let url_hash = hex::encode(&hash_bytes[..6]); // 6 bytes = 12 hex chars

    // Check for already-downloaded file
    for ext in &[".m4a", ".opus", ".mp3", ".ogg", ".wav", ".webm"] {
        let candidate = target_dir.join(format!("yt_{}{}", url_hash, ext));
        if candidate.exists() {
            eprintln!(
                "  cached audio: {}",
                candidate.file_name().unwrap_or_default().to_string_lossy()
            );
            return Ok(candidate);
        }
    }

    let out_template = target_dir.join(format!("yt_{}.%(ext)s", url_hash));
    let out_template_str = out_template.to_string_lossy().to_string();

    eprintln!("  downloading audio: {} ...", &url[..url.len().min(80)]);

    let status = Command::new("yt-dlp")
        .args([
            "--format",
            "bestaudio[ext=m4a]/bestaudio/best",
            "--output",
            &out_template_str,
            "--quiet",
            "--no-warnings",
            "--no-playlist",
            url,
        ])
        .status()?;

    if !status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("yt-dlp failed with status {:?}", status.code()),
        ));
    }

    // Find the downloaded file (yt-dlp may pick any extension)
    let prefix = format!("yt_{}", url_hash);
    for ext in &[".m4a", ".opus", ".mp3", ".ogg", ".wav", ".webm"] {
        let candidate = target_dir.join(format!("{}{}", prefix, ext));
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    // Scan directory for any matching file
    for entry in std::fs::read_dir(target_dir)? {
        let entry = entry?;
        let fname = entry.file_name().to_string_lossy().to_string();
        if fname.starts_with(&prefix) && fname != prefix {
            return Ok(entry.path());
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("Downloaded file not found for hash {}", url_hash),
    ))
}

/// Transcribe an audio/video file using faster-whisper (called as an external process).
///
/// Returns the transcript text.
pub fn transcribe(
    audio_path: &Path,
    model: &str,
    language: Option<&str>,
) -> std::io::Result<String> {
    // Build faster-whisper CLI command
    // faster-whisper CLI: faster-whisper <file> --model <model> [--language <lang>] --output_format txt
    let mut cmd = Command::new("faster-whisper");
    cmd.arg(audio_path)
        .arg("--model")
        .arg(model)
        .arg("--output_format")
        .arg("txt");

    if let Some(lang) = language {
        cmd.arg("--language").arg(lang);
    }

    let output = cmd.output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("faster-whisper failed: {}", stderr.trim()),
        ));
    }

    let transcript = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(transcript)
}

/// Transcribe a video/audio file or URL to a .txt transcript.
///
/// If video_path is a URL, audio is downloaded first via yt-dlp.
/// Returns the path to the saved transcript file.
/// Uses cached transcript if it exists unless force=true.
pub fn transcribe_to_file(
    video_path: &str,
    output_dir: Option<&Path>,
    initial_prompt: Option<&str>,
    force: bool,
) -> std::io::Result<PathBuf> {
    let out_dir = output_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(TRANSCRIPTS_DIR));
    std::fs::create_dir_all(&out_dir)?;

    let audio_path = if is_url(video_path) {
        let downloads_dir = out_dir.join("downloads");
        download_audio(video_path, &downloads_dir)?
    } else {
        PathBuf::from(video_path)
    };

    let stem = audio_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("transcript");
    let transcript_path = out_dir.join(format!("{}.txt", stem));

    if transcript_path.exists() && !force {
        return Ok(transcript_path);
    }

    let model = model_name();
    let prompt = initial_prompt.unwrap_or(FALLBACK_PROMPT);

    eprintln!(
        "  transcribing {} (model={}) ...",
        audio_path.file_name().unwrap_or_default().to_string_lossy(),
        model
    );

    // Build the command: use faster-whisper as a module via python, passing initial_prompt
    // faster-whisper CLI doesn't natively support initial_prompt; use python -c
    let python_script = format!(
        r#"
from faster_whisper import WhisperModel
import sys
model = WhisperModel({model_repr}, device="cpu", compute_type="int8")
segments, info = model.transcribe(
    {path_repr},
    beam_size=5,
    initial_prompt={prompt_repr},
)
lines = [seg.text.strip() for seg in segments if seg.text.strip()]
print("\n".join(lines))
lang = info.language if hasattr(info, "language") else "unknown"
print(f"[graphify] lang={{lang}}, {{len(lines)}} segments", file=sys.stderr)
"#,
        model_repr = format!("{:?}", model),
        path_repr = format!("{:?}", audio_path.to_string_lossy().as_ref()),
        prompt_repr = format!("{:?}", prompt),
    );

    let output = Command::new("python3")
        .arg("-c")
        .arg(&python_script)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("faster-whisper transcription failed: {}", stderr.trim()),
        ));
    }

    let transcript = String::from_utf8_lossy(&output.stdout).to_string();
    std::fs::write(&transcript_path, &transcript)?;

    let stderr_msg = String::from_utf8_lossy(&output.stderr);
    eprintln!(
        "  transcript saved -> {} ({})",
        transcript_path.display(),
        stderr_msg.trim()
    );

    Ok(transcript_path)
}

/// Transcribe a list of video/audio files or URLs.
///
/// Returns paths to transcript .txt files. Already-transcribed files are returned from cache.
pub fn transcribe_all(
    video_files: &[&str],
    output_dir: Option<&Path>,
    initial_prompt: Option<&str>,
) -> Vec<String> {
    if video_files.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();
    for &vf in video_files {
        match transcribe_to_file(vf, output_dir, initial_prompt, false) {
            Ok(path) => results.push(path.to_string_lossy().to_string()),
            Err(e) => eprintln!("  warning: could not transcribe {}: {}", vf, e),
        }
    }
    results
}

/// Build a domain hint for Whisper from god nodes extracted from the corpus.
pub fn build_whisper_prompt(god_nodes: &[serde_json::Value]) -> String {
    if god_nodes.is_empty() {
        return FALLBACK_PROMPT.to_string();
    }

    if let Ok(override_prompt) = std::env::var("GRAPHIFY_WHISPER_PROMPT") {
        if !override_prompt.is_empty() {
            return override_prompt;
        }
    }

    let labels: Vec<&str> = god_nodes
        .iter()
        .take(10)
        .filter_map(|n| n.get("label").and_then(|v| v.as_str()))
        .filter(|s| !s.is_empty())
        .collect();

    if labels.is_empty() {
        return FALLBACK_PROMPT.to_string();
    }

    let topics = labels
        .iter()
        .take(5)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Technical discussion about {}. Use proper punctuation and paragraph breaks.",
        topics
    )
}
