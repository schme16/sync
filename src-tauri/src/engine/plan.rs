//! Three-way reconciliation planner.
//!
//! For every candidate path the planner compares the current local entry (L) and the current
//! remote entry (R) against the base (B): the state both sides had after the last successful sync.
//! That tells it *which side changed*, which is what makes deletions, renames and conflicts
//! unambiguous. The planner is pure: no I/O, so it is exhaustively unit-tested.

use crate::config::SyncMode;
use crate::fsutil::descendant_range;
use crate::model::{BaseEntry, BaseMap, LocalEntry, LocalMap, RemoteView};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum St {
    Absent,
    New,
    Unchanged,
    /// Only the local mtime changed; content hash still matches the base.
    Touched,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Both sides already agree: (re)write the base entry.
    Record,
    /// Local timestamp moved but content is identical: refresh base stat info only.
    TouchBase,
    /// Make the remote match local (upload file / create folder). `replace_remote` means the remote
    /// item has a different kind and must be trashed first.
    Push { replace_remote: bool },
    /// Make local match remote (download file / create folder).
    Pull { replace_local: bool },
    DeleteLocal,
    DeleteRemote,
    /// Both sides are gone (or tracking stops): drop the base entry.
    Forget,
    /// Both sides changed differently: move the local copy aside, then pull the remote version.
    Conflict,
}

pub struct PlanInput<'a> {
    pub keys: &'a [String],
    pub l: &'a LocalMap,
    pub b: &'a BaseMap,
    pub r: &'a HashMap<String, RemoteView>,
    pub hashes: &'a HashMap<String, String>,
    pub mode: SyncMode,
    pub propagate_deletes: bool,
}

fn stat_unchanged(l: &LocalEntry, b: &BaseEntry) -> bool {
    l.is_dir == b.is_dir && (l.is_dir || (l.size == b.size && l.mtime_ns == b.mtime_ns))
}

pub fn local_state(key: &str, l: Option<&LocalEntry>, b: Option<&BaseEntry>, hashes: &HashMap<String, String>) -> St {
    match (l, b) {
        (None, None) => St::Absent,
        (Some(_), None) => St::New,
        (None, Some(_)) => St::Deleted,
        (Some(l), Some(b)) => {
            if stat_unchanged(l, b) {
                St::Unchanged
            } else if l.is_dir != b.is_dir {
                St::Modified
            } else if l.size == b.size && b.md5.is_some() && hashes.get(key) == b.md5.as_ref() {
                St::Touched
            } else {
                St::Modified
            }
        }
    }
}

pub fn remote_state(r: Option<&RemoteView>, b: Option<&BaseEntry>) -> St {
    match (r, b) {
        (None, None) => St::Absent,
        (Some(_), None) => St::New,
        (None, Some(_)) => St::Deleted,
        (Some(r), Some(b)) => {
            if r.is_dir != b.is_dir || r.id != b.remote_id {
                St::Modified
            } else if r.is_dir || r.md5 == b.remote_md5 {
                St::Unchanged
            } else {
                St::Modified
            }
        }
    }
}

/// Keys whose local MD5 must be computed before planning. Hashing only happens when stat data is
/// inconclusive, so a steady-state sync never reads file contents.
pub fn hash_needs(keys: &[String], l: &LocalMap, b: &BaseMap, r: &HashMap<String, RemoteView>) -> Vec<String> {
    let mut out = Vec::new();
    for k in keys {
        let Some(le) = l.get(k) else { continue };
        if le.is_dir {
            continue;
        }
        let be = b.get(k);
        if let Some(be) = be {
            if stat_unchanged(le, be) {
                continue;
            }
            if !be.is_dir && be.size == le.size && be.md5.is_some() {
                out.push(k.clone());
                continue;
            }
        }
        if let Some(re) = r.get(k) {
            if !re.is_dir && re.size == le.size && re.md5.is_some() {
                out.push(k.clone());
            }
        }
    }
    out
}

