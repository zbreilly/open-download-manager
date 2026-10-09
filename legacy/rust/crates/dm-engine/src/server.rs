//! Open Download Manager — local control server + embedded web UI.
//!
//! Serves a small management page and a JSON API on 127.0.0.1:
//!   GET  /                     web UI
//!   GET  /api/jobs             job list (JSON)
//!   POST /api/add?url=&out=    enqueue a job
//!   POST /api/requeue          re-queue failed jobs
//!   POST /api/run?max=N        run pending jobs (bounded, non-blocking)

use crate::jobs::{JobRecord, JobState, JobStore};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};



const UI_INDEX: &str = include_str!("ui/index.html");
const UI_TOKENS: &str = include_str!("ui/tokens.css");
const UI_APPCSS: &str = include_str!("ui/app.css");
const UI_APPJS: &str = include_str!("ui/app.js");
const UI_PROGRESS: &str = include_str!("ui/progress.html");
const UI_SCHEDULER: &str = include_str!("ui/scheduler.html");
const UI_SETTINGS: &str = include_str!("ui/settings.html");
const UI_SPRITE: &str = include_str!("ui/sprite.svg");
const FONT_PLEX: &[u8] = include_bytes!("ui/fonts/ibmplexsans-var.woff2");
const FONT_JBMONO: &[u8] = include_bytes!("ui/fonts/jetbrainsmono-var.woff2");
const UI_APPICON: &[u8] = include_bytes!("ui/app-icon.png");

/// per-job speed EMA + added-at (server-side, feeds the UI contract)
static SPEED_EMA: std::sync::LazyLock<Mutex<std::collections::HashMap<u64, (f64, std::time::Instant, u64)>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
/// per-job speed history (last 60 samples, 1 Hz) for the sparkline
static SPEED_HISTORY: std::sync::LazyLock<Mutex<std::collections::HashMap<u64, Vec<f64>>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
/// live per-job limiters so /api/limit can re-throttle running downloads
static ACTIVE_LIMITS: std::sync::LazyLock<Mutex<std::collections::HashMap<u64, crate::SpeedLimiter>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
/// Download categories (analyzer evidence: ref_chunk_02 "usesubf" +
/// "category folder" fields; broker GetFileTypes -> Extensions list).
/// General is the catch-all with NO folder (straight to the download dir).
#[derive(Clone, Debug, PartialEq)]
pub struct Category {
    pub name: String,
    pub folder: Option<String>, // None = save in the root download dir
    pub extensions: Vec<String>,
}

fn default_categories() -> Vec<Category> {
    let cats: &[(&str, Option<&str>, &[&str])] = &[
        ("Compressed", Some("Compressed"),
         &["zip","rar","7z","tar","gz","bz2","xz","zst","iso","cab","arj","lzh"]),
        ("Documents", Some("Documents"),
         &["pdf","doc","docx","xls","xlsx","ppt","pptx","txt","rtf","odt","ods","epub","md"]),
        ("Music", Some("Music"),
         &["mp3","wav","wma","ogg","flac","aac","m4a","mid","midi","opus","wv"]),
        ("Programs", Some("Programs"),
         &["exe","msi","apk","dmg","deb","rpm","appimage","pkg","msix","bat","sh"]),
        ("Video", Some("Video"),
         &["mp4","avi","mkv","mov","wmv","flv","webm","m4v","mpg","mpeg","ts","3gp","vob"]),
        ("General", None, &[]),
    ];
    cats.iter()
        .map(|(n, f, e)| Category {
            name: n.to_string(),
            folder: f.map(|s| s.to_string()),
            extensions: e.iter().map(|s| s.to_string()).collect(),
        })
        .collect()
}

static CATEGORIES: std::sync::LazyLock<Mutex<Vec<Category>>> =
    std::sync::LazyLock::new(|| {
        let mut m = Mutex::new(default_categories());
        // load persisted table from settings ("name|folder|e1,e2;...")
        if let Some(enc) = SETTINGS.lock().unwrap().get("categories") {
            let parsed: Vec<Category> = enc
                .split(';')
                .filter_map(|row| {
                    let f: Vec<&str> = row.split('|').collect();
                    if f.len() < 3 { return None; }
                    Some(Category {
                        name: f[0].to_string(),
                        folder: if f[1].is_empty() { None } else { Some(f[1].to_string()) },
                        extensions: f[2].split(',').filter(|x| !x.is_empty())
                            .map(|x| x.to_string()).collect(),
                    })
                })
                .collect();
            if !parsed.is_empty() {
                *m.lock().unwrap() = parsed;
            }
        }
        m
    });

fn categories_to_setting(cats: &[Category]) -> String {
    cats.iter()
        .map(|c| format!("{}|{}|{}", c.name, c.folder.clone().unwrap_or_default(),
            c.extensions.join(",")))
        .collect::<Vec<_>>()
        .join(";")
}

fn save_categories(cats: Vec<Category>) {
    let enc = categories_to_setting(&cats);
    SETTINGS.lock().unwrap().insert("categories".into(), enc);
    *CATEGORIES.lock().unwrap() = cats;
}

fn categories_json(cats: &[Category]) -> String {
    let items: Vec<String> = cats
        .iter()
        .map(|c| {
            let folder = c.folder
                .clone()
                .map(|f| format!(r#""{}""#, json_escape(&f)))
                .unwrap_or_else(|| "null".into());
            let exts: Vec<String> = c.extensions
                .iter()
                .map(|e| format!(r#""{}""#, json_escape(e)))
                .collect();
            format!(
                r#"{{"name":"{}","folder":{folder},"extensions":[{}]}}"#,
                json_escape(&c.name),
                exts.join(",")
            )
        })
        .collect();
    format!(r#"{{"categories":[{}]}}"#, items.join(","))
}

fn categories_from_json(body: &str) -> Option<Vec<Category>> {
    // minimal parse of [{"name":..,"folder":..|null,"extensions":[..]}]
    let mut out = Vec::new();
    for obj in body.split("}").filter(|p| p.contains("\"name\"")) {
        let name = json_field(obj, "name")?;
        let folder = json_field(obj, "folder").filter(|f| f != "null" && !f.is_empty());
        let mut exts = Vec::new();
        if let Some(arr_start) = obj.find("\"extensions\"") {
            if let Some(open) = obj[arr_start..].find('[') {
                if let Some(close) = obj[arr_start + open..].find(']') {
                    let arr = &obj[arr_start + open + 1..arr_start + open + close];
                    exts = arr
                        .split(',')
                        .map(|x| x.trim().trim_matches('"').to_string())
                        .filter(|x| !x.is_empty())
                        .collect();
                }
            }
        }
        out.push(Category { name, folder, extensions: exts });
    }
    (!out.is_empty()).then_some(out)
}

/// Route a filename into its category subfolder under `base` (analyzer
/// evidence: the original's "usesubf" + per-category folder fields).
fn route_into_category(file_name: &str, base: &Path) -> PathBuf {
    let ext = file_name.rsplit('.').next().unwrap_or("").to_lowercase();
    let cats = CATEGORIES.lock().unwrap();
    for c in cats.iter() {
        if c.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext)) {
            if let Some(folder) = &c.folder {
                let dir = base.join(folder);
                let _ = std::fs::create_dir_all(&dir);
                return dir.join(file_name);
            }
        }
    }
    base.join(file_name)
}

/// launch-agent path (overridable for tests)
fn launch_agent_path() -> PathBuf {
    if let Ok(p) = std::env::var("ODM_LAUNCH_AGENT_PATH") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/LaunchAgents/app.opendm.odm.plist")
}

/// Register/unregister the app as a login item (macOS LaunchAgent).
fn apply_launch_at_login(enabled: bool) {
    let path = launch_agent_path();
    if enabled {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let app = "/Applications/OpenDM.app";
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n  <key>Label</key><string>app.opendm.odm</string>\n  <key>ProgramArguments</key>\n  <array><string>/usr/bin/open</string><string>-a</string><string>{app}</string></array>\n  <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><false/>\n</dict></plist>\n"
        , app = app);
        let _ = std::fs::write(&path, plist);
        let _ = std::process::Command::new("launchctl")
            .args(["unload", &path.to_string_lossy()]).output();
        let _ = std::process::Command::new("launchctl")
            .args(["load", &path.to_string_lossy()]).output();
    } else if path.exists() {
        let _ = std::process::Command::new("launchctl")
            .args(["unload", &path.to_string_lossy()]).output();
        let _ = std::fs::remove_file(&path);
    }
}

/// Clipboard monitor: poll pbpaste; auto-add direct-download URLs that match
/// a monitored category extension. Deduped; gated on the setting.
static CLIPBOARD_SEEN: std::sync::LazyLock<Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));
static CLIPBOARD_RUN: std::sync::LazyLock<AtomicBool> =
    std::sync::LazyLock::new(|| AtomicBool::new(false));

