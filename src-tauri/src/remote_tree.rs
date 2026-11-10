//! In-memory mirror of the remote Drive folder tree beneath a sync pair's root folder.
//!
//! Drive identifies files by id, not path, and happily allows several siblings with the same name.
//! The tree keeps every node but exposes a path-addressed *view* where each (parent, folded name)
//! slot resolves to exactly one primary node; shadowed duplicates are reported as warnings.

use crate::fsutil::{child_key, fold, valid_local_name};
use crate::model::RemoteView;
use std::collections::{BTreeMap, HashMap};

pub const FOLDER_MIME: &str = "application/vnd.google-apps.folder";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RKind {
    File,
    Dir,
    /// Google-native documents, shortcuts, forms… These cannot be downloaded as bytes.
    Other,
}

impl RKind {
    pub fn from_mime(mime: &str) -> RKind {
        if mime == FOLDER_MIME {
            RKind::Dir
        } else if mime.starts_with("application/vnd.google-apps.") {
            RKind::Other
        } else {
            RKind::File
        }
    }
    pub fn to_i64(self) -> i64 {
        match self {
            RKind::File => 0,
            RKind::Dir => 1,
            RKind::Other => 2,
        }
    }
    pub fn from_i64(v: i64) -> RKind {
        match v {
            0 => RKind::File,
            1 => RKind::Dir,
            _ => RKind::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RNode {
    pub id: String,
    pub parent: String,
    pub name: String,
    pub kind: RKind,
    pub size: u64,
    pub md5: Option<String>,
    pub mtime_ms: i64,
}

impl RNode {
    fn syncable(&self) -> bool {
        self.kind != RKind::Other && valid_local_name(&self.name)
    }
}

pub struct RemoteTree {
    pub root_id: String,
    nodes: HashMap<String, RNode>,
    children: HashMap<String, BTreeMap<String, Vec<String>>>,
}

impl RemoteTree {
    pub fn new(root_id: &str) -> RemoteTree {
        RemoteTree { root_id: root_id.to_string(), nodes: HashMap::new(), children: HashMap::new() }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn get(&self, id: &str) -> Option<&RNode> {
        self.nodes.get(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    /// Whether `id` is the root or a folder in the tree.
    pub fn is_known_folder(&self, id: &str) -> bool {
        id == self.root_id || self.nodes.get(id).is_some_and(|n| n.kind == RKind::Dir)
    }

    fn detach(&mut self, id: &str) {
        if let Some(old) = self.nodes.get(id) {
            let key = fold(&old.name).into_owned();
            if let Some(ch) = self.children.get_mut(&old.parent) {
                if let Some(v) = ch.get_mut(&key) {
                    v.retain(|x| x != id);
                    if v.is_empty() {
                        ch.remove(&key);
                    }
                }
            }
        }
    }

    /// Inserts or updates a node (handles renames and moves).
    pub fn upsert(&mut self, node: RNode) {
        self.detach(&node.id);
        let slot = self.children.entry(node.parent.clone()).or_default().entry(fold(&node.name).into_owned()).or_default();
        slot.push(node.id.clone());
        slot.sort();
        self.nodes.insert(node.id.clone(), node);
    }

    /// Removes a node and everything below it. Returns the removed ids.
    pub fn remove_subtree(&mut self, id: &str) -> Vec<String> {
        let mut removed = Vec::new();
        self.detach(id);
        let mut stack = vec![id.to_string()];
        while let Some(cur) = stack.pop() {
            if self.nodes.remove(&cur).is_some() {
                removed.push(cur.clone());
            }
            if let Some(ch) = self.children.remove(&cur) {
                for ids in ch.into_values() {
                    stack.extend(ids);
                }
            }
        }
        removed
    }

    pub fn primary(&self, parent: &str, folded_name: &str) -> Option<&RNode> {
        let ids = self.children.get(parent)?.get(folded_name)?;
        ids.iter().filter_map(|i| self.nodes.get(i)).find(|n| n.syncable())
    }

    /// Resolves a folded key to its primary node.
    pub fn lookup(&self, key: &str) -> Option<&RNode> {
        if key.is_empty() {
            return None;
        }
        let mut parent = self.root_id.as_str();
        let mut node = None;
        for comp in key.split('/') {
            let n = self.primary(parent, comp)?;
            parent = &n.id;
            node = Some(n);
        }
        node
    }

    /// Folder id for a key ("" is the root). Only returns ids of folders.
    pub fn dir_id(&self, key: &str) -> Option<String> {
        if key.is_empty() {
            return Some(self.root_id.clone());
        }
        self.lookup(key).filter(|n| n.kind == RKind::Dir).map(|n| n.id.clone())
    }

    /// Folded key and real path of a node, if it is visible (attached to root and not shadowed).
    pub fn path_of(&self, id: &str) -> Option<(String, String)> {
        let mut comps: Vec<&str> = Vec::new();
        let mut cur = self.nodes.get(id)?;
        let mut guard = 0;
        loop {
            let p = self.primary(&cur.parent, &fold(&cur.name))?;
            if p.id != cur.id {
                return None;
            }
            comps.push(&cur.name);
            if cur.parent == self.root_id {
                break;
            }
            cur = self.nodes.get(&cur.parent)?;
            guard += 1;
            if guard > 4096 {
                return None; // cycle protection
            }
        }
        comps.reverse();
        let real = comps.join("/");
        Some((fold(&real).into_owned(), real))
    }

    fn to_view(n: &RNode, path: String) -> RemoteView {
        RemoteView { id: n.id.clone(), path, is_dir: n.kind == RKind::Dir, size: n.size, md5: n.md5.clone(), mtime_ms: n.mtime_ms }
    }

    pub fn view(&self, key: &str) -> Option<RemoteView> {
        let n = self.lookup(key)?;
        let (_, real) = self.path_of(&n.id)?;
        Some(Self::to_view(n, real))
    }

    /// All visible descendants of `key` (not including `key` itself).
    pub fn subtree_views(&self, key: &str) -> Vec<(String, RemoteView)> {
        let mut out = Vec::new();
        let (start_id, start_real) = if key.is_empty() {
            (self.root_id.clone(), String::new())
        } else {
            match self.lookup(key) {
                Some(n) if n.kind == RKind::Dir => match self.path_of(&n.id) {
                    Some((_, real)) => (n.id.clone(), real),
                    None => return out,
                },
                _ => return out,
            }
        };
        let mut stack = vec![(start_id, key.to_string(), start_real)];
        while let Some((id, k, real)) = stack.pop() {
            let Some(ch) = self.children.get(&id) else { continue };
            for (folded, _) in ch.iter() {
                if let Some(n) = self.primary(&id, folded) {
                    let ck = child_key(&k, folded);
                    let cr = child_key(&real, &n.name);
                    if n.kind == RKind::Dir {
                        stack.push((n.id.clone(), ck.clone(), cr.clone()));
                    }
                    out.push((ck, Self::to_view(n, cr)));
                }
            }
        }
        out
    }

    pub fn all_nodes(&self) -> impl Iterator<Item = &RNode> {
        self.nodes.values()
    }

    /// Drops nodes whose parent chain does not reach the root (used after a whole-drive listing).
    pub fn prune_detached(&mut self) -> Vec<String> {
        let mut attached: HashMap<String, bool> = HashMap::new();
        attached.insert(self.root_id.clone(), true);
        let ids: Vec<String> = self.nodes.keys().cloned().collect();
        for id in &ids {
            let mut chain = Vec::new();
            let mut cur = id.clone();
            let result = loop {
                if let Some(&a) = attached.get(&cur) {
                    break a;
                }
                chain.push(cur.clone());
                match self.nodes.get(&cur) {
                    Some(n) if chain.len() < 4096 => cur = n.parent.clone(),
                    _ => break false,
                }
            };
            for c in chain {
                attached.insert(c, result);
            }
        }
        let detached: Vec<String> = ids.into_iter().filter(|i| !attached.get(i).copied().unwrap_or(false)).collect();
        for id in &detached {
            self.detach(id);
            self.nodes.remove(id);
            self.children.remove(id);
        }
        detached
    }

    /// Human readable problems: duplicate names and names that are invalid on this OS.
    pub fn issues(&self, limit: usize) -> Vec<String> {
        let mut out = Vec::new();
        for (parent, ch) in &self.children {
            for ids in ch.values() {
                let nodes: Vec<&RNode> = ids.iter().filter_map(|i| self.nodes.get(i)).filter(|n| n.kind != RKind::Other).collect();
                let where_ = |n: &RNode| match self.path_of(parent) {
                    Some((_, p)) => child_key(&p, &n.name),
                    None => n.name.clone(),
                };
                if nodes.len() > 1 {
                    out.push(format!("Drive has {} items named \"{}\"; only one is synced", nodes.len(), where_(nodes[0])));
                }
                for n in nodes {
                    if !valid_local_name(&n.name) {
                        out.push(format!("\"{}\" is not a valid file name on this OS and is skipped", where_(n)));
                    }
                }
                if out.len() >= limit {
                    return out;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, parent: &str, name: &str, kind: RKind) -> RNode {
        RNode { id: id.into(), parent: parent.into(), name: name.into(), kind, size: 1, md5: Some("x".into()), mtime_ms: 0 }
    }

    #[test]
    fn paths_moves_and_dups() {
        let mut t = RemoteTree::new("root");
        t.upsert(node("d1", "root", "Docs", RKind::Dir));
        t.upsert(node("f1", "d1", "a.txt", RKind::File));
        t.upsert(node("g1", "d1", "Sheet", RKind::Other));
        let k = fold("Docs/a.txt").into_owned();
        assert_eq!(t.lookup(&k).unwrap().id, "f1");
        assert_eq!(t.path_of("f1").unwrap().1, "Docs/a.txt");
        assert_eq!(t.subtree_views("").len(), 2); // native doc excluded
        // rename the folder
        t.upsert(node("d1", "root", "Papers", RKind::Dir));
        assert_eq!(t.path_of("f1").unwrap().1, "Papers/a.txt");
        // duplicate: lowest id wins
        t.upsert(node("f0", "d1", "a.txt", RKind::File));
        assert_eq!(t.lookup(&fold("Papers/a.txt")).unwrap().id, "f0");
        assert!(t.path_of("f1").is_none());
        assert_eq!(t.issues(10).len(), 1);
        // remove subtree
        let removed = t.remove_subtree("d1");
        assert_eq!(removed.len(), 4);
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn prune() {
        let mut t = RemoteTree::new("root");
        t.upsert(node("a", "root", "a", RKind::Dir));
        t.upsert(node("b", "a", "b", RKind::File));
        t.upsert(node("x", "elsewhere", "x", RKind::Dir));
        t.upsert(node("y", "x", "y", RKind::File));
        let gone = t.prune_detached();
        assert_eq!(gone.len(), 2);
        assert_eq!(t.len(), 2);
    }
}
