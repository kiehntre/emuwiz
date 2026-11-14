//! Media dependency graph.
//!
//! Nodes are files (and the catalogue's audited sets); edges are grounded
//! relationships, each with its verification level. The graph is built from
//! the shared descriptor parsers (`game_package`: CUE/BIN, GDI tracks, M3U
//! discs) and from DAT-declared set dependencies recorded by the audit.
//!
//! Rules:
//!
//! - An edge exists only when a parser or a catalogue record establishes it.
//!   A filename that merely looks related never creates an edge, and never
//!   authorises anything.
//! - A relationship that could not be established (a descriptor that cannot
//!   be read, a missing track) is recorded as an explicit [`Unknown`], not
//!   dropped. Unknowns next to a file block its relocation (fail closed).
//! - Analyses report cycles, ambiguous ownership (a member claimed by more
//!   than one set), duplicate references and missing components.
//!
//! Not modelled in this slice (reported as unknown in previews, never as
//! "no dependency"): saves, patch provenance, per-game emulator
//! configuration and external launcher/playlist references.
//!
//! [`Unknown`]: Verification::Unknown

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::game_package::{descriptor_references, is_descriptor};

/// Bound on directory entries examined when building a neighbourhood.
pub const MAX_NEIGHBOURHOOD_ENTRIES: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A plain media file (ROM, image, archive).
    File,
    /// A CUE, GDI or M3U descriptor that names other files.
    Descriptor,
    /// A catalogue-declared dependency target (parent set, BIOS, device).
    DeclaredTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Descriptor -> track or disc it names.
    Names,
    /// Set -> DAT-declared requirement (parent, BIOS, device, merged ROM).
    Requires,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "level", rename_all = "snake_case")]
pub enum Verification {
    /// Established by parsing the file's own structure.
    Structural { by: String },
    /// Declared by an imported DAT through the set audit.
    Declared { by: String },
    /// Could not be established; never treated as absent.
    Unknown { why: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub kind: NodeKind,
    pub key: String,
    pub path: Option<PathBuf>,
    pub present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub kind: EdgeKind,
    pub verification: Verification,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Nodes whose relationships could not be established, with why.
    pub unknowns: Vec<(usize, String)>,
    /// Edges seen more than once (same from, to and kind).
    pub duplicate_references: Vec<(usize, usize)>,
    #[serde(skip)]
    index: BTreeMap<String, usize>,
}

impl MediaGraph {
    pub fn node(&mut self, kind: NodeKind, key: String, path: Option<PathBuf>) -> usize {
        if let Some(&i) = self.index.get(&key) {
            return i;
        }
        let present = path
            .as_deref()
            .is_some_and(|p| std::fs::symlink_metadata(p).is_ok());
        self.nodes.push(Node {
            kind,
            key: key.clone(),
            path,
            present,
        });
        self.index.insert(key, self.nodes.len() - 1);
        self.nodes.len() - 1
    }

    pub fn file(&mut self, path: &Path) -> usize {
        let kind = if is_descriptor(path) {
            NodeKind::Descriptor
        } else {
            NodeKind::File
        };
        self.node(
            kind,
            format!("file:{}", path.display()),
            Some(path.to_path_buf()),
        )
    }

    pub fn lookup(&self, path: &Path) -> Option<usize> {
        self.index.get(&format!("file:{}", path.display())).copied()
    }

    pub fn edge(&mut self, from: usize, to: usize, kind: EdgeKind, verification: Verification) {
        if self
            .edges
            .iter()
            .any(|e| e.from == from && e.to == to && e.kind == kind)
        {
            self.duplicate_references.push((from, to));
            return;
        }
        self.edges.push(Edge {
            from,
            to,
            kind,
            verification,
        });
    }

    pub fn inbound(&self, node: usize) -> impl Iterator<Item = &Edge> {
        self.edges.iter().filter(move |e| e.to == node)
    }

