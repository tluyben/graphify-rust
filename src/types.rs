use std::collections::HashMap;
use indexmap::IndexMap;
use serde_json::Value;

pub type NodeAttrs = HashMap<String, Value>;
pub type EdgeAttrs = HashMap<String, Value>;

#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub nodes: IndexMap<String, NodeAttrs>,
    pub adj: HashMap<String, HashMap<String, EdgeAttrs>>,
    pub directed: bool,
    pub graph: HashMap<String, Value>,
}

impl Graph {
    pub fn new(directed: bool) -> Self {
        Self {
            nodes: IndexMap::new(),
            adj: HashMap::new(),
            directed,
            graph: HashMap::new(),
        }
    }

    pub fn number_of_nodes(&self) -> usize {
        self.nodes.len()
    }

    pub fn number_of_edges(&self) -> usize {
        if self.directed {
            self.adj.values().map(|nbrs| nbrs.len()).sum()
        } else {
            // Each undirected edge is stored in both directions; count each once.
            let total: usize = self.adj.values().map(|nbrs| nbrs.len()).sum();
            total / 2
        }
    }

    pub fn is_directed(&self) -> bool {
        self.directed
    }

    /// Upsert a node, merging attrs into any existing attributes.
    pub fn add_node(&mut self, id: &str, attrs: NodeAttrs) {
        let entry = self.nodes.entry(id.to_string()).or_default();
        entry.extend(attrs);
        // Ensure adjacency entry exists.
        self.adj.entry(id.to_string()).or_default();
    }

    /// Add an edge (and its reverse for undirected graphs).
    pub fn add_edge(&mut self, src: &str, tgt: &str, attrs: EdgeAttrs) {
        // Ensure both nodes exist in the adjacency map.
        self.adj.entry(src.to_string()).or_default();
        self.adj.entry(tgt.to_string()).or_default();

        self.adj
            .get_mut(src)
            .unwrap()
            .insert(tgt.to_string(), attrs.clone());

        if !self.directed && src != tgt {
            self.adj
                .get_mut(tgt)
                .unwrap()
                .insert(src.to_string(), attrs);
        }
    }

    pub fn has_node(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn has_edge(&self, src: &str, tgt: &str) -> bool {
        self.adj.get(src).map_or(false, |nbrs| nbrs.contains_key(tgt))
    }

    pub fn get_node(&self, id: &str) -> Option<&NodeAttrs> {
        self.nodes.get(id)
    }

    pub fn get_edge(&self, src: &str, tgt: &str) -> Option<&EdgeAttrs> {
        self.adj.get(src)?.get(tgt)
    }

    pub fn neighbors(&self, id: &str) -> Vec<&str> {
        self.adj
            .get(id)
            .map(|nbrs| nbrs.keys().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    pub fn degree(&self, id: &str) -> usize {
        self.adj.get(id).map(|nbrs| nbrs.len()).unwrap_or(0)
    }

    /// Returns all edges. For undirected graphs each edge is returned once
    /// (with the lexicographically smaller node id as the first element).
    pub fn edges_iter(&self) -> Vec<(&str, &str, &EdgeAttrs)> {
        let mut result = Vec::new();
        if self.directed {
            for (src, nbrs) in &self.adj {
                for (tgt, attrs) in nbrs {
                    result.push((src.as_str(), tgt.as_str(), attrs));
                }
            }
        } else {
            for (src, nbrs) in &self.adj {
                for (tgt, attrs) in nbrs {
                    if src <= tgt {
                        result.push((src.as_str(), tgt.as_str(), attrs));
                    }
                }
            }
        }
        result
    }

    pub fn to_undirected(&self) -> Graph {
        let mut g = Graph::new(false);
        for (id, attrs) in &self.nodes {
            g.add_node(id, attrs.clone());
        }
        for (src, nbrs) in &self.adj {
            for (tgt, attrs) in nbrs {
                if !g.has_edge(src, tgt) {
                    g.add_edge(src, tgt, attrs.clone());
                }
            }
        }
        g
    }

    pub fn subgraph(&self, nodes: &[&str]) -> Graph {
        let node_set: std::collections::HashSet<&str> = nodes.iter().copied().collect();
        let mut g = Graph::new(self.directed);
        for &id in nodes {
            if let Some(attrs) = self.nodes.get(id) {
                g.add_node(id, attrs.clone());
            }
        }
        for &src in nodes {
            if let Some(nbrs) = self.adj.get(src) {
                for (tgt, attrs) in nbrs {
                    if node_set.contains(tgt.as_str()) {
                        // For undirected, add_edge stores both directions;
                        // only call once per pair.
                        if self.directed || src <= tgt.as_str() {
                            g.add_edge(src, tgt, attrs.clone());
                        }
                    }
                }
            }
        }
        g
    }

    pub fn remove_nodes_from(&mut self, ids: &[String]) {
        for id in ids {
            self.nodes.shift_remove(id.as_str());
            self.adj.remove(id.as_str());
        }
        // Remove dangling adjacency entries pointing to removed nodes.
        let id_set: std::collections::HashSet<&str> =
            ids.iter().map(|s| s.as_str()).collect();
        for nbrs in self.adj.values_mut() {
            nbrs.retain(|tgt, _| !id_set.contains(tgt.as_str()));
        }
    }
}
