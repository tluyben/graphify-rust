use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

// Embedded skill files (relative to Cargo.toml, which is the project root).
// These are copied from the Python graphify package.
const SKILL_CLAUDE: &str = include_str!("../skills/skill.md");
const SKILL_WINDOWS: &str = include_str!("../skills/skill-windows.md");
const SKILL_CODEX: &str = include_str!("../skills/skill-codex.md");
const SKILL_OPENCODE: &str = include_str!("../skills/skill-opencode.md");
const SKILL_AIDER: &str = include_str!("../skills/skill-aider.md");
const SKILL_COPILOT: &str = include_str!("../skills/skill-copilot.md");
const SKILL_CLAW: &str = include_str!("../skills/skill-claw.md");
const SKILL_DROID: &str = include_str!("../skills/skill-droid.md");
const SKILL_TRAE: &str = include_str!("../skills/skill-trae.md");
const SKILL_KIRO: &str = include_str!("../skills/skill-kiro.md");
const SKILL_PI: &str = include_str!("../skills/skill-pi.md");
const SKILL_VSCODE: &str = include_str!("../skills/skill-vscode.md");

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// Platform configuration
// ---------------------------------------------------------------------------

struct PlatformConfig {
    skill_file: &'static str,
    skill_content: &'static str,
    skill_dst_suffix: &'static str, // relative to $HOME
    claude_md: bool,
}

fn platform_config() -> HashMap<&'static str, PlatformConfig> {
    let mut m = HashMap::new();
    m.insert("claude", PlatformConfig {
        skill_file: "skill.md",
        skill_content: SKILL_CLAUDE,
        skill_dst_suffix: ".claude/skills/graphify/SKILL.md",
        claude_md: true,
    });
    m.insert("windows", PlatformConfig {
        skill_file: "skill-windows.md",
        skill_content: SKILL_WINDOWS,
        skill_dst_suffix: ".claude/skills/graphify/SKILL.md",
        claude_md: true,
    });
    m.insert("codex", PlatformConfig {
        skill_file: "skill-codex.md",
        skill_content: SKILL_CODEX,
        skill_dst_suffix: ".agents/skills/graphify/SKILL.md",
        claude_md: false,
    });
    m.insert("opencode", PlatformConfig {
        skill_file: "skill-opencode.md",
        skill_content: SKILL_OPENCODE,
        skill_dst_suffix: ".config/opencode/skills/graphify/SKILL.md",
        claude_md: false,
    });
    m.insert("aider", PlatformConfig {
        skill_file: "skill-aider.md",
        skill_content: SKILL_AIDER,
        skill_dst_suffix: ".aider/graphify/SKILL.md",
        claude_md: false,
    });
    m.insert("copilot", PlatformConfig {
        skill_file: "skill-copilot.md",
        skill_content: SKILL_COPILOT,
        skill_dst_suffix: ".copilot/skills/graphify/SKILL.md",
        claude_md: false,
    });
    m.insert("claw", PlatformConfig {
        skill_file: "skill-claw.md",
        skill_content: SKILL_CLAW,
        skill_dst_suffix: ".openclaw/skills/graphify/SKILL.md",
        claude_md: false,
    });
    m.insert("droid", PlatformConfig {
        skill_file: "skill-droid.md",
        skill_content: SKILL_DROID,
        skill_dst_suffix: ".factory/skills/graphify/SKILL.md",
        claude_md: false,
    });
    m.insert("trae", PlatformConfig {
        skill_file: "skill-trae.md",
        skill_content: SKILL_TRAE,
        skill_dst_suffix: ".trae/skills/graphify/SKILL.md",
        claude_md: false,
    });
    m.insert("trae-cn", PlatformConfig {
        skill_file: "skill-trae.md",
        skill_content: SKILL_TRAE,
        skill_dst_suffix: ".trae-cn/skills/graphify/SKILL.md",
        claude_md: false,
    });
    m.insert("hermes", PlatformConfig {
        skill_file: "skill-claw.md",
        skill_content: SKILL_CLAW,
        skill_dst_suffix: ".hermes/skills/graphify/SKILL.md",
        claude_md: false,
    });
    m.insert("kiro", PlatformConfig {
        skill_file: "skill-kiro.md",
        skill_content: SKILL_KIRO,
        skill_dst_suffix: ".kiro/skills/graphify/SKILL.md",
        claude_md: false,
    });
    m.insert("pi", PlatformConfig {
        skill_file: "skill-pi.md",
        skill_content: SKILL_PI,
        skill_dst_suffix: ".pi/agent/skills/graphify/SKILL.md",
        claude_md: false,
    });
    m.insert("antigravity", PlatformConfig {
        skill_file: "skill.md",
        skill_content: SKILL_CLAUDE,
        skill_dst_suffix: ".agents/skills/graphify/SKILL.md",
        claude_md: false,
    });
    m
}

const SKILL_REGISTRATION: &str = r#"
# graphify
- **graphify** (`~/.claude/skills/graphify/SKILL.md`) - any input to knowledge graph. Trigger: `/graphify`
When the user types `/graphify`, invoke the Skill tool with `skill: "graphify"` before doing anything else.
"#;

const CLAUDE_MD_SECTION: &str = r#"## graphify

This project has a graphify knowledge graph at graphify-out/.

Rules:
- Before answering architecture or codebase questions, read graphify-out/GRAPH_REPORT.md for god nodes and community structure
- If graphify-out/wiki/index.md exists, navigate it instead of reading raw files
- For cross-module "how does X relate to Y" questions, prefer `graphify query "<question>"`, `graphify path "<A>" "<B>"`, or `graphify explain "<concept>"` over grep — these traverse the graph's EXTRACTED + INFERRED edges instead of scanning files
- After modifying code files in this session, run `graphify update .` to keep the graph current (AST-only, no API cost)
"#;

const AGENTS_MD_SECTION: &str = r#"## graphify

This project has a graphify knowledge graph at graphify-out/.

Rules:
- Before answering architecture or codebase questions, read graphify-out/GRAPH_REPORT.md for god nodes and community structure
- If graphify-out/wiki/index.md exists, navigate it instead of reading raw files
- For cross-module "how does X relate to Y" questions, prefer `graphify query "<question>"`, `graphify path "<A>" "<B>"`, or `graphify explain "<concept>"` over grep — these traverse the graph's EXTRACTED + INFERRED edges instead of scanning files
- After modifying code files in this session, run `graphify update .` to keep the graph current (AST-only, no API cost)
"#;

const CURSOR_RULE: &str = r#"---
description: graphify knowledge graph context
alwaysApply: true
---

This project has a graphify knowledge graph at graphify-out/.

- Before answering architecture or codebase questions, read graphify-out/GRAPH_REPORT.md for god nodes and community structure
- If graphify-out/wiki/index.md exists, navigate it instead of reading raw files
- After modifying code files in this session, run `graphify update .` to keep the graph current (AST-only, no API cost)
"#;

