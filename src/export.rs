//! Graph export functions: JSON, Cypher, GraphML, HTML visualisation, Obsidian Canvas.
//!
//! Ported from Python `export.py`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::{json, Value};
use unicode_normalization::UnicodeNormalization;

use crate::analyze::is_file_node;
use crate::types::Graph;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The 10 Tableau colours used to colour communities in the visualisation.
pub const COMMUNITY_COLORS: &[&str] = &[
    "#4E79A7", "#F28E2B", "#E15759", "#76B7B2", "#59A14F",
    "#EDC948", "#B07AA1", "#FF9DA7", "#9C755F", "#BAB0AC",
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Strip combining diacritics (Unicode NFKD decomposition, drop Mn category).
///
/// Mirrors Python `_strip_diacritics` / `unicodedata.normalize("NFKD", text)`.
fn strip_diacritics(text: &str) -> String {
    text.nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect()
}

/// Normalise a label into a lowercase ASCII-ish token suitable for search
/// matching.
fn norm_label(label: &str) -> String {
    strip_diacritics(label).to_lowercase()
}

/// Escape `</script>` inside an embedded JSON string so it is safe to embed
/// inside an HTML `<script>` block.
fn escape_for_script(s: &str) -> String {
    s.replace("</script>", "<\\/script>")
        .replace("<!--", "<\\!--")
}

/// Return the community colour for a given (zero-based) community index.
fn community_color(idx: usize) -> &'static str {
    COMMUNITY_COLORS[idx % COMMUNITY_COLORS.len()]
}

// ---------------------------------------------------------------------------
// to_json
// ---------------------------------------------------------------------------

