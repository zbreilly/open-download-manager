//! The menu bar and the menu-bar (status) item.
use crate::app::app;
use crate::kit::{self, checked, disabled_item, item, key_item, mtm, ns, responder_item, separator};
use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::sel;
use objc2_app_kit::*;
use objc2_foundation::NSString;
use odm_ui::{b, fmt, n, s, APP_NAME};
use serde_json::{json, Value};

const CMD: NSEventModifierFlags = NSEventModifierFlags::Command;

fn shift_cmd() -> NSEventModifierFlags {
    NSEventModifierFlags::Command | NSEventModifierFlags::Shift
}

/// A menu of standard actions: AppKit enables its items by asking the
/// responder chain.
fn standard_menu(title: &str, items: Vec<Retained<NSMenuItem>>) -> Retained<NSMenu> {
    let m = kit::menu(title);
    unsafe { m.setAutoenablesItems(true) };
    for i in &items {
        m.addItem(i);
    }
    m
}

fn with_mask(i: Retained<NSMenuItem>, mask: NSEventModifierFlags) -> Retained<NSMenuItem> {
    unsafe { i.setKeyEquivalentModifierMask(mask) };
    i
}

/// For a list shortcut that clashes with text editing (⌘⌫, ⌘.): when it
/// was pressed while text is being edited, or outside the main window, the
/// key's standard editing action goes to the focused view instead. Chosen
/// with the mouse, the command always runs.
fn list_command(fallback: Sel, f: impl FnOnce()) {
    let nsapp = NSApplication::sharedApplication(mtm());
    let from_mouse = unsafe { nsapp.currentEvent() }
        .map(|e| {
            let t = unsafe { e.r#type() };
            t == NSEventType::LeftMouseDown || t == NSEventType::LeftMouseUp || t == NSEventType::RightMouseDown || t == NSEventType::RightMouseUp || t == NSEventType::OtherMouseUp
        })
        .unwrap_or(false);
    let key = unsafe { nsapp.keyWindow() };
    let main_is_key = match (&key, app().main_window()) {
        (Some(k), Some(m)) => std::ptr::eq(&**k, m.window()),
        _ => false,
    };
    let editing = key.and_then(|w| unsafe { w.firstResponder() }).map(|r| r.downcast_ref::<NSText>().is_some()).unwrap_or(false);
    if from_mouse || (main_is_key && !editing) {
        f();
    } else {
        unsafe { nsapp.sendAction_to_from(fallback, None, None) };
    }
}

fn selected() -> Vec<Value> {
    app().main_window().map(|m| m.selected()).unwrap_or_default()
}

fn ids(items: &[Value]) -> Vec<String> {
    items.iter().map(|d| s(d, "id").to_string()).collect()
}

pub fn can_resume(items: &[Value]) -> bool {
    items.iter().any(|d| matches!(s(d, "status"), "paused" | "error" | "queued"))
}

pub fn can_pause(items: &[Value]) -> bool {
    items.iter().any(|d| fmt::is_active(s(d, "status")) || s(d, "status") == "queued")
}

fn enabled(i: Retained<NSMenuItem>, on: bool) -> Retained<NSMenuItem> {
    unsafe { i.setEnabled(on) };
    i
}

fn fill_downloads(menu: &NSMenu) {
    let items = selected();
    let first = items.first().cloned();
    let all = ids(&items);
    let limit_on = b(&app().store.borrow().settings, "globalLimitEnabled");
    let complete = first.as_ref().map(|d| s(d, "status") == "complete").unwrap_or(false);
    let list: Vec<Retained<NSMenuItem>> = vec![
        enabled(
            key_item("Resume", "r", CMD, {
                let all = all.clone();
                move || app().resume(&all)
            }),
            can_resume(&items),
        ),
        enabled(
            key_item("Pause", ".", CMD, {
                let all = all.clone();
                move || list_command(sel!(cancelOperation:), || app().pause(&all))
            }),
            can_pause(&items),
        ),
        key_item("Pause All", ".", shift_cmd(), || app().pause_all()),
        separator(),
        enabled(
            key_item("Open", "o", CMD, {
                let d = first.clone();
                move || {
                    if let Some(d) = &d {
                        app().open_file(d)
                    }
                }
            }),
            complete,
        ),
        enabled(
            key_item("Show in Finder", "r", shift_cmd(), {
                let d = first.clone();
                move || {
                    if let Some(d) = &d {
                        crate::system::reveal_download(d)
                    }
                }
            }),
            first.is_some(),
        ),
        enabled(
            key_item("Copy Link", "c", shift_cmd(), {
                let d = first.clone();
                move || {
                    if let Some(d) = &d {
                        crate::system::copy(s(d, "url"))
                    }
                }
            }),
            first.is_some(),
        ),
        separator(),
        enabled(
            key_item("Delete…", "\u{8}", CMD, {
                let all = all.clone();
                move || list_command(sel!(deleteToBeginningOfLine:), || app().ask_delete(&all))
            }),
            first.is_some(),
        ),
        item("Delete Completed", true, || app().delete_completed()),
        separator(),
        item("Start Main Queue", true, || app().start_queue("main")),
        item("Stop Main Queue", true, || app().stop_queue("main")),
        key_item("Scheduler…", "j", CMD, || app().open_scheduler()),
        separator(),
        checked(key_item("Speed Limiter", "l", NSEventModifierFlags::Command | NSEventModifierFlags::Option, || app().toggle_limit()), limit_on),
    ];
    for i in &list {
        menu.addItem(i);
    }
}

fn top(bar: &NSMenu, menu: &NSMenu) {
    let holder = unsafe { NSMenuItem::new(mtm()) };
    unsafe { holder.setSubmenu(Some(menu)) };
    bar.addItem(&holder);
}

pub fn install_main_menu() {
    let nsapp = NSApplication::sharedApplication(mtm());
    let bar = kit::menu("");

    let app_menu = standard_menu(
        APP_NAME,
        vec![
            responder_item(&format!("About {APP_NAME}"), sel!(orderFrontStandardAboutPanel:), ""),
            separator(),
            key_item("Settings…", ",", CMD, || app().open_settings()),
            separator(),
            responder_item(&format!("Hide {APP_NAME}"), sel!(hide:), "h"),
            with_mask(responder_item("Hide Others", sel!(hideOtherApplications:), "h"), NSEventModifierFlags::Command | NSEventModifierFlags::Option),
            responder_item("Show All", sel!(unhideAllApplications:), ""),
            separator(),
            responder_item(&format!("Quit {APP_NAME}"), sel!(terminate:), "q"),
        ],
    );
    top(&bar, &app_menu);

    let file = standard_menu(
        "File",
        vec![
            key_item("Add URL…", "n", CMD, || app().open_add(json!({}))),
            key_item("Add Batch Download…", "n", shift_cmd(), || app().open_add(json!({ "batch": true }))),
            key_item("Site Grabber…", "g", shift_cmd(), || app().open_grabber(None)),
            separator(),
            item("Import Link List…", true, || app().import_list()),
            item("Export Link List…", true, || app().export_list()),
            separator(),
            responder_item("Close Window", sel!(performClose:), "w"),
        ],
    );
    top(&bar, &file);

    let edit = standard_menu(
        "Edit",
        vec![
            responder_item("Undo", sel!(undo:), "z"),
            with_mask(responder_item("Redo", sel!(redo:), "z"), shift_cmd()),
            separator(),
            responder_item("Cut", sel!(cut:), "x"),
            responder_item("Copy", sel!(copy:), "c"),
            responder_item("Paste", sel!(paste:), "v"),
            responder_item("Delete", sel!(delete:), ""),
            responder_item("Select All", sel!(selectAll:), "a"),
        ],
    );
    top(&bar, &edit);

    let downloads = kit::dynamic_menu("Downloads", fill_downloads);
    top(&bar, &downloads);

    let window = standard_menu(
        "Window",
        vec![
            responder_item("Minimize", sel!(performMiniaturize:), "m"),
            responder_item("Zoom", sel!(performZoom:), ""),
            separator(),
            key_item(APP_NAME, "0", CMD, || app().open_main()),
            separator(),
            responder_item("Bring All to Front", sel!(arrangeInFront:), ""),
        ],
    );
    top(&bar, &window);

    nsapp.setMainMenu(Some(&bar));
    nsapp.setWindowsMenu(Some(&window));
}

/// The icon in the menu bar, with the app's quick actions.
pub struct StatusItem {
    item: Retained<NSStatusItem>,
}

impl StatusItem {
    pub fn new() -> Self {
        let item = unsafe { NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength) };
        let menu = kit::dynamic_menu("", fill_status);
        unsafe { item.setMenu(Some(&menu)) };
        let this = StatusItem { item };
        this.update("idle", None);
        this
    }

    /// Show a state ("idle", "downloading", …) and, when asked for, the
    /// current speed next to it.
    pub fn update(&self, state: &str, speed: Option<&str>) {
        let Some(button) = (unsafe { self.item.button(mtm()) }) else { return };
        unsafe {
            button.setImage(Some(&crate::system::tray_image(state)));
            button.setImagePosition(if speed.is_some() { NSCellImagePosition::ImageLeading } else { NSCellImagePosition::ImageOnly });
            let title: Retained<NSString> = ns(speed.unwrap_or(""));
            button.setTitle(&title);
            button.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(NSFont::systemFontSize(), NSFontWeightRegular)));
        }
    }
}