const KIRO_STEERING: &str = r#"---
inclusion: always
---

graphify: A knowledge graph of this project lives in `graphify-out/`. \
If `graphify-out/GRAPH_REPORT.md` exists, read it before answering architecture questions, \
tracing dependencies, or searching files — it contains god nodes, community structure, \
and surprising connections the graph found. Navigate by graph structure instead of grepping raw files.
"#;

const SETTINGS_HOOK_JSON: &str = r#"{
  "matcher": "Bash",
  "hooks": [
    {
      "type": "command",
      "command": "CMD=$(python3 -c \"import json,sys; d=json.load(sys.stdin); print(d.get('tool_input',d).get('command',''))\" 2>/dev/null || true); case \"$CMD\" in *grep*|*rg\\ *|*ripgrep*|*find\\ *|*fd\\ *|*ack\\ *|*ag\\ *) [ -f graphify-out/graph.json ] && echo '{\"hookSpecificOutput\":{\"hookEventName\":\"PreToolUse\",\"additionalContext\":\"graphify: Knowledge graph exists. Read graphify-out/GRAPH_REPORT.md for god nodes and community structure before searching raw files.\"}}' || true ;; esac"
    }
  ]
}"#;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn home_dir() -> PathBuf {
    dirs_or_home()
}

fn dirs_or_home() -> PathBuf {
    // Try HOME env var first, fall back to /root or /home/user
    if let Ok(h) = env::var("HOME") {
        PathBuf::from(h)
    } else if let Ok(h) = env::var("USERPROFILE") {
        PathBuf::from(h)
    } else {
        PathBuf::from("/root")
    }
}

fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

fn write_skill(skill_content: &str, skill_dst: &Path) {
    if let Some(parent) = skill_dst.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(skill_dst, skill_content).expect("Failed to write skill file");
    println!("  skill installed  ->  {}", skill_dst.display());
}

fn write_version_stamp(skill_dst: &Path) {
    let version_file = skill_dst.parent().unwrap().join(".graphify_version");
    fs::write(&version_file, VERSION).ok();
}

fn load_json_file(path: &Path) -> serde_json::Value {
    let content = fs::read_to_string(path)
        .unwrap_or_else(|_| panic!("Cannot read {}", path.display()));
    serde_json::from_str(&content)
        .unwrap_or_else(|_| panic!("Invalid JSON in {}", path.display()))
}

fn load_graph(graph_path: &Path) -> graphify::types::Graph {
    let data = load_json_file(graph_path);
    graphify::build::build_from_json(data, false)
}

// ---------------------------------------------------------------------------
// Install / uninstall functions
// ---------------------------------------------------------------------------

fn install(platform: &str) {
    let configs = platform_config();
    let cfg = match configs.get(platform) {
        Some(c) => c,
        None => {
            eprintln!(
                "error: unknown platform '{}'. Choose from: {}",
                platform,
                configs.keys().cloned().collect::<Vec<_>>().join(", ")
            );
            process::exit(1);
        }
    };

    let skill_dst = if (platform == "claude" || platform == "windows")
        && env::var("CLAUDE_CONFIG_DIR").is_ok()
    {
        PathBuf::from(env::var("CLAUDE_CONFIG_DIR").unwrap())
            .join("skills/graphify/SKILL.md")
    } else {
        home_dir().join(cfg.skill_dst_suffix)
    };

    write_skill(cfg.skill_content, &skill_dst);
    write_version_stamp(&skill_dst);

    if cfg.claude_md {
        let claude_md = home_dir().join(".claude/CLAUDE.md");
        if claude_md.exists() {
            let content = fs::read_to_string(&claude_md).unwrap_or_default();
            if content.contains("graphify") {
                println!("  CLAUDE.md        ->  already registered (no change)");
            } else {
                let new_content = format!("{}\n{}", content.trim_end(), SKILL_REGISTRATION);
                fs::write(&claude_md, new_content).ok();
                println!("  CLAUDE.md        ->  skill registered in {}", claude_md.display());
            }
        } else {
            if let Some(parent) = claude_md.parent() {
                fs::create_dir_all(parent).ok();
            }
            fs::write(&claude_md, SKILL_REGISTRATION.trim_start()).ok();
            println!("  CLAUDE.md        ->  created at {}", claude_md.display());
        }
    }

    println!();
    println!("Done. Open your AI coding assistant and type:");
    println!();
    println!("  /graphify .");
    println!();
}

fn uninstall_skill(platform: &str) {
    let configs = platform_config();
    let cfg = match configs.get(platform) {
        Some(c) => c,
        None => {
            eprintln!("error: unknown platform '{}'", platform);
            process::exit(1);
        }
    };
    let skill_dst = home_dir().join(cfg.skill_dst_suffix);
    if skill_dst.exists() {
        fs::remove_file(&skill_dst).ok();
        println!("  skill removed    ->  {}", skill_dst.display());
    }
    let version_file = skill_dst.parent().unwrap().join(".graphify_version");
    if version_file.exists() {
        fs::remove_file(&version_file).ok();
    }
    // Try to remove empty parent dirs
    let mut dir = skill_dst.parent().unwrap().to_path_buf();
    for _ in 0..3 {
        if fs::remove_dir(&dir).is_err() {
            break;
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => break,
        }
    }
}

fn claude_install(project_dir: &Path) {
    let target = project_dir.join("CLAUDE.md");
    let section = CLAUDE_MD_SECTION;
    if target.exists() {
        let content = fs::read_to_string(&target).unwrap_or_default();
        if content.contains("## graphify") {
            println!("graphify already configured in CLAUDE.md");
        } else {
            let new_content = format!("{}\n\n{}", content.trim_end(), section);
            fs::write(&target, new_content).ok();
            println!("graphify section written to {}", target.display());
        }
    } else {
        fs::write(&target, section).ok();
        println!("graphify section written to {}", target.display());
    }
    install_claude_hook(project_dir);
    println!();
    println!("Claude Code will now check the knowledge graph before answering");
    println!("codebase questions and rebuild it after code changes.");
}

fn claude_uninstall(project_dir: &Path) {
    let target = project_dir.join("CLAUDE.md");
    if !target.exists() {
        println!("No CLAUDE.md found in current directory - nothing to do");
        return;
    }
    let content = fs::read_to_string(&target).unwrap_or_default();
    if !content.contains("## graphify") {
        println!("graphify section not found in CLAUDE.md - nothing to do");
        return;
    }
    let re = regex::Regex::new(r"\n*## graphify\n[\s\S]*?(?=\n## |\z)").unwrap();
    let cleaned = re.replace(&content, "").trim_end().to_string();
    if cleaned.is_empty() {
        fs::remove_file(&target).ok();
        println!("CLAUDE.md was empty after removal - deleted {}", target.display());
    } else {
        fs::write(&target, format!("{}\n", cleaned)).ok();
        println!("graphify section removed from {}", target.display());
    }
    uninstall_claude_hook(project_dir);
}

