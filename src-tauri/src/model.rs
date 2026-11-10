//! Core data types shared by the scanner, the remote tree, the planner and the state database.
//!
//! Every map is keyed by a *folded* relative path (`a/b/c.txt`, lower-cased on Windows). Each
//! entry also stores its real `path` with the original case, used for display and creation.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct LocalEntry {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime_ns: i64,
    pub file_id: u64,
}

/// The state both sides had the last time this path was successfully synced.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BaseEntry {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime_ns: i64,
    pub file_id: u64,
    pub md5: Option<String>,
    pub remote_id: String,
    pub remote_md5: Option<String>,
    pub remote_mtime_ms: i64,
}

/// A remote file as seen through the path-addressed view of the remote tree.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteView {
    pub id: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub md5: Option<String>,
    pub mtime_ms: i64,
}

pub type LocalMap = BTreeMap<String, LocalEntry>;
pub type BaseMap = BTreeMap<String, BaseEntry>;

pub fn range_keys<V>(map: &BTreeMap<String, V>, prefix: &str) -> Vec<String> {
    let (lo, hi) = crate::fsutil::descendant_range(prefix);
    map.range(lo..hi).map(|(k, _)| k.clone()).collect()
}

/// Removes `prefix` and all of its descendants from a map, returning the removed keys.
pub fn remove_subtree<V>(map: &mut BTreeMap<String, V>, prefix: &str) -> Vec<String> {
    let mut keys = range_keys(map, prefix);
    if map.remove(prefix).is_some() {
        keys.push(prefix.to_string());
    }
    for k in &keys {
        map.remove(k);
    }
    keys
}

/// Moves a subtree from `from` to `to` (keys and the `path` field via `repath`).
pub fn move_subtree<V>(map: &mut BTreeMap<String, V>, from: &str, to: &str, mut repath: impl FnMut(&mut V, &str)) -> Vec<(String, String)> {
    let mut moved = Vec::new();
    let mut keys = vec![from.to_string()];
    keys.extend(range_keys(map, from));
    for k in keys {
        if let Some(mut v) = map.remove(&k) {
            let nk = format!("{to}{}", &k[from.len()..]);
            repath(&mut v, &nk);
            moved.push((k, nk.clone()));
            map.insert(nk, v);
        }
    }
    moved
}

/// Swaps the folded prefix of a real path for a new real prefix, keeping the suffix's case.
pub fn rebase_real_path(old_real: &str, old_prefix_len_components: usize, new_prefix_real: &str) -> String {
    let rest: Vec<&str> = old_real.split('/').skip(old_prefix_len_components).collect();
    if rest.is_empty() {
        new_prefix_real.to_string()
    } else if new_prefix_real.is_empty() {
        rest.join("/")
    } else {
        format!("{new_prefix_real}/{}", rest.join("/"))
    }
}

/// Direct children of `prefix` in a sorted map, skipping over each child's own subtree.
pub fn direct_children<V>(map: &BTreeMap<String, V>, prefix: &str) -> Vec<String> {
    use std::ops::Bound::{Excluded, Included};
    let (lo, hi) = crate::fsutil::descendant_range(prefix);
    let plen = lo.len();
    let mut out = Vec::new();
    let mut start = Included(lo);
    while let Some((k, _)) = map.range((start.clone(), Excluded(hi.clone()))).next() {
        match k[plen..].find('/') {
            // a grandchild: everything under that child sorts before "child0"
            Some(i) => start = Included(format!("{}0", &k[..plen + i])),
            None => {
                out.push(k.clone());
                start = Excluded(k.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn children_and_subtrees() {
        let mut m: BTreeMap<String, u8> = BTreeMap::new();
        for k in ["a", "a-b", "a-b/x", "a.txt", "a/x", "a/x/y", "a/z", "b", "b/c"] {
            m.insert(k.to_string(), 0);
        }
        assert_eq!(direct_children(&m, ""), vec!["a", "a-b", "a.txt", "b"]);
        assert_eq!(direct_children(&m, "a"), vec!["a/x", "a/z"]);
        let removed = remove_subtree(&mut m, "a");
        assert_eq!(removed.len(), 4);
        assert!(m.contains_key("a-b/x") && m.contains_key("a.txt"));
        let moved = move_subtree(&mut m, "b", "q", |_, _| {});
        assert_eq!(moved.len(), 2);
        assert!(m.contains_key("q/c"));
        assert_eq!(rebase_real_path("Old/Sub/File.TXT", 1, "New"), "New/Sub/File.TXT");
    }
}