pub fn plan(inp: &PlanInput) -> BTreeMap<String, Decision> {
    let push = inp.mode.allows_push();
    let pull = inp.mode.allows_pull();
    let prop = inp.propagate_deletes;
    let mut out: BTreeMap<String, Decision> = BTreeMap::new();

    for k in inp.keys {
        let l = inp.l.get(k);
        let b = inp.b.get(k);
        let r = inp.r.get(k);
        let ls = local_state(k, l, b, inp.hashes);
        let rs = remote_state(r, b);
        let same_content = || match (l, r) {
            (Some(l), Some(r)) if l.is_dir && r.is_dir => true,
            (Some(l), Some(r)) if !l.is_dir && !r.is_dir => r.md5.is_some() && inp.hashes.get(k) == r.md5.as_ref() && l.size == r.size,
            _ => false,
        };
        let kind_differs = || matches!((l, r), (Some(l), Some(r)) if l.is_dir != r.is_dir);
        use St::*;
        let d = match (ls, rs) {
            (Absent, Absent) => None,
            (Unchanged, Unchanged) => None,
            (Touched, Unchanged) => Some(Decision::TouchBase),
            (New, Absent) => push.then_some(Decision::Push { replace_remote: false }),
            (Absent, New) => pull.then_some(Decision::Pull { replace_local: false }),
            (New, New) | (Modified, Modified) | (Touched, Modified) | (Modified, New) | (New, Modified) => {
                if same_content() {
                    Some(Decision::Record)
                } else if ls == Touched {
                    // local content is what we last synced; only remote really changed
                    pull.then(|| Decision::Pull { replace_local: kind_differs() })
                } else {
                    match inp.mode {
                        SyncMode::TwoWay => Some(Decision::Conflict),
                        SyncMode::LocalToRemote => Some(Decision::Push { replace_remote: kind_differs() }),
                        SyncMode::RemoteToLocal => Some(Decision::Pull { replace_local: kind_differs() }),
                    }
                }
            }
            (Unchanged, Modified) => {
                let same_as_base = matches!((r, b), (Some(r), Some(b)) if !r.is_dir && !b.is_dir && r.md5.is_some() && r.md5 == b.md5);
                if same_as_base {
                    Some(Decision::Record) // re-uploaded elsewhere with identical bytes: just adopt the new id
                } else {
                    pull.then(|| Decision::Pull { replace_local: kind_differs() })
                }
            }
            (Unchanged | Touched, Deleted) => {
                if !pull {
                    None
                } else if prop {
                    Some(Decision::DeleteLocal)
                } else if push {
                    Some(Decision::Push { replace_remote: false })
                } else {
                    Some(Decision::Forget)
                }
            }
            (Modified, Unchanged) => {
                if !push {
                    None
                } else if same_content() {
                    Some(Decision::Record)
                } else {
                    Some(Decision::Push { replace_remote: kind_differs() })
                }
            }
            (Deleted, Unchanged) => {
                if !push {
                    None
                } else if prop {
                    Some(Decision::DeleteRemote)
                } else if pull {
                    Some(Decision::Pull { replace_local: false })
                } else {
                    Some(Decision::Forget)
                }
            }
            (Deleted, Modified) => {
                if pull {
                    Some(Decision::Pull { replace_local: false })
                } else if prop {
                    Some(Decision::DeleteRemote)
                } else {
                    Some(Decision::Forget)
                }
            }
            (Modified, Deleted) => {
                if push {
                    Some(Decision::Push { replace_remote: false })
                } else if prop {
                    Some(Decision::DeleteLocal)
                } else {
                    Some(Decision::Forget)
                }
            }
            (Deleted, Deleted) => Some(Decision::Forget),
            // Remaining combinations cannot occur (New/Absent imply no base; the others imply one).
            _ => None,
        };
        if let Some(d) = d {
            out.insert(k.clone(), d);
        }
    }

    fixup_directories(inp, &mut out);
    out
}

fn descendants<'m>(m: &'m BTreeMap<String, Decision>, k: &str) -> impl Iterator<Item = (&'m String, &'m Decision)> {
    let (lo, hi) = descendant_range(k);
    m.range(lo..hi)
}