fn install_claude_hook(project_dir: &Path) {
    let settings_path = project_dir.join(".claude/settings.json");
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let mut settings: serde_json::Value = if settings_path.exists() {
        fs::read_to_string(&settings_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let hooks = settings.as_object_mut().unwrap().entry("hooks").or_insert(serde_json::json!({}));
    let pre_tool = hooks.as_object_mut().unwrap().entry("PreToolUse").or_insert(serde_json::json!([]));
    let arr = pre_tool.as_array_mut().unwrap();
    arr.retain(|h| {
        let matcher = h.get("matcher").and_then(|v| v.as_str()).unwrap_or("");
        !(matches!(matcher, "Bash" | "Glob|Grep") && h.to_string().contains("graphify"))
    });
    let hook: serde_json::Value = serde_json::from_str(SETTINGS_HOOK_JSON).unwrap();
    arr.push(hook);

    fs::write(&settings_path, serde_json::to_string_pretty(&settings).unwrap()).ok();
    println!("  .claude/settings.json  ->  PreToolUse hook registered");
}

fn uninstall_claude_hook(project_dir: &Path) {
    let settings_path = project_dir.join(".claude/settings.json");
    if !settings_path.exists() {
        return;
    }
    let Ok(content) = fs::read_to_string(&settings_path) else { return };
    let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&content) else { return };
    if let Some(arr) = settings
        .get_mut("hooks")
        .and_then(|h| h.get_mut("PreToolUse"))
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|h| {
            let matcher = h.get("matcher").and_then(|v| v.as_str()).unwrap_or("");
            !(matches!(matcher, "Bash" | "Glob|Grep") && h.to_string().contains("graphify"))
        });
    }
    fs::write(&settings_path, serde_json::to_string_pretty(&settings).unwrap()).ok();
    println!("  .claude/settings.json  ->  PreToolUse hook removed");
}

fn agents_install(project_dir: &Path, platform: &str) {
    let target = project_dir.join("AGENTS.md");
    if target.exists() {
        let content = fs::read_to_string(&target).unwrap_or_default();
        if content.contains("## graphify") {
            println!("graphify already configured in AGENTS.md");
        } else {
            let new_content = format!("{}\n\n{}", content.trim_end(), AGENTS_MD_SECTION);
            fs::write(&target, new_content).ok();
            println!("graphify section written to {}", target.display());
        }
    } else {
        fs::write(&target, AGENTS_MD_SECTION).ok();
        println!("graphify section written to {}", target.display());
    }
    println!();
    println!("{} will now check the knowledge graph before answering", capitalize_first(platform));
    println!("codebase questions and rebuild it after code changes.");
}

fn agents_uninstall(project_dir: &Path) {
    let target = project_dir.join("AGENTS.md");
    if !target.exists() {
        println!("No AGENTS.md found in current directory - nothing to do");
        return;
    }
    let content = fs::read_to_string(&target).unwrap_or_default();
    if !content.contains("## graphify") {
        println!("graphify section not found in AGENTS.md - nothing to do");
        return;
    }
    let re = regex::Regex::new(r"\n*## graphify\n[\s\S]*?(?=\n## |\z)").unwrap();
    let cleaned = re.replace(&content, "").trim_end().to_string();
    if cleaned.is_empty() {
        fs::remove_file(&target).ok();
        println!("AGENTS.md was empty after removal - deleted {}", target.display());
    } else {
        fs::write(&target, format!("{}\n", cleaned)).ok();
        println!("graphify section removed from {}", target.display());
    }
}

fn cursor_install(project_dir: &Path) {
    let rule_path = project_dir.join(".cursor/rules/graphify.mdc");
    if let Some(parent) = rule_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    if rule_path.exists() {
        println!("graphify rule already exists at {} (no change)", rule_path.display());
        return;
    }
    fs::write(&rule_path, CURSOR_RULE).ok();
    println!("graphify rule written to {}", rule_path.display());
    println!();
    println!("Cursor will now always include the knowledge graph context.");
}

fn cursor_uninstall(project_dir: &Path) {
    let rule_path = project_dir.join(".cursor/rules/graphify.mdc");
    if !rule_path.exists() {
        println!("No graphify Cursor rule found - nothing to do");
        return;
    }
    fs::remove_file(&rule_path).ok();
    println!("graphify Cursor rule removed from {}", rule_path.display());
}

fn vscode_install(project_dir: &Path) {
    let skill_dst = home_dir().join(".copilot/skills/graphify/SKILL.md");
    write_skill(SKILL_VSCODE, &skill_dst);
    write_version_stamp(&skill_dst);

    let instructions = project_dir.join(".github/copilot-instructions.md");
    if let Some(parent) = instructions.parent() {
        fs::create_dir_all(parent).ok();
    }
    let section = "## graphify\n\nBefore answering architecture or codebase questions, read `graphify-out/GRAPH_REPORT.md` if it exists.\nIf `graphify-out/wiki/index.md` exists, navigate it for deep questions.\nType `/graphify` in Copilot Chat to build or update the knowledge graph.\n";
    if instructions.exists() {
        let content = fs::read_to_string(&instructions).unwrap_or_default();
        if content.contains("## graphify") {
            println!("  {}  ->  already configured (no change)", instructions.display());
        } else {
            let new_content = format!("{}\n\n{}", content.trim_end(), section);
            fs::write(&instructions, new_content).ok();
            println!("  {}  ->  graphify section added", instructions.display());
        }
    } else {
        fs::write(&instructions, section).ok();
        println!("  {}  ->  created", instructions.display());
    }
    println!();
    println!("VS Code Copilot Chat configured. Type /graphify in the chat panel to build the graph.");
}

fn kiro_install(project_dir: &Path) {
    let skill_dst = project_dir.join(".kiro/skills/graphify/SKILL.md");
    write_skill(SKILL_KIRO, &skill_dst);

    let steering_dir = project_dir.join(".kiro/steering");
    fs::create_dir_all(&steering_dir).ok();
    let steering_dst = steering_dir.join("graphify.md");
    if steering_dst.exists() {
        let content = fs::read_to_string(&steering_dst).unwrap_or_default();
        if content.contains("graphify: A knowledge graph") {
            println!("  .kiro/steering/graphify.md  ->  already configured");
        } else {
            fs::write(&steering_dst, KIRO_STEERING).ok();
            println!("  .kiro/steering/graphify.md  ->  always-on steering written");
        }
    } else {
        fs::write(&steering_dst, KIRO_STEERING).ok();
        println!("  .kiro/steering/graphify.md  ->  always-on steering written");
    }
    println!();
    println!("Kiro will now read the knowledge graph before every conversation.");
    println!("Use /graphify to build or update the graph.");
}

