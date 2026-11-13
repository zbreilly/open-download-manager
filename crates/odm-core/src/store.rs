//! SQLite persistence: downloads, queues and key/value settings, each
//! stored as JSON documents (schema-tolerant across versions).

use crate::model::{Download, Queue, Settings};
use crate::secret;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub struct Store {
    conn: Mutex<Connection>,
    /// a secret was found stored unprotected (see [`Store::protect_existing`])
    plain_seen: AtomicBool,
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

/// Copy of a download with the live-only fields cleared and its secrets
/// (cookies, password, custom header values) protected.
fn stored(d: &Download) -> Download {
    let mut d = d.clone();
    d.speed_bps = 0;
    d.eta_secs = None;
    d.speed_history.clear();
    d.active_connections = 0;
    let seal = |v: &mut Option<String>| *v = v.take().map(|s| secret::seal(&s)).filter(|s| !s.is_empty());
    seal(&mut d.cookies);
    seal(&mut d.password);
    for (_, v) in d.headers.iter_mut() {
        *v = secret::seal(v);
    }
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
        let store = Self::init(conn)?;
        store.protect_existing();
        Ok(store)
    }

    pub fn in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA secure_delete=ON;
             CREATE TABLE IF NOT EXISTS downloads (id TEXT PRIMARY KEY, added TEXT, data TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS queues (id TEXT PRIMARY KEY, data TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        Ok(Self { conn: Mutex::new(conn), plain_seen: AtomicBool::new(false) })
    }

    /// Open a stored secret, remembering if it was stored unprotected.
    fn reveal(&self, stored: &str) -> String {
        let o = secret::open(stored);
        if o.was_plain {
            self.plain_seen.store(true, Ordering::Relaxed);
        }
        o.value
    }

    /// Databases written by versions that stored secrets as plain text:
    /// store everything again (protected) and rebuild the file so the old
    /// text does not linger in freed pages.
    fn protect_existing(&self) {
        let downloads = self.load_downloads();
        let settings = self.get_kv("settings").map(|_| self.load_settings());
        if !self.plain_seen.swap(false, Ordering::Relaxed) {
            return;
        }
        tracing::info!("protecting cookies and passwords stored by an earlier version");
        let items: Vec<(String, Option<Download>)> = downloads.into_iter().map(|d| (d.id.clone(), Some(d))).collect();
        self.write_downloads(&items);
        if let Some(s) = settings {
            self.save_settings(&s);
        }
        let c = self.conn.lock();
        log_err("checkpoint", c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0)));
        log_err("vacuum", c.execute_batch("VACUUM"));
    }

    pub fn load_downloads(&self) -> Vec<Download> {
        let mut list: Vec<Download> = self.load_json("SELECT data FROM downloads ORDER BY added", "load downloads");
        for d in list.iter_mut() {
            let open = |v: &mut Option<String>| *v = v.take().map(|s| self.reveal(&s)).filter(|s| !s.is_empty());
            open(&mut d.cookies);
            open(&mut d.password);
            for (_, v) in d.headers.iter_mut() {
                *v = self.reveal(v);
            }
        }
        list
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

    /// A secret kept in the key/value table (protected at rest).
    pub fn get_secret(&self, key: &str) -> Option<String> {
        self.get_kv(key).map(|v| secret::open(&v).value).filter(|v| !v.is_empty())
    }

    pub fn set_secret(&self, key: &str, value: &str) {
        self.set_kv(key, &secret::seal(value));
    }

    pub fn load_settings(&self) -> Settings {
        let mut s: Settings = self
            .get_kv("settings")
            .and_then(|s| match serde_json::from_str(&s) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::error!("database: settings unreadable, using defaults: {e}");
                    None
                }
            })
            .unwrap_or_default();
        s.proxy_pass = self.reveal(&s.proxy_pass);
        s
    }

    pub fn save_settings(&self, s: &Settings) {
        let mut s = s.clone();
        s.proxy_pass = secret::seal(&s.proxy_pass);
        match serde_json::to_string(&s) {
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