    pub fn outbound(&self, node: usize) -> impl Iterator<Item = &Edge> {
        self.edges.iter().filter(move |e| e.from == node)
    }

    /// Strongly connected components with more than one node, or a self loop.
    pub fn cycles(&self) -> Vec<Vec<usize>> {
        // Iterative Tarjan: no recursion, so a hostile graph cannot overflow.
        let n = self.nodes.len();
        let mut adjacency = vec![Vec::new(); n];
        for e in &self.edges {
            adjacency[e.from].push(e.to);
        }
        let mut index = vec![usize::MAX; n];
        let mut low = vec![0usize; n];
        let mut on_stack = vec![false; n];
        let mut stack = Vec::new();
        let mut next = 0usize;
        let mut out = Vec::new();
        for root in 0..n {
            if index[root] != usize::MAX {
                continue;
            }
            let mut work: Vec<(usize, usize)> = vec![(root, 0)];
            while let Some(&mut (v, ref mut child)) = work.last_mut() {
                if *child == 0 {
                    index[v] = next;
                    low[v] = next;
                    next += 1;
                    stack.push(v);
                    on_stack[v] = true;
                }
                if *child < adjacency[v].len() {
                    let w = adjacency[v][*child];
                    *child += 1;
                    if index[w] == usize::MAX {
                        work.push((w, 0));
                    } else if on_stack[w] {
                        low[v] = low[v].min(index[w]);
                    }
                    continue;
                }
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    low[parent] = low[parent].min(low[v]);
                }
                if low[v] == index[v] {
                    let mut component = Vec::new();
                    while let Some(w) = stack.pop() {
                        on_stack[w] = false;
                        component.push(w);
                        if w == v {
                            break;
                        }
                    }
                    let self_loop = component.len() == 1 && adjacency[v].contains(&v);
                    if component.len() > 1 || self_loop {
                        component.sort_unstable();
                        out.push(component);
                    }
                }
            }
        }
        out
    }

    /// Members named by more than one descriptor that are not themselves
    /// nested (an M3U naming a CUE that names a BIN is one ownership chain).
    pub fn ambiguous_owners(&self) -> Vec<(usize, Vec<usize>)> {
        let mut owners: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for e in self.edges.iter().filter(|e| e.kind == EdgeKind::Names) {
            owners.entry(e.to).or_default().insert(e.from);
        }
        owners
            .into_iter()
            .filter_map(|(member, set)| {
                // Drop owners that are themselves owned by another owner of
                // this member (nesting, not ambiguity).
                let independent: Vec<usize> = set
                    .iter()
                    .copied()
                    .filter(|o| {
                        !self
                            .inbound(*o)
                            .any(|e| e.kind == EdgeKind::Names && set.contains(&e.from))
                    })
                    .collect();
                (independent.len() > 1).then_some((member, independent))
            })
            .collect()
    }

    pub fn missing(&self) -> Vec<usize> {
        (0..self.nodes.len())
            .filter(|&i| self.nodes[i].path.is_some() && !self.nodes[i].present)
            .collect()
    }

    /// Deterministic digest of nodes, edges and unknowns: plans bind it so a
    /// changed neighbourhood invalidates them (I5).
    pub fn digest(&self) -> String {
        let mut lines: Vec<String> = self
            .edges
            .iter()
            .map(|e| {
                format!(
                    "E|{}|{}|{:?}|{:?}",
                    self.nodes[e.from].key, self.nodes[e.to].key, e.kind, e.verification
                )
            })
            .chain(
                self.unknowns
                    .iter()
                    .map(|(n, why)| format!("U|{}|{why}", self.nodes[*n].key)),
            )
            .chain(
                self.nodes
                    .iter()
                    .map(|n| format!("N|{}|{:?}|{}", n.key, n.kind, n.present)),
            )
            .collect();
        lines.sort();
        let mut h = Sha256::new();
        for line in lines {
            h.update(line.as_bytes());
            h.update(b"\n");
        }
        super::journal::hex(&h.finalize())
    }
}

