//! Per-pair SQLite state: the last-synced base snapshot, a mirror of the remote folder tree, and
//! small key/value metadata such as the Drive changes page token.

use crate::model::{BaseEntry, BaseMap};
use crate::remote_tree::{RKind, RNode};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub struct StateDb {
    conn: Connection,
}

#[derive(Default)]
pub struct Batch {
    pub base_put: Vec<(String, BaseEntry)>,
    pub base_del: Vec<String>,
    pub remote_put: Vec<RNode>,
    pub remote_del: Vec<String>,
    pub meta: Vec<(String, String)>,
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.base_put.is_empty() && self.base_del.is_empty() && self.remote_put.is_empty() && self.remote_del.is_empty() && self.meta.is_empty()
    }
}

impl StateDb {
    pub fn open(path: &Path) -> Result<StateDb> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA temp_store=MEMORY;
             CREATE TABLE IF NOT EXISTS base(
                key TEXT PRIMARY KEY, path TEXT NOT NULL, is_dir INTEGER NOT NULL, size INTEGER NOT NULL,
                mtime_ns INTEGER NOT NULL, file_id INTEGER NOT NULL, md5 TEXT, remote_id TEXT NOT NULL,
                remote_md5 TEXT, remote_mtime_ms INTEGER NOT NULL) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS remote(
                id TEXT PRIMARY KEY, parent TEXT NOT NULL, name TEXT NOT NULL, kind INTEGER NOT NULL,
                size INTEGER NOT NULL, md5 TEXT, mtime_ms INTEGER NOT NULL) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;",
        )?;
        Ok(StateDb { conn })
    }

    pub fn load_base(&self) -> Result<BaseMap> {
        let mut st = self.conn.prepare("SELECT key,path,is_dir,size,mtime_ns,file_id,md5,remote_id,remote_md5,remote_mtime_ms FROM base")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                BaseEntry {
                    path: r.get(1)?,
                    is_dir: r.get::<_, i64>(2)? != 0,
                    size: r.get::<_, i64>(3)? as u64,
                    mtime_ns: r.get(4)?,
                    file_id: r.get::<_, i64>(5)? as u64,
                    md5: r.get(6)?,
                    remote_id: r.get(7)?,
                    remote_md5: r.get(8)?,
                    remote_mtime_ms: r.get(9)?,
                },
            ))
        })?;
        let mut map = BaseMap::new();
        for row in rows {
            let (k, v) = row?;
            map.insert(k, v);
        }
        Ok(map)
    }

    pub fn load_remote(&self) -> Result<Vec<RNode>> {
        let mut st = self.conn.prepare("SELECT id,parent,name,kind,size,md5,mtime_ms FROM remote")?;
        let rows = st.query_map([], |r| {
            Ok(RNode {
                id: r.get(0)?,
                parent: r.get(1)?,
                name: r.get(2)?,
                kind: RKind::from_i64(r.get(3)?),
                size: r.get::<_, i64>(4)? as u64,
                md5: r.get(5)?,
                mtime_ms: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn apply(&mut self, b: &Batch) -> Result<()> {
        if b.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut del = tx.prepare_cached("DELETE FROM base WHERE key=?1")?;
            for k in &b.base_del {
                del.execute([k])?;
            }
            let mut put = tx.prepare_cached(
                "INSERT OR REPLACE INTO base(key,path,is_dir,size,mtime_ns,file_id,md5,remote_id,remote_md5,remote_mtime_ms)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            )?;
            for (k, e) in &b.base_put {
                put.execute(params![
                    k,
                    e.path,
                    e.is_dir as i64,
                    e.size as i64,
                    e.mtime_ns,
                    e.file_id as i64,
                    e.md5,
                    e.remote_id,
                    e.remote_md5,
                    e.remote_mtime_ms
                ])?;
            }
            let mut rdel = tx.prepare_cached("DELETE FROM remote WHERE id=?1")?;
            for id in &b.remote_del {
                rdel.execute([id])?;
            }
            let mut rput = tx.prepare_cached("INSERT OR REPLACE INTO remote(id,parent,name,kind,size,md5,mtime_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)")?;
            for n in &b.remote_put {
                rput.execute(params![n.id, n.parent, n.name, n.kind.to_i64(), n.size as i64, n.md5, n.mtime_ms])?;
            }
            let mut meta = tx.prepare_cached("INSERT OR REPLACE INTO meta(key,value) VALUES(?1,?2)")?;
            for (k, v) in &b.meta {
                meta.execute([k, v])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Wipes all state (used when a pair's local or remote root changes).
    pub fn reset(&mut self) -> Result<()> {
        self.conn.execute_batch("DELETE FROM base; DELETE FROM remote; DELETE FROM meta;")?;
        Ok(())
    }
}
