// git hook integration - install/uninstall graphify post-commit and post-checkout hooks
#![allow(dead_code)]
use regex::Regex;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const HOOK_MARKER: &str = "# graphify-hook-start";
pub const HOOK_MARKER_END: &str = "# graphify-hook-end";
pub const CHECKOUT_MARKER: &str = "# graphify-checkout-hook-start";
pub const CHECKOUT_MARKER_END: &str = "# graphify-checkout-hook-end";

const PYTHON_DETECT: &str = "\
# Detect the correct Python interpreter (handles pipx, venv, system installs)\n\
GRAPHIFY_BIN=$(command -v graphify 2>/dev/null)\n\
if [ -n \"$GRAPHIFY_BIN\" ]; then\n\
    case \"$GRAPHIFY_BIN\" in\n\
        *.exe) _SHEBANG=\"\" ;;\n\
        *)     _SHEBANG=$(head -1 \"$GRAPHIFY_BIN\" | sed 's/^#![[:space:]]*//')\
 ;;\n\
    esac\n\
    case \"$_SHEBANG\" in\n\
        */env\\ *) GRAPHIFY_PYTHON=\"${_SHEBANG#*/env }\" ;;\n\
        *)         GRAPHIFY_PYTHON=\"$_SHEBANG\" ;;\n\
    esac\n\
    # Allowlist: only keep characters valid in a filesystem path to prevent\n\
    # injection if the shebang contains shell metacharacters\n\
    case \"$GRAPHIFY_PYTHON\" in\n\
        *[!a-zA-Z0-9/_.@-]*) GRAPHIFY_PYTHON=\"\" ;;\n\
    esac\n\
    if [ -n \"$GRAPHIFY_PYTHON\" ] && ! \"$GRAPHIFY_PYTHON\" -c \"import graphify\" \
2>/dev/null; then\n\
        GRAPHIFY_PYTHON=\"\"\n\
    fi\n\
fi\n\
# Fall back: try python3, then python (Windows has no python3 shim)\n\
if [ -z \"$GRAPHIFY_PYTHON\" ]; then\n\
    if command -v python3 >/dev/null 2>&1 && python3 -c \"import graphify\" \
2>/dev/null; then\n\
        GRAPHIFY_PYTHON=\"python3\"\n\
    elif command -v python >/dev/null 2>&1 && python -c \"import graphify\" \
2>/dev/null; then\n\
        GRAPHIFY_PYTHON=\"python\"\n\
    else\n\
        exit 0\n\
    fi\n\
fi\n";

fn hook_script() -> String {
    let head = "\
# graphify-hook-start\n\
# Auto-rebuilds the knowledge graph after each commit (code files only, no LLM needed).\n\
# Installed by: graphify hook install\n\
\n\
# Skip during rebase/merge/cherry-pick to avoid blocking --continue with unstaged changes\n\
GIT_DIR=$(git rev-parse --git-dir 2>/dev/null)\n\
[ -d \"$GIT_DIR/rebase-merge\" ] && exit 0\n\
[ -d \"$GIT_DIR/rebase-apply\" ] && exit 0\n\
[ -f \"$GIT_DIR/MERGE_HEAD\" ] && exit 0\n\
[ -f \"$GIT_DIR/CHERRY_PICK_HEAD\" ] && exit 0\n\
\n\
CHANGED=$(git diff --name-only HEAD~1 HEAD 2>/dev/null || git diff --name-only HEAD 2>/dev/null)\n\
if [ -z \"$CHANGED\" ]; then\n\
    exit 0\n\
fi\n\
\n";
    let tail = "\
export GRAPHIFY_CHANGED=\"$CHANGED\"\n\
\n\
# Run rebuild detached so git commit returns immediately.\n\
# Full repo rebuilds can take hours; blocking the post-commit hook stalls the shell.\n\
_GRAPHIFY_LOG=\"${HOME}/.cache/graphify-rebuild.log\"\n\
mkdir -p \"$(dirname \"$_GRAPHIFY_LOG\")\"\n\
echo \"[graphify hook] launching background rebuild (log: $_GRAPHIFY_LOG)\"\n\
nohup $GRAPHIFY_PYTHON -c \"\n\
import os, sys\n\
from pathlib import Path\n\
\n\
changed_raw = os.environ.get('GRAPHIFY_CHANGED', '')\n\
changed = [Path(f.strip()) for f in changed_raw.strip().splitlines() if f.strip()]\n\
\n\
if not changed:\n\
    sys.exit(0)\n\
\n\
print(f'[graphify hook] {len(changed)} file(s) changed - rebuilding graph...')\n\
\n\
try:\n\
    import os as _os\n\
    from graphify.watch import _rebuild_code\n\
    _force = _os.environ.get('GRAPHIFY_FORCE', '').lower() in ('1', 'true', 'yes')\n\
    _rebuild_code(Path('.'), force=_force)\n\
except Exception as exc:\n\
    print(f'[graphify hook] Rebuild failed: {exc}')\n\
    sys.exit(1)\n\
\" > \"$_GRAPHIFY_LOG\" 2>&1 < /dev/null &\n\
disown 2>/dev/null || true\n\
# graphify-hook-end\n";
    format!("{}{}{}", head, PYTHON_DETECT, tail)
}

