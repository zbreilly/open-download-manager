//! Serializable data model shared with the UI (camelCase JSON).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Connecting,
    Downloading,
    Paused,
    Assembling,
    Complete,
    Error,
}

impl Status {
    pub fn is_active(self) -> bool {
        matches!(self, Status::Connecting | Status::Downloading | Status::Assembling)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SegState {
    Idle,
    Connecting,
    Receiving,
    Reconnecting,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentView {
    pub start: u64,
    /// exclusive end; `u64::MAX` = unknown (stream to EOF)
    pub end: u64,
    pub received: u64,
    pub state: SegState,
    #[serde(default)]
    pub speed_bps: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checksum {
    /// "sha256" | "sha1" | "md5"
    pub algo: String,
    pub expected: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub final_url: Option<String>,
    pub file_name: String,
    pub ext: String,
    pub category: String,
    pub queue_id: Option<String>,
    pub save_dir: String,
    pub total_bytes: Option<u64>,
    pub received_bytes: u64,
    #[serde(default)]
    pub speed_bps: u64,
    #[serde(default)]
    pub eta_secs: Option<u64>,
    pub status: Status,
    #[serde(default)]
    pub scheduled_at: Option<String>,
    pub resumable: bool,
    pub max_connections: u32,
    #[serde(default)]
    pub active_connections: u32,
    #[serde(default)]
    pub segments: Vec<SegmentView>,
    #[serde(default)]
    pub speed_history: Vec<u64>,
    pub error: Option<String>,
    pub added_at: String,
    pub completed_at: Option<String>,

    // ---- extended fields ----
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub referrer: Option<String>,
    #[serde(default)]
    pub cookies: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub checksum: Option<Checksum>,
    #[serde(default)]
    pub checksum_ok: Option<bool>,
    /// per-download speed limit (bytes/s); None = follow settings
    #[serde(default)]
    pub speed_limit_bps: Option<u64>,
    /// "notify" | "openFile" | "openFolder" | "none"
    #[serde(default = "default_when_finished")]
    pub when_finished: String,
    /// started outside a queue (manual start / "Start download")
    #[serde(default)]
    pub manual: bool,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub last_modified: Option<String>,
    #[serde(default)]
    pub retries_used: u32,
    /// "overwrite" | "rename" | "resume"
    #[serde(default)]
    pub on_exists: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// name came from the URL; may be replaced by Content-Disposition
    #[serde(default)]
    pub auto_name: bool,
    /// folder came from the category default; follows category changes
    #[serde(default)]
    pub auto_dir: bool,
    #[serde(default)]
    pub restarts: u32,
    /// The part file was not created by the engine: "file" = the user's
    /// existing file was taken over to resume it, "part" = an existing
    /// `.odmpart` was taken over. Such files are never deleted automatically.
    #[serde(default)]
    pub adopted: Option<String>,
}

fn default_when_finished() -> String {
    "notify".into()
}

impl Download {
    pub fn final_path(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.save_dir).join(&self.file_name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AddRequest {
    pub url: String,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub save_dir: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub queue_id: Option<String>,
    /// true = start now; false = add to queue / paused
    #[serde(default)]
    pub start: bool,
    #[serde(default)]
    pub connections: Option<u32>,
    #[serde(default)]
    pub referrer: Option<String>,
    #[serde(default)]
    pub cookies: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub checksum: Option<Checksum>,
    #[serde(default)]
    pub on_exists: Option<String>,
    #[serde(default)]
    pub remember_folder: bool,
    #[serde(default)]
    pub description: Option<String>,
    /// pre-probed info (skip a second probe)
    #[serde(default)]
    pub total_bytes: Option<u64>,
    #[serde(default)]
    pub resumable: Option<bool>,
    #[serde(default)]
    pub mime: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub final_url: String,
    pub file_name: String,
    pub total_bytes: Option<u64>,
    pub mime: Option<String>,
    pub resumable: bool,
    pub protocol: String,
    pub category: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressItem {
    pub id: String,
    pub received_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: u64,
    pub eta_secs: Option<u64>,
    pub active_connections: u32,
    pub segments: Vec<SegmentView>,
    pub status: Status,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub speed_bps: u64,
    pub active: u32,
    pub paused: u32,
    pub queued: u32,
    pub scheduled: u32,
    pub failed: u32,
    pub complete: u32,
    pub free_bytes: Option<u64>,
    pub limit_on: bool,
    pub limit_bps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Queue {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default = "one")]
    pub max_concurrent: u32,
    /// how many times to retry a failed download (0 = never)
    #[serde(default)]
    pub retry_failed: u32,
    #[serde(default)]
    pub start_enabled: bool,
    #[serde(default = "default_start")]
    pub start_time: String,
    #[serde(default)]
    pub stop_enabled: bool,
    #[serde(default = "default_stop")]
    pub stop_time: String,
    /// Mon..Sun
    #[serde(default = "all_days")]
    pub days: Vec<bool>,
    /// "none" | "sleep" | "shutdown" | "exit"
    #[serde(default = "none_str")]
    pub on_finish: String,
    /// download ids in run order. When saving a queue, an omitted / null /
    /// partial order keeps the current members (see `Manager::save_queue`).
    #[serde(default, deserialize_with = "null_as_empty")]
    pub order: Vec<String>,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub next_run: Option<String>,
}

fn null_as_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(Option::<Vec<String>>::deserialize(d)?.unwrap_or_default())
}

fn one() -> u32 {
    1
}
fn default_start() -> String {
    "01:00".into()
}
fn default_stop() -> String {
    "07:00".into()
}
fn all_days() -> Vec<bool> {
    vec![true; 7]
}
fn none_str() -> String {
    "none".into()
}

impl Queue {
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            builtin: false,
            max_concurrent: 2,
            retry_failed: 3,
            start_enabled: false,
            start_time: default_start(),
            stop_enabled: false,
            stop_time: default_stop(),
            days: all_days(),
            on_finish: none_str(),
            order: vec![],
            running: false,
            next_run: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub extensions: Vec<String>,
    #[serde(default)]
    pub folder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    // General
    pub launch_at_login: bool,
    pub minimize_to_tray: bool,
    pub clipboard_monitor: bool,
    pub show_progress_on_start: bool,
    pub show_complete_dialog: bool,
    pub confirm_delete: bool,
    // Connection & speed
    pub default_connections: u32,
    pub max_simultaneous: u32,
    pub global_limit_enabled: bool,
    pub global_limit_bps: u64,
    pub per_download_limit_enabled: bool,
    pub per_download_limit_bps: u64,
    pub limit_on_startup: bool,
    // Browser integration
    pub capture_enabled: bool,
    pub capture_extensions: Vec<String>,
    pub bypass_key: String,
    pub capture_paused_until: Option<i64>,
    pub capture_min_size_kb: u64,
    pub extension_port: u16,
    // Categories & save locations
    pub categories: Vec<Category>,
    pub default_folder: String,
    pub temp_folder: Option<String>,
    // Proxy
    pub proxy_mode: String,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub proxy_user: String,
    pub proxy_pass: String,
    // Notifications
    pub notify_complete: bool,
    pub notify_failure: bool,
    pub notify_queue_finished: bool,
    // Appearance
    pub theme: String,
    pub density: String,
    pub tray_speed: bool,
    // Advanced
    pub user_agent: String,
    pub max_retries: u32,
    pub retry_delay_secs: u64,
    pub timeout_secs: u64,
    pub min_segment_kb: u64,
    pub verify_tls: bool,
}

pub const DEFAULT_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

pub fn default_categories() -> Vec<Category> {
    let c = |id: &str, name: &str, icon: &str, exts: &str| Category {
        id: id.into(),
        name: name.into(),
        icon: icon.into(),
        extensions: exts.split_whitespace().map(|s| s.to_string()).collect(),
        folder: None,
    };
    vec![
        c("video", "Video", "cat-video", "mp4 mkv avi mov wmv flv webm m4v mpg mpeg 3gp ts m2ts vob ogv rm rmvb asf qt"),
        c("music", "Music", "cat-music", "mp3 wav flac aac m4a ogg opus wma aif aiff ape mid midi ra"),
        c("documents", "Documents", "cat-documents", "pdf doc docx xls xlsx ppt pptx odt ods odp rtf txt epub mobi csv pps"),
        c("compressed", "Compressed", "cat-compressed", "zip rar 7z tar gz tgz bz2 xz zst lz lzma cab arj z sit sitx"),
        c("programs", "Programs", "cat-programs", "exe msi dmg pkg deb rpm appimage apk msix msu iso img bin jar"),
        c("general", "General", "cat-general", ""),
    ]
}

/// Where files of a category are saved: its own folder if set, otherwise a
/// subfolder named after the category inside the default folder. "General"
/// (and anything unknown) goes straight into the default folder.
pub fn category_folder(s: &Settings, category: &str) -> String {
    let Some(c) = s.categories.iter().find(|c| c.id == category) else {
        return s.default_folder.clone();
    };
    if let Some(f) = c.folder.as_ref().filter(|f| !f.trim().is_empty()) {
        return f.clone();
    }
    if c.id == "general" || c.name.trim().is_empty() {
        return s.default_folder.clone();
    }
    let name = crate::util::sanitize_filename(c.name.trim());
    std::path::Path::new(&s.default_folder).join(name).to_string_lossy().into_owned()
}

/// Keep each extension in exactly one category (first one wins), lowercase,
/// without dots; "general" never lists extensions.
pub fn normalize_categories(cats: &mut [Category]) {
    let mut seen = std::collections::HashSet::new();
    for c in cats.iter_mut() {
        let list: Vec<String> = std::mem::take(&mut c.extensions);
        if c.id == "general" {
            continue;
        }
        for e in list {
            let e = e.trim().trim_start_matches('.').to_ascii_lowercase();
            if !e.is_empty() && seen.insert(e.clone()) {
                c.extensions.push(e);
            }
        }
    }
}

pub fn default_capture_exts() -> Vec<String> {
    "3gp 7z aac ace aif apk arj asf avi bin bz2 dmg deb exe flac flv gz img iso jar lzh m4a m4v mkv mov mp3 mp4 mpeg mpg msi msu ogg ogv pdf pkg rar rpm tar tgz wav webm wma wmv xz z zip zst appimage"
        .split_whitespace()
        .map(String::from)
        .collect()
}

impl Default for Settings {
    fn default() -> Self {
        let dl = dirs::download_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        Self {
            launch_at_login: false,
            minimize_to_tray: true,
            clipboard_monitor: false,
            show_progress_on_start: false,
            show_complete_dialog: false,
            confirm_delete: true,
            default_connections: 16,
            max_simultaneous: 4,
            global_limit_enabled: false,
            global_limit_bps: 25 * 1024 * 1024,
            per_download_limit_enabled: false,
            per_download_limit_bps: 5 * 1024 * 1024,
            limit_on_startup: false,
            capture_enabled: true,
            capture_extensions: default_capture_exts(),
            bypass_key: "alt".into(),
            capture_paused_until: None,
            capture_min_size_kb: 0,
            extension_port: 8901,
            categories: default_categories(),
            default_folder: dl.to_string_lossy().into_owned(),
            temp_folder: None,
            proxy_mode: "system".into(),
            proxy_host: String::new(),
            proxy_port: 8080,
            proxy_user: String::new(),
            proxy_pass: String::new(),
            notify_complete: true,
            notify_failure: true,
            notify_queue_finished: true,
            theme: "dark".into(),
            density: "comfortable".into(),
            tray_speed: false,
            user_agent: DEFAULT_UA.into(),
            max_retries: 10,
            retry_delay_secs: 3,
            timeout_secs: 30,
            min_segment_kb: 512,
            verify_tls: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrabbedLink {
    pub url: String,
    pub file_name: String,
    pub ext: String,
    pub category: String,
    pub source: String,
}