fn fill_status(menu: &NSMenu) {
    let app = app();
    let (line, limit_on) = {
        let st = app.store.borrow();
        let line = if n(&st.totals, "active") > 0 { st.activity_line() } else { "No active downloads".to_string() };
        (line, b(&st.settings, "globalLimitEnabled"))
    };
    let mut list: Vec<Retained<NSMenuItem>> = vec![];
    if let Some(e) = &*app.start_error.borrow() {
        list.push(disabled_item(&format!("Engine error: {e}")));
    }
    list.extend([
        disabled_item(&line),
        separator(),
        item(&format!("Show {APP_NAME}"), true, || crate::app::app().open_main()),
        item("Add URL…", true, || crate::app::app().open_add(json!({}))),
        item("Site Grabber…", true, || crate::app::app().open_grabber(None)),
        separator(),
        item("Start Main Queue", true, || crate::app::app().start_queue("main")),
        item("Pause All", true, || crate::app::app().pause_all()),
        checked(item("Speed Limiter", true, || crate::app::app().toggle_limit()), limit_on),
        separator(),
        item("Scheduler…", true, || crate::app::app().open_scheduler()),
        item("Settings…", true, || crate::app::app().open_settings()),
        separator(),
        item(&format!("Quit {APP_NAME}"), true, || crate::app::app().quit()),
    ]);
    for i in &list {
        menu.addItem(i);
    }
}

/// Build the menus that are filled when they open (see `ODM_DO`).
#[cfg(debug_assertions)]
pub fn debug_menus() {
    let titles = |m: &NSMenu| unsafe { m.itemArray() }.iter().map(|i| format!("{}{}", unsafe { i.title() }, if unsafe { i.isEnabled() } { "" } else { " (off)" })).collect::<Vec<_>>();
    let m = kit::menu("");
    fill_downloads(&m);
    eprintln!("downloads menu: {:?}", titles(&m));
    let m = kit::menu("");
    fill_status(&m);
    eprintln!("status menu: {:?}", titles(&m));
}