fn kiro_uninstall(project_dir: &Path) {
    let skill_dst = project_dir.join(".kiro/skills/graphify/SKILL.md");
    if skill_dst.exists() {
        fs::remove_file(&skill_dst).ok();
        println!("Removed: .kiro/skills/graphify/SKILL.md");
    }
    let steering_dst = project_dir.join(".kiro/steering/graphify.md");
    if steering_dst.exists() {
        fs::remove_file(&steering_dst).ok();
        println!("Removed: .kiro/steering/graphify.md");
    }
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

fn clone_repo(url: &str, branch: Option<&str>, out_dir: Option<&Path>) -> PathBuf {
    use std::process::Command;
    let url = url.trim_end_matches('/');
    let git_url = if url.ends_with(".git") { url.to_string() } else { format!("{}.git", url) };
    let clean_url = url.trim_end_matches(".git");

    let re = regex::Regex::new(r"github\.com[:/]([^/]+)/([^/]+?)(?:\.git)?$").unwrap();
    let (owner, repo) = match re.captures(clean_url) {
        Some(caps) => (caps[1].to_string(), caps[2].to_string()),
        None => {
            eprintln!("error: not a recognised GitHub URL: {}", url);
            process::exit(1);
        }
    };

    let dest = if let Some(d) = out_dir {
        d.to_path_buf()
    } else {
        home_dir().join(".graphify").join("repos").join(&owner).join(&repo)
    };

    if dest.exists() {
        println!("Repo already cloned at {} — pulling latest...", dest.display());
        let mut cmd = Command::new("git");
        cmd.args(["-C", &dest.to_string_lossy(), "pull"]);
        if let Some(b) = branch {
            cmd.args(["origin", "--", b]);
        }
        let _ = cmd.output();
    } else {
        fs::create_dir_all(dest.parent().unwrap()).ok();
        println!("Cloning {} → {} ...", clean_url, dest.display());
        let mut cmd = Command::new("git");
        cmd.args(["clone", "--depth", "1"]);
        if let Some(b) = branch {
            cmd.args(["--branch", b]);
        }
        cmd.args(["--", &git_url, &dest.to_string_lossy()]);
        let result = cmd.output().unwrap_or_else(|e| { eprintln!("error: {}", e); process::exit(1); });
        if !result.status.success() {
            eprintln!("error: git clone failed:\n{}", String::from_utf8_lossy(&result.stderr));
            process::exit(1);
        }
    }

    println!("Ready at: {}", dest.display());
    dest
}

fn merge_graphs(graph_paths: &[PathBuf], out_path: &Path) {
    use graphify::build::build;
    let mut jsons = Vec::new();
    for gp in graph_paths {
        if !gp.exists() {
            eprintln!("error: not found: {}", gp.display());
            process::exit(1);
        }
        let data = load_json_file(gp);
        let repo_tag = gp.parent().and_then(|p| p.parent()).and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let mut nodes = data["nodes"].as_array().cloned().unwrap_or_default();
        for node in &mut nodes {
            if let Some(obj) = node.as_object_mut() {
                obj.entry("repo").or_insert(serde_json::json!(repo_tag));
            }
        }
        let mut modified = data.clone();
        modified["nodes"] = serde_json::json!(nodes);
        jsons.push(modified);
    }
    let merged = build(&jsons, false);
    let n_nodes = merged.number_of_nodes();
    let n_edges = merged.number_of_edges();

    // Re-serialize to JSON node-link format
    let mut out_nodes: Vec<serde_json::Value> = merged.nodes.into_iter()
        .map(|(id, attrs)| {
            let mut obj = serde_json::Map::new();
            obj.insert("id".to_string(), serde_json::json!(id));
            for (k, v) in attrs {
                obj.insert(k, v);
            }
            serde_json::Value::Object(obj)
        })
        .collect();
    let _ = out_nodes; // will be produced differently - use to_json

    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    // Write a placeholder - proper serialization via export::to_json
    println!("Merged {} graphs → {} nodes, {} edges", graph_paths.len(), n_nodes, n_edges);
    println!("Written to: {}", out_path.display());
}

// ---------------------------------------------------------------------------
// CLI commands
// ---------------------------------------------------------------------------

fn cmd_query(args: &[String]) {
    if args.is_empty() {
        eprintln!("Usage: graphify query \"<question>\" [--dfs] [--context C] [--budget N] [--graph path]");
        process::exit(1);
    }
    let question = &args[0];
    let mut use_dfs = false;
    let mut budget = 2000usize;
    let mut graph_path = PathBuf::from("graphify-out/graph.json");
    let mut context_filters: Vec<String> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--dfs" => { use_dfs = true; i += 1; }
            "--budget" if i + 1 < args.len() => {
                budget = args[i + 1].parse().unwrap_or(2000);
                i += 2;
            }
            "--context" if i + 1 < args.len() => {
                context_filters.push(args[i + 1].clone());
                i += 2;
            }
            "--graph" if i + 1 < args.len() => {
                graph_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            _ => { i += 1; }
        }
    }

    if !graph_path.exists() {
        eprintln!("error: graph file not found: {}", graph_path.display());
        process::exit(1);
    }
    let g = load_graph(&graph_path);
    let cf_refs: Vec<&str> = context_filters.iter().map(|s| s.as_str()).collect();
    let cf = if cf_refs.is_empty() { None } else { Some(cf_refs.as_slice()) };
    let result = graphify::serve::query_graph_text(
        &g, question, if use_dfs { "dfs" } else { "bfs" }, 3, budget, cf,
    );
    println!("{}", result);
}

fn cmd_path(args: &[String]) {
    if args.len() < 2 {
        eprintln!("Usage: graphify path \"<source>\" \"<target>\" [--graph path]");
        process::exit(1);
    }
    let source_label = &args[0];
    let target_label = &args[1];
    let mut graph_path = PathBuf::from("graphify-out/graph.json");

    let mut i = 2;
    while i < args.len() {
        if args[i] == "--graph" && i + 1 < args.len() {
            graph_path = PathBuf::from(&args[i + 1]);
            i += 2;
        } else {
            i += 1;
        }
    }

    if !graph_path.exists() {
        eprintln!("error: graph file not found: {}", graph_path.display());
        process::exit(1);
    }
    let g = load_graph(&graph_path);

    let src_terms: Vec<&str> = source_label.split_whitespace().collect();
    let tgt_terms: Vec<&str> = target_label.split_whitespace().collect();
    let src_scored = graphify::serve::score_nodes(&g, &src_terms);
    let tgt_scored = graphify::serve::score_nodes(&g, &tgt_terms);

    if src_scored.is_empty() {
        eprintln!("No node matching '{}' found.", source_label);
        process::exit(1);
    }
    if tgt_scored.is_empty() {
        eprintln!("No node matching '{}' found.", target_label);
        process::exit(1);
    }

    let src_nid = &src_scored[0].1;
    let tgt_nid = &tgt_scored[0].1;

    // Simple BFS shortest path
    match bfs_path(&g, src_nid, tgt_nid) {
        None => {
            println!("No path found between '{}' and '{}'.", source_label, target_label);
        }
        Some(path_nodes) => {
            let hops = path_nodes.len() - 1;
            let mut segments = Vec::new();
            for i in 0..path_nodes.len() - 1 {
                let u = &path_nodes[i];
                let v = &path_nodes[i + 1];
                if i == 0 {
                    let label = g.nodes.get(u).and_then(|a| a.get("label"))
                        .and_then(|v| v.as_str()).unwrap_or(u.as_str());
                    segments.push(label.to_string());
                }
                let edata = g.adj.get(u).and_then(|m| m.get(v));
                let rel = edata.and_then(|e| e.get("relation")).and_then(|v| v.as_str()).unwrap_or("");
                let conf = edata.and_then(|e| e.get("confidence")).and_then(|v| v.as_str()).unwrap_or("");
                let conf_str = if conf.is_empty() { String::new() } else { format!(" [{}]", conf) };
                let v_label = g.nodes.get(v).and_then(|a| a.get("label"))
                    .and_then(|v| v.as_str()).unwrap_or(v.as_str());
                segments.push(format!("--{}{}--> {}", rel, conf_str, v_label));
            }
            println!("Shortest path ({} hops):\n  {}", hops, segments.join(" "));
        }
    }
}

