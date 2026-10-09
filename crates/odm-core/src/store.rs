//! SQLite persistence: downloads, queues and key/value settings, each
//! stored as JSON documents (schema-tolerant across versions).

use crate::model::{Download, Queue, Settings};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use std::path::Path;

pub struct Store {
    conn: Mutex<Connection>,
}

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
        let c = self.conn.lock();
        let mut st = match c.prepare("SELECT data FROM downloads ORDER BY added") {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let rows = st.query_map([], |r| r.get::<_, String>(0));
        match rows {
            Ok(rows) => rows
                .filter_map(|r| r.ok())
                .filter_map(|s| serde_json::from_str(&s).ok())
                .collect(),
            Err(_) => vec![],
        }
    }

    pub fn save_download(&self, d: &Download) {
        let mut d = d.clone();
        // live-only fields
        d.speed_bps = 0;
        d.eta_secs = None;
        d.speed_history.clear();
        d.active_connections = 0;
        if let Ok(json) = serde_json::to_string(&d) {
            let _ = self.conn.lock().execute(
                "INSERT INTO downloads (id, added, data) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET data = excluded.data",
                params![d.id, d.added_at, json],
            );
        }
    }

    pub fn delete_download(&self, id: &str) {
        let _ = self.conn.lock().execute("DELETE FROM downloads WHERE id = ?1", params![id]);
    }

    pub fn load_queues(&self) -> Vec<Queue> {
        let c = self.conn.lock();
        let mut st = match c.prepare("SELECT data FROM queues") {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let rows = st.query_map([], |r| r.get::<_, String>(0));
        match rows {
            Ok(rows) => rows
                .filter_map(|r| r.ok())
                .filter_map(|s| serde_json::from_str(&s).ok())
                .collect(),
            Err(_) => vec![],
        }
    }

    pub fn save_queue(&self, q: &Queue) {
        if let Ok(json) = serde_json::to_string(q) {
            let _ = self.conn.lock().execute(
                "INSERT INTO queues (id, data) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET data = excluded.data",
                params![q.id, json],
            );
        }
    }

    pub fn delete_queue(&self, id: &str) {
        let _ = self.conn.lock().execute("DELETE FROM queues WHERE id = ?1", params![id]);
    }

    pub fn get_kv(&self, key: &str) -> Option<String> {
        self.conn
            .lock()
            .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| r.get(0))
            .ok()
    }

    pub fn set_kv(&self, key: &str, value: &str) {
        let _ = self.conn.lock().execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        );
    }

    pub fn load_settings(&self) -> Settings {
        self.get_kv("settings")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_settings(&self, s: &Settings) {
        if let Ok(j) = serde_json::to_string(s) {
            self.set_kv("settings", &j);
        }
    }
}