pub fn clipboard_download_candidate(text: &str) -> Option<String> {
    let t = text.trim();
    if t.contains('\n') || t.contains(' ') || t.lines().count() != 1 {
        return None; // single URL only, no surrounding text
    }
    if !t.starts_with("http://") && !t.starts_with("https://") {
        return None;
    }
    let ext = t.rsplit('.').next().unwrap_or("")
        .split(['?', '/']).next().unwrap_or("").to_lowercase();
    if ext.is_empty() || ext == t.to_lowercase() {
        return None;
    }
    let cats = CATEGORIES.lock().unwrap();
    if cats.iter().any(|c| c.extensions.iter().any(|e| *e == ext)) {
        Some(t.to_string())
    } else {
        None
    }
}

fn start_clipboard_monitor(store: &Arc<Mutex<JobStore>>) {
    if CLIPBOARD_RUN.swap(true, Ordering::SeqCst) {
        return; // already running
    }
    let store = Arc::clone(store);
    std::thread::spawn(move || {
        let mut last_raw = String::new();
        while CLIPBOARD_RUN.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_secs(2));
            #[cfg(target_os = "macos")]
            let clip = std::process::Command::new("pbpaste")
                .output().ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            #[cfg(not(target_os = "macos"))]
            let clip = String::new();
            if clip == last_raw {
                continue;
            }
            last_raw = clip.clone();
            if let Some(url) = clipboard_download_candidate(&clip) {
                let mut seen = CLIPBOARD_SEEN.lock().unwrap();
                if seen.contains(&url) {
                    continue;
                }
                seen.insert(url.clone());
                drop(seen);
                let dir = default_download_dir();
                let _ = std::fs::create_dir_all(&dir);
                let name = url.split('/').next_back().unwrap_or("clipboard.bin");
                let name = name.split('?').next().unwrap_or("clipboard.bin");
                let out = route_into_category(name, &dir);
                if path_allowed(&out) {
                    if let Ok(id) = store.lock().unwrap().add(&url, &out) {
                        ADDED_AT.lock().unwrap().insert(id, chrono_ish_now());
                        eprintln!("[clipboard] auto-added: {url}");
                    }
                }
            }
        }
    });
}

fn stop_clipboard_monitor() {
    CLIPBOARD_RUN.store(false, Ordering::SeqCst);
}

/// extension presence: last ping timestamp
static EXT_LAST_SEEN: std::sync::LazyLock<Mutex<Option<std::time::Instant>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));
static ADDED_AT: std::sync::LazyLock<Mutex<std::collections::HashMap<u64, String>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
static ACTIVE: std::sync::LazyLock<queue::JobControls> =
    std::sync::LazyLock::new(queue::JobControls::new);
/// armed power action: (action, deadline) — gated by allowPowerActions
static SCHEDULED_ACTION: std::sync::LazyLock<Mutex<Option<(String, Instant)>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));
/// live per-job progress: store id -> (done, total)
static PROGRESS: std::sync::LazyLock<Mutex<std::collections::HashMap<u64, (u64, Option<u64>)>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
/// (speedBps, active, paused, queued, failed)
static TOTALS: std::sync::LazyLock<Mutex<(f64, u64, u64, u64, u64)>> =
    std::sync::LazyLock::new(|| Mutex::new((0.0, 0, 0, 0, 0)));
/// app settings (key=value), defaults per spec section 7
fn settings_path(cfg: &ServerConfig) -> PathBuf {
    PathBuf::from(format!("{}.settings", cfg.jobs_path.display()))
}

fn persist_settings(cfg: &ServerConfig) {
    let st = SETTINGS.lock().unwrap();
    let text: String = st.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");
    let _ = std::fs::write(settings_path(cfg), text);
}

fn load_persisted_settings(cfg: &ServerConfig) {
    if let Ok(text) = std::fs::read_to_string(settings_path(cfg)) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                SETTINGS.lock().unwrap().insert(k.to_string(), v.to_string());
            }
        }
    }
}

static SETTINGS: std::sync::LazyLock<Mutex<std::collections::BTreeMap<String, String>>> =
    std::sync::LazyLock::new(|| {
        let mut m = std::collections::BTreeMap::new();
        m.insert("theme".into(), "dark".into());
        m.insert("density".into(), "comfortable".into());
        m.insert("simultaneous".into(), "4".into());
        m.insert("globalLimit".into(), "0".into());
        m.insert("perDownloadLimit".into(), "0".into());
        m.insert("confirmDelete".into(), "1".into());
        Mutex::new(m)
    });
/// queues (JSON lines persisted to <jobs>.queues)
static QUEUES: std::sync::LazyLock<Mutex<Vec<std::collections::BTreeMap<String, String>>>> =
    std::sync::LazyLock::new(|| {
        let mut v = Vec::new();
        let mut q = std::collections::BTreeMap::new();
        q.insert("id".to_string(), "main".to_string());
        q.insert("name".to_string(), "Main queue".to_string());
        q.insert("maxParallel".to_string(), "2".to_string());
        v.push(q);
        Mutex::new(v)
    });