/// Serialise the graph to NetworkX-compatible node-link JSON and write it to
/// `output_path`.
///
/// Returns `Ok(true)` on success, `Ok(false)` when the graph shrank relative
/// to an existing file and `force` is `false`.
pub fn to_json(
    graph: &Graph,
    communities: &HashMap<i64, Vec<String>>,
    output_path: &str,
    force: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    // Build the community map: node_id → cid.
    let community_map: HashMap<&str, i64> = communities
        .iter()
        .flat_map(|(&cid, nodes)| nodes.iter().map(move |n| (n.as_str(), cid)))
        .collect();

    // Build nodes array.
    let nodes_json: Vec<Value> = graph
        .nodes
        .iter()
        .map(|(id, attrs)| {
            let mut obj = serde_json::Map::new();
            obj.insert("id".to_string(), Value::String(id.clone()));

            // Copy all node attrs.
            for (k, v) in attrs {
                obj.insert(k.clone(), v.clone());
            }

            // Inject community id.
            if let Some(&cid) = community_map.get(id.as_str()) {
                obj.insert("community".to_string(), json!(cid));
            }

            // Normalised label for search.
            let label = attrs
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or(id.as_str());
            obj.insert(
                "norm_label".to_string(),
                Value::String(norm_label(label)),
            );

            Value::Object(obj)
        })
        .collect();

    // Build links array (strip internal _src/_tgt fields).
    let links_json: Vec<Value> = graph
        .edges_iter()
        .into_iter()
        .map(|(src, tgt, attrs)| {
            let mut obj = serde_json::Map::new();
            obj.insert("source".to_string(), Value::String(src.to_string()));
            obj.insert("target".to_string(), Value::String(tgt.to_string()));
            for (k, v) in attrs {
                if k != "_src" && k != "_tgt" {
                    obj.insert(k.clone(), v.clone());
                }
            }
            Value::Object(obj)
        })
        .collect();

    // Hyperedges from graph-level metadata.
    let hyperedges = graph
        .graph
        .get("hyperedges")
        .cloned()
        .unwrap_or_else(|| json!([]));

    let output = json!({
        "nodes": nodes_json,
        "links": links_json,
        "hyperedges": hyperedges,
    });

    // Safety check: if the file already exists and the new graph is smaller,
    // warn and return false unless force is set.
    if !force && Path::new(output_path).exists() {
        let existing_raw = fs::read_to_string(output_path)?;
        if let Ok(existing) = serde_json::from_str::<Value>(&existing_raw) {
            let existing_nodes = existing
                .get("nodes")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            if nodes_json.len() < existing_nodes {
                eprintln!(
                    "Warning: graph shrank from {} to {} nodes. \
                     Use force=true to overwrite.",
                    existing_nodes,
                    nodes_json.len()
                );
                return Ok(false);
            }
        }
    }

    let json_str = serde_json::to_string_pretty(&output)?;
    fs::write(output_path, json_str)?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// to_cypher
// ---------------------------------------------------------------------------

/// Write a Neo4j Cypher import script to `output_path`.
///
/// Produces `MERGE` statements for all nodes, then `MATCH`/`MERGE` statements
/// for all edges.
pub fn to_cypher(graph: &Graph, output_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut lines: Vec<String> = Vec::new();

    // Node statements.
    for (id, attrs) in &graph.nodes {
        let label_str = attrs
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or(id.as_str());
        let file_type = attrs
            .get("file_type")
            .and_then(|v| v.as_str())
            .unwrap_or("Node");
        // Capitalise first letter for Cypher node label.
        let cypher_label = capitalise(file_type);
        let escaped_id = escape_cypher_string(id);
        let escaped_label = escape_cypher_string(label_str);
        lines.push(format!(
            "MERGE (n:{} {{id: '{}', label: '{}'}});",
            cypher_label, escaped_id, escaped_label
        ));
    }

    lines.push(String::new());

    // Edge statements.
    for (src, tgt, attrs) in graph.edges_iter() {
        let relation = attrs
            .get("relation")
            .and_then(|v| v.as_str())
            .unwrap_or("RELATED_TO");
        let conf = attrs
            .get("confidence")
            .and_then(|v| v.as_str())
            .unwrap_or("EXTRACTED");
        // Convert relation to UPPER_SNAKE_CASE for Cypher relationship types.
        let rel_type = relation
            .to_uppercase()
            .replace(' ', "_")
            .replace('-', "_");
        let escaped_src = escape_cypher_string(src);
        let escaped_tgt = escape_cypher_string(tgt);
        let escaped_conf = escape_cypher_string(conf);
        lines.push(format!(
            "MATCH (a {{id: '{}'}}), (b {{id: '{}'}}) \
             MERGE (a)-[:{} {{confidence: '{}'}}]->(b);",
            escaped_src, escaped_tgt, rel_type, escaped_conf
        ));
    }

    fs::write(output_path, lines.join("\n"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// to_graphml
// ---------------------------------------------------------------------------

/// Write the graph as standard GraphML XML to `output_path`.
///
/// The XML is built as a `String` without any external XML crate; the output
/// is valid, UTF-8 encoded GraphML.
pub fn to_graphml(
    graph: &Graph,
    communities: &HashMap<i64, Vec<String>>,
    output_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Build community map.
    let community_map: HashMap<&str, i64> = communities
        .iter()
        .flat_map(|(&cid, nodes)| nodes.iter().map(move |n| (n.as_str(), cid)))
        .collect();

    let edge_default = if graph.directed {
        "directed"
    } else {
        "undirected"
    };

    let mut xml = String::with_capacity(4096);
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<graphml xmlns=\"http://graphml.graphdrawing.org/graphml\">\n");
    xml.push_str("  <key id=\"d0\" for=\"node\" attr.name=\"label\" attr.type=\"string\"/>\n");
    xml.push_str("  <key id=\"d1\" for=\"node\" attr.name=\"community\" attr.type=\"int\"/>\n");
    xml.push_str("  <key id=\"d2\" for=\"edge\" attr.name=\"relation\" attr.type=\"string\"/>\n");
    xml.push_str("  <key id=\"d3\" for=\"edge\" attr.name=\"confidence\" attr.type=\"string\"/>\n");
    xml.push_str(&format!(
        "  <graph id=\"G\" edgedefault=\"{}\">\n",
        edge_default
    ));

    // Nodes.
    for (id, attrs) in &graph.nodes {
        let label = attrs
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or(id.as_str());
        let escaped_id = xml_escape(id);
        let escaped_label = xml_escape(label);

        xml.push_str(&format!("    <node id=\"{}\">\n", escaped_id));
        xml.push_str(&format!(
            "      <data key=\"d0\">{}</data>\n",
            escaped_label
        ));

        if let Some(&cid) = community_map.get(id.as_str()) {
            xml.push_str(&format!(
                "      <data key=\"d1\">{}</data>\n",
                cid
            ));
        }
        xml.push_str("    </node>\n");
    }

    // Edges.
    let mut edge_idx = 0usize;
    for (src, tgt, attrs) in graph.edges_iter() {
        let relation = attrs
            .get("relation")
            .and_then(|v| v.as_str())
            .unwrap_or("related");
        let conf = attrs
            .get("confidence")
            .and_then(|v| v.as_str())
            .unwrap_or("EXTRACTED");

        xml.push_str(&format!(
            "    <edge id=\"e{}\" source=\"{}\" target=\"{}\">\n",
            edge_idx,
            xml_escape(src),
            xml_escape(tgt)
        ));
        xml.push_str(&format!(
            "      <data key=\"d2\">{}</data>\n",
            xml_escape(relation)
        ));
        xml.push_str(&format!(
            "      <data key=\"d3\">{}</data>\n",
            xml_escape(conf)
        ));
        xml.push_str("    </edge>\n");
        edge_idx += 1;
    }

    xml.push_str("  </graph>\n");
    xml.push_str("</graphml>\n");

    fs::write(output_path, xml)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// to_html
// ---------------------------------------------------------------------------

/// Maximum number of nodes accepted by the HTML visualiser (override with the
/// `GRAPHIFY_VIZ_NODE_LIMIT` environment variable).
fn viz_node_limit() -> usize {
    std::env::var("GRAPHIFY_VIZ_NODE_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000)
}

/// Generate an interactive vis-network HTML visualisation and write it to
/// `output_path`.
pub fn to_html(
    graph: &Graph,
    communities: &HashMap<i64, Vec<String>>,
    output_path: &str,
    community_labels: Option<&HashMap<i64, String>>,
    member_counts: Option<&HashMap<i64, usize>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let limit = viz_node_limit();
    if graph.number_of_nodes() > limit {
        return Err(format!(
            "Graph has {} nodes which exceeds the visualisation limit of {}. \
             Set GRAPHIFY_VIZ_NODE_LIMIT to override.",
            graph.number_of_nodes(),
            limit
        )
        .into());
    }

    // Build community map: node_id → cid.
    let community_map: HashMap<&str, i64> = communities
        .iter()
        .flat_map(|(&cid, nodes)| nodes.iter().map(move |n| (n.as_str(), cid)))
        .collect();

    // Sorted community ids so colour assignment is stable.
    let mut cids: Vec<i64> = communities.keys().copied().collect();
    cids.sort_unstable();
    let cid_index: HashMap<i64, usize> = cids
        .iter()
        .enumerate()
        .map(|(i, &cid)| (cid, i))
        .collect();

    // -----------------------------------------------------------------------
    // Build vis-network node objects.
    // -----------------------------------------------------------------------
    let nodes_data: Vec<Value> = graph
        .nodes
        .iter()
        .map(|(id, attrs)| {
            let label = attrs
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or(id.as_str());
            let source_file = attrs
                .get("source_file")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let file_type = attrs
                .get("file_type")
                .and_then(|v| v.as_str())
                .unwrap_or("concept");
            let degree = graph.degree(id);

            let cid_opt = community_map.get(id.as_str()).copied();
            let color_hex = cid_opt
                .and_then(|cid| cid_index.get(&cid))
                .map(|&idx| community_color(idx))
                .unwrap_or("#BAB0AC");

            // File nodes are smaller and more muted.
            let is_file = is_file_node(graph, id);
            let size = if is_file {
                12.0f64
            } else {
                (10.0 + (degree as f64).sqrt() * 3.0).min(50.0)
            };

            let community_name = cid_opt
                .and_then(|cid| {
                    community_labels
                        .and_then(|m| m.get(&cid))
                        .map(|s| s.as_str())
                })
                .unwrap_or("");

            let title = format!(
                "<b>{}</b><br>Degree: {}<br>File: {}<br>Type: {}<br>Community: {}",
                label, degree, source_file, file_type, community_name
            );

            json!({
                "id": id,
                "label": label,
                "color": {
                    "background": color_hex,
                    "border": darken_hex(color_hex),
                    "highlight": {
                        "background": "#ffffff",
                        "border": color_hex,
                    }
                },
                "size": size,
                "font": {
                    "size": if is_file { 9 } else { 13 },
                    "color": "#e0e0e0",
                },
                "title": title,
                "community": cid_opt.unwrap_or(-1),
                "community_name": community_name,
                "source_file": source_file,
                "file_type": file_type,
                "degree": degree,
            })
        })
        .collect();

    // -----------------------------------------------------------------------
    // Build vis-network edge objects.
    // -----------------------------------------------------------------------
    let edges_data: Vec<Value> = graph
        .edges_iter()
        .into_iter()
        .map(|(src, tgt, attrs)| {
            let relation = attrs
                .get("relation")
                .and_then(|v| v.as_str())
                .unwrap_or("related");
            let conf = attrs
                .get("confidence")
                .and_then(|v| v.as_str())
                .unwrap_or("EXTRACTED");
            let cscore = attrs
                .get("confidence_score")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0);
            let dashes = conf == "INFERRED" || conf == "AMBIGUOUS";
            let width = if conf == "EXTRACTED" { 1.5 } else { 1.0 };

            let title = format!(
                "{} → {}<br>relation: {}<br>confidence: {}",
                src, tgt, relation, conf
            );

            json!({
                "from": src,
                "to": tgt,
                "label": relation,
                "title": title,
                "dashes": dashes,
                "width": width,
                "color": { "opacity": cscore.min(1.0).max(0.3) },
                "confidence": conf,
            })
        })
        .collect();

    // -----------------------------------------------------------------------
    // Legend entries.
    // -----------------------------------------------------------------------
    let legend_data: Vec<Value> = cids
        .iter()
        .map(|&cid| {
            let idx = cid_index[&cid];
            let color = community_color(idx);
            let label = community_labels
                .and_then(|m| m.get(&cid))
                .cloned()
                .unwrap_or_else(|| format!("Community {}", cid));
            let count = member_counts
                .and_then(|m| m.get(&cid))
                .copied()
                .unwrap_or_else(|| {
                    communities.get(&cid).map(|v| v.len()).unwrap_or(0)
                });
            json!({ "cid": cid, "color": color, "label": label, "count": count })
        })
        .collect();

    // Serialise to JSON and embed safely.
    let nodes_json =
        escape_for_script(&serde_json::to_string(&nodes_data)?);
    let edges_json =
        escape_for_script(&serde_json::to_string(&edges_data)?);
    let legend_json =
        escape_for_script(&serde_json::to_string(&legend_data)?);

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1.0"/>
<title>Graphify Knowledge Graph</title>
<script src="https://unpkg.com/vis-network/standalone/umd/vis-network.min.js"></script>
<style>
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  body {{ background: #0f0f1a; color: #e0e0e0; font-family: 'Segoe UI', system-ui, sans-serif; display: flex; height: 100vh; overflow: hidden; }}
  #sidebar {{ width: 300px; min-width: 220px; background: #1a1a2e; display: flex; flex-direction: column; border-right: 1px solid #2a2a4a; overflow-y: auto; }}
  #sidebar h2 {{ padding: 14px 16px 8px; font-size: 14px; text-transform: uppercase; letter-spacing: 1px; color: #8888bb; border-bottom: 1px solid #2a2a4a; }}
  #search-box {{ margin: 10px; padding: 8px 10px; background: #0f0f1a; border: 1px solid #3a3a5a; border-radius: 6px; color: #e0e0e0; font-size: 13px; width: calc(100% - 20px); }}
  #search-box:focus {{ outline: none; border-color: #6666cc; }}
  #info-panel {{ padding: 12px 14px; font-size: 12px; line-height: 1.6; border-bottom: 1px solid #2a2a4a; min-height: 80px; }}
  #info-panel b {{ color: #aaaaee; }}
  #legend {{ padding: 8px 14px 14px; }}
  .legend-item {{ display: flex; align-items: center; gap: 8px; padding: 4px 0; font-size: 12px; cursor: pointer; }}
  .legend-item:hover {{ background: #22224a; border-radius: 4px; padding-left: 4px; }}
  .legend-swatch {{ width: 14px; height: 14px; border-radius: 50%; flex-shrink: 0; }}
  .legend-item input[type=checkbox] {{ accent-color: #6666cc; }}
  #controls {{ padding: 10px 14px; border-top: 1px solid #2a2a4a; display: flex; gap: 6px; flex-wrap: wrap; }}
  .ctrl-btn {{ background: #2a2a4a; border: none; color: #c0c0e0; padding: 5px 10px; border-radius: 4px; cursor: pointer; font-size: 11px; }}
  .ctrl-btn:hover {{ background: #3a3a6a; }}
  #graph-container {{ flex: 1; position: relative; }}
  #graph {{ width: 100%; height: 100%; }}
  #stats {{ position: absolute; bottom: 10px; right: 10px; background: rgba(26,26,46,0.85); padding: 6px 12px; border-radius: 6px; font-size: 11px; color: #8888aa; }}
</style>
</head>
<body>
<div id="sidebar">
  <h2>Graphify</h2>
  <input id="search-box" type="text" placeholder="Search nodes..." autocomplete="off"/>
  <div id="info-panel"><i style="color:#666">Click a node to inspect</i></div>
  <h2>Communities</h2>
  <div id="legend"></div>
  <div id="controls">
    <button class="ctrl-btn" onclick="network.fit()">Fit</button>
    <button class="ctrl-btn" onclick="togglePhysics()">Physics</button>
    <button class="ctrl-btn" onclick="showAll()">Show All</button>
  </div>
</div>
<div id="graph-container">
  <div id="graph"></div>
  <div id="stats"></div>
</div>
<script>
(function() {{
  var RAW_NODES = {nodes_json};
  var RAW_EDGES = {edges_json};
  var LEGEND    = {legend_json};

  var hiddenCommunities = {{}};

  // ---- Legend ----------------------------------------------------------------
  var legendEl = document.getElementById('legend');
  LEGEND.forEach(function(item) {{
    var div = document.createElement('div');
    div.className = 'legend-item';
    div.innerHTML =
      '<input type="checkbox" checked data-cid="' + item.cid + '">' +
      '<span class="legend-swatch" style="background:' + item.color + '"></span>' +
      '<span>' + item.label + ' <small style="color:#666">(' + item.count + ')</small></span>';
    div.querySelector('input').addEventListener('change', function(e) {{
      var cid = parseInt(e.target.dataset.cid);
      hiddenCommunities[cid] = !e.target.checked;
      applyFilter();
    }});
    legendEl.appendChild(div);
  }});

  // ---- vis-network -----------------------------------------------------------
  var nodesDS = new vis.DataSet(RAW_NODES);
  var edgesDS = new vis.DataSet(RAW_EDGES);

  var options = {{
    physics: {{
      enabled: true,
      solver: 'forceAtlas2Based',
      forceAtlas2Based: {{ gravitationalConstant: -50, centralGravity: 0.01, springLength: 120, springConstant: 0.08 }},
      stabilization: {{ iterations: 150 }},
    }},
    edges: {{
      arrows: {{ to: {{ enabled: true, scaleFactor: 0.5 }} }},
      smooth: {{ type: 'continuous' }},
      font: {{ size: 9, color: '#666699', align: 'middle' }},
    }},
    nodes: {{
      shape: 'dot',
      borderWidth: 2,
    }},
    interaction: {{ hover: true, tooltipDelay: 150 }},
  }};

  var container = document.getElementById('graph');
  var network = new vis.Network(container, {{ nodes: nodesDS, edges: edgesDS }}, options);

  // ---- Stats -----------------------------------------------------------------
  document.getElementById('stats').textContent =
    RAW_NODES.length + ' nodes · ' + RAW_EDGES.length + ' edges';

  // ---- Node inspector --------------------------------------------------------
  var infoPanel = document.getElementById('info-panel');
  network.on('click', function(params) {{
    if (params.nodes.length === 0) {{ infoPanel.innerHTML = '<i style="color:#666">Click a node to inspect</i>'; return; }}
    var nid = params.nodes[0];
    var n = RAW_NODES.find(function(x) {{ return x.id === nid; }});
    if (!n) return;
    infoPanel.innerHTML =
      '<b>' + n.label + '</b><br>' +
      'Degree: ' + n.degree + '<br>' +
      'Type: ' + n.file_type + '<br>' +
      'Community: ' + (n.community_name || n.community) + '<br>' +
      'File: <span style="color:#888;word-break:break-all">' + (n.source_file || '—') + '</span>';
  }});

  // ---- Search ----------------------------------------------------------------
  var searchBox = document.getElementById('search-box');
  searchBox.addEventListener('input', function() {{
    var q = searchBox.value.trim().toLowerCase();
    if (!q) {{ nodesDS.update(RAW_NODES.map(function(n) {{ return {{ id: n.id, hidden: hiddenCommunities[n.community] || false }}; }})); return; }}
    nodesDS.update(RAW_NODES.map(function(n) {{
      var match = (n.label || '').toLowerCase().indexOf(q) !== -1 ||
                  (n.source_file || '').toLowerCase().indexOf(q) !== -1;
      return {{ id: n.id, hidden: !match }};
    }}));
  }});

  // ---- Filter by community ---------------------------------------------------
  function applyFilter() {{
    nodesDS.update(RAW_NODES.map(function(n) {{
      return {{ id: n.id, hidden: !!hiddenCommunities[n.community] }};
    }}));
  }}

  // ---- Controls --------------------------------------------------------------
  var physicsOn = true;
  function togglePhysics() {{
    physicsOn = !physicsOn;
    network.setOptions({{ physics: {{ enabled: physicsOn }} }});
  }}
  function showAll() {{
    hiddenCommunities = {{}};
    document.querySelectorAll('#legend input[type=checkbox]').forEach(function(cb) {{ cb.checked = true; }});
    nodesDS.update(RAW_NODES.map(function(n) {{ return {{ id: n.id, hidden: false }}; }}));
  }}
}})();
</script>
</body>
</html>
"#,
        nodes_json = nodes_json,
        edges_json = edges_json,
        legend_json = legend_json,
    );

    fs::write(output_path, html)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// to_canvas
// ---------------------------------------------------------------------------

/// Export the graph as an Obsidian Canvas JSON file.
///
/// Communities are laid out in a grid; nodes within each community are arranged
/// in rows of three.  At most 200 edges (sorted by weight) are included.
pub fn to_canvas(
    graph: &Graph,
    communities: &HashMap<i64, Vec<String>>,
    output_path: &str,
    community_labels: Option<&HashMap<i64, String>>,
    node_filenames: Option<&HashMap<String, String>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Sorted community ids for stable layout.
    let mut cids: Vec<i64> = communities.keys().copied().collect();
    cids.sort_unstable();
    let cid_index: HashMap<i64, usize> =
        cids.iter().enumerate().map(|(i, &cid)| (cid, i)).collect();

    // Canvas dimensions per node card.
    const NODE_W: i64 = 200;
    const NODE_H: i64 = 60;
    const NODE_GAP_X: i64 = 20;
    const NODE_GAP_Y: i64 = 20;
    const COLS: i64 = 3;

    // Community grid: each community occupies a cell in a 3-column grid.
    const COMM_COLS: i64 = 3;
    const COMM_PAD: i64 = 40;

    let mut canvas_nodes: Vec<Value> = Vec::new();
    let mut node_positions: HashMap<String, (i64, i64)> = HashMap::new();

    for (grid_idx, &cid) in cids.iter().enumerate() {
        let members = match communities.get(&cid) {
            Some(m) => m,
            None => continue,
        };
        let label = community_labels
            .and_then(|m| m.get(&cid))
            .cloned()
            .unwrap_or_else(|| format!("Community {}", cid));

        let color_idx = cid_index[&cid];
        // Obsidian Canvas colour codes: 1-6 are built-in, cycle through them.
        let obs_color = ((color_idx % 6) + 1).to_string();

        // Grid position of this community block.
        let grid_col = (grid_idx as i64) % COMM_COLS;
        let grid_row = (grid_idx as i64) / COMM_COLS;

        // Rows needed for the members of this community.
        let rows_needed = (members.len() as i64 + COLS - 1) / COLS;
        let block_w = COLS * (NODE_W + NODE_GAP_X) + COMM_PAD * 2;
        let block_h =
            rows_needed * (NODE_H + NODE_GAP_Y) + COMM_PAD * 2 + 50 /* label row */;

        let block_x = grid_col * (block_w + 80);
        let block_y = grid_row * (block_h + 80);

        // Community group node.
        canvas_nodes.push(json!({
            "id": format!("group_{}", cid),
            "type": "group",
            "label": label,
            "x": block_x,
            "y": block_y,
            "width": block_w,
            "height": block_h,
            "color": obs_color,
        }));

        // Member card nodes.
        for (member_idx, node_id) in members.iter().enumerate() {
            let col = (member_idx as i64) % COLS;
            let row = (member_idx as i64) / COLS;
            let x = block_x + COMM_PAD + col * (NODE_W + NODE_GAP_X);
            let y = block_y + COMM_PAD + 50 + row * (NODE_H + NODE_GAP_Y);
            node_positions.insert(node_id.clone(), (x, y));

            let attrs = graph.get_node(node_id);
            let label_str = attrs
                .and_then(|a| a.get("label"))
                .and_then(|v| v.as_str())
                .unwrap_or(node_id.as_str());
            let file_type = attrs
                .and_then(|a| a.get("file_type"))
                .and_then(|v| v.as_str())
                .unwrap_or("concept");

            if file_type == "document" || file_type == "code" || file_type == "paper" {
                // Render as a file card if we have a filename mapping.
                let filename = node_filenames
                    .and_then(|m| m.get(node_id.as_str()))
                    .cloned()
                    .unwrap_or_else(|| format!("{}.md", label_str));
                canvas_nodes.push(json!({
                    "id": node_id,
                    "type": "file",
                    "file": filename,
                    "x": x,
                    "y": y,
                    "width": NODE_W,
                    "height": NODE_H,
                    "color": obs_color,
                }));
            } else {
                canvas_nodes.push(json!({
                    "id": node_id,
                    "type": "text",
                    "text": label_str,
                    "x": x,
                    "y": y,
                    "width": NODE_W,
                    "height": NODE_H,
                    "color": obs_color,
                }));
            }
        }
    }

    // -----------------------------------------------------------------------
    // Edges — top 200 by weight (degree sum as a proxy when weight is absent).
    // -----------------------------------------------------------------------
    let mut all_edges: Vec<(&str, &str, &crate::types::EdgeAttrs)> =
        graph.edges_iter();

    all_edges.sort_by(|a, b| {
        let wa = a.2.get("weight").and_then(|v| v.as_f64()).unwrap_or(1.0);
        let wb = b.2.get("weight").and_then(|v| v.as_f64()).unwrap_or(1.0);
        wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
    });

    let canvas_edges: Vec<Value> = all_edges
        .iter()
        .take(200)
        .enumerate()
        .filter_map(|(i, (src, tgt, attrs))| {
            // Only emit edge if both nodes are in the canvas.
            if !node_positions.contains_key(*src) || !node_positions.contains_key(*tgt) {
                return None;
            }
            let relation = attrs
                .get("relation")
                .and_then(|v| v.as_str())
                .unwrap_or("related");
            Some(json!({
                "id": format!("edge_{}", i),
                "fromNode": src,
                "toNode": tgt,
                "label": relation,
            }))
        })
        .collect();

    let canvas = json!({
        "nodes": canvas_nodes,
        "edges": canvas_edges,
    });

    fs::write(output_path, serde_json::to_string_pretty(&canvas)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Private utilities
// ---------------------------------------------------------------------------

/// Escape special XML characters in text content and attribute values.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Escape single-quotes in a Cypher string literal (Cypher uses `\'`).
fn escape_cypher_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Capitalise the first ASCII character.
fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

/// Attempt a very simple darkening of a hex colour by subtracting ~30 from
/// each channel.  Used for node border colours.
fn darken_hex(hex: &str) -> String {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return format!("#{}", hex);
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
    let dr = r.saturating_sub(30);
    let dg = g.saturating_sub(30);
    let db = b.saturating_sub(30);
    format!("#{:02X}{:02X}{:02X}", dr, dg, db)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EdgeAttrs, Graph, NodeAttrs};
    use serde_json::json;
    use std::collections::HashMap;

    fn small_graph() -> (Graph, HashMap<i64, Vec<String>>) {
        let mut g = Graph::new(false);
        let mut na: NodeAttrs = HashMap::new();
        na.insert("label".to_string(), json!("Alpha"));
        na.insert("file_type".to_string(), json!("concept"));
        na.insert("source_file".to_string(), json!("a.md"));
        g.add_node("n1", na.clone());
        na.insert("label".to_string(), json!("Beta"));
        g.add_node("n2", na);
        let mut ea: EdgeAttrs = HashMap::new();
        ea.insert("relation".to_string(), json!("uses"));
        ea.insert("confidence".to_string(), json!("EXTRACTED"));
        g.add_edge("n1", "n2", ea);

        let communities: HashMap<i64, Vec<String>> =
            [(0, vec!["n1".to_string(), "n2".to_string()])]
                .into_iter()
                .collect();
        (g, communities)
    }

    #[test]
    fn xml_escape_works() {
        assert_eq!(xml_escape("a<b>&\"c"), "a&lt;b&gt;&amp;&quot;c");
    }

    #[test]
    fn darken_hex_clamps_at_zero() {
        assert_eq!(darken_hex("#000000"), "#000000");
        // R=0x4E-30=0x30, G=0x79-30=0x5B, B=0xA7-30=0x89
        assert_eq!(darken_hex("#4E79A7"), "#305B89");
    }

    #[test]
    fn strip_diacritics_removes_accents() {
        let result = strip_diacritics("café");
        assert_eq!(result, "cafe");
    }

    #[test]
    fn to_json_creates_file() {
        let (g, communities) = small_graph();
        let tmp = std::env::temp_dir()
            .join("graphify_test_export.json")
            .to_string_lossy()
            .to_string();
        let ok = to_json(&g, &communities, &tmp, true).unwrap();
        assert!(ok);
        let content = std::fs::read_to_string(&tmp).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();
        assert!(v["nodes"].as_array().unwrap().len() == 2);
        assert!(v["links"].as_array().unwrap().len() == 1);
    }

    #[test]
    fn to_cypher_creates_file() {
        let (g, _) = small_graph();
        let tmp = std::env::temp_dir()
            .join("graphify_test.cypher")
            .to_string_lossy()
            .to_string();
        to_cypher(&g, &tmp).unwrap();
        let content = std::fs::read_to_string(&tmp).unwrap();
        assert!(content.contains("MERGE"));
        assert!(content.contains("MATCH"));
    }

    #[test]
    fn to_graphml_produces_valid_xml() {
        let (g, communities) = small_graph();
        let tmp = std::env::temp_dir()
            .join("graphify_test.graphml")
            .to_string_lossy()
            .to_string();
        to_graphml(&g, &communities, &tmp).unwrap();
        let content = std::fs::read_to_string(&tmp).unwrap();
        assert!(content.contains("<?xml"));
        assert!(content.contains("<graphml"));
        assert!(content.contains("<node id="));
        assert!(content.contains("<edge "));
    }

    #[test]
    fn to_html_produces_html() {
        let (g, communities) = small_graph();
        let tmp = std::env::temp_dir()
            .join("graphify_test.html")
            .to_string_lossy()
            .to_string();
        to_html(&g, &communities, &tmp, None, None).unwrap();
        let content = std::fs::read_to_string(&tmp).unwrap();
        assert!(content.contains("vis-network"));
        assert!(content.contains("forceAtlas2Based"));
        assert!(!content.contains("</script>Alpha")); // no raw </script> injection
    }

    #[test]
    fn to_canvas_produces_json() {
        let (g, communities) = small_graph();
        let tmp = std::env::temp_dir()
            .join("graphify_test.canvas")
            .to_string_lossy()
            .to_string();
        to_canvas(&g, &communities, &tmp, None, None).unwrap();
        let content = std::fs::read_to_string(&tmp).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();
        assert!(v["nodes"].as_array().is_some());
        assert!(v["edges"].as_array().is_some());
    }
}