fn checkout_script() -> String {
    let head = "\
# graphify-checkout-hook-start\n\
# Auto-rebuilds the knowledge graph (code only) when switching branches.\n\
# Installed by: graphify hook install\n\
\n\
PREV_HEAD=$1\n\
NEW_HEAD=$2\n\
BRANCH_SWITCH=$3\n\
\n\
# Only run on branch switches, not file checkouts\n\
if [ \"$BRANCH_SWITCH\" != \"1\" ]; then\n\
    exit 0\n\
fi\n\
\n\
# Only run if graphify-out/ exists (graph has been built before)\n\
if [ ! -d \"graphify-out\" ]; then\n\
    exit 0\n\
fi\n\
\n\
# Skip during rebase/merge/cherry-pick\n\
GIT_DIR=$(git rev-parse --git-dir 2>/dev/null)\n\
[ -d \"$GIT_DIR/rebase-merge\" ] && exit 0\n\
[ -d \"$GIT_DIR/rebase-apply\" ] && exit 0\n\
[ -f \"$GIT_DIR/MERGE_HEAD\" ] && exit 0\n\
[ -f \"$GIT_DIR/CHERRY_PICK_HEAD\" ] && exit 0\n\
\n";
    let tail = "\
_GRAPHIFY_LOG=\"${HOME}/.cache/graphify-rebuild.log\"\n\
mkdir -p \"$(dirname \"$_GRAPHIFY_LOG\")\"\n\
echo \"[graphify] Branch switched - launching background rebuild (log: $_GRAPHIFY_LOG)\"\n\
nohup $GRAPHIFY_PYTHON -c \"\n\
from graphify.watch import _rebuild_code\n\
from pathlib import Path\n\
import os, sys\n\
try:\n\
    _force = os.environ.get('GRAPHIFY_FORCE', '').lower() in ('1', 'true', 'yes')\n\
    _rebuild_code(Path('.'), force=_force)\n\
except Exception as exc:\n\
    print(f'[graphify] Rebuild failed: {exc}')\n\
    sys.exit(1)\n\
\" > \"$_GRAPHIFY_LOG\" 2>&1 < /dev/null &\n\
disown 2>/dev/null || true\n\
# graphify-checkout-hook-end\n";
    format!("{}{}{}", head, PYTHON_DETECT, tail)
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    } else if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(path)
}

/// Walk up from `path` to find the nearest directory containing `.git`.
pub fn git_root(path: &Path) -> Option<PathBuf> {
    let current = path.canonicalize().ok()?;
    let mut candidates: Vec<PathBuf> = vec![current.clone()];
    let mut p: &Path = current.as_path();
    loop {
        match p.parent() {
            Some(parent) if parent != p => {
                candidates.push(parent.to_path_buf());
                p = parent;
            }
            _ => break,
        }
    }
    for candidate in candidates {
        if candidate.join(".git").exists() {
            return Some(candidate);
        }
    }
    None
}