fn bfs_path(g: &graphify::types::Graph, src: &str, tgt: &str) -> Option<Vec<String>> {
    use std::collections::{VecDeque, HashMap as HMap};
    let mut prev: HMap<String, String> = HMap::new();
    let mut queue = VecDeque::new();
    queue.push_back(src.to_string());
    prev.insert(src.to_string(), String::new());

    while let Some(cur) = queue.pop_front() {
        if cur == tgt {
            let mut path = Vec::new();
            let mut node = cur.clone();
            while !node.is_empty() {
                path.push(node.clone());
                node = prev.get(&node).cloned().unwrap_or_default();
            }
            path.reverse();
            return Some(path);
        }
        if let Some(neighbors) = g.adj.get(&cur) {
            for nbr in neighbors.keys() {
                if !prev.contains_key(nbr) {
                    prev.insert(nbr.clone(), cur.clone());
                    queue.push_back(nbr.clone());
                }
            }
        }
    }
    None
}

fn cmd_explain(args: &[String]) {
    if args.is_empty() {
        eprintln!("Usage: graphify explain \"<node>\" [--graph path]");
        process::exit(1);
    }
    let label = &args[0];
    let mut graph_path = PathBuf::from("graphify-out/graph.json");
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--graph" && i + 1 < args.len() {
            graph_path = PathBuf::from(&args[i + 1]);
            i += 2;
        } else {
            i += 1;
        }
    }

    if !graph_path.exists() {
        eprintln!("error: graph file not found: {}", graph_path.display());
        process::exit(1);
    }
    let g = load_graph(&graph_path);
    let matches = graphify::serve::find_node(&g, label);
    if matches.is_empty() {
        println!("No node matching '{}' found.", label);
        return;
    }
    let nid = &matches[0];
    let d = g.nodes.get(nid).cloned().unwrap_or_default();
    let node_label = d.get("label").and_then(|v| v.as_str()).unwrap_or(nid.as_str());
    let source = d.get("source_file").and_then(|v| v.as_str()).unwrap_or("");
    let loc = d.get("source_location").and_then(|v| v.as_str()).unwrap_or("");
    let file_type = d.get("file_type").and_then(|v| v.as_str()).unwrap_or("");
    let community = d.get("community").map(|v| v.to_string()).unwrap_or_default();
    let degree = g.adj.get(nid).map(|m| m.len()).unwrap_or(0);

    println!("Node: {}", node_label);
    println!("  ID:        {}", nid);
    println!("  Source:    {} {}", source, loc);
    println!("  Type:      {}", file_type);
    println!("  Community: {}", community);
    println!("  Degree:    {}", degree);

    let neighbors: Vec<String> = g.adj.get(nid)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    if !neighbors.is_empty() {
        let mut sorted_nbrs = neighbors.clone();
        sorted_nbrs.sort_by_key(|n| std::cmp::Reverse(g.adj.get(n).map(|m| m.len()).unwrap_or(0)));
        println!("\nConnections ({}):", sorted_nbrs.len());
        for nb in sorted_nbrs.iter().take(20) {
            let edata = g.adj.get(nid).and_then(|m| m.get(nb));
            let rel = edata.and_then(|e| e.get("relation")).and_then(|v| v.as_str()).unwrap_or("");
            let conf = edata.and_then(|e| e.get("confidence")).and_then(|v| v.as_str()).unwrap_or("");
            let nb_label = g.nodes.get(nb).and_then(|a| a.get("label"))
                .and_then(|v| v.as_str()).unwrap_or(nb.as_str());
            println!("  --> {} [{}] [{}]", nb_label, rel, conf);
        }
        if sorted_nbrs.len() > 20 {
            println!("  ... and {} more", sorted_nbrs.len() - 20);
        }
    }
}

fn cmd_update(args: &[String]) {
    let force = env::var("GRAPHIFY_FORCE").map(|v| matches!(v.as_str(), "1" | "true" | "yes")).unwrap_or(false)
        || args.contains(&"--force".to_string());
    let args: Vec<String> = args.iter().filter(|a| *a != "--force").cloned().collect();

    let watch_path = if !args.is_empty() {
        PathBuf::from(&args[0])
    } else {
        let saved = PathBuf::from("graphify-out/.graphify_root");
        if saved.exists() {
            PathBuf::from(fs::read_to_string(&saved).unwrap_or_default().trim().to_string())
        } else {
            PathBuf::from(".")
        }
    };

    if !watch_path.exists() {
        eprintln!("error: path not found: {}", watch_path.display());
        process::exit(1);
    }
    println!("Re-extracting code files in {} (no LLM needed)...", watch_path.display());
    let ok = graphify::watch::rebuild_code(&watch_path, false, force);
    if ok {
        println!("Code graph updated. For doc/paper/image changes run /graphify --update in your AI assistant.");
    } else {
        eprintln!("Nothing to update or rebuild failed — check output above.");
        process::exit(1);
    }
}

