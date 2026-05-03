use graphify::detect::{classify_file, count_words, detect, FileType};
use std::path::Path;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// classify_file tests
// ---------------------------------------------------------------------------

#[test]
fn test_classify_python() {
    assert_eq!(classify_file(Path::new("foo.py")), Some(FileType::Code));
}

#[test]
fn test_classify_typescript() {
    assert_eq!(classify_file(Path::new("bar.ts")), Some(FileType::Code));
}

#[test]
fn test_classify_markdown() {
    assert_eq!(
        classify_file(Path::new("README.md")),
        Some(FileType::Document)
    );
}

#[test]
fn test_classify_pdf() {
    assert_eq!(
        classify_file(Path::new("paper.pdf")),
        Some(FileType::Paper)
    );
}

#[test]
fn test_classify_unknown_returns_none() {
    assert_eq!(classify_file(Path::new("archive.zip")), None);
}

#[test]
fn test_classify_image() {
    assert_eq!(
        classify_file(Path::new("screenshot.png")),
        Some(FileType::Image)
    );
    assert_eq!(
        classify_file(Path::new("design.jpg")),
        Some(FileType::Image)
    );
    assert_eq!(
        classify_file(Path::new("diagram.webp")),
        Some(FileType::Image)
    );
}

// ---------------------------------------------------------------------------
// Video extension tests
// ---------------------------------------------------------------------------

#[test]
fn test_classify_video_extensions() {
    assert_eq!(
        classify_file(Path::new("lecture.mp4")),
        Some(FileType::Video)
    );
    assert_eq!(
        classify_file(Path::new("podcast.mp3")),
        Some(FileType::Video)
    );
    assert_eq!(
        classify_file(Path::new("talk.mov")),
        Some(FileType::Video)
    );
    assert_eq!(
        classify_file(Path::new("recording.wav")),
        Some(FileType::Video)
    );
    assert_eq!(
        classify_file(Path::new("webinar.webm")),
        Some(FileType::Video)
    );
    assert_eq!(
        classify_file(Path::new("audio.m4a")),
        Some(FileType::Video)
    );
}

// ---------------------------------------------------------------------------
// count_words tests
// ---------------------------------------------------------------------------

#[test]
fn test_count_words_sample_md() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.md");
    let words = count_words(&path);
    assert!(words > 5, "Expected more than 5 words, got {}", words);
}

// ---------------------------------------------------------------------------
// classify_file paper heuristic tests (uses tempfile)
// ---------------------------------------------------------------------------

#[test]
fn test_classify_md_paper_by_signals() {
    let dir = TempDir::new().unwrap();
    let paper = dir.path().join("paper.md");
    std::fs::write(
        &paper,
        "# Abstract\n\nWe propose a new method. See [1] and [23].\n\
         This work was published in the Journal of AI. ArXiv preprint.\n\
         See Equation 3 for details. \\cite{vaswani2017}.\n",
    )
    .unwrap();
    assert_eq!(classify_file(&paper), Some(FileType::Paper));
}

#[test]
fn test_classify_md_doc_without_signals() {
    let dir = TempDir::new().unwrap();
    let doc = dir.path().join("notes.md");
    std::fs::write(&doc, "# My Notes\n\nHere are some notes about the project.\n").unwrap();
    assert_eq!(classify_file(&doc), Some(FileType::Document));
}

// ---------------------------------------------------------------------------
// detect() tests
// ---------------------------------------------------------------------------

#[test]
fn test_detect_finds_fixtures() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let result = detect(&fixtures, false);
    assert!(
        result.total_files >= 2,
        "Expected at least 2 files, got {}",
        result.total_files
    );
    assert!(result.files.contains_key("code"));
    assert!(result.files.contains_key("document"));
}

#[test]
fn test_detect_skips_dotfiles() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let result = detect(&fixtures, false);
    for files in result.files.values() {
        for f in files {
            // Check no hidden files (starting with dot) are included
            let filename = Path::new(f)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            assert!(
                !filename.starts_with('.') || filename == ".graphifyignore",
                "Found dotfile: {}",
                f
            );
        }
    }
}

#[test]
fn test_detect_includes_video_key() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("main.py"), "x = 1").unwrap();
    let result = detect(dir.path(), false);
    assert!(result.files.contains_key("video"));
}

#[test]
fn test_detect_finds_video_files() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("lecture.mp4"), b"fake video data").unwrap();
    std::fs::write(dir.path().join("notes.md"), "# Notes\nSome content here.").unwrap();
    let result = detect(dir.path(), false);
    assert_eq!(result.files["video"].len(), 1);
    assert!(result.files["video"].iter().any(|f| f.contains("lecture.mp4")));
}

#[test]
fn test_detect_video_not_in_words() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("clip.mp4"), &[0u8; 100]).unwrap();
    let result = detect(dir.path(), false);
    assert_eq!(result.total_words, 0);
}

// ---------------------------------------------------------------------------
// .graphifyignore tests
// ---------------------------------------------------------------------------

#[test]
fn test_graphifyignore_excludes_file() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(".graphifyignore"), "vendor/\n*.generated.py\n").unwrap();
    let vendor = dir.path().join("vendor");
    std::fs::create_dir_all(&vendor).unwrap();
    std::fs::write(vendor.join("lib.py"), "x = 1").unwrap();
    std::fs::write(dir.path().join("main.py"), "print('hi')").unwrap();
    std::fs::write(dir.path().join("schema.generated.py"), "x = 1").unwrap();

    let result = detect(dir.path(), false);
    let file_list = &result.files["code"];
    assert!(file_list.iter().any(|f| f.contains("main.py")));
    assert!(!file_list.iter().any(|f| f.contains("vendor")));
    assert!(!file_list.iter().any(|f| f.contains("generated")));
    assert_eq!(result.graphifyignore_patterns.len(), 2);
}