/// Folder level rules, applied deepest first:
/// * A folder deletion is only executed if every decision beneath it is also a deletion; if
///   anything inside was added or edited on the deleting side's opposite, the folder is restored
///   instead and its unchanged children are deleted individually.
/// * When a whole folder is deleted, the per-child decisions collapse into that one operation.
/// * When a folder is replaced by a file (or moved aside by a conflict), its children are dropped.
fn fixup_directories(inp: &PlanInput, out: &mut BTreeMap<String, Decision>) {
    let keys: Vec<String> = out.keys().rev().cloned().collect();
    for k in keys {
        let Some(d) = out.get(&k).cloned() else { continue };
        let l_dir = inp.l.get(&k).is_some_and(|e| e.is_dir);
        let r_dir = inp.r.get(&k).is_some_and(|e| e.is_dir);
        let b_dir = inp.b.get(&k).is_some_and(|e| e.is_dir);
        match d {
            Decision::DeleteLocal if l_dir || b_dir => {
                let blocked = descendants(out, &k).any(|(_, d)| matches!(d, Decision::Push { .. } | Decision::Conflict | Decision::Record));
                if blocked {
                    if inp.mode.allows_push() {
                        out.insert(k.clone(), Decision::Push { replace_remote: false });
                    } else {
                        out.remove(&k);
                    }
                } else {
                    let drop: Vec<String> = descendants(out, &k).map(|(k, _)| k.clone()).collect();
                    for c in drop {
                        out.remove(&c);
                    }
                }
            }
            Decision::DeleteRemote if r_dir || b_dir => {
                let blocked = descendants(out, &k).any(|(_, d)| matches!(d, Decision::Pull { .. } | Decision::Conflict | Decision::Record));
                if blocked {
                    if inp.mode.allows_pull() {
                        out.insert(k.clone(), Decision::Pull { replace_local: false });
                    } else {
                        out.remove(&k);
                    }
                } else {
                    let drop: Vec<String> = descendants(out, &k).map(|(k, _)| k.clone()).collect();
                    for c in drop {
                        out.remove(&c);
                    }
                }
            }
            Decision::Push { replace_remote: true } if r_dir => {
                let drop: Vec<String> = descendants(out, &k).map(|(k, _)| k.clone()).collect();
                for c in drop {
                    out.remove(&c);
                }
            }
            Decision::Pull { replace_local: true } | Decision::Conflict if l_dir => {
                let drop: Vec<String> = descendants(out, &k).map(|(k, _)| k.clone()).collect();
                for c in drop {
                    out.remove(&c);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct W {
        l: LocalMap,
        b: BaseMap,
        r: HashMap<String, RemoteView>,
        h: HashMap<String, String>,
    }

    impl W {
        fn new() -> W {
            W { l: LocalMap::new(), b: BaseMap::new(), r: HashMap::new(), h: HashMap::new() }
        }
        fn local(&mut self, k: &str, dir: bool, size: u64, mtime: i64) -> &mut Self {
            self.l.insert(k.into(), LocalEntry { path: k.into(), is_dir: dir, size, mtime_ns: mtime, file_id: 0 });
            self
        }
        fn remote(&mut self, k: &str, id: &str, dir: bool, size: u64, md5: &str) -> &mut Self {
            self.r.insert(
                k.into(),
                RemoteView { id: id.into(), path: k.into(), is_dir: dir, size, md5: (!dir).then(|| md5.to_string()), mtime_ms: 0 },
            );
            self
        }
        fn base(&mut self, k: &str, id: &str, dir: bool, size: u64, mtime: i64, md5: &str) -> &mut Self {
            let m = (!dir).then(|| md5.to_string());
            self.b.insert(
                k.into(),
                BaseEntry {
                    path: k.into(),
                    is_dir: dir,
                    size,
                    mtime_ns: mtime,
                    file_id: 0,
                    md5: m.clone(),
                    remote_id: id.into(),
                    remote_md5: m,
                    remote_mtime_ms: 0,
                },
            );
            self
        }
        /// file present & identical on both sides and in base
        fn synced(&mut self, k: &str, id: &str) -> &mut Self {
            self.local(k, false, 10, 1).remote(k, id, false, 10, "m").base(k, id, false, 10, 1, "m")
        }
        fn synced_dir(&mut self, k: &str, id: &str) -> &mut Self {
            self.local(k, true, 0, 0).remote(k, id, true, 0, "").base(k, id, true, 0, 0, "")
        }
        fn run(&self, mode: SyncMode, prop: bool) -> BTreeMap<String, Decision> {
            let mut keys: Vec<String> = self.l.keys().chain(self.b.keys()).chain(self.r.keys()).cloned().collect();
            keys.sort();
            keys.dedup();
            plan(&PlanInput { keys: &keys, l: &self.l, b: &self.b, r: &self.r, hashes: &self.h, mode, propagate_deletes: prop })
        }
    }

    use Decision::*;

    #[test]
    fn steady_state_is_noop() {
        let mut w = W::new();
        w.synced_dir("d", "D").synced("d/a", "A").synced("b", "B");
        assert!(w.run(SyncMode::TwoWay, true).is_empty());
    }

    #[test]
    fn basic_propagation() {
        let mut w = W::new();
        w.local("new_local", false, 5, 1);
        w.remote("new_remote", "R", false, 5, "x");
        w.synced("edited_local", "E1").local("edited_local", false, 11, 2);
        w.synced("edited_remote", "E2").remote("edited_remote", "E2", false, 10, "changed");
        w.synced("gone_local", "G1");
        w.l.remove("gone_local");
        w.synced("gone_remote", "G2");
        w.r.remove("gone_remote");
        let p = w.run(SyncMode::TwoWay, true);
        assert_eq!(p["new_local"], Push { replace_remote: false });
        assert_eq!(p["new_remote"], Pull { replace_local: false });
        assert_eq!(p["edited_local"], Push { replace_remote: false });
        assert_eq!(p["edited_remote"], Pull { replace_local: false });
        assert_eq!(p["gone_local"], DeleteRemote);
        assert_eq!(p["gone_remote"], DeleteLocal);
    }

    #[test]
    fn touch_only_does_not_upload() {
        let mut w = W::new();
        w.synced("a", "A").local("a", false, 10, 999);
        w.h.insert("a".into(), "m".into());
        assert_eq!(w.run(SyncMode::TwoWay, true)["a"], TouchBase);
    }

    #[test]
    fn initial_adoption_matches_by_hash() {
        let mut w = W::new();
        w.local("same", false, 3, 1).remote("same", "S", false, 3, "abc");
        w.h.insert("same".into(), "abc".into());
        w.local("diff", false, 3, 1).remote("diff", "D", false, 3, "abc");
        w.h.insert("diff".into(), "zzz".into());
        w.local("dir", true, 0, 0).remote("dir", "DIR", true, 0, "");
        let p = w.run(SyncMode::TwoWay, true);
        assert_eq!(p["same"], Record);
        assert_eq!(p["diff"], Conflict);
        assert_eq!(p["dir"], Record);
        assert_eq!(w.run(SyncMode::LocalToRemote, true)["diff"], Push { replace_remote: false });
        assert_eq!(w.run(SyncMode::RemoteToLocal, true)["diff"], Pull { replace_local: false });
    }

    #[test]
    fn hash_needs_only_when_inconclusive() {
        let mut w = W::new();
        w.synced("steady", "S");
        w.synced("touched", "T").local("touched", false, 10, 5);
        w.synced("grown", "G").local("grown", false, 99, 5);
        w.local("newsame", false, 7, 0).remote("newsame", "N", false, 7, "q");
        w.local("newdiff", false, 7, 0).remote("newdiff", "M", false, 8, "q");
        let mut keys: Vec<String> = w.l.keys().cloned().collect();
        keys.sort();
        let need = hash_needs(&keys, &w.l, &w.b, &w.r);
        assert_eq!(need, vec!["newsame".to_string(), "touched".to_string()]);
    }

    #[test]
    fn folder_becomes_file_locally() {
        // The Insync failure case: a folder is deleted and a file with the same name takes its place.
        let mut w = W::new();
        w.synced_dir("x", "X").synced("x/a", "A").synced("x/b", "B");
        w.l.remove("x/a");
        w.l.remove("x/b");
        w.local("x", false, 4, 9);
        let p = w.run(SyncMode::TwoWay, true);
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!(p["x"], Push { replace_remote: true });
    }

    #[test]
    fn file_becomes_folder_remotely() {
        let mut w = W::new();
        w.synced("x", "X");
        w.remote("x", "X2", true, 0, "").remote("x/inner", "I", false, 1, "i");
        let p = w.run(SyncMode::TwoWay, true);
        assert_eq!(p["x"], Pull { replace_local: true });
        assert_eq!(p["x/inner"], Pull { replace_local: false });
    }

    #[test]
    fn folder_delete_collapses() {
        let mut w = W::new();
        w.synced_dir("d", "D").synced("d/a", "A").synced_dir("d/s", "S").synced("d/s/b", "B");
        for k in ["d", "d/a", "d/s", "d/s/b"] {
            w.r.remove(k);
        }
        let p = w.run(SyncMode::TwoWay, true);
        assert_eq!(p.len(), 1);
        assert_eq!(p["d"], DeleteLocal);
    }

    #[test]
    fn folder_delete_blocked_by_new_content() {
        let mut w = W::new();
        w.synced_dir("d", "D").synced("d/a", "A");
        w.r.remove("d");
        w.r.remove("d/a");
        w.local("d/new", false, 1, 1);
        let p = w.run(SyncMode::TwoWay, true);
        assert_eq!(p["d"], Push { replace_remote: false });
        assert_eq!(p["d/a"], DeleteLocal);
        assert_eq!(p["d/new"], Push { replace_remote: false });
    }

    #[test]
    fn edit_beats_delete() {
        let mut w = W::new();
        w.synced("a", "A").local("a", false, 50, 2);
        w.r.remove("a");
        w.synced("b", "B").remote("b", "B", false, 10, "new");
        w.l.remove("b");
        let p = w.run(SyncMode::TwoWay, true);
        assert_eq!(p["a"], Push { replace_remote: false });
        assert_eq!(p["b"], Pull { replace_local: false });
    }

    #[test]
    fn one_way_modes_ignore_target_side_changes() {
        let mut w = W::new();
        w.remote("remote_only", "R", false, 1, "r");
        w.local("local_only", false, 1, 1);
        w.synced("remote_edit", "E").remote("remote_edit", "E", false, 10, "zz");
        let up = w.run(SyncMode::LocalToRemote, true);
        assert_eq!(up.get("remote_only"), None);
        assert_eq!(up.get("remote_edit"), None);
        assert_eq!(up["local_only"], Push { replace_remote: false });
        let down = w.run(SyncMode::RemoteToLocal, true);
        assert_eq!(down.get("local_only"), None);
        assert_eq!(down["remote_only"], Pull { replace_local: false });
    }

    #[test]
    fn no_delete_propagation() {
        let mut w = W::new();
        w.synced("a", "A");
        w.r.remove("a");
        w.synced("b", "B");
        w.l.remove("b");
        let two = w.run(SyncMode::TwoWay, false);
        assert_eq!(two["a"], Push { replace_remote: false }); // restored
        assert_eq!(two["b"], Pull { replace_local: false });
        let up = w.run(SyncMode::LocalToRemote, false);
        assert_eq!(up["b"], Forget);
        assert_eq!(up.get("a"), None);
    }

    #[test]
    fn remote_replaced_with_new_id() {
        let mut w = W::new();
        w.synced("a", "A").remote("a", "A2", false, 10, "m");
        // same content, new id (e.g. re-uploaded by another client): just re-record
        w.h.insert("a".into(), "m".into());
        let p = w.run(SyncMode::TwoWay, true);
        assert_eq!(p["a"], Record);
    }
}
