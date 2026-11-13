//! Window management: every dialog is its own native window, like IDM.

use crate::state::AppState;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub struct ViewSpec {
    pub title: &'static str,
    pub w: f64,
    pub h: f64,
    pub min_w: f64,
    pub min_h: f64,
    pub resizable: bool,
}

pub fn spec(view: &str) -> ViewSpec {
    match view {
        "main" => ViewSpec { title: "Open Download Manager", w: 1280.0, h: 860.0, min_w: 900.0, min_h: 600.0, resizable: true },
        "add" => ViewSpec { title: "Add download", w: 560.0, h: 720.0, min_w: 480.0, min_h: 560.0, resizable: true },
        "progress" => ViewSpec { title: "Download", w: 760.0, h: 680.0, min_w: 600.0, min_h: 520.0, resizable: true },
        "scheduler" => ViewSpec { title: "Queues & scheduler", w: 820.0, h: 680.0, min_w: 720.0, min_h: 560.0, resizable: true },
        "settings" => ViewSpec { title: "Settings", w: 960.0, h: 700.0, min_w: 820.0, min_h: 560.0, resizable: true },
        "grabber" => ViewSpec { title: "Site grabber", w: 900.0, h: 680.0, min_w: 700.0, min_h: 520.0, resizable: true },
        "about" => ViewSpec { title: "About Open Download Manager", w: 420.0, h: 400.0, min_w: 420.0, min_h: 400.0, resizable: false },
        _ => ViewSpec { title: "Open Download Manager", w: 800.0, h: 600.0, min_w: 600.0, min_h: 400.0, resizable: true },
    }
}

pub fn theme_param(app: &AppHandle) -> String {
    app.state::<AppState>().mgr.settings().theme
}

fn bg_for(theme: &str) -> tauri::window::Color {
    let light = theme == "light";
    if light {
        tauri::window::Color(0xF3, 0xF4, 0xF6, 0xFF)
    } else {
        tauri::window::Color(0x0D, 0x0F, 0x12, 0xFF)
    }
}

/// Open (or focus) a view. `key` distinguishes multiple windows of one
/// view (e.g. one progress window per download). `params` is handed to
/// the window through a one-time token.
pub fn open(app: &AppHandle, view: &str, key: Option<&str>, params: Option<serde_json::Value>) -> tauri::Result<WebviewWindow> {
    let label = match (view, key) {
        ("main", _) => "main".to_string(),
        ("add", _) => format!("add-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
        (v, Some(k)) => format!("{v}-{}", sanitize_label(k)),
        (v, None) => v.to_string(),
    };
    if let Some(w) = app.get_webview_window(&label) {
        if let Some(p) = params {
            let _ = tauri::Emitter::emit_to(app, label.as_str(), "view_params", p);
        }
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(w);
    }
    if view == "add" && !can_open_add(app) {
        // links arriving faster than they are dealt with (a page firing
        // odm: links, a script): bring an open Add window forward instead
        // of opening ever more
        if let Some(w) = add_windows(app).into_iter().next() {
            let _ = w.unminimize();
            let _ = w.set_focus();
            return Ok(w);
        }
    }
    let state = app.state::<AppState>();
    let token = uuid::Uuid::new_v4().simple().to_string();
    if let Some(p) = params {
        state.pending.lock().insert(token.clone(), p);
    }
    let theme = theme_param(app);
    let mut url = format!("index.html?view={view}&theme={theme}&t={token}");
    if let Some(k) = key {
        url.push_str(&format!("&id={}", url_escape(k)));
    }
    let s = spec(view);
    let mut b = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title(s.title)
        .inner_size(s.w, s.h)
        .min_inner_size(s.min_w, s.min_h)
        .resizable(s.resizable)
        .background_color(bg_for(&theme))
        // HTML5 drag & drop (rows -> queues, links dropped from a browser)
        .disable_drag_drop_handler()
        // with that, a file dropped on the window would make the webview
        // load it: the windows only ever show the app's own pages
        .on_navigation(is_app_url)
        .visible(true);
    b = match theme.as_str() {
        "light" => b.theme(Some(tauri::Theme::Light)),
        "dark" => b.theme(Some(tauri::Theme::Dark)),
        _ => b,
    };
    if view != "main" {
        // dialogs open centered over the main window when it is visible
        b = b.center();
    }
    let w = b.build()?;
    let _ = w.set_focus();
    Ok(w)
}

/// Most Add download windows open at once.
pub const MAX_ADD_WINDOWS: usize = 8;

fn add_windows(app: &AppHandle) -> Vec<WebviewWindow> {
    app.webview_windows().into_iter().filter(|(label, _)| label.starts_with("add-")).map(|(_, w)| w).collect()
}

/// May another Add download window be opened?
pub fn can_open_add(app: &AppHandle) -> bool {
    add_windows(app).len() < MAX_ADD_WINDOWS
}

/// Is `u` one of the app's own pages (the bundled frontend, or the dev
/// server in development builds)?
fn is_app_url(u: &tauri::Url) -> bool {
    match u.scheme() {
        "tauri" => true,
        "about" => u.path() == "blank",
        "http" | "https" => match u.host_str() {
            Some("tauri.localhost") => true,
            Some("localhost") => tauri::is_dev(),
            _ => false,
        },
        _ => false,
    }
}

pub fn show_main(app: &AppHandle) {
    let _ = open(app, "main", None, None);
}

fn sanitize_label(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(48).collect()
}

fn url_escape(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Apply a theme change to every open window.
pub fn retheme_all(app: &AppHandle, theme: &str) {
    for (_, w) in app.webview_windows() {
        let _ = w.set_background_color(Some(bg_for(theme)));
        let t = match theme {
            "light" => Some(tauri::Theme::Light),
            "dark" => Some(tauri::Theme::Dark),
            _ => None,
        };
        let _ = w.set_theme(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_app_pages_load() {
        let u = |s: &str| tauri::Url::parse(s).unwrap();
        assert!(is_app_url(&u("http://tauri.localhost/index.html?view=main")));
        assert!(is_app_url(&u("https://tauri.localhost/index.html")));
        assert!(is_app_url(&u("tauri://localhost/index.html")));
        assert!(is_app_url(&u("about:blank")));
        assert!(!is_app_url(&u("file:///C:/Users/me/Downloads/page.html")));
        assert!(!is_app_url(&u("https://example.com/")));
        assert!(!is_app_url(&u("http://tauri.localhost.evil.example/")));
        assert!(!is_app_url(&u("data:text/html,<script>1</script>")));
    }
}