#[test]
fn test_graphifyignore_missing_is_fine() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("main.py"), "x = 1").unwrap();
    let result = detect(dir.path(), false);
    assert_eq!(result.graphifyignore_patterns.len(), 0);
}

#[test]
fn test_graphifyignore_comments_ignored() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join(".graphifyignore"),
        "# this is a comment\n\nmain.py\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("main.py"), "x = 1").unwrap();
    std::fs::write(dir.path().join("other.py"), "x = 2").unwrap();
    let result = detect(dir.path(), false);
    assert!(!result.files["code"].iter().any(|f| f.contains("main.py")));
    assert!(result.files["code"].iter().any(|f| f.contains("other.py")));
}

#[test]
fn test_graphifyignore_hermetic_without_vcs() {
    // Without a VCS root, parent .graphifyignore does NOT apply (hermetic).
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(".graphifyignore"), "vendor/\n").unwrap();
    let sub = dir.path().join("packages").join("mylib");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("main.py"), "x = 1").unwrap();
    let vendor = sub.join("vendor");
    std::fs::create_dir_all(&vendor).unwrap();
    std::fs::write(vendor.join("dep.py"), "y = 2").unwrap();

    let result = detect(&sub, false);
    let code_files = &result.files["code"];
    assert!(code_files.iter().any(|f| f.contains("main.py")));
    // parent .graphifyignore must NOT leak into a non-VCS scan
    assert!(code_files.iter().any(|f| f.contains("vendor")));
    assert_eq!(result.graphifyignore_patterns.len(), 0);
}

#[test]
fn test_graphifyignore_discovered_from_parent_in_vcs() {
    // Inside a VCS repo, parent .graphifyignore applies to subdirectory scans.
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".graphifyignore"), "vendor/\n").unwrap();
    let sub = dir.path().join("packages").join("mylib");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("main.py"), "x = 1").unwrap();
    let vendor = sub.join("vendor");
    std::fs::create_dir_all(&vendor).unwrap();
    std::fs::write(vendor.join("dep.py"), "y = 2").unwrap();

    let result = detect(&sub, false);
    let code_files = &result.files["code"];
    assert!(code_files.iter().any(|f| f.contains("main.py")));
    assert!(!code_files.iter().any(|f| f.contains("vendor")));
    assert!(result.graphifyignore_patterns.len() >= 1);
}

#[test]
fn test_graphifyignore_stops_at_git_boundary() {
    // Upward search stops at the git repo root (.git directory).
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(".graphifyignore"), "main.py\n").unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let sub = repo.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("main.py"), "x = 1").unwrap();

    let result = detect(&sub, false);
    let code_files = &result.files["code"];
    assert!(code_files.iter().any(|f| f.contains("main.py")));
    assert_eq!(result.graphifyignore_patterns.len(), 0);
}

#[test]
fn test_graphifyignore_at_git_root_is_included() {
    // A .graphifyignore at the git repo root is included when scanning a subdir.
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join(".graphifyignore"), "vendor/\n").unwrap();
    let sub = repo.join("packages").join("mylib");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("main.py"), "x = 1").unwrap();
    let vendor = sub.join("vendor");
    std::fs::create_dir_all(&vendor).unwrap();
    std::fs::write(vendor.join("dep.py"), "y = 2").unwrap();

    let result = detect(&sub, false);
    let code_files = &result.files["code"];
    assert!(code_files.iter().any(|f| f.contains("main.py")));
    assert!(!code_files.iter().any(|f| f.contains("vendor")));
    assert_eq!(result.graphifyignore_patterns.len(), 1);
}

#[test]
fn test_detect_handles_circular_symlinks() {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("a");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("main.py"), "x = 1").unwrap();
    // Create a circular symlink
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path(), sub.join("loop")).unwrap();

    let result = detect(dir.path(), true);
    assert!(result.files["code"].iter().any(|f| f.contains("main.py")));
}

// ---------------------------------------------------------------------------
// Symlink tests (Unix only)
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn test_detect_follows_symlinked_directory() {
    let dir = TempDir::new().unwrap();
    let real_dir = dir.path().join("real_lib");
    std::fs::create_dir_all(&real_dir).unwrap();
    std::fs::write(real_dir.join("util.py"), "x = 1").unwrap();
    std::os::unix::fs::symlink(&real_dir, dir.path().join("linked_lib")).unwrap();

    let result_no = detect(dir.path(), false);
    let result_yes = detect(dir.path(), true);

    assert!(result_no.files["code"].iter().any(|f| f.contains("real_lib")));
    assert!(!result_no.files["code"].iter().any(|f| f.contains("linked_lib")));
    assert!(result_yes.files["code"].iter().any(|f| f.contains("linked_lib")));
}

#[cfg(unix)]
#[test]
fn test_detect_follows_symlinked_file() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("real.py"), "x = 1").unwrap();
    std::os::unix::fs::symlink(dir.path().join("real.py"), dir.path().join("link.py")).unwrap();

    let result = detect(dir.path(), true);
    let code = &result.files["code"];
    assert!(code.iter().any(|f| f.contains("real.py")));
    assert!(code.iter().any(|f| f.contains("link.py")));
}
