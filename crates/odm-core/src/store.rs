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

/// On macOS and Linux secrets are stored as they are (see `secret`), so the
/// database must at least be unreadable to other accounts: the files are
/// made private to the user. SQLite gives its `-wal` and `-shm` files the
/// same mode as the database when it creates them later.
#[cfg(unix)]
fn restrict_to_user(db: &Path) {
    use std::os::unix::fs::PermissionsExt;
    for suffix in ["", "-wal", "-shm"] {
        let mut name = db.as_os_str().to_os_string();
        name.push(suffix);
        let file = std::path::PathBuf::from(name);
        if !file.exists() {
            continue;
        }
        if let Err(e) = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!("could not make {} private to the user: {e}", file.display());
        }
    }
}

/// On Windows the profile folder is already private and secrets are
/// encrypted (see `secret`).
#[cfg(not(unix))]
fn restrict_to_user(_db: &Path) {}

/// Create the data folder. A folder made here is private to the user
/// (other accounts cannot even list it); one that already exists is left
/// as it is, since the caller may have pointed at a folder it shares.
#[cfg(unix)]
fn create_private_dir(dir: &Path) {
    use std::os::unix::fs::DirBuilderExt;
    if !dir.exists() {
        let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir);
    }
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
}

/// While the keystore does not answer ([`secret::locked`]), the secrets a
/// download was stored with are sealed and were loaded as empty. Writing
/// the download back must not wipe them: `new` (the document about to be
/// stored) takes them over from `old` (the stored one), as long as it is
/// still the same unfinished download of the same address. A finished
/// download, or one pointed at another address, has dropped its secrets on
/// purpose.
fn keep_unread_secrets(old: &serde_json::Value, new: &mut serde_json::Value) {
    use serde_json::Value;
    if new.get("status").and_then(Value::as_str) == Some("complete") || old.get("url") != new.get("url") {
        return;
    }
    let sealed = |v: Option<&Value>| v.and_then(Value::as_str).is_some_and(secret::is_sealed);
    let empty = |v: Option<&Value>| v.is_none_or(|v| v.is_null() || v.as_str() == Some(""));
    for field in ["cookies", "password"] {
        if empty(new.get(field)) && sealed(old.get(field)) {
            new[field] = old[field].clone();
        }
    }
    // custom headers: [[name, value], …]; the names are stored as they are
    let pairs = |v: &Value| -> Vec<(String, String)> { serde_json::from_value(v.get("headers").cloned().unwrap_or_default()).unwrap_or_default() };
    let (was, mut now) = (pairs(old), pairs(new));
    let same_names = was.len() == now.len() && was.iter().zip(&now).all(|(a, b)| a.0 == b.0);
    if same_names && was.iter().any(|(_, v)| secret::is_sealed(v)) {
        for (kept, cur) in was.iter().zip(now.iter_mut()) {
            if cur.1.is_empty() && secret::is_sealed(&kept.1) {
                cur.1 = kept.1.clone();
            }
        }
        new["headers"] = serde_json::json!(now);
    }
}