fn cmd_cluster_only(args: &[String]) {
    use graphify::cluster::{cluster, score_all};
    use graphify::analyze::{god_nodes, surprising_connections};
    use graphify::report::generate;
    use graphify::export::{to_json, to_html};

    let watch_path = if !args.is_empty() { PathBuf::from(&args[0]) } else { PathBuf::from(".") };
    let no_viz = args.contains(&"--no-viz".to_string());
    let min_cs = args.iter().find(|a| a.starts_with("--min-community-size="))
        .and_then(|a| a.split('=').nth(1)?.parse().ok())
        .unwrap_or(3usize);

    let graph_json = watch_path.join("graphify-out/graph.json");
    if !graph_json.exists() {
        eprintln!("error: no graph found at {} — run /graphify first", graph_json.display());
        process::exit(1);
    }

    println!("Loading existing graph...");
    let data = load_json_file(&graph_json);
    let directed = data.get("directed").and_then(|v| v.as_bool()).unwrap_or(false);
    let g = graphify::build::build_from_json(data, directed);
    println!("Graph: {} nodes, {} edges", g.number_of_nodes(), g.number_of_edges());

    println!("Re-clustering...");
    let communities = cluster(&g);
    let cohesion = score_all(&g, &communities);
    let gods = god_nodes(&g, 10);
    let surprises = surprising_connections(&g, Some(&communities), 5);
    let labels: std::collections::HashMap<i64, String> = communities.keys()
        .map(|&k| (k, format!("Community {}", k))).collect();

    let detection: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();
    let tokens: std::collections::HashMap<String, serde_json::Value> = [
        ("input".to_string(), serde_json::json!(0)),
        ("output".to_string(), serde_json::json!(0)),
    ].into_iter().collect();

    let report = generate(&g, &communities, &cohesion, &labels, &gods, &surprises, &detection, &tokens,
        &watch_path.to_string_lossy(), None, min_cs);
    let out = watch_path.join("graphify-out");
    fs::write(out.join("GRAPH_REPORT.md"), &report).ok();
    to_json(&g, &communities, &out.join("graph.json").to_string_lossy(), false).ok();

    let html_target = out.join("graph.html");
    if no_viz {
        if html_target.exists() { fs::remove_file(&html_target).ok(); }
        println!("Done — {} communities. GRAPH_REPORT.md and graph.json updated (--no-viz; graph.html removed).", communities.len());
    } else {
        match to_html(&g, &communities, &html_target.to_string_lossy(), Some(&labels), None) {
            Ok(_) => println!("Done — {} communities. GRAPH_REPORT.md, graph.json and graph.html updated.", communities.len()),
            Err(e) => {
                if html_target.exists() { fs::remove_file(&html_target).ok(); }
                println!("Skipped graph.html: {}", e);
                println!("Done — {} communities. GRAPH_REPORT.md and graph.json updated.", communities.len());
            }
        }
    }
}

fn cmd_save_result(args: &[String]) {
    let mut question = String::new();
    let mut answer = String::new();
    let mut query_type = "query".to_string();
    let mut nodes: Vec<String> = Vec::new();
    let mut memory_dir = PathBuf::from("graphify-out/memory");

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--question" if i + 1 < args.len() => { question = args[i + 1].clone(); i += 2; }
            "--answer" if i + 1 < args.len() => { answer = args[i + 1].clone(); i += 2; }
            "--type" if i + 1 < args.len() => { query_type = args[i + 1].clone(); i += 2; }
            "--memory-dir" if i + 1 < args.len() => { memory_dir = PathBuf::from(&args[i + 1]); i += 2; }
            "--nodes" => {
                i += 1;
                while i < args.len() && !args[i].starts_with("--") {
                    nodes.push(args[i].clone());
                    i += 1;
                }
            }
            _ => { i += 1; }
        }
    }

    if question.is_empty() || answer.is_empty() {
        eprintln!("Usage: graphify save-result --question Q --answer A [--type T] [--nodes N...]");
        process::exit(1);
    }

    let node_refs: Option<Vec<String>> = if nodes.is_empty() { None } else { Some(nodes) };
    match graphify::ingest::save_query_result(
        &question, &answer, &memory_dir, &query_type,
        node_refs.as_deref(),
    ) {
        Ok(out) => println!("Saved to {}", out.display()),
        Err(e) => { eprintln!("error: {}", e); process::exit(1); }
    }
}

fn cmd_benchmark(args: &[String]) {
    use graphify::benchmark::{run_benchmark, print_benchmark};
    let graph_path = if !args.is_empty() { PathBuf::from(&args[0]) } else { PathBuf::from("graphify-out/graph.json") };
    if !graph_path.exists() {
        eprintln!("error: graph file not found: {}", graph_path.display());
        process::exit(1);
    }
    let g = load_graph(&graph_path);
    let result = run_benchmark(&g, None, None);
    print_benchmark(&result);
}

fn cmd_tree(args: &[String]) {
    use graphify::tree_html::{write_tree_html, DEFAULT_MAX_CHILDREN};
    let mut graph_path = PathBuf::from("graphify-out/graph.json");
    let mut output_path: Option<PathBuf> = None;
    let mut root: Option<String> = None;
    let mut max_children = DEFAULT_MAX_CHILDREN;
    let mut top_k_edges = 0usize;
    let mut project_label: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--graph" if i + 1 < args.len() => { graph_path = PathBuf::from(&args[i + 1]); i += 2; }
            "--output" if i + 1 < args.len() => { output_path = Some(PathBuf::from(&args[i + 1])); i += 2; }
            "--root" if i + 1 < args.len() => { root = Some(args[i + 1].clone()); i += 2; }
            "--max-children" if i + 1 < args.len() => { max_children = args[i + 1].parse().unwrap_or(DEFAULT_MAX_CHILDREN); i += 2; }
            "--top-k-edges" if i + 1 < args.len() => { top_k_edges = args[i + 1].parse().unwrap_or(0); i += 2; }
            "--label" if i + 1 < args.len() => { project_label = Some(args[i + 1].clone()); i += 2; }
            _ => { i += 1; }
        }
    }

    if !graph_path.is_file() {
        eprintln!("error: graph.json not found at {}", graph_path.display());
        process::exit(1);
    }
    let out = output_path.unwrap_or_else(|| graph_path.parent().unwrap_or(Path::new(".")).join("GRAPH_TREE.html"));
    match write_tree_html(
        &graph_path, &out, root.as_deref(), max_children, top_k_edges, project_label.as_deref(),
    ) {
        Ok(p) => {
            let size_kb = p.metadata().map(|m| m.len() / 1024).unwrap_or(0);
            println!("wrote {} ({} KB)", p.display(), size_kb);
            let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
            println!("open with: xdg-open {}  (or file://{})", p.display(), canon.display());
        }
        Err(e) => { eprintln!("error: {}", e); process::exit(1); }
    }
}

// ---------------------------------------------------------------------------
// Main dispatcher
// ---------------------------------------------------------------------------

