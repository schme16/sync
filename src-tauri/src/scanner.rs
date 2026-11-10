//! Parallel local tree scanner and the virtual layout that grafts external folders (mounts) into a
//! sync pair as if they lived inside its base directory.

use crate::config::Mount;
use crate::fsutil::{self, child_key, fold, is_within, join_rel, parent_key};
use crate::ignore_rules::Rules;
use crate::model::LocalEntry;
use parking_lot::Mutex;
use rayon::prelude::*;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct MountPoint {
    pub key: String,
    pub real: String,
    pub target: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub root: PathBuf,
    /// Sorted longest key first so nested mounts resolve correctly.
    pub mounts: Vec<MountPoint>,
}

impl Layout {
    pub fn new(root: &str, mounts: &[Mount]) -> Layout {
        let mut m: Vec<MountPoint> = mounts
            .iter()
            .filter(|m| !m.rel_path.trim_matches('/').is_empty() && !m.local_path.is_empty())
            .map(|m| {
                let real = m.rel_path.replace('\\', "/").trim_matches('/').to_string();
                MountPoint { key: fold(&real).into_owned(), real, target: absolute(Path::new(&m.local_path)) }
            })
            .collect();
        m.sort_by(|a, b| b.key.len().cmp(&a.key.len()));
        Layout { root: absolute(Path::new(root)), mounts: m }
    }

    pub fn mount_for(&self, key: &str) -> Option<&MountPoint> {
        self.mounts.iter().find(|m| is_within(key, &m.key))
    }

    /// Absolute local path for a key. Works for existing items on case-insensitive file systems
    /// even though keys are folded; new items are created via `resolve(parent).join(real_name)`.
    pub fn resolve(&self, key: &str) -> PathBuf {
        match self.mount_for(key) {
            Some(m) => join_rel(&m.target, key[m.key.len()..].trim_start_matches('/')),
            None => join_rel(&self.root, key),
        }
    }

    /// Maps an absolute path reported by the watcher back to a key.
    pub fn key_of(&self, abs: &Path) -> Option<String> {
        for m in &self.mounts {
            if let Some(rel) = strip_prefix_ci(abs, &m.target) {
                return Some(if rel.is_empty() { m.key.clone() } else { child_key(&m.key, &fold(&rel)) });
            }
        }
        strip_prefix_ci(abs, &self.root).map(|r| fold(&r).into_owned())
    }

    pub fn watch_roots(&self) -> Vec<PathBuf> {
        let mut v = vec![self.root.clone()];
        v.extend(self.mounts.iter().map(|m| m.target.clone()));
        v
    }