static RECENT_FOLDERS: std::sync::LazyLock<Mutex<Vec<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(Vec::new()));
/// advanced per-job request headers (auth/referrer/cookies), in-memory
static JOB_HEADERS: std::sync::LazyLock<Mutex<std::collections::HashMap<u64, Vec<(String, String)>>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

pub struct ServerConfig {
    pub port: u16,
    pub jobs_path: PathBuf,
    pub parts: usize,
    pub max_parallel: usize,
}

/// Bind the UI port (useful for picking an ephemeral port in tests).
pub fn bind_listener(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", port))
}

/// Serve on an already-bound listener (see [`bind_listener`]).
pub fn serve_on(
    cfg: ServerConfig,
    listener: TcpListener,
) -> Result<(), Box<dyn std::error::Error>> {
    load_persisted_settings(&cfg);
    let store = Arc::new(Mutex::new(JobStore::open(&cfg.jobs_path)?));
    let running = Arc::new(AtomicBool::new(false));
    let token = Arc::new(load_or_create_token(&cfg.jobs_path));
    eprintln!(
        "API token (also at {}.token): {}",
        cfg.jobs_path.display(),
        token
    );
    let cfg = Arc::new(cfg);
    eprintln!(
        "Open Download Manager UI: http://127.0.0.1:{}  (jobs: {})",
        cfg.port,
        cfg.jobs_path.display()
    );
    // scheduler: evaluate queue time windows every 15 s
    {
        let store = Arc::clone(&store);
        let running = Arc::clone(&running);
        let cfg_sched = Arc::clone(&cfg);
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(15));
            let queues = QUEUES.lock().unwrap().clone();
            for q in queues {
                let start_on = q.get("startEnabled").map(|v| v == "1").unwrap_or(false);
                let stop_on = q.get("stopEnabled").map(|v| v == "1").unwrap_or(false);
                if !start_on && !stop_on {
                    continue;
                }
                let in_window = |at: &str| -> bool {
                    let Some((h, m)) = at.split_once(':') else { return false };
                    let (wh, wm) = (
                        h.parse::<u32>().unwrap_or(0),
                        m.parse::<u32>().unwrap_or(0),
                    );
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let mins = ((now / 60) % 1440) as u32;
                    let (a, b) = (wh * 60 + wm, mins);
                    a <= b
                };
                // start: any pending jobs + window open (or already past start)
                if start_on && in_window(q.get("startAt").map(|s| s.as_str()).unwrap_or("00:00")) {
                    let max = SETTINGS
                        .lock()
                        .unwrap()
                        .get("simultaneous")
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(2);
                    start_pending_downloads(&store, max, &running, cfg_sched.parts);
                }
                if stop_on
                    && in_window(q.get("stopAt").map(|s| s.as_str()).unwrap_or("00:00"))
                {
                    for j in store.lock().unwrap().list() {
                        if j.state == JobState::Running {
                            if let Some(f) = ACTIVE.flag(j.id) {
                                f.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                }
            }
        });
    }

    if SETTINGS.lock().unwrap().get("clipboardMonitor").map(|v| v == "1").unwrap_or(false) {
        start_clipboard_monitor(&store);
    }

    // pre-create all category folders in the download dir (analyzer
    // evidence: the original creates these on first run)
    {
        let dir = default_download_dir();
        let _ = std::fs::create_dir_all(&dir);
        for c in CATEGORIES.lock().unwrap().iter() {
            if let Some(folder) = &c.folder {
                let _ = std::fs::create_dir_all(dir.join(folder));
            }
        }
    }

    for stream in listener.incoming() {
        let stream = stream?;
        let store = Arc::clone(&store);
        let cfg = Arc::clone(&cfg);
        let running = Arc::clone(&running);
        let token = Arc::clone(&token);
        std::thread::spawn(move || {
            let _ = s_handle(store, cfg, running, token, stream);
        });
    }
    Ok(())
}

pub fn serve(cfg: ServerConfig) -> Result<(), Box<dyn std::error::Error>> {
    let listener = bind_listener(cfg.port)?;
    serve_on(cfg, listener)
}

fn s_handle(
    store: Arc<Mutex<JobStore>>,
    cfg: Arc<ServerConfig>,
    running: Arc<AtomicBool>,
    token: Arc<String>,
    mut stream: TcpStream,
) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut b = [0u8; 1024];
    loop {
        let n = stream.read(&mut b)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&b[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 32 * 1024 {
            break;
        }
    }
    // split head from any body bytes already received
    let (head_end, text) = match buf.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(p) => (p + 4, String::from_utf8_lossy(&buf[..p]).into_owned()),
        None => (buf.len(), String::from_utf8_lossy(&buf).into_owned()),
    };
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    if std::env::var("ODM_DEBUG").is_ok() {
        eprintln!("[req] {method} {path}");
    }

    // tiny JSON-body capture for extension POSTs: Content-Length bytes may
    // have arrived with the head — only read the remainder from the socket
    let json_body = if method == "POST"
        && (path.starts_with("/api/extension") || path.starts_with("/api/categories"))
    {
        let cl: usize = text
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
            .and_then(|l| l.split(':').nth(1)?.trim().parse().ok())
            .unwrap_or(0);
        let mut have = buf[head_end..].to_vec();
        while have.len() < cl {
            let mut b = [0u8; 1024];
            let n = stream.read(&mut b)?;
            if n == 0 {
                break;
            }
            have.extend_from_slice(&b[..n]);
        }
        have.truncate(cl);
        String::from_utf8_lossy(&have).into_owned()
    } else {
        String::new()
    };

    // DNS-rebinding defense: only loopback Host headers are honored
    let host = text
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("host:"))
        .and_then(|l| l.splitn(2, ':').nth(1))
        .map(|h| h.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let host_ok = ["127.0.0.1", "localhost", "[::1]"]
        .iter()
        .any(|h| host == *h || host.starts_with(&format!("{h}:")));
    if !host_ok {
        let resp = "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        stream.write_all(resp.as_bytes())?;
        return Ok(());
    }

    // token auth for the API surface (header or ?token=)
    if path.starts_with("/api/") {
        let hdr_tok = text
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("x-odm-token:"))
            .and_then(|l| l.splitn(2, ':').nth(1))
            .map(|v| v.trim().to_string());
        let q_tok = parse_query(query).get("token").cloned();
        let authed = hdr_tok.as_deref() == Some(token.as_str())
            || q_tok.as_deref() == Some(token.as_str());
        if !authed {
            let resp = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            stream.write_all(resp.as_bytes())?;
            return Ok(());
        }
    }

    let (code, ctype, mut body): (u16, &str, Vec<u8>) =
        route(&store, &cfg, &running, method, path, query, &json_body);
    // same-origin token injection for the UI pages
    if ctype.starts_with("text/html") {
        if let Some(pos) = body.windows(7).position(|w| w == b"</head>") {
            let inject = format!(
                "<script>window.ODM_TOKEN={:?};</script>",
                token.as_str()
            );
            let mut patched = Vec::with_capacity(body.len() + inject.len());
            patched.extend_from_slice(&body[..pos + 7]);
            patched.extend_from_slice(inject.as_bytes());
            patched.extend_from_slice(&body[pos + 7..]);
            let build_stamp = format!(
                "<div style=\"position:fixed;bottom:2px;right:6px;font:9px monospace;color:var(--text-muted,#999);opacity:.4;z-index:9999;pointer-events:none\">build {}</div>",
                env!("CARGO_PKG_VERSION"));
            // inject build stamp before </body>
            if let Some(bpos) = patched.windows(7).rposition(|w| w == b"</body>") {
                let mut stamped = Vec::with_capacity(patched.len() + build_stamp.len());
                stamped.extend_from_slice(&patched[..bpos]);
                stamped.extend_from_slice(build_stamp.as_bytes());
                stamped.extend_from_slice(&patched[bpos..]);
                body = stamped;
            } else {
                body = patched;
            }
        }
    }
    let cache = if ctype.starts_with("text/html") || ctype.starts_with("text/css")
        || ctype.starts_with("text/javascript") || ctype.starts_with("image/svg")
    {
        "Cache-Control: no-cache, no-store, must-revalidate\r\n"
    } else {
        ""
    };
    let resp = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{cache}Connection: close\r\n\r\n",
        code,
        if code == 200 { "OK" } else if code == 409 { "Conflict" } else { "Error" },
        ctype,
        body.len()
    );
    stream.write_all(resp.as_bytes())?;
    stream.write_all(&body)
}