fn hooks_dir(root: &Path) -> PathBuf {
    // Respect core.hooksPath if set (e.g. Husky)
    if let Ok(output) = Command::new("git")
        .args(["-C", &root.to_string_lossy(), "config", "core.hooksPath"])
        .output()
    {
        if output.status.success() {
            let custom = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !custom.is_empty() {
                let mut p = expand_tilde(&custom);
                if !p.is_absolute() {
                    p = root.join(p);
                }
                let _ = fs::create_dir_all(&p);
                return p;
            }
        }
    }
    let d = root.join(".git").join("hooks");
    let _ = fs::create_dir_all(&d);
    d
}

fn install_hook(hooks_dir: &Path, name: &str, script: &str, marker: &str) -> String {
    let hook_path = hooks_dir.join(name);
    if hook_path.exists() {
        let content = fs::read_to_string(&hook_path).unwrap_or_default();
        if content.contains(marker) {
            return format!("already installed at {}", hook_path.display());
        }
        let new_content = format!("{}\n\n{}", content.trim_end(), script);
        fs::write(&hook_path, new_content).ok();
        return format!(
            "appended to existing {} hook at {}",
            name,
            hook_path.display()
        );
    }
    let content = format!("#!/bin/sh\n{}", script);
    fs::write(&hook_path, &content).ok();
    // Set executable permission (rwxr-xr-x = 0o755)
    #[cfg(unix)]
    if let Ok(metadata) = fs::metadata(&hook_path) {
        let mut perms = metadata.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_path, perms).ok();
    }
    format!("installed at {}", hook_path.display())
}

fn uninstall_hook(hooks_dir: &Path, name: &str, marker: &str, marker_end: &str) -> String {
    let hook_path = hooks_dir.join(name);
    if !hook_path.exists() {
        return format!("no {} hook found - nothing to remove.", name);
    }
    let content = fs::read_to_string(&hook_path).unwrap_or_default();
    if !content.contains(marker) {
        return format!("graphify hook not found in {} - nothing to remove.", name);
    }
    // Use regex with DOTALL ((?s)) to remove the marker block
    let pattern = format!(
        "(?s){}.*?{}\n?",
        regex::escape(marker),
        regex::escape(marker_end)
    );
    let new_content = match Regex::new(&pattern) {
        Ok(re) => re.replace_all(&content, "").trim().to_string(),
        Err(_) => content.trim().to_string(),
    };

    if new_content.is_empty() || new_content == "#!/bin/bash" || new_content == "#!/bin/sh" {
        fs::remove_file(&hook_path).ok();
        return format!("removed {} hook at {}", name, hook_path.display());
    }
    fs::write(&hook_path, format!("{}\n", new_content)).ok();
    format!(
        "graphify removed from {} at {} (other hook content preserved)",
        name,
        hook_path.display()
    )
}

/// Install graphify post-commit and post-checkout hooks in the nearest git repo.
pub fn install(path: &Path) -> Result<String, String> {
    let root = git_root(path)
        .ok_or_else(|| format!("No git repository found at or above {}", path.display()))?;
    let hdir = hooks_dir(&root);
    let hs = hook_script();
    let cs = checkout_script();
    let commit_msg = install_hook(&hdir, "post-commit", &hs, HOOK_MARKER);
    let checkout_msg = install_hook(&hdir, "post-checkout", &cs, CHECKOUT_MARKER);
    Ok(format!(
        "post-commit: {}\npost-checkout: {}",
        commit_msg, checkout_msg
    ))
}

/// Remove graphify post-commit and post-checkout hooks.
pub fn uninstall(path: &Path) -> Result<String, String> {
    let root = git_root(path)
        .ok_or_else(|| format!("No git repository found at or above {}", path.display()))?;
    let hdir = hooks_dir(&root);
    let commit_msg = uninstall_hook(&hdir, "post-commit", HOOK_MARKER, HOOK_MARKER_END);
    let checkout_msg = uninstall_hook(&hdir, "post-checkout", CHECKOUT_MARKER, CHECKOUT_MARKER_END);
    Ok(format!(
        "post-commit: {}\npost-checkout: {}",
        commit_msg, checkout_msg
    ))
}