    fn mounts_under(&self, key: &str) -> impl Iterator<Item = &MountPoint> {
        let key = key.to_string();
        self.mounts.iter().filter(move |m| parent_key(&m.key) == key)
    }
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Component-wise prefix strip; case-insensitive on Windows. Returns the remainder with `/`.
fn strip_prefix_ci(abs: &Path, base: &Path) -> Option<String> {
    let norm = |p: &Path| {
        let s = p.to_string_lossy().replace('\\', "/");
        let s = s.strip_prefix("//?/").map(|s| s.to_string()).unwrap_or(s);
        s.trim_end_matches('/').to_string()
    };
    let a = norm(abs);
    let b = norm(base);
    let (ac, bc) = if cfg!(windows) { (a.to_lowercase(), b.to_lowercase()) } else { (a.clone(), b.clone()) };
    if ac == bc {
        return Some(String::new());
    }
    if ac.len() > bc.len() && ac.starts_with(&bc) && ac.as_bytes()[bc.len()] == b'/' {
        // Lower-casing can change byte lengths for some scripts; fall back to component counting.
        let skip = b.split('/').count();
        return Some(a.split('/').skip(skip).collect::<Vec<_>>().join("/"));
    }
    None
}

#[derive(Default)]
pub struct ScanResult {
    pub entries: Vec<(String, LocalEntry)>,
    /// Absolute paths of every directory visited (Linux uses these to place inotify watches).
    pub dirs: Vec<PathBuf>,
    /// Keys of directories that could not be read: their contents are unknown, never "deleted".
    pub unreadable: Vec<String>,
}

/// Lists the direct children of `key`, applying ignore rules and mount grafting.
pub fn list_children(layout: &Layout, rules: &Rules, key: &str, real: &str) -> std::io::Result<Vec<(String, LocalEntry)>> {
    let abs = layout.resolve(key);
    let raw = fsutil::list_dir(&abs)?;
    let mut out = Vec::with_capacity(raw.len());
    let mounts: Vec<&MountPoint> = layout.mounts_under(key).collect();
    for e in raw {
        let folded = fold(&e.name);
        let ck = child_key(key, &folded);
        if mounts.iter().any(|m| m.key == ck) {
            continue; // shadowed by a mount
        }
        let cr = child_key(real, &e.name);
        if rules.is_ignored_shallow(&cr, &e.name, e.is_dir) {
            continue;
        }
        out.push((ck, LocalEntry { path: cr, is_dir: e.is_dir, size: e.size, mtime_ns: e.mtime_ns, file_id: e.file_id }));
    }
    for m in mounts {
        if let Ok(Some(st)) = fsutil::stat(&m.target) {
            if st.is_dir && !rules.is_ignored(&m.real, true) {
                out.push((m.key.clone(), LocalEntry { path: m.real.clone(), is_dir: true, size: 0, mtime_ns: st.mtime_ns, file_id: st.file_id }));
            }
        }
    }
    Ok(out)
}

/// Recursively scans everything below `key` in parallel.
pub fn scan_tree(layout: &Layout, rules: &Rules, key: &str, real: &str) -> ScanResult {
    let out = Mutex::new(ScanResult::default());
    crate::hash::pool().install(|| scan_dir(layout, rules, key, real, &out));
    out.into_inner()
}

fn scan_dir(layout: &Layout, rules: &Rules, key: &str, real: &str, out: &Mutex<ScanResult>) {
    let abs = layout.resolve(key);
    match list_children(layout, rules, key, real) {
        Ok(children) => {
            let subdirs: Vec<(String, String)> = children.iter().filter(|(_, e)| e.is_dir).map(|(k, e)| (k.clone(), e.path.clone())).collect();
            {
                let mut o = out.lock();
                o.dirs.push(abs);
                o.entries.extend(children);
            }
            subdirs.par_iter().for_each(|(k, r)| scan_dir(layout, rules, k, r, out));
        }
        Err(e) => {
            tracing::warn!("cannot read {}: {e}", abs.display());
            out.lock().unreadable.push(key.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_with_mount_and_ignore() {
        let root = tempfile::tempdir().unwrap();
        let ext = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("Proj/node_modules/x")).unwrap();
        std::fs::write(root.path().join("Proj/node_modules/x/i.js"), b"1").unwrap();
        std::fs::write(root.path().join("Proj/Main.rs"), b"fn main(){}").unwrap();
        std::fs::write(ext.path().join("notes.md"), b"hi").unwrap();
        let layout = Layout::new(
            &root.path().to_string_lossy(),
            &[Mount { local_path: ext.path().to_string_lossy().into_owned(), rel_path: "Linked/Notes".into() }],
        );
        let (rules, _) = Rules::new(Some("node_modules/"), "");
        // the mount's parent folder must exist physically for the graft to appear while walking
        std::fs::create_dir(root.path().join("Linked")).unwrap();
        let r = scan_tree(&layout, &rules, "", "");
        let mut keys: Vec<String> = r.entries.iter().map(|(k, _)| k.clone()).collect();
        keys.sort();
        let mut expect: Vec<String> =
            ["Linked", "Linked/Notes", "Linked/Notes/notes.md", "Proj", "Proj/Main.rs"].iter().map(|s| fold(s).into_owned()).collect();
        expect.sort();
        assert_eq!(keys, expect);
        let real: Vec<&str> = r.entries.iter().map(|(_, e)| e.path.as_str()).collect();
        assert!(real.contains(&"Linked/Notes/notes.md"));
        assert!(real.contains(&"Proj/Main.rs"));
        let mk = fold("Linked/Notes/notes.md").into_owned();
        assert_eq!(layout.key_of(&ext.path().join("notes.md")).unwrap(), mk);
        assert_eq!(layout.resolve(&mk), ext.path().join("notes.md"));
    }
}