/// Mark Pending jobs Running and execute them; runs on its own thread.
fn start_pending_downloads(
    store: &Arc<Mutex<JobStore>>,
    max: usize,
    running: &Arc<AtomicBool>,
    fallback_parts: usize,
) {
    if running.swap(true, Ordering::SeqCst) {
        return; // a run is already in progress on this server
    }
    let running = Arc::clone(running);
    let store = Arc::clone(store);
    std::thread::spawn(move || {
        let (count, jobs, id_map, controls, run_opts) = {
            let settings = SETTINGS.lock().unwrap().clone();
            let global_limit_mbps: u64 = settings
                .get("globalLimit")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let per_dl_mbps: u64 = settings
                .get("perDownloadLimit")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let mut st = store.lock().unwrap();
            let pending = st.pending();
            for j in &pending {
                let _ = st.set_state(j.id, JobState::Running);
            }
            let jobs = pending
                .iter()
                .map(|j| (j.url.clone(), j.out.clone()))
                .collect();
            let id_map: Vec<u64> = pending.iter().map(|j| j.id).collect();
            let mut per_job = std::collections::HashMap::new();
            let job_headers = JOB_HEADERS.lock().unwrap();
            for j in &pending {
                if per_dl_mbps > 0 {
                    per_job.insert(
                        j.id,
                        crate::SpeedLimiter::new(per_dl_mbps * 1024 * 1024),
                    );
                }
                if let Some(_hs) = job_headers.get(&j.id) {
                    // (headers are consumed through JOB_HEADERS at start)
                }
            }
            for j in &pending {
                // every running job gets a limiter handle so /api/limit can
                // re-throttle it live (default unlimited)
                let l = per_job
                    .get(&j.id)
                    .cloned()
                    .unwrap_or_else(|| crate::SpeedLimiter::new(0));
                ACTIVE_LIMITS.lock().unwrap().insert(j.id, l);
            }
            let default_parts: usize = settings
                .get("defaultParts")
                .and_then(|v| v.parse().ok())
                .unwrap_or(fallback_parts)
                .clamp(1, 32);
            let run_opts = queue::JobRunOptions {
                limiter: if global_limit_mbps > 0 {
                    Some(crate::SpeedLimiter::new(global_limit_mbps * 1024 * 1024))
                } else {
                    None
                },
                per_job_limiter: per_job,
                headers: job_headers.clone(),
                parts: default_parts,
            };
            // pause flags MUST live in the static ACTIVE map — /api/pause
            // consults it. JobControls is Arc-backed, so ACTIVE.clone()
            // shares the same underlying registry as the run.
            for j in &pending {
                ACTIVE.register(j.id);
            }
            let controls = ACTIVE.clone();
            (pending.len(), jobs, id_map, controls, run_opts)
        };
        if count > 0 {
            if std::env::var("ODM_DEBUG").is_ok() {
                eprintln!("[run] starting {count} jobs, ids={id_map:?}");
            }
            let cb_store = Arc::clone(&store);
            queue::run_jobs_opts(jobs, max, Some(&id_map), Some(&controls), Some(&run_opts), None, &mut move |ev|{
                if let queue::JobEvent::Progress { id, done, total } = &ev {
                    PROGRESS.lock().unwrap().insert(*id, (*done, *total));
                    // 1s-EMA speed per spec section 8. Lock ordering: take
                    // each mutex in its own scope — never nest.
                    let speed_sum = {
                        let mut ema = SPEED_EMA.lock().unwrap();
                        let now = std::time::Instant::now();
                        let entry = ema.entry(*id).or_insert((0.0, now, *done));
                        let dt = now.duration_since(entry.1).as_secs_f64();
                        if dt >= 0.2 {
                            let inst = ((*done).saturating_sub(entry.2)) as f64 / dt;
                            entry.0 = if entry.0 == 0.0 {
                                inst
                            } else {
                                entry.0 * 0.7 + inst * 0.3
                            };
                            entry.1 = now;
                            entry.2 = *done;
                        }
                        ema.values().map(|e| e.0).sum::<f64>()
                    };
                    TOTALS.lock().unwrap().0 = speed_sum;
                    return;
                }
                if let queue::JobEvent::Paused { id } = &ev {
                    if let Ok(mut st) = cb_store.lock() {
                        let _ = st.set_state(*id, JobState::Paused);
                    }
                    PROGRESS.lock().unwrap().remove(id);
                    ACTIVE.release(*id);
                    return;
                }
                match &ev {
                    queue::JobEvent::Done { id, .. } => {
                        let last = PROGRESS.lock().unwrap().get(id).copied();
                        if let Ok(mut st) = cb_store.lock() {
                            let _ = st.set_state(*id, JobState::Done);
                            if let Some((done, total)) = last {
                                let _ = st.set_progress(*id, done, total);
                            }
                        }
                        if let Some(e) = SPEED_EMA.lock().unwrap().get_mut(id) {
                            e.0 = 0.0;
                        }
                        ACTIVE.release(*id);
                    }
                    queue::JobEvent::Failed { id, error } => {
                        if std::env::var("ODM_DEBUG").is_ok() {
                            eprintln!("[failed] id={id}: {error}");
                        }
                        if let Ok(mut st) = cb_store.lock() {
                            let _ = st.set_state(*id, JobState::Failed);
                        }
                        PROGRESS.lock().unwrap().remove(id);
                        ACTIVE.release(*id);
                    }
                    _ => {}
                }
            });
        }
        // queue-finished hook (spec section 6): arm a gated power action
        let st = store.lock().unwrap();
        let list = st.list();
        let any_done = list.iter().any(|j| j.state == JobState::Done);
        let all_settled = !list.is_empty()
            && list
                .iter()
                .all(|j| matches!(j.state, JobState::Done | JobState::Failed | JobState::Paused));
        let when_done = SETTINGS
            .lock()
            .unwrap()
            .get("whenDone")
            .cloned()
            .unwrap_or_default();
        let power_allowed = SETTINGS
            .lock()
            .unwrap()
            .get("allowPowerActions")
            .map(|v| v == "1")
            .unwrap_or(false);
        drop(st);
        if any_done
            && all_settled
            && power_allowed
            && (when_done == "sleep" || when_done == "shutdown")
        {
            *SCHEDULED_ACTION.lock().unwrap() =
                Some((when_done.clone(), Instant::now() + Duration::from_secs(60)));
            eprintln!(
                "queue finished: {} in 60s (cancel via /api/cancel-action)",
                when_done
            );
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(1));
                let armed = SCHEDULED_ACTION.lock().unwrap().clone();
                match armed {
                    Some((action, deadline)) => {
                        if Instant::now() >= deadline {
                            // gated by allowPowerActions; default build no-ops
                            #[cfg(target_os = "macos")]
                            {
                                let _ = std::process::Command::new("pmset")
                                    .arg(if action == "shutdown" { "shutdownnow" } else { "sleepnow" })
                                    .status();
                            }
                            *SCHEDULED_ACTION.lock().unwrap() = None;
                            break;
                        }
                    }
                    None => break, // cancelled
                }
            });
        }
        running.store(false, Ordering::SeqCst);
    });
}

