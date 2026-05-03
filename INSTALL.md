# Installation

## Quick start

```bash
cargo install graphify
graphify install
```

Then open your AI coding assistant and type `/graphify .`.

## Platform-specific setup

| Platform | Command |
|----------|---------|
| Claude Code (Linux/Mac) | `graphify install` |
| Claude Code (Windows) | `graphify install` (auto-detected) or `graphify install --platform windows` |
| Codex | `graphify install --platform codex` |
| OpenCode | `graphify install --platform opencode` |
| GitHub Copilot CLI | `graphify install --platform copilot` |
| VS Code Copilot Chat | `graphify vscode install` |
| Aider | `graphify install --platform aider` |
| OpenClaw | `graphify install --platform claw` |
| Factory Droid | `graphify install --platform droid` |
| Trae | `graphify install --platform trae` |
| Trae CN | `graphify install --platform trae-cn` |
| Gemini CLI | `graphify install --platform gemini` |
| Hermes | `graphify install --platform hermes` |
| Kiro | `graphify kiro install` |
| Pi | `graphify install --platform pi` |
| Cursor | `graphify cursor install` |
| Google Antigravity | `graphify antigravity install` |

## Claude Code hook (optional)

Automatically keep the graph fresh on every commit:

```bash
graphify claude install   # installs CLAUDE.md entry + PreToolUse hook
graphify hook install     # installs post-commit / post-checkout git hooks
```

## .graphifyignore

Exclude paths from the graph using gitignore syntax:

```
vendor/
node_modules/
dist/
*.generated.py
```

Discovery never crosses a VCS boundary (`.git`, `.hg`, etc.).

## Uninstalling

```bash
graphify claude uninstall
graphify cursor uninstall
graphify kiro uninstall
graphify hook uninstall
```