/// Check if graphify hooks are installed.
pub fn status(path: &Path) -> String {
    let root = match git_root(path) {
        Some(r) => r,
        None => return "Not in a git repository.".to_string(),
    };
    let hdir = hooks_dir(&root);

    let check = |name: &str, marker: &str| -> String {
        let p = hdir.join(name);
        if !p.exists() {
            return "not installed".to_string();
        }
        let content = fs::read_to_string(&p).unwrap_or_default();
        if content.contains(marker) {
            "installed".to_string()
        } else {
            "not installed (hook exists but graphify not found)".to_string()
        }
    };

    let commit = check("post-commit", HOOK_MARKER);
    let checkout = check("post-checkout", CHECKOUT_MARKER);
    format!("post-commit: {}\npost-checkout: {}", commit, checkout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_fake_git_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join(".git/hooks")).unwrap();
        dir
    }

    #[test]
    fn test_git_root_finds_dot_git() {
        let dir = make_fake_git_repo();
        let found = git_root(dir.path());
        assert!(found.is_some());
        assert_eq!(
            found.unwrap().canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn test_install_creates_hook() {
        let dir = make_fake_git_repo();
        let result = install(dir.path());
        assert!(result.is_ok(), "install failed: {:?}", result);
        let msg = result.unwrap();
        assert!(msg.contains("post-commit:"));
        assert!(msg.contains("post-checkout:"));

        let hook_path = dir.path().join(".git/hooks/post-commit");
        assert!(hook_path.exists());
        let content = fs::read_to_string(&hook_path).unwrap();
        assert!(content.contains(HOOK_MARKER));
        assert!(content.contains(HOOK_MARKER_END));
    }

    #[test]
    fn test_install_already_installed() {
        let dir = make_fake_git_repo();
        install(dir.path()).unwrap();
        let result2 = install(dir.path()).unwrap();
        assert!(result2.contains("already installed"));
    }

    #[test]
    fn test_uninstall_removes_hook() {
        let dir = make_fake_git_repo();
        install(dir.path()).unwrap();
        let result = uninstall(dir.path()).unwrap();
        assert!(result.contains("post-commit:"));
        // Hook should be removed since it only contained graphify content
        let hook_path = dir.path().join(".git/hooks/post-commit");
        assert!(!hook_path.exists(), "hook file should have been removed");
    }

    #[test]
    fn test_uninstall_preserves_existing_content() {
        let dir = make_fake_git_repo();
        let hook_path = dir.path().join(".git/hooks/post-commit");
        fs::write(&hook_path, "#!/bin/sh\necho 'other hook'\n").unwrap();
        install(dir.path()).unwrap();
        uninstall(dir.path()).unwrap();
        let content = fs::read_to_string(&hook_path).unwrap();
        assert!(content.contains("other hook"));
        assert!(!content.contains(HOOK_MARKER));
    }

    #[test]
    fn test_status_not_installed() {
        let dir = make_fake_git_repo();
        let s = status(dir.path());
        assert!(s.contains("not installed"));
    }

    #[test]
    fn test_status_installed() {
        let dir = make_fake_git_repo();
        install(dir.path()).unwrap();
        let s = status(dir.path());
        assert!(s.contains("installed"));
    }

    #[test]
    fn test_no_git_repo_returns_error() {
        let dir = TempDir::new().unwrap();
        assert!(install(dir.path()).is_err());
        assert!(uninstall(dir.path()).is_err());
    }

    #[test]
    fn test_status_no_git_repo() {
        let dir = TempDir::new().unwrap();
        let s = status(dir.path());
        assert_eq!(s, "Not in a git repository.");
    }

    #[test]
    fn test_hook_script_contains_markers() {
        let hs = hook_script();
        assert!(hs.contains(HOOK_MARKER));
        assert!(hs.contains(HOOK_MARKER_END));
        let cs = checkout_script();
        assert!(cs.contains(CHECKOUT_MARKER));
        assert!(cs.contains(CHECKOUT_MARKER_END));
    }

    #[test]
    fn test_uninstall_no_hook_found() {
        let dir = make_fake_git_repo();
        let result = uninstall(dir.path()).unwrap();
        assert!(result.contains("nothing to remove"));
    }
}
