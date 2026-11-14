//! Tray icon (StatusNotifierItem over D-Bus): native on Plasma, and on
//! GNOME with the AppIndicator extension (Ubuntu and Fedora ship or offer
//! it). Without a tray host `Tray::spawn` returns None and the app should
//! not hide itself on close.
use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, MenuItem, StandardItem};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    AddUrl,
    Grabber,
    StartMainQueue,
    PauseAll,
    ToggleLimiter,
    Scheduler,
    Settings,
    Quit,
}

/// Icon states, matching the design's tray icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Downloading,
    Paused,
    Error,
    Complete,
}

pub struct Tray {
    handle: ksni::blocking::Handle<Item>,
}

struct Item {
    state: TrayState,
    tooltip: String,
    status_line: String,
    limiter: bool,
    on_action: Arc<dyn Fn(TrayAction) + Send + Sync>,
}

fn png(state: TrayState) -> &'static [u8] {
    match state {
        TrayState::Idle => include_bytes!("../../../src-tauri/icons/tray/tray-idle-32.png"),
        TrayState::Downloading => include_bytes!("../../../src-tauri/icons/tray/tray-downloading-32.png"),
        TrayState::Paused => include_bytes!("../../../src-tauri/icons/tray/tray-paused-32.png"),
        TrayState::Error => include_bytes!("../../../src-tauri/icons/tray/tray-error-32.png"),
        TrayState::Complete => include_bytes!("../../../src-tauri/icons/tray/tray-complete-32.png"),
    }
}

/// Decode a PNG into the ARGB32 (network byte order) pixmap SNI wants.
fn pixmap(bytes: &[u8]) -> Option<ksni::Icon> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let px = &buf[..info.buffer_size()];
    let data: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => px.as_chunks::<4>().0.iter().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect(),
        png::ColorType::Rgb => px.as_chunks::<3>().0.iter().flat_map(|p| [255, p[0], p[1], p[2]]).collect(),
        png::ColorType::GrayscaleAlpha => px.as_chunks::<2>().0.iter().flat_map(|p| [p[1], p[0], p[0], p[0]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|g| [255, *g, *g, *g]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some(ksni::Icon { width: info.width as i32, height: info.height as i32, data })
}

impl Item {
    fn item(&self, label: &str, action: TrayAction) -> MenuItem<Self> {
        StandardItem {
            label: label.into(),
            activate: Box::new(move |t: &mut Self| (t.on_action)(action)),
            ..Default::default()
        }
        .into()
    }
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        crate::APP_ID.into()
    }

    fn title(&self) -> String {
        crate::APP_NAME.into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::ApplicationStatus
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        pixmap(png(self.state)).into_iter().collect()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: crate::APP_NAME.into(), description: self.tooltip.clone(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        (self.on_action)(TrayAction::Show);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem { label: self.status_line.clone(), enabled: false, ..Default::default() }.into(),
            MenuItem::Separator,
            self.item("Show Open Download Manager", TrayAction::Show),
            self.item("Add URL…", TrayAction::AddUrl),
            self.item("Site Grabber…", TrayAction::Grabber),
            MenuItem::Separator,
            self.item("Start Main Queue", TrayAction::StartMainQueue),
            self.item("Pause All", TrayAction::PauseAll),
            CheckmarkItem {
                label: "Speed Limiter".into(),
                checked: self.limiter,
                activate: Box::new(|t: &mut Self| (t.on_action)(TrayAction::ToggleLimiter)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            self.item("Scheduler…", TrayAction::Scheduler),
            self.item("Settings…", TrayAction::Settings),
            MenuItem::Separator,
            self.item("Quit", TrayAction::Quit),
        ]
    }
}

impl Tray {
    /// Show the tray icon. `on_action` runs on a background thread.
    pub fn spawn(on_action: impl Fn(TrayAction) + Send + Sync + 'static) -> Option<Tray> {
        let item = Item {
            state: TrayState::Idle,
            tooltip: "No active downloads".into(),
            status_line: "No active downloads".into(),
            limiter: false,
            on_action: Arc::new(on_action),
        };
        // fails when no StatusNotifierWatcher is running (no tray host)
        item.spawn().ok().map(|handle| Tray { handle })
    }

    /// Icon, and tooltip text (e.g. "2 active · 4.1 MB/s").
    pub fn set_state(&self, state: TrayState, tooltip: &str) {
        let tooltip = tooltip.to_string();
        self.handle.update(move |t| {
            t.state = state;
            t.tooltip = tooltip;
        });
    }

    /// Check mark of the "Speed limiter" menu item.
    pub fn set_limiter(&self, on: bool) {
        self.handle.update(move |t| t.limiter = on);
    }

    /// First menu line, e.g. "2 active · 4.1 MB/s" or "No active downloads".
    pub fn set_status_line(&self, text: &str) {
        let text = text.to_string();
        self.handle.update(move |t| t.status_line = text);
    }
}