/// Builds the descriptor graph of one directory (non-recursive).
pub fn build_directory(dir: &Path) -> Result<MediaGraph, String> {
    let mut graph = MediaGraph::default();
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("{} cannot be listed: {e}", dir.display()))?;
    let mut descriptors = Vec::new();
    for (count, entry) in entries.enumerate() {
        if count >= MAX_NEIGHBOURHOOD_ENTRIES {
            return Err(format!(
                "{} has more than {MAX_NEIGHBOURHOOD_ENTRIES} entries; its dependencies were not established",
                dir.display()
            ));
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if is_descriptor(&path) {
            descriptors.push(path);
        }
    }
    descriptors.sort();
    for descriptor in descriptors {
        let d = graph.file(&descriptor);
        match descriptor_references(&descriptor) {
            Ok(members) => {
                for member in members {
                    let m = graph.file(&member);
                    graph.edge(
                        d,
                        m,
                        EdgeKind::Names,
                        Verification::Structural {
                            by: "descriptor parser".into(),
                        },
                    );
                }
            }
            Err(why) => graph.unknowns.push((d, why)),
        }
    }
    Ok(graph)
}

/// Whether a single file may be relocated on its own, and why not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub blockers: Vec<String>,
    pub notes: Vec<String>,
    /// Digest of the neighbourhood graph (bound into the plan).
    pub digest: String,
    pub edges: usize,
    pub unknowns: usize,
}

/// Filesystem-only verdict for relocating `source` alone. Re-run at apply.
pub fn relocation_verdict(source: &Path) -> Verdict {
    let mut blockers = Vec::new();
    let mut notes = Vec::new();
    let Some(dir) = source.parent() else {
        return Verdict {
            blockers: vec!["the file has no folder".into()],
            notes,
            digest: String::new(),
            edges: 0,
            unknowns: 0,
        };
    };
    let graph = match build_directory(dir) {
        Ok(g) => g,
        Err(why) => {
            return Verdict {
                blockers: vec![why],
                notes,
                digest: String::new(),
                edges: 0,
                unknowns: 0,
            };
        }
    };
    let name = |i: usize| {
        graph.nodes[i]
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    };
    if is_descriptor(source) {
        blockers.push(
            "this file is a set descriptor (CUE/GDI/M3U); its tracks or discs depend on it, and moving it alone would break the set"
                .into(),
        );
    }
    if let Some(node) = graph.lookup(source) {
        for e in graph.inbound(node) {
            blockers.push(format!(
                "this file is named by {}; it must move with that set",
                name(e.from)
            ));
        }
        for e in graph.outbound(node) {
            blockers.push(format!(
                "this file names {}; it must move with it",
                name(e.to)
            ));
        }
        for (owner, members) in graph
            .ambiguous_owners()
            .into_iter()
            .filter(|(m, _)| *m == node)
        {
            let _ = owner;
            blockers.push(format!(
                "this file is claimed by {} sets; ownership is ambiguous",
                members.len()
            ));
        }
        if graph.cycles().iter().any(|c| c.contains(&node)) {
            blockers.push("this file is part of a descriptor cycle".into());
        }
    }
    for (d, why) in &graph.unknowns {
        blockers.push(format!(
            "{} could not be read ({why}); whether it uses this file is unknown, so the move is refused",
            name(*d)
        ));
    }
    if !graph.duplicate_references.is_empty() {
        notes.push(format!(
            "{} duplicate descriptor reference(s) in this folder",
            graph.duplicate_references.len()
        ));
    }
    notes.push(
        "saves, patch provenance, per-game emulator settings and launcher playlists are not modelled; \
         any of them that name this file by path will need updating separately"
            .into(),
    );
    Verdict {
        blockers,
        notes,
        digest: graph.digest(),
        edges: graph.edges.len(),
        unknowns: graph.unknowns.len(),
    }
}