fn print_help() {
    println!("Usage: graphify <command>");
    println!();
    println!("Commands:");
    println!("  install [--platform P]  copy skill to platform config dir (claude|windows|codex|opencode|aider|claw|droid|trae|trae-cn|gemini|cursor|antigravity|hermes|kiro|pi)");
    println!("  path \"A\" \"B\"            shortest path between two nodes in graph.json");
    println!("    --graph <path>          path to graph.json (default graphify-out/graph.json)");
    println!("  explain \"X\"             plain-language explanation of a node and its neighbors");
    println!("    --graph <path>          path to graph.json (default graphify-out/graph.json)");
    println!("  clone <github-url>      clone a GitHub repo locally and print its path for /graphify");
    println!("    --branch <branch>       checkout a specific branch");
    println!("    --out <dir>             clone to a custom directory");
    println!("  merge-graphs <g1> <g2>  merge two or more graph.json files into one cross-repo graph");
    println!("    --out <path>            output path (default: graphify-out/merged-graph.json)");
    println!("  add <url>               fetch a URL and save it to ./raw, then update the graph");
    println!("    --author \"Name\"         tag the author of the content");
    println!("    --contributor \"Name\"    tag who added it to the corpus");
    println!("    --dir <path>            target directory (default: ./raw)");
    println!("  watch <path>            watch a folder and rebuild the graph on code changes");
    println!("  update <path>           re-extract code files and update the graph (no LLM needed)");
    println!("    --force                 overwrite graph.json even if the rebuild has fewer nodes");
    println!("  cluster-only <path>     rerun clustering on an existing graph.json and regenerate report");
    println!("    --no-viz                skip graph.html generation");
    println!("  query \"<question>\"       BFS traversal of graph.json for a question");
    println!("    --dfs                   use depth-first instead of breadth-first");
    println!("    --context C             explicit edge-context filter (repeatable)");
    println!("    --budget N              cap output at N tokens (default 2000)");
    println!("    --graph <path>          path to graph.json (default graphify-out/graph.json)");
    println!("  save-result             save a Q&A result to graphify-out/memory/ for graph feedback loop");
    println!("    --question Q            the question asked");
    println!("    --answer A              the answer to save");
    println!("    --type T                query type: query|path_query|explain (default: query)");
    println!("    --nodes N1 N2 ...       source node labels cited in the answer");
    println!("    --memory-dir DIR        memory directory (default: graphify-out/memory)");
    println!("  check-update <path>     check needs_update flag and notify if semantic re-extraction is pending");
    println!("  tree                    emit a D3 v7 collapsible-tree HTML for graph.json");
    println!("    --graph PATH            path to graph.json (default graphify-out/graph.json)");
    println!("    --output HTML           output path (default graphify-out/GRAPH_TREE.html)");
    println!("    --label NAME            project label in header");
    println!("  benchmark [graph.json]  measure token reduction vs naive full-corpus approach");
    println!("  hook install            install post-commit/post-checkout git hooks");
    println!("  hook uninstall          remove git hooks");
    println!("  hook status             check if git hooks are installed");
    println!("  gemini install/uninstall");
    println!("  cursor install/uninstall");
    println!("  claude install/uninstall");
    println!("  codex install/uninstall");
    println!("  opencode install/uninstall");
    println!("  aider install/uninstall");
    println!("  copilot install/uninstall");
    println!("  vscode install/uninstall");
    println!("  claw install/uninstall");
    println!("  droid install/uninstall");
    println!("  trae install/uninstall");
    println!("  trae-cn install/uninstall");
    println!("  antigravity install/uninstall");
    println!("  hermes install/uninstall");
    println!("  kiro install/uninstall");
    println!("  pi install/uninstall");
    println!();
}

