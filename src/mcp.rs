//! MCP stdio server — exposes graph query tools over the Model Context Protocol.
//! Implements the protocol as newline-delimited JSON-RPC 2.0 (no external MCP crate needed).
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::Path;
use serde_json::{json, Map, Value};

use crate::{
    analyze::god_nodes,
    build::build_from_json,
    security::sanitize_label,
    serve::{communities_from_graph, find_node, query_graph_text},
    types::{Graph, NodeAttrs},
};

const PROTOCOL_VERSION: &str = "2024-11-05";

// ---------------------------------------------------------------------------
// Protocol helpers
// ---------------------------------------------------------------------------

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn err(id: &Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tool_ok(text: String) -> Value {
    json!({"content": [{"type": "text", "text": text}]})
}

fn tool_err(text: String) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": true})
}

// ---------------------------------------------------------------------------
// Tool schema definitions
// ---------------------------------------------------------------------------

fn tools_list() -> Value {
    json!([
        {
            "name": "query_graph",
            "description": "Search the knowledge graph using BFS or DFS. Returns relevant nodes and edges as text context.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": {"type": "string", "description": "Natural language question or keyword search"},
                    "mode": {"type": "string", "enum": ["bfs", "dfs"], "default": "bfs",
                             "description": "bfs=broad context, dfs=trace a specific path"},
                    "depth": {"type": "integer", "default": 3, "description": "Traversal depth (1-6)"},
                    "token_budget": {"type": "integer", "default": 2000, "description": "Max output tokens"},
                    "context_filter": {"type": "array", "items": {"type": "string"},
                                       "description": "Optional explicit edge-context filter, e.g. ['call', 'field']"}
                },
                "required": ["question"]
            }
        },
        {
            "name": "get_node",
            "description": "Get full details for a specific node by label or ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "label": {"type": "string", "description": "Node label or ID to look up"}
                },
                "required": ["label"]
            }
        },
        {
            "name": "get_neighbors",
            "description": "Get all direct neighbors of a node with edge details.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "label": {"type": "string"},
                    "relation_filter": {"type": "string", "description": "Optional: filter by relation type"}
                },
                "required": ["label"]
            }
        },
        {
            "name": "get_community",
            "description": "Get all nodes in a community by community ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "community_id": {"type": "integer", "description": "Community ID (0-indexed by size)"}
                },
                "required": ["community_id"]
            }
        },
        {
            "name": "god_nodes",
            "description": "Return the most connected nodes — the core abstractions of the knowledge graph.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "top_n": {"type": "integer", "default": 10}
                }
            }
        },
        {
            "name": "graph_stats",
            "description": "Return summary statistics: node count, edge count, communities, confidence breakdown.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "shortest_path",
            "description": "Find the shortest path between two concepts in the knowledge graph.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": {"type": "string", "description": "Source concept label or keyword"},
                    "target": {"type": "string", "description": "Target concept label or keyword"},
                    "max_hops": {"type": "integer", "default": 8, "description": "Maximum hops to consider"}
                },
                "required": ["source", "target"]
            }
        }
    ])
}

// ---------------------------------------------------------------------------
// BFS shortest-path (self-contained to keep serve.rs clean)
// ---------------------------------------------------------------------------

fn bfs_shortest_path(graph: &Graph, src: &str, tgt: &str) -> Option<Vec<String>> {
    use std::collections::{HashMap, HashSet, VecDeque};
    if src == tgt {
        return Some(vec![src.to_string()]);
    }
    let mut prev: HashMap<String, String> = HashMap::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    visited.insert(src.to_string());
    queue.push_back(src.to_string());
    let mut found = false;
    'outer: while let Some(current) = queue.pop_front() {
        let neighbors: Vec<String> = graph
            .adj
            .get(current.as_str())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        for neighbor in neighbors {
            if visited.contains(&neighbor) {
                continue;
            }
            prev.insert(neighbor.clone(), current.clone());
            if neighbor == tgt {
                found = true;
                break 'outer;
            }
            visited.insert(neighbor.clone());
            queue.push_back(neighbor);
        }
    }
    if !found {
        return None;
    }
    let mut path = vec![tgt.to_string()];
    let mut cur = tgt.to_string();
    while let Some(p) = prev.get(&cur) {
        path.push(p.clone());
        if p == src {
            break;
        }
        cur = p.clone();
    }
    path.reverse();
    Some(path)
}

// ---------------------------------------------------------------------------
// Tool dispatch
// ---------------------------------------------------------------------------

