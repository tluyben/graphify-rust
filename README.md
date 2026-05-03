# graphify (Rust)

A Rust port of [graphify](https://github.com/safishamsi/graphify) — an AI coding assistant skill that turns any folder of files into a navigable knowledge graph.

Type `/graphify` in Claude Code, Codex, OpenCode, Cursor, Gemini CLI, Copilot, Aider, Kiro, and more. It reads your files, builds a knowledge graph, and gives you back structure you didn't know was there.

**[Installation →](INSTALL.md)**

```
/graphify .
```

```
graphify-out/
├── graph.html       interactive graph — open in any browser
├── GRAPH_REPORT.md  god nodes, surprising connections, suggested questions
├── graph.json       persistent graph — query weeks later without re-reading
└── cache/           SHA256 cache — re-runs only process changed files
```

25 languages via tree-sitter AST. Multimodal: code, PDFs, markdown, images, video.

## Commands

```
graphify install [--platform P]   install skill for your AI assistant
graphify query "<question>"       BFS/DFS traversal of graph.json
graphify path "A" "B"            shortest path between two nodes
graphify explain "X"             explain a node and its neighbors
graphify add <url>               fetch a URL and add to the graph
graphify watch <path>            watch and rebuild on file changes
graphify update <path>           re-extract without LLM
graphify cluster-only <path>     rerun clustering on existing graph.json
graphify merge-graphs <g1> <g2>  merge two graph.json files
graphify clone <github-url>      clone a repo and graph it
graphify benchmark [graph.json]  measure token reduction
graphify hook install|uninstall  manage git hooks
graphify tree                    D3 collapsible-tree HTML
```

## Library usage

```rust
use graphify::{detect::detect, build::build_from_json, cluster::cluster, report::generate};
use std::path::Path;

let result = detect(Path::new("."), false);
// ... extract, build, cluster, report
```

## Notes

- MCP server not included — use the Python version for MCP
- LLM extraction uses Claude Code subagents via the installed skill
- Video transcription requires `yt-dlp` and `faster-whisper`

## License

MIT