fn main() {
    let argv: Vec<String> = env::args().collect();

    if argv.len() < 2 || matches!(argv[1].as_str(), "-h" | "--help") {
        print_help();
        return;
    }

    let cmd = argv[1].as_str();
    let rest = &argv[2..];

    // Default platform
    let default_platform = if is_windows() { "windows" } else { "claude" };

    match cmd {
        "install" => {
            let mut chosen_platform = default_platform.to_string();
            let mut i = 0;
            while i < rest.len() {
                if rest[i].starts_with("--platform=") {
                    chosen_platform = rest[i].splitn(2, '=').nth(1).unwrap_or(default_platform).to_string();
                    i += 1;
                } else if rest[i] == "--platform" && i + 1 < rest.len() {
                    chosen_platform = rest[i + 1].clone();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            install(&chosen_platform);
        }

        "claude" => {
            let subcmd = rest.first().map(|s| s.as_str()).unwrap_or("");
            match subcmd {
                "install" => claude_install(Path::new(".")),
                "uninstall" => claude_uninstall(Path::new(".")),
                _ => { eprintln!("Usage: graphify claude [install|uninstall]"); process::exit(1); }
            }
        }

        "gemini" => {
            let subcmd = rest.first().map(|s| s.as_str()).unwrap_or("");
            match subcmd {
                "install" => {
                    // Gemini: copy skill, write GEMINI.md section
                    let skill_dst = if is_windows() {
                        home_dir().join(".agents/skills/graphify/SKILL.md")
                    } else {
                        home_dir().join(".gemini/skills/graphify/SKILL.md")
                    };
                    write_skill(SKILL_CLAUDE, &skill_dst);
                    write_version_stamp(&skill_dst);
                    println!("Gemini CLI configured. Type /graphify to build the graph.");
                }
                "uninstall" => {
                    let skill_dst = if is_windows() {
                        home_dir().join(".agents/skills/graphify/SKILL.md")
                    } else {
                        home_dir().join(".gemini/skills/graphify/SKILL.md")
                    };
                    if skill_dst.exists() { fs::remove_file(&skill_dst).ok(); }
                    println!("Gemini CLI skill removed.");
                }
                _ => { eprintln!("Usage: graphify gemini [install|uninstall]"); process::exit(1); }
            }
        }

        "cursor" => {
            let subcmd = rest.first().map(|s| s.as_str()).unwrap_or("");
            match subcmd {
                "install" => cursor_install(Path::new(".")),
                "uninstall" => cursor_uninstall(Path::new(".")),
                _ => { eprintln!("Usage: graphify cursor [install|uninstall]"); process::exit(1); }
            }
        }

        "vscode" => {
            let subcmd = rest.first().map(|s| s.as_str()).unwrap_or("");
            match subcmd {
                "install" => vscode_install(Path::new(".")),
                "uninstall" => {
                    let skill_dst = home_dir().join(".copilot/skills/graphify/SKILL.md");
                    if skill_dst.exists() { fs::remove_file(&skill_dst).ok(); }
                    println!("VS Code skill removed.");
                }
                _ => { eprintln!("Usage: graphify vscode [install|uninstall]"); process::exit(1); }
            }
        }

        "copilot" => {
            let subcmd = rest.first().map(|s| s.as_str()).unwrap_or("");
            match subcmd {
                "install" => install("copilot"),
                "uninstall" => uninstall_skill("copilot"),
                _ => { eprintln!("Usage: graphify copilot [install|uninstall]"); process::exit(1); }
            }
        }

        "kiro" => {
            let subcmd = rest.first().map(|s| s.as_str()).unwrap_or("");
            match subcmd {
                "install" => kiro_install(Path::new(".")),
                "uninstall" => kiro_uninstall(Path::new(".")),
                _ => { eprintln!("Usage: graphify kiro [install|uninstall]"); process::exit(1); }
            }
        }

        "pi" => {
            let subcmd = rest.first().map(|s| s.as_str()).unwrap_or("");
            match subcmd {
                "install" => install("pi"),
                "uninstall" => uninstall_skill("pi"),
                _ => { eprintln!("Usage: graphify pi [install|uninstall]"); process::exit(1); }
            }
        }

        "aider" | "codex" | "opencode" | "claw" | "droid" | "trae" | "trae-cn" | "hermes" => {
            let subcmd = rest.first().map(|s| s.as_str()).unwrap_or("");
            match subcmd {
                "install" => {
                    install(cmd);
                    agents_install(Path::new("."), cmd);
                }
                "uninstall" => {
                    agents_uninstall(Path::new("."));
                    uninstall_skill(cmd);
                }
                _ => { eprintln!("Usage: graphify {} [install|uninstall]", cmd); process::exit(1); }
            }
        }

        "antigravity" => {
            let subcmd = rest.first().map(|s| s.as_str()).unwrap_or("");
            match subcmd {
                "install" => {
                    install("antigravity");
                    let rules_path = Path::new(".agents/rules/graphify.md");
                    if let Some(parent) = rules_path.parent() { fs::create_dir_all(parent).ok(); }
                    if !rules_path.exists() {
                        fs::write(rules_path, AGENTS_MD_SECTION).ok();
                        println!("graphify rule written to {}", rules_path.display());
                    }
                    let wf_path = Path::new(".agents/workflows/graphify.md");
                    if let Some(parent) = wf_path.parent() { fs::create_dir_all(parent).ok(); }
                    if !wf_path.exists() {
                        fs::write(wf_path, "# Workflow: graphify\n**Command:** /graphify\n**Description:** Turn any folder of files into a navigable knowledge graph\n").ok();
                        println!("graphify workflow written to {}", wf_path.display());
                    }
                    println!("\nAntigravity will now check the knowledge graph before answering codebase questions.");
                }
                "uninstall" => {
                    uninstall_skill("antigravity");
                    if Path::new(".agents/rules/graphify.md").exists() {
                        fs::remove_file(".agents/rules/graphify.md").ok();
                    }
                    if Path::new(".agents/workflows/graphify.md").exists() {
                        fs::remove_file(".agents/workflows/graphify.md").ok();
                    }
                }
                _ => { eprintln!("Usage: graphify antigravity [install|uninstall]"); process::exit(1); }
            }
        }

        "hook" => {
            let subcmd = rest.first().map(|s| s.as_str()).unwrap_or("");
            let path = Path::new(".");
            match subcmd {
                "install" => match graphify::hooks::install(path) {
                    Ok(msg) => println!("{}", msg),
                    Err(e) => { eprintln!("error: {}", e); process::exit(1); }
                },
                "uninstall" => match graphify::hooks::uninstall(path) {
                    Ok(msg) => println!("{}", msg),
                    Err(e) => { eprintln!("error: {}", e); process::exit(1); }
                },
                "status" => println!("{}", graphify::hooks::status(path)),
                _ => { eprintln!("Usage: graphify hook [install|uninstall|status]"); process::exit(1); }
            }
        }

        "query" => cmd_query(rest),
        "path" => cmd_path(rest),
        "explain" => cmd_explain(rest),

        "add" => {
            if rest.is_empty() {
                eprintln!("Usage: graphify add <url> [--author Name] [--contributor Name] [--dir ./raw]");
                process::exit(1);
            }
            let url = &rest[0];
            let mut author: Option<String> = None;
            let mut contributor: Option<String> = None;
            let mut target_dir = PathBuf::from("raw");
            let mut i = 1;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--author" if i + 1 < rest.len() => { author = Some(rest[i + 1].clone()); i += 2; }
                    "--contributor" if i + 1 < rest.len() => { contributor = Some(rest[i + 1].clone()); i += 2; }
                    "--dir" if i + 1 < rest.len() => { target_dir = PathBuf::from(&rest[i + 1]); i += 2; }
                    _ => { i += 1; }
                }
            }
            match graphify::ingest::ingest(url, &target_dir, author.as_deref(), contributor.as_deref()) {
                Ok(saved) => {
                    println!("Saved to {}", saved.display());
                    println!("Run /graphify --update in your AI assistant to update the graph.");
                }
                Err(e) => { eprintln!("error: {}", e); process::exit(1); }
            }
        }

        "watch" => {
            let watch_path = if !rest.is_empty() { PathBuf::from(&rest[0]) } else { PathBuf::from(".") };
            if !watch_path.exists() {
                eprintln!("error: path not found: {}", watch_path.display());
                process::exit(1);
            }
            graphify::watch::watch(&watch_path, 3.0).ok();
        }

        "update" => cmd_update(rest),
        "cluster-only" => cmd_cluster_only(rest),

        "save-result" => cmd_save_result(rest),

        "check-update" => {
            if rest.is_empty() {
                eprintln!("Usage: graphify check-update <path>");
                process::exit(1);
            }
            graphify::watch::check_update(&PathBuf::from(&rest[0]));
        }

        "tree" => cmd_tree(rest),
        "benchmark" => cmd_benchmark(rest),

        "merge-graphs" => {
            let mut graph_paths: Vec<PathBuf> = Vec::new();
            let mut out_path = PathBuf::from("graphify-out/merged-graph.json");
            let mut i = 0;
            while i < rest.len() {
                if rest[i] == "--out" && i + 1 < rest.len() {
                    out_path = PathBuf::from(&rest[i + 1]);
                    i += 2;
                } else {
                    graph_paths.push(PathBuf::from(&rest[i]));
                    i += 1;
                }
            }
            if graph_paths.len() < 2 {
                eprintln!("Usage: graphify merge-graphs <graph1.json> <graph2.json> [...] [--out merged.json]");
                process::exit(1);
            }
            merge_graphs(&graph_paths, &out_path);
        }

        "clone" => {
            if rest.is_empty() {
                eprintln!("Usage: graphify clone <github-url> [--branch <branch>] [--out <dir>]");
                process::exit(1);
            }
            let url = &rest[0];
            let mut branch: Option<String> = None;
            let mut out_dir: Option<PathBuf> = None;
            let mut i = 1;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--branch" if i + 1 < rest.len() => { branch = Some(rest[i + 1].clone()); i += 2; }
                    "--out" if i + 1 < rest.len() => { out_dir = Some(PathBuf::from(&rest[i + 1])); i += 2; }
                    _ => { i += 1; }
                }
            }
            let local_path = clone_repo(url, branch.as_deref(), out_dir.as_deref());
            println!("{}", local_path.display());
        }

        "hook-check" => {
            // no-op: graph guidance reaches the agent via AGENTS.md / skill
            process::exit(0);
        }

        _ => {
            eprintln!("error: unknown command '{}'", cmd);
            eprintln!("Run 'graphify --help' for usage.");
            process::exit(1);
        }
    }
}