const UPSERT_DOWNLOAD: &str = "INSERT INTO downloads (id, added, data) VALUES (?1, ?2, ?3)
     ON CONFLICT(id) DO UPDATE SET data = excluded.data";

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(d) = path.parent() {
            create_private_dir(d);
        }
        let conn = Connection::open(path)?;
        let store = Self::init(conn)?;
        restrict_to_user(path);
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
                    let doc = serde_json::to_value(&d).map(|mut doc| {
                        if secret::locked() {
                            let old: Option<String> =
                                tx.query_row("SELECT data FROM downloads WHERE id = ?1", params![d.id], |r| r.get(0)).ok();
                            if let Some(old) = old.and_then(|o| serde_json::from_str(&o).ok()) {
                                keep_unread_secrets(&old, &mut doc);
                            }
                        }
                        doc.to_string()
                    });
                    match doc {
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

    /// The value stored under `key`; `Ok(None)` when there is none. An
    /// error means the database could not be read, which callers that would
    /// act on "nothing stored" (deleting, overwriting) must not mistake for it.
    pub fn try_get_kv(&self, key: &str) -> rusqlite::Result<Option<String>> {
        match self.conn.lock().query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| r.get(0)) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Like [`try_get_kv`](Self::try_get_kv), with a read error logged and
    /// treated as "nothing stored".
    pub fn get_kv(&self, key: &str) -> Option<String> {
        self.try_get_kv(key).unwrap_or_else(|e| {
            tracing::error!("database: read {key}: {e}");
            None
        })
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

    /// The secret under `key` is stored unprotected although secrets are
    /// protected now (it was written before they were): store it again.
    pub fn secret_was_plain(&self, key: &str) -> bool {
        self.get_kv(key).is_some_and(|v| secret::open(&v).was_plain)
    }

    /// The secret under `key` is sealed and the keystore did not answer:
    /// it is there and cannot be read in this run. Do not replace it.
    pub fn secret_unreadable(&self, key: &str) -> bool {
        self.get_kv(key).is_some_and(|v| secret::open(&v).unreadable)
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
        if s.proxy_pass.is_empty() && secret::locked() {
            // sealed, and loaded as empty because the keystore did not
            // answer: keep what is stored
            let kept = self
                .get_kv("settings")
                .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
                .and_then(|v| v.get("proxyPass").and_then(|p| p.as_str().map(String::from)))
                .filter(|p| secret::is_sealed(p));
            if let Some(kept) = kept {
                s.proxy_pass = kept;
            }
        }
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

#[cfg(test)]
mod tests {
    use super::keep_unread_secrets;
    use serde_json::json;

    fn stored() -> serde_json::Value {
        json!({ "url": "https://a.example/f", "status": "paused", "cookies": "odm-key1:AAAA", "password": "odm-key1:BBBB",
            "headers": [["X-Token", "odm-key1:CCCC"], ["Accept", "odm-key1:DDDD"]] })
    }

    /// The same download as it is in memory while the keystore is silent.
    fn unread() -> serde_json::Value {
        json!({ "url": "https://a.example/f", "status": "paused", "cookies": null, "headers": [["X-Token", ""], ["Accept", ""]] })
    }

    #[test]
    fn unread_secrets_are_written_back_as_they_were() {
        let mut new = unread();
        keep_unread_secrets(&stored(), &mut new);
        assert_eq!(new["cookies"], "odm-key1:AAAA");
        assert_eq!(new["password"], "odm-key1:BBBB");
        assert_eq!(new["headers"], json!([["X-Token", "odm-key1:CCCC"], ["Accept", "odm-key1:DDDD"]]));
    }

    #[test]
    fn what_was_set_in_this_run_wins() {
        let mut new = unread();
        new["cookies"] = json!("fresh=1");
        new["headers"] = json!([["X-Token", "typed now"], ["Accept", ""]]);
        keep_unread_secrets(&stored(), &mut new);
        assert_eq!(new["cookies"], "fresh=1");
        assert_eq!(new["headers"], json!([["X-Token", "typed now"], ["Accept", "odm-key1:DDDD"]]));
    }

    #[test]
    fn secrets_dropped_on_purpose_stay_dropped() {
        // finished: the credentials that fetched the file are not kept
        let mut done = unread();
        done["status"] = json!("complete");
        done["headers"] = json!([]);
        keep_unread_secrets(&stored(), &mut done);
        assert!(done["cookies"].is_null() && done.get("password").is_none() && done["headers"] == json!([]));
        // pointed at another address: the old site's credentials must not follow
        let mut moved = unread();
        moved["url"] = json!("https://other.example/f");
        keep_unread_secrets(&stored(), &mut moved);
        assert!(moved["cookies"].is_null() && moved.get("password").is_none());
        assert_eq!(moved["headers"], json!([["X-Token", ""], ["Accept", ""]]));
        // other headers than were stored: nothing to match them to
        let mut edited = unread();
        edited["headers"] = json!([["X-Other", ""]]);
        keep_unread_secrets(&stored(), &mut edited);
        assert_eq!(edited["headers"], json!([["X-Other", ""]]));
    }

    #[test]
    fn plain_values_are_not_taken_over() {
        let old = json!({ "url": "https://a.example/f", "status": "paused", "cookies": "plain=1", "headers": [["X-Token", "plain"]] });
        let mut new = unread();
        new["headers"] = json!([["X-Token", ""]]);
        keep_unread_secrets(&old, &mut new);
        assert!(new["cookies"].is_null());
        assert_eq!(new["headers"], json!([["X-Token", ""]]));
    }
}
