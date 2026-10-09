//! SQLite persistence: downloads, queues and key/value settings, each
//! stored as JSON documents (schema-tolerant across versions).

use crate::model::{Download, Queue, Settings};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use std::path::Path;
use std::time::Duration;

pub struct Store {
    conn: Mutex<Connection>,
}

fn log_err<T>(what: &str, r: rusqlite::Result<T>) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::error!("database: {what}: {e}");
            None
        }
    }
}

/// Copy of a download with the live-only fields cleared.
fn stored(d: &Download) -> Download {
    let mut d = d.clone();
    d.speed_bps = 0;
    d.eta_secs = None;
    d.speed_history.clear();
    d.active_connections = 0;
    d
}

const UPSERT_DOWNLOAD: &str = "INSERT INTO downloads (id, added, data) VALUES (?1, ?2, ?3)
     ON CONFLICT(id) DO UPDATE SET data = excluded.data";

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS downloads (id TEXT PRIMARY KEY, added TEXT, data TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS queues (id TEXT PRIMARY KEY, data TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn load_downloads(&self) -> Vec<Download> {
        self.load_json("SELECT data FROM downloads ORDER BY added", "load downloads")
    }

    fn load_json<T: serde::de::DeserializeOwned>(&self, sql: &str, what: &str) -> Vec<T> {
        let c = self.conn.lock();
        let Some(mut st) = log_err(what, c.prepare(sql)) else { return vec![] };
        let Some(rows) = log_err(what, st.query_map([], |r| r.get::<_, String>(0))) else { return vec![] };
        rows.filter_map(|r| log_err(what, r))
            .filter_map(|s| match serde_json::from_str(&s) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::error!("database: {what}: skipping unreadable row: {e}");
                    None
                }
            })
            .collect()
    }

    pub fn save_download(&self, d: &Download) {
        self.write_downloads(&[(d.id.clone(), Some(d.clone()))]);
    }

    /// Upsert (`Some`) or delete (`None`) several downloads in one transaction.
    pub fn write_downloads(&self, items: &[(String, Option<Download>)]) {
        if items.is_empty() {
            return;
        }
        let mut c = self.conn.lock();
        let Some(tx) = log_err("begin", c.transaction()) else { return };
        for (id, d) in items {
            match d {
                Some(d) => {
                    let d = stored(d);
                    match serde_json::to_string(&d) {
                        Ok(json) => {
                            log_err("save download", tx.execute(UPSERT_DOWNLOAD, params![d.id, d.added_at, json]));
                        }
                        Err(e) => tracing::error!("database: encode download {id}: {e}"),
                    }
                }
                None => {
                    log_err("delete download", tx.execute("DELETE FROM downloads WHERE id = ?1", params![id]));
                }
            }
        }
        log_err("commit", tx.commit());
    }

    pub fn delete_download(&self, id: &str) {
        self.write_downloads(&[(id.to_string(), None)]);
    }

    pub fn load_queues(&self) -> Vec<Queue> {
        self.load_json("SELECT data FROM queues", "load queues")
    }

    pub fn save_queue(&self, q: &Queue) {
        self.save_queues(std::slice::from_ref(q));
    }

    pub fn save_queues(&self, qs: &[Queue]) {
        let mut c = self.conn.lock();
        let Some(tx) = log_err("begin", c.transaction()) else { return };
        for q in qs {
            match serde_json::to_string(q) {
                Ok(json) => {
                    log_err(
                        "save queue",
                        tx.execute(
                            "INSERT INTO queues (id, data) VALUES (?1, ?2)
                             ON CONFLICT(id) DO UPDATE SET data = excluded.data",
                            params![q.id, json],
                        ),
                    );
                }
                Err(e) => tracing::error!("database: encode queue {}: {e}", q.id),
            }
        }
        log_err("commit", tx.commit());
    }

    pub fn delete_queue(&self, id: &str) {
        log_err("delete queue", self.conn.lock().execute("DELETE FROM queues WHERE id = ?1", params![id]));
    }

    pub fn get_kv(&self, key: &str) -> Option<String> {
        match self.conn.lock().query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| r.get(0)) {
            Ok(v) => Some(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => {
                tracing::error!("database: read {key}: {e}");
                None
            }
        }
    }

    pub fn set_kv(&self, key: &str, value: &str) {
        log_err(
            "save setting",
            self.conn.lock().execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            ),
        );
    }

    pub fn load_settings(&self) -> Settings {
        self.get_kv("settings")
            .and_then(|s| match serde_json::from_str(&s) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::error!("database: settings unreadable, using defaults: {e}");
                    None
                }
            })
            .unwrap_or_default()
    }

    pub fn save_settings(&self, s: &Settings) {
        match serde_json::to_string(s) {
            Ok(j) => self.set_kv("settings", &j),
            Err(e) => tracing::error!("database: encode settings: {e}"),
        }
    }

    /// Fold the write-ahead log back into the main database file.
    pub fn checkpoint(&self) {
        let c = self.conn.lock();
        let r = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0));
        log_err("checkpoint", r);
    }
}