fn route(
    store: &Arc<Mutex<JobStore>>,
    cfg: &Arc<ServerConfig>,
    running: &Arc<AtomicBool>,
    method: &str,
    path: &str,
    query: &str,
    json_body: &str,
) -> (u16, &'static str, Vec<u8>) {
    match (method, path) {
        ("GET", "/") => (200, "text/html; charset=utf-8", UI_INDEX.to_string().into_bytes()),
        ("GET", "/tokens.css") => (200, "text/css; charset=utf-8", UI_TOKENS.to_string().into_bytes()),
        ("GET", "/app.css") => (200, "text/css; charset=utf-8", UI_APPCSS.to_string().into_bytes()),
        ("GET", "/app.js") => (200, "text/javascript; charset=utf-8", UI_APPJS.to_string().into_bytes()),
        ("GET", "/progress") => (200, "text/html; charset=utf-8", UI_PROGRESS.to_string().into_bytes()),
        ("GET", "/scheduler") => (200, "text/html; charset=utf-8", UI_SCHEDULER.to_string().into_bytes()),
        ("GET", "/settings") => (200, "text/html; charset=utf-8", UI_SETTINGS.to_string().into_bytes()),
        ("GET", "/fonts/ibmplexsans-var.woff2") =>
            (200, "font/woff2", FONT_PLEX.to_vec()),
        ("GET", "/fonts/jetbrainsmono-var.woff2") =>
            (200, "font/woff2", FONT_JBMONO.to_vec()),
        ("GET", "/icons/sprite.svg") => (200, "image/svg+xml", UI_SPRITE.to_string().into_bytes()),
        ("GET", "/app-icon.png") => (200, "image/png", UI_APPICON.to_vec()),
        ("GET", "/api/jobs") => {
            let (list, stored_prog) = {
                let st = store.lock().unwrap();
                let prog = st
                    .list()
                    .into_iter()
                    .map(|j| (j.id, (j.received, j.total)))
                    .collect::<std::collections::HashMap<_, _>>();
                (st.list(), prog)
            };
            let live = PROGRESS.lock().unwrap().clone();
            // live values win; store-persisted values fill the gaps
            let merged: std::collections::HashMap<u64, (u64, Option<u64>)> = {
                let mut m = stored_prog;
                for (k, v) in live.iter() {
                    m.insert(*k, *v);
                }
                m
            };
            (200, "application/json", jobs_json(&list, &merged).into_bytes())
        }
        ("POST", "/api/add") => {
            let q = parse_query(query);
            let (Some(url), Some(out)) = (q.get("url"), q.get("out")) else {
                return (400, "text/plain", "need url and out".into());
            };
            let mut out_path = PathBuf::from(out);
            // route ONLY when saving directly into the download folder root
            // (parent == the download dir itself — not siblings, which broke
            // tests writing to sibling temp dirs)
            if out_path.parent() == Some(default_download_dir().as_path()) {
                if let Some(fname) = out_path.file_name() {
                    let routed = route_into_category(
                        &fname.to_string_lossy(), &default_download_dir());
                    if routed != out_path { out_path = routed; }
                }
            }
            if !path_allowed(&out_path) {
                return (403, "text/plain", "path not allowed".into());
            }
            let mut st = store.lock().unwrap();
            match st.add(url, &out_path) {
                Ok(id) => {
                    ADDED_AT.lock().unwrap().insert(
                        id,
                        chrono_ish_now(),
                    );
                    // advanced fields (spec section 4): auth, referrer,
                    // cookie, extra headers — applied to this job's requests
                    let mut hs: Vec<(String, String)> = Vec::new();
                    if let (Some(u), Some(pw)) = (q.get("authUser"), q.get("authPass")) {
                        let cred = crate::http::b64(format!("{u}:{pw}").as_bytes());
                        hs.push(("Authorization".into(), format!("Basic {cred}")));
                    }
                    if let Some(r) = q.get("referrer") {
                        if !r.is_empty() {
                            hs.push(("Referer".into(), r.to_string()));
                        }
                    }
                    if let Some(c) = q.get("cookie") {
                        if !c.is_empty() {
                            hs.push(("Cookie".into(), c.to_string()));
                        }
                    }
                    if let Some(xh) = q.get("headers") {
                        for pair in xh.split(',') {
                            if let Some((k, v)) = pair.split_once('=') {
                                hs.push((k.trim().to_string(), v.trim().to_string()));
                            }
                        }
                    }
                    if !hs.is_empty() {
                        JOB_HEADERS.lock().unwrap().insert(id, hs);
                    }
                    if let Some(dir) = Path::new(out).parent() {
                        let dir = dir.to_string_lossy().into_owned();
                        let mut rec = RECENT_FOLDERS.lock().unwrap();
                        if !rec.contains(&dir) {
                            rec.insert(0, dir);
                            rec.truncate(4); // spec: last 4 distinct folders
                        }
                    }
                    (200, "application/json", format!(r#"{{"added":{id}}}"#).into_bytes())
                }
                Err(e) => (500, "text/plain", format!("error: {e}").into_bytes()),
            }
        }
        ("GET", "/api/totals") => {
            let t = TOTALS.lock().unwrap().clone();
            (200, "application/json", format!(
                r#"{{"speedBps":{:.0},"active":{},"paused":{},"queued":{},"failed":{}}}"#,
                t.0, t.1, t.2, t.3, t.4).into_bytes())
        }
        ("POST", "/api/remove") => {
            let q = parse_query(query);
            let Some(id) = q.get("id").and_then(|v| v.parse::<u64>().ok()) else {
                return (400, "text/plain", "need id".into());
            };
            let with_files = q.get("files").map(|v| v == "1").unwrap_or(false);
            let target = store.lock().unwrap().list().into_iter().find(|j| j.id == id).map(|j| j.out);
            let ok = store.lock().unwrap().remove(id).is_ok();
            if ok && with_files {
                if let Some(t) = target {
                    if path_allowed(&t) && t.exists() {
                        let _ = std::fs::remove_file(&t);
                    }
                    let mut part = t.clone().into_os_string();
                    part.push(".part");
                    let part = PathBuf::from(part);
                    if path_allowed(&part) && part.exists() {
                        let _ = std::fs::remove_file(part);
                    }
                }
            }
            (200, "application/json", format!(r#"{{"removed":{ok}}}"#).into_bytes())
        }
        ("GET", "/api/probe") => {
            let q = parse_query(query);
            let Some(url) = q.get("url") else {
                return (400, "text/plain", "need url".into());
            };
            match crate::multi::probe_url(url) {
                Ok(p) => {
                    let total = p
                        .total_bytes
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "null".into());
                    let ct = p
                        .content_type
                        .map(|c| format!(r#""{}""#, json_escape(&c)))
                        .unwrap_or_else(|| "null".into());
                    let fname = p
                        .file_name
                        .map(|f| format!(r#""{}""#, json_escape(&f)))
                        .unwrap_or_else(|| "null".into());
                    (
                        200,
                        "application/json",
                        format!(
                            r#"{{"fileName":{fname},"totalBytes":{total},"contentType":{ct},"resumable":{},"protocol":"{}"}}"#,
                            p.resumable,
                            json_escape(&p.protocol)
                        ).into_bytes(),
                    )
                }
                Err(e) => (502, "text/plain", format!("probe failed: {e}").into_bytes()),
            }
        }
        ("GET", "/api/segments") => {
            let q = parse_query(query);
            let Some(id) = q.get("id").and_then(|v| v.parse::<u64>().ok()) else {
                return (400, "text/plain", "need id".into());
            };
            let st = store.lock().unwrap();
            let Some(job) = st.list().into_iter().find(|j| j.id == id) else {
                return (404, "text/plain", "no such job".into());
            };
            drop(st);
            let mut segs = Vec::new();
            // multi runs checkpoint per-segment state next to the .part file
            let mut sp = job.out.as_os_str().to_os_string();
            sp.push(".part.state");
            if let Ok(text) = std::fs::read_to_string(PathBuf::from(sp)) {
                for line in text.lines() {
                    let f: Vec<&str> = line.split_whitespace().collect();
                    if f.len() == 4 {
                        if let (Ok(start), Ok(end), Ok(done)) = (
                            f[1].parse::<u64>(),
                            f[2].parse::<u64>(),
                            f[3].parse::<u64>(),
                        ) {
                            let state = if done >= end - start + 1 {
                                "done"
                            } else if done > 0 {
                                "receiving"
                            } else {
                                "connecting"
                            };
                            segs.push(format!(
                                r#"{{"start":{start},"end":{end},"received":{done},"state":"{state}"}}"#
                            ));
                        }
                    }
                }
            }
            if segs.is_empty() {
                // single-stream: one aggregate segment
                let (received, total) = PROGRESS
                    .lock()
                    .unwrap()
                    .get(&id)
                    .copied()
                    .unwrap_or((job.received, job.total));
                let end = total.unwrap_or(0).saturating_sub(1);
                segs.push(format!(
                    r#"{{"start":0,"end":{end},"received":{received},"state":"{}"}}"#,
                    if total.unwrap_or(0) > 0 && received >= total.unwrap_or(0) {
                        "done"
                    } else {
                        "receiving"
                    }
                ));
            }
            (200, "application/json", format!(r#"{{"segments":[{}]}}"#, segs.join(",")).into_bytes())
        }
        ("GET", "/api/action-status") => {
            let armed = SCHEDULED_ACTION.lock().unwrap().clone();
            match armed {
                Some((action, deadline)) => {
                    let left = deadline
                        .saturating_duration_since(std::time::Instant::now())
                        .as_secs();
                    (
                        200,
                        "application/json",
                        format!(
                            r#"{{"armed":true,"action":"{}","secondsLeft":{}}}"#,
                            json_escape(&action),
                            left
                        )
                        .into_bytes(),
                    )
                }
                None => (
                    200,
                    "application/json",
                    r#"{"armed":false}"#.to_string().into_bytes(),
                ),
            }
        }
        ("POST", "/api/cancel-action") => {
            *SCHEDULED_ACTION.lock().unwrap() = None;
            (200, "application/json", r#"{"cancelled":true}"#.as_bytes().to_vec())
        }
        ("GET", "/api/categories") => {
            let cats = CATEGORIES.lock().unwrap().clone();
            (200, "application/json", categories_json(&cats).into_bytes())
        }
        ("POST", "/api/categories") => {
            match categories_from_json(json_body) {
                Some(cats) => {
                    save_categories(cats);
                    (200, "application/json", r#"{"saved":true}"#.into())
                }
                None => (400, "text/plain", "bad categories payload".into()),
            }
        }
        ("GET", "/api/extension") => {
            let seen = EXT_LAST_SEEN
                .lock()
                .unwrap()
                .map(|t| t.elapsed() < std::time::Duration::from_secs(150))
                .unwrap_or(false);
            (
                200,
                "application/json",
                format!(r#"{{"connected":{seen},"browsers":[]}}"#).into_bytes(),
            )
        }
        ("OPTIONS", _) => (204, "text/plain", Vec::new()),
        ("POST", "/api/extension/status") => {
            *EXT_LAST_SEEN.lock().unwrap() = Some(std::time::Instant::now());
            (
                200,
                "application/json",
                r#"{"connected":true,"server":"odm-serve"}"#.to_string().into_bytes(),
            )
        }
        ("POST", "/api/extension/add") => {
            // body: {"url":"...", "referrer":"...", "fileName":"..." (optional)}
            let Some(url) = json_field(json_body, "url").filter(|u| !u.is_empty()) else {
                return (400, "text/plain", "need url".into());
            };
            let referrer = json_field(json_body, "referrer").unwrap_or_default();
            let file_name = json_field(json_body, "fileName")
                .filter(|f| !f.is_empty())
                .unwrap_or_else(|| {
                    url.split('?')
                        .next()
                        .and_then(|u| u.rsplit('/').next())
                        .unwrap_or("download.bin")
                        .to_string()
                });
            let dir = default_download_dir();
            let _ = std::fs::create_dir_all(&dir);
            let Some(file_name) = sanitize_file_name(&file_name) else {
                return (400, "text/plain", "bad file name".into());
            };
            let out = route_into_category(&file_name, &dir);
            if !path_allowed(&out) {
                return (403, "text/plain", "path not allowed".into());
            }
            let mut st = store.lock().unwrap();
            let added = st.add(&url, &out);
            if let Ok(id) = added {
                ADDED_AT.lock().unwrap().insert(id, chrono_ish_now());
                if !referrer.is_empty() {
                    JOB_HEADERS.lock().unwrap().insert(
                        id,
                        vec![("Referer".into(), referrer)],
                    );
                }
                drop(st);
                // extension adds auto-start
                let max = SETTINGS
                    .lock()
                    .unwrap()
                    .get("simultaneous")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(4);
                start_pending_downloads(store, max, running, cfg.parts);
                (
                    200,
                    "application/json",
                    format!(r#"{{"added":{id}}}"#).into_bytes(),
                )
            } else {
                (500, "text/plain", "add failed".into())
            }
        }
        ("GET", "/api/settings") => {
            let st = SETTINGS.lock().unwrap();
            let items: Vec<String> = st
                .iter()
                .map(|(k, v)| format!(r#""{}":"{}""#, json_escape(k), json_escape(v)))
                .collect();
            (200, "application/json", format!("{{{}}}", items.join(",")).into_bytes())
        }
        ("POST", "/api/settings") => {
            let q = parse_query(query);
            let mut changed = Vec::new();
            {
                let mut st = SETTINGS.lock().unwrap();
                for (k, v) in q {
                    st.insert(k.clone(), v.clone());
                    changed.push((k, v));
                }
            }
            persist_settings(cfg);
            for (k, v) in changed {
                match k.as_str() {
                    "launchAtLogin" => apply_launch_at_login(v == "1"),
                    "clipboardMonitor" => {
                        if v == "1" {
                            start_clipboard_monitor(store);
                        } else {
                            stop_clipboard_monitor();
                        }
                    }
                    _ => {}
                }
            }
            (200, "application/json", r#"{"saved":true}"#.into())
        }
        ("GET", "/api/queues") => {
            let q = QUEUES.lock().unwrap();
            let items: Vec<String> = q
                .iter()
                .map(|m| {
                    let fields: Vec<String> = m
                        .iter()
                        .map(|(k, v)| format!(r#""{}":"{}""#, json_escape(k), json_escape(v)))
                        .collect();
                    format!("{{{}}}", fields.join(","))
                })
                .collect();
            (200, "application/json", format!(r#"{{"queues":[{}]}}"#, items.join(",")).into_bytes())
        }
        ("POST", "/api/queues") => {
            let q = parse_query(query);
            let id = q.get("id").cloned().unwrap_or_else(|| {
                format!("q{}", std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos()).unwrap_or(0))
            });
            let mut queue = std::collections::BTreeMap::new();
            queue.insert("id".to_string(), id);
            for k in ["name", "maxParallel", "startEnabled", "startAt",
                      "stopEnabled", "stopAt", "days", "whenDone", "retryFailed"] {
                if let Some(v) = q.get(k) {
                    queue.insert(k.to_string(), v.to_string());
                }
            }
            let mut queues = QUEUES.lock().unwrap();
            queues.retain(|m| m.get("id") != queue.get("id"));
            queues.push(queue);
            (200, "application/json", r#"{"saved":true}"#.into())
        }
        ("POST", "/api/open") => {
            let q = parse_query(query);
            let Some(p) = q.get("path") else {
                return (400, "text/plain", "need path".into());
            };
            let folder = q.get("folder").map(|v| v == "1").unwrap_or(false);
            let target = if folder {
                PathBuf::from(p.as_str())
            } else {
                PathBuf::from(p.as_str())
            };
            if !path_allowed(&target) {
                return (403, "text/plain", "path not allowed".into());
            }
            #[cfg(target_os = "macos")]
            let program = "open";
            #[cfg(target_os = "linux")]
            let program = "xdg-open";
            #[cfg(all(not(target_os = "macos"), not(target_os = "linux")))]
            let program = "explorer";
            let ok = std::process::Command::new(program).arg(&target).spawn().is_ok();
            (200, "application/json", format!(r#"{{"opened":{ok}}}"#).into_bytes())
        }
        ("GET", "/api/recent") => {
            let r = RECENT_FOLDERS.lock().unwrap();
            let items: Vec<String> = r
                .iter()
                .map(|f| format!(r#""{}""#, json_escape(f)))
                .collect();
            (200, "application/json", format!(r#"{{"folders":[{}]}}"#, items.join(",")).into_bytes())
        }
        ("POST", "/api/limit") => {
            let q = parse_query(query);
            let Some(id) = q.get("id").and_then(|v| v.parse::<u64>().ok()) else {
                return (400, "text/plain", "need id".into());
            };
            let mbps: u64 = q
                .get("mbps")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let ok = {
                let map = ACTIVE_LIMITS.lock().unwrap();
                match map.get(&id) {
                    Some(l) => {
                        l.set_limit(mbps * 1024 * 1024);
                        true
                    }
                    None => false,
                }
            };
            (200, "application/json", format!(r#"{{"limited":{ok}}}"#).into_bytes())
        }
        ("GET", "/api/exists") => {
            let q = parse_query(query);
            let Some(p) = q.get("path") else {
                return (400, "text/plain", "need path".into());
            };
            let exists = Path::new(p.as_str()).exists();
            (200, "application/json", format!(r#"{{"exists":{exists}}}"#).into_bytes())
        }
        ("POST", "/api/requeue") => {
            let n = store.lock().unwrap().requeue_failed();
            (200, "application/json", format!(r#"{{"requeued":{n}}}"#).into_bytes())
        }
        ("POST", "/api/pause") => {
            let q = parse_query(query);
            let Some(id) = q.get("id").and_then(|v| v.parse::<u64>().ok()) else {
                return (400, "text/plain", "need id".into());
            };
            // sets the engine's control flag; the download checkpoints to
            // .part/.state and the job lands in the Paused state
            let ok = ACTIVE.pause(id);
            if std::env::var("ODM_DEBUG").is_ok() {
                eprintln!("[srv-pause] id={id} flag_found={ok}");
            }
            (200, "application/json", format!(r#"{{"pause_requested":{ok}}}"#).into_bytes())
        }
        ("POST", "/api/resume") => {
            let q = parse_query(query);
            let Some(id) = q.get("id").and_then(|v| v.parse::<u64>().ok()) else {
                return (400, "text/plain", "need id".into());
            };
            let mut st = store.lock().unwrap();
            let ok = st.set_state(id, JobState::Pending).is_ok();
            drop(st);
            (200, "application/json", format!(r#"{{"resumed":{ok}}}"#).into_bytes())
        }
        ("POST", "/api/run") => {
            let q = parse_query(query);
            let max = q
                .get("max")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(cfg.max_parallel);
            if let Some(parts) = q.get("parts").and_then(|v| v.parse::<usize>().ok()) {
                SETTINGS.lock().unwrap().insert("defaultParts".into(), parts.to_string());
            }
            let store = Arc::clone(store);
            let running = Arc::clone(running);
            let fp = cfg.parts;
            std::thread::spawn(move || {
                start_pending_downloads(&store, max, &running, fp);
            });
            (200, "application/json", format!(r#"{{"started":true,"max":{max}}}"#).into_bytes())
        }
        _ => (404, "text/plain", "not found".into()),
    }

}
use crate::queue;

use std::path::{Path, PathBuf};

fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            map.insert(percent_decode(k), percent_decode(v));
        }
    }
    map
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() + 1 && i + 2 <= bytes.len() - 1 + 1 => {
                let hex = &s[i + 1..(i + 3).min(s.len())];
                if hex.len() == 2 {
                    if let Ok(v) = u8::from_str_radix(hex, 16) {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                }
                out.push(b'%');
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn json_field(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let i = json.find(&needle)? + needle.len();
    let rest = &json[i..];
    let c = rest.find(':')? + 1;
    let rest = &rest[c..];
    let rest = rest.trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        // read until unescaped quote
        let mut out = String::new();
        let mut chars = stripped.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(e) = chars.next() {
                        out.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            other => other,
                        });
                    }
                }
                '"' => return Some(out),
                c => out.push(c),
            }
        }
        None
    } else {
        let end = rest
            .find(|c: char| c == ',' || c == '}')
            .unwrap_or(rest.len());
        Some(rest[..end].trim().to_string())
    }
}

/// Load or create the per-instance API token (stored next to the jobs file).
/// Local-only threat model: the token blocks drive-by requests from
/// websites; the UI receives it by same-origin injection.
pub fn load_or_create_token(jobs_path: &Path) -> String {
    let tf = PathBuf::from(format!("{}.token", jobs_path.display()));
    if let Ok(t) = std::fs::read_to_string(&tf) {
        let t = t.trim().to_string();
        if t.len() >= 16 {
            return t;
        }
    }
    let t = random_token();
    let _ = std::fs::write(&tf, &t);
    t
}

fn random_token() -> String {
    // 32 hex chars: exactly 16 bytes from the OS CSPRNG.
    // (NEVER fs::read urandom — it has no EOF and would eat all RAM.)
    use std::io::Read;
    let mut buf = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok();
    if ok {
        return buf.iter().map(|x| format!("{x:02x}")).collect();
    }
    // fallback: time + pid mix (non-CSPRNG platforms only)
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("{:016x}{:016x}", nanos as u64, pid as u64)
}

/// Lexically normalize a path (resolve `..` and `.` without touching disk).
fn normalize_path(p: &Path) -> PathBuf {
    let mut out: Vec<std::ffi::OsString> = Vec::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => { out.pop(); }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str().to_os_string()),
        }
    }
    out.iter().collect()
}

/// Canonicalize the deepest existing ancestor so symlinked roots
/// (macOS /tmp -> /private/tmp) compare correctly.
fn canon_ancestor(p: &Path) -> PathBuf {
    let mut cur = normalize_path(p);
    let mut suffix = PathBuf::new();
    loop {
        if let Ok(c) = cur.canonicalize() {
            return c.join(suffix);
        }
        match cur.file_name() {
            Some(f) => {
                suffix = PathBuf::from(f).join(suffix);
                cur.pop();
            }
            None => return cur.join(suffix),
        }
    }
}

/// Writes/deletes/opens are only permitted under HOME, the system temp
/// dir, /tmp, or the configured default download dir (path-traversal guard).
fn path_allowed(p: &Path) -> bool {
    let target = canon_ancestor(p);
    let home = std::env::var("HOME").unwrap_or_default();
    let mut roots: Vec<PathBuf> = vec![
        std::env::temp_dir(),
        PathBuf::from("/tmp"),
        default_download_dir(),
    ];
    if !home.is_empty() {
        roots.push(PathBuf::from(home));
    }
    roots.iter().any(|r| {
        let root = canon_ancestor(r);
        target.starts_with(&root)
    })
}

/// Reject file names carrying path components (used for extension adds).
fn sanitize_file_name(name: &str) -> Option<String> {
    let n = name.trim();
    if n.is_empty() || n == "." || n == ".."
        || n.contains('/') || n.contains('\\')
        || n.contains('\u{0}') {
        return None;
    }
    Some(n.to_string())
}

fn default_download_dir() -> PathBuf {
    if let Some(d) = SETTINGS.lock().unwrap().get("defaultDir") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    #[cfg(target_os = "macos")]
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    #[cfg(not(target_os = "macos"))]
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join("Downloads")
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn spec_status(state: JobState, has_progress: bool) -> &'static str {
    match state {
        JobState::Pending => "queued",
        JobState::Running => if has_progress { "downloading" } else { "connecting" },
        JobState::Paused => "paused",
        JobState::Done => "complete",
        JobState::Failed => "error",
    }
}

fn file_name_of(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn chrono_ish_now() -> String {
    // "Oct 6" style short date from system time (no external crates)
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    // civil-from-days (Howard Hinnant algorithm)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let _y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {}", months[(m - 1) as usize], d)
}

fn jobs_json(
    jobs: &[JobRecord],
    progress: &std::collections::HashMap<u64, (u64, Option<u64>)>,
) -> String {
    let ema = SPEED_EMA.lock().unwrap();
    let added = ADDED_AT.lock().unwrap();
    let hist = SPEED_HISTORY.lock().unwrap();
    let mut items = Vec::new();
    for j in jobs {
        let prog = progress.get(&j.id).copied();
        let has_progress = prog.is_some();
        let (done, total) = prog.unwrap_or((0, None));
        let received = done;
        let total_bytes = total;
        let speed = ema.get(&j.id).map(|e| e.0).unwrap_or(0.0);
        let eta = match (total_bytes, speed) {
            (Some(t), s) if s > 1.0 => Some(((t as f64 - received as f64) / s) as u64),
            _ => None,
        };
        // keep the legacy keys ("state", "done", "total") for existing tests,
        // add the spec Download subset for the UI
        let file = file_name_of(&j.out);
        let ext = file.rsplit('.').next().unwrap_or("?").to_uppercase()[..4.min(
            file.rsplit('.').next().unwrap_or("?").len(),
        )]
        .to_string();
        let added = added.get(&j.id).cloned().unwrap_or_else(chrono_ish_now);
        let history: Vec<String> = hist
            .get(&j.id)
            .map(|h| h.iter().map(|v| format!("{v:.0}")).collect())
            .unwrap_or_default();
        items.push(format!(
            r#"{{"id":{},"state":"{}","status":"{}","done":{},"total":{},"receivedBytes":{},"totalBytes":{},"speedBps":{:.0},"etaSecs":{},"fileName":"{}","ext":"{}","out":"{}","url":"{}","addedAt":"{}","resumable":true,"speedHistory":[{}]}}"#,
            j.id,
            j.state.as_str(),
            spec_status(j.state, has_progress),
            done,
            total.map(|t| t.to_string()).unwrap_or_else(|| "null".into()),
            received,
            total_bytes.map(|t| t.to_string()).unwrap_or_else(|| "null".into()),
            speed,
            eta.map(|e| e.to_string()).unwrap_or_else(|| "null".into()),
            json_escape(&file),
            json_escape(&ext),
            json_escape(&j.out.display().to_string()),
            json_escape(&j.url),
            json_escape(&added),
            history.join(","),
        ));
    }
    format!(r#"{{"jobs":[{}]}}"#, items.join(","))
}