fn dispatch(
    graph: &Graph,
    communities: &HashMap<i64, Vec<String>>,
    name: &str,
    args: &Map<String, Value>,
) -> Value {
    match name {
        "query_graph" => {
            let question = match args.get("question").and_then(|v| v.as_str()) {
                Some(q) => q.to_string(),
                None => return tool_err("Missing required argument: question".to_string()),
            };
            let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("bfs");
            let depth = (args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize).min(6);
            let budget = args.get("token_budget").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;
            let filters: Vec<String> = args
                .get("context_filter")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let filter_refs: Vec<&str> = filters.iter().map(String::as_str).collect();
            let cf = if filter_refs.is_empty() { None } else { Some(filter_refs.as_slice()) };
            tool_ok(query_graph_text(graph, &question, mode, depth, budget, cf))
        }

        "get_node" => {
            let label = match args.get("label").and_then(|v| v.as_str()) {
                Some(l) => l.to_lowercase(),
                None => return tool_err("Missing required argument: label".to_string()),
            };
            let matches: Vec<(&String, &NodeAttrs)> = graph
                .nodes
                .iter()
                .filter(|(nid, attrs)| {
                    let node_label = attrs
                        .get("label")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_lowercase();
                    node_label.contains(&label) || label.contains(&node_label) || nid.to_lowercase() == label
                })
                .collect();
            if matches.is_empty() {
                return tool_err(format!("No node matching '{}' found.", label));
            }
            let (nid, attrs) = matches[0];
            let node_label = attrs.get("label").and_then(|v| v.as_str()).unwrap_or(nid);
            let degree = graph.degree(nid);
            let text = [
                format!("Node: {}", sanitize_label(Some(node_label))),
                format!("  ID: {}", nid),
                format!(
                    "  Source: {} {}",
                    attrs.get("source_file").and_then(|v| v.as_str()).unwrap_or(""),
                    attrs.get("source_location").and_then(|v| v.as_str()).unwrap_or("")
                ),
                format!("  Type: {}", attrs.get("file_type").and_then(|v| v.as_str()).unwrap_or("")),
                format!(
                    "  Community: {}",
                    attrs.get("community").and_then(|v| v.as_i64()).map(|c| c.to_string()).unwrap_or_default()
                ),
                format!("  Degree: {}", degree),
            ]
            .join("\n");
            tool_ok(text)
        }

        "get_neighbors" => {
            let label = match args.get("label").and_then(|v| v.as_str()) {
                Some(l) => l,
                None => return tool_err("Missing required argument: label".to_string()),
            };
            let rel_filter = args
                .get("relation_filter")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            let matches = find_node(graph, label);
            if matches.is_empty() {
                return tool_err(format!("No node matching '{}' found.", label));
            }
            let nid = &matches[0];
            let node_label = graph
                .nodes
                .get(nid)
                .and_then(|a| a.get("label"))
                .and_then(|v| v.as_str())
                .unwrap_or(nid);
            let mut lines = vec![format!("Neighbors of {}:", sanitize_label(Some(node_label)))];
            if let Some(neighbors) = graph.adj.get(nid.as_str()) {
                for (neighbor, edge_attrs) in neighbors {
                    let rel = edge_attrs.get("relation").and_then(|v| v.as_str()).unwrap_or("");
                    if !rel_filter.is_empty() && !rel.to_lowercase().contains(&rel_filter) {
                        continue;
                    }
                    let conf = edge_attrs.get("confidence").and_then(|v| v.as_str()).unwrap_or("");
                    let neigh_label = graph
                        .nodes
                        .get(neighbor)
                        .and_then(|a| a.get("label"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(neighbor);
                    lines.push(format!("  --> {} [{}] [{}]", sanitize_label(Some(neigh_label)), rel, conf));
                }
            }
            tool_ok(lines.join("\n"))
        }

        "get_community" => {
            let cid = match args.get("community_id").and_then(|v| v.as_i64()) {
                Some(c) => c,
                None => return tool_err("Missing required argument: community_id".to_string()),
            };
            match communities.get(&cid) {
                None => tool_err(format!("Community {} not found.", cid)),
                Some(nodes) => {
                    let mut lines = vec![format!("Community {} ({} nodes):", cid, nodes.len())];
                    for n in nodes {
                        let attrs = graph.nodes.get(n);
                        let lbl = attrs
                            .and_then(|a| a.get("label"))
                            .and_then(|v| v.as_str())
                            .unwrap_or(n);
                        let src = attrs
                            .and_then(|a| a.get("source_file"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        lines.push(format!("  {} [{}]", lbl, src));
                    }
                    tool_ok(lines.join("\n"))
                }
            }
        }

        "god_nodes" => {
            let top_n = args.get("top_n").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let nodes = god_nodes(graph, top_n);
            let mut lines = vec!["God nodes (most connected):".to_string()];
            for (i, n) in nodes.iter().enumerate() {
                let label = n.get("label").and_then(|v| v.as_str()).unwrap_or("");
                let degree = n.get("degree").and_then(|v| v.as_u64()).unwrap_or(0);
                lines.push(format!("  {}. {} - {} edges", i + 1, label, degree));
            }
            tool_ok(lines.join("\n"))
        }

        "graph_stats" => {
            let edges = graph.edges_iter();
            let total = edges.len().max(1);
            let extracted = edges.iter().filter(|(_, _, e)| e.get("confidence").and_then(|v| v.as_str()) == Some("EXTRACTED")).count();
            let inferred = edges.iter().filter(|(_, _, e)| e.get("confidence").and_then(|v| v.as_str()) == Some("INFERRED")).count();
            let ambiguous = edges.iter().filter(|(_, _, e)| e.get("confidence").and_then(|v| v.as_str()) == Some("AMBIGUOUS")).count();
            tool_ok(format!(
                "Nodes: {}\nEdges: {}\nCommunities: {}\nEXTRACTED: {}%\nINFERRED: {}%\nAMBIGUOUS: {}%",
                graph.number_of_nodes(),
                graph.number_of_edges(),
                communities.len(),
                extracted * 100 / total,
                inferred * 100 / total,
                ambiguous * 100 / total,
            ))
        }

        "shortest_path" => {
            let source = match args.get("source").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return tool_err("Missing required argument: source".to_string()),
            };
            let target = match args.get("target").and_then(|v| v.as_str()) {
                Some(t) => t,
                None => return tool_err("Missing required argument: target".to_string()),
            };
            let max_hops = args.get("max_hops").and_then(|v| v.as_u64()).unwrap_or(8) as usize;
            let src_matches = find_node(graph, source);
            let tgt_matches = find_node(graph, target);
            if src_matches.is_empty() {
                return tool_err(format!("No node matching source '{}' found.", source));
            }
            if tgt_matches.is_empty() {
                return tool_err(format!("No node matching target '{}' found.", target));
            }
            let src_id = &src_matches[0];
            let tgt_id = &tgt_matches[0];
            match bfs_shortest_path(graph, src_id, tgt_id) {
                None => {
                    let sl = graph.nodes.get(src_id).and_then(|a| a.get("label")).and_then(|v| v.as_str()).unwrap_or(src_id);
                    let tl = graph.nodes.get(tgt_id).and_then(|a| a.get("label")).and_then(|v| v.as_str()).unwrap_or(tgt_id);
                    tool_ok(format!("No path found between '{}' and '{}'.", sl, tl))
                }
                Some(path) => {
                    let hops = path.len().saturating_sub(1);
                    if hops > max_hops {
                        return tool_err(format!("Path exceeds max_hops={} ({} hops found).", max_hops, hops));
                    }
                    let mut segments = Vec::new();
                    for i in 0..path.len().saturating_sub(1) {
                        let u = &path[i];
                        let v = &path[i + 1];
                        let u_lbl = graph.nodes.get(u).and_then(|a| a.get("label")).and_then(|v| v.as_str()).unwrap_or(u);
                        let v_lbl = graph.nodes.get(v).and_then(|a| a.get("label")).and_then(|v| v.as_str()).unwrap_or(v);
                        let edge = graph.adj.get(u.as_str()).and_then(|m| m.get(v.as_str()));
                        let rel = edge.and_then(|e| e.get("relation")).and_then(|v| v.as_str()).unwrap_or("");
                        let conf = edge.and_then(|e| e.get("confidence")).and_then(|v| v.as_str()).unwrap_or("");
                        let conf_str = if conf.is_empty() { String::new() } else { format!(" [{}]", conf) };
                        if i == 0 {
                            segments.push(sanitize_label(Some(u_lbl)).to_string());
                        }
                        segments.push(format!("--{}{}--> {}", rel, conf_str, sanitize_label(Some(v_lbl))));
                    }
                    tool_ok(format!("Shortest path ({} hops):\n  {}", hops, segments.join(" ")))
                }
            }
        }

        _ => tool_err(format!("Unknown tool: {}", name)),
    }
}

// ---------------------------------------------------------------------------
// Request routing
// ---------------------------------------------------------------------------

fn handle(graph: &Graph, communities: &HashMap<i64, Vec<String>>, msg: &Value) -> Option<Value> {
    let id = msg.get("id")?; // notifications have no id — don't respond
    let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");

    match method {
        "initialize" => Some(ok(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "graphify", "version": env!("CARGO_PKG_VERSION")}
            }),
        )),
        "ping" => Some(ok(id, json!({}))),
        "tools/list" => Some(ok(id, json!({"tools": tools_list()}))),
        "tools/call" => {
            let params = msg.get("params");
            let name = params.and_then(|p| p.get("name")).and_then(|v| v.as_str()).unwrap_or("");
            let args = params
                .and_then(|p| p.get("arguments"))
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            let result = dispatch(graph, communities, name, &args);
            Some(ok(id, result))
        }
        _ => Some(err(id, -32601, &format!("Method not found: {}", method))),
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn serve(graph_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(graph_path);
    if !path.exists() {
        return Err(format!("Graph file not found: {}", graph_path).into());
    }
    let data = std::fs::read_to_string(path)?;
    let json_val: Value = serde_json::from_str(&data)
        .map_err(|e| format!("graph.json is corrupted ({}). Re-run /graphify to rebuild.", e))?;
    let graph = build_from_json(json_val, false);
    let communities = communities_from_graph(&graph);

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(response) = handle(&graph, &communities, &msg) {
            let mut json = serde_json::to_string(&response)?;
            json.push('\n');
            out.write_all(json.as_bytes())?;
            out.flush()?;
        }
    }
    Ok(())
}
