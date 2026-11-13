//! What the app asks of macOS: notifications, the login item, keeping the
//! Mac awake, sleep and shutdown, the clipboard, Finder, and icons.
use crate::kit::{mtm, ns};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, MainThreadOnly, Message};
use objc2_app_kit::*;
use objc2_foundation::*;
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use objc2_uniform_type_identifiers::UTType;
use objc2_user_notifications::*;
use odm_ui::s;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A GUI app has no console: what went wrong goes to the system log, to
/// stderr, and to `startup-error.log` in the data folder.
pub fn record_failure(message: &str) {
    eprintln!("Open Download Manager: {message}");
    let dir = crate::engine::data_dir();
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    // private to the user, like the rest of the data folder
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir);
    let log = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(dir.join("startup-error.log"));
    if let Ok(mut log) = log {
        let _ = writeln!(log, "{message}");
    }
}

/// Running from an app bundle (notifications and the login item need one).
fn bundled() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some()
}

/// A file inside the app bundle's Resources folder.
pub fn resource(name: &str) -> Option<PathBuf> {
    let dir = NSBundle::mainBundle().resourcePath()?.to_string();
    let p = Path::new(&dir).join(name);
    p.exists().then_some(p)
}

// ---------------------------------------------------------------------------
// notifications

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ODMNotificationDelegate"]
    struct NotificationDelegate;

    unsafe impl NSObjectProtocol for NotificationDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
        /// Show banners while the app is frontmost too (the default is to
        /// drop them silently).
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            done: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            done.call((UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::Sound | UNNotificationPresentationOptions::List,));
        }
    }
);

thread_local! {
    static NOTIFICATION_DELEGATE: RefCell<Option<Retained<NotificationDelegate>>> = const { RefCell::new(None) };
    static NOTIFICATION_SERIAL: RefCell<u64> = const { RefCell::new(0) };
}

/// Ask for permission to notify and show banners while frontmost.
pub fn start_notifications() {
    if !bundled() {
        return;
    }
    let delegate: Retained<NotificationDelegate> = unsafe { msg_send![NotificationDelegate::alloc(mtm()), init] };
    let center = unsafe { UNUserNotificationCenter::currentNotificationCenter() };
    unsafe { center.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
    NOTIFICATION_DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
    let done = RcBlock::new(|_granted: Bool, _error: *mut NSError| {});
    unsafe {
        center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &done);
    }
}

pub fn notify(title: &str, body: &str) {
    if !bundled() {
        return;
    }
    let serial = NOTIFICATION_SERIAL.with(|n| {
        *n.borrow_mut() += 1;
        *n.borrow()
    });
    unsafe {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&ns(title));
        content.setBody(&ns(body));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&ns(&format!("odm-{serial}")), &content, None);
        UNUserNotificationCenter::currentNotificationCenter().addNotificationRequest_withCompletionHandler(&request, None);
    }
}

// ---------------------------------------------------------------------------
// login item

/// Registered as a login item (enabled, or waiting for the user's approval:
/// it is listed and runs once allowed).
pub fn autostart_enabled() -> bool {
    if !bundled() {
        return false;
    }
    let status = unsafe { SMAppService::mainAppService().status() };
    status == SMAppServiceStatus::Enabled || status == SMAppServiceStatus::RequiresApproval
}

/// Register or unregister the login item. Returns what the system has now
/// and, when the request did not simply work, something to tell the user.
pub fn set_autostart(on: bool) -> (bool, Option<String>) {
    const WHERE: &str = "System Settings › General › Login Items";
    if !bundled() {
        return (false, on.then(|| "Opening at login needs the installed app.".to_string()));
    }
    let service = unsafe { SMAppService::mainAppService() };
    let status = unsafe { service.status() };
    let mut failure = String::new();
    let result = unsafe {
        if on && status != SMAppServiceStatus::Enabled {
            service.registerAndReturnError()
        } else if !on && (status == SMAppServiceStatus::Enabled || status == SMAppServiceStatus::RequiresApproval) {
            service.unregisterAndReturnError()
        } else {
            Ok(())
        }
    };
    if let Err(e) = result {
        failure = format!(" ({})", e.localizedDescription());
    }
    let now = unsafe { service.status() };
    if now == SMAppServiceStatus::Enabled {
        if on {
            (true, None)
        } else {
            (true, Some(format!("Open Download Manager could not be removed from your login items{failure}. You can remove it in {WHERE}.")))
        }
    } else if now == SMAppServiceStatus::RequiresApproval {
        if on {
            unsafe { SMAppService::openSystemSettingsLoginItems() };
            (true, Some(format!("To open Open Download Manager at login, allow it in {WHERE}.")))
        } else {
            (true, Some(format!("Open Download Manager is still listed in your login items. You can remove it in {WHERE}.")))
        }
    } else if on {
        (false, Some(format!("Open Download Manager could not be added to your login items{failure}. You can add it in {WHERE}.")))
    } else {
        (false, None)
    }
}

// ---------------------------------------------------------------------------
// power

thread_local! {
    static AWAKE: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>> = const { RefCell::new(None) };
}

/// Hold off idle sleep while anything is downloading.
pub fn keep_awake(on: bool) {
    AWAKE.with(|a| {
        let mut a = a.borrow_mut();
        let info = NSProcessInfo::processInfo();
        if on && a.is_none() {
            let options = NSActivityOptions::IdleSystemSleepDisabled | NSActivityOptions::UserInitiated;
            *a = Some(unsafe { info.beginActivityWithOptions_reason(options, &ns("Downloading files")) });
        } else if !on {
            if let Some(token) = a.take() {
                unsafe { info.endActivity(&token) };
            }
        }
    });
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    std::process::Command::new(program).args(args).spawn().map(|_| ()).map_err(|e| format!("{program}: {e}"))
}

pub fn sleep() -> Result<(), String> {
    run("/usr/bin/pmset", &["sleepnow"])
}

pub fn shut_down() -> Result<(), String> {
    run("/usr/bin/osascript", &["-e", "tell application \"System Events\" to shut down"])
}

// ---------------------------------------------------------------------------
// clipboard

pub fn clipboard_count() -> isize {
    unsafe { NSPasteboard::generalPasteboard().changeCount() }
}

pub fn clipboard_text() -> Option<String> {
    unsafe { NSPasteboard::generalPasteboard().stringForType(NSPasteboardTypeString) }.map(|s| s.to_string())
}

pub fn copy(text: &str) {
    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        pb.setString_forType(&ns(text), NSPasteboardTypeString);
    }
}

/// Copy something clipboard managers should not keep in their history.
pub fn copy_secret(text: &str) {
    copy(text);
    unsafe { NSPasteboard::generalPasteboard().setString_forType(&ns(""), &ns("org.nspasteboard.ConcealedType")) };
}

// ---------------------------------------------------------------------------
// Finder

fn file_url(path: &Path, directory: bool) -> Retained<NSURL> {
    unsafe { NSURL::fileURLWithPath_isDirectory(&ns(&path.to_string_lossy()), directory) }
}

pub fn open_path(path: &Path) -> bool {
    unsafe { NSWorkspace::sharedWorkspace().openURL(&file_url(path, path.is_dir())) }
}

pub fn reveal(path: &Path) {
    let urls = NSArray::from_retained_slice(&[file_url(path, false)]);
    unsafe { NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&urls) };
}

pub fn download_path(d: &Value) -> PathBuf {
    Path::new(s(d, "saveDir")).join(s(d, "fileName"))
}

/// Open a finished download. False when the file is gone.
pub fn open_download(d: &Value) -> bool {
    let path = download_path(d);
    path.exists() && open_path(&path)
}

/// Show a download in Finder: the file, its unfinished part, or at least
/// its folder.
pub fn reveal_download(d: &Value) {
    let full = download_path(d);
    let mut part = full.clone().into_os_string();
    part.push(".odmpart");
    let part = PathBuf::from(part);
    if full.exists() {
        reveal(&full);
    } else if part.exists() {
        reveal(&part);
    } else {
        open_path(Path::new(s(d, "saveDir")));
    }
}

// ---------------------------------------------------------------------------
// icons

thread_local! {
    static ICONS: RefCell<HashMap<String, Retained<NSImage>>> = RefCell::new(HashMap::new());
    static TRAY: RefCell<HashMap<String, Retained<NSImage>>> = RefCell::new(HashMap::new());
}

/// The icon Finder shows for a file type.
pub fn file_icon(extension: &str) -> Retained<NSImage> {
    let key = extension.to_lowercase();
    ICONS.with(|cache| {
        if let Some(i) = cache.borrow().get(&key) {
            return i.clone();
        }
        let kind = unsafe { UTType::typeWithFilenameExtension(&ns(&key)) }.unwrap_or_else(|| unsafe { objc2_uniform_type_identifiers::UTTypeData }.retain());
        let image: Retained<NSImage> = unsafe { msg_send![&*NSWorkspace::sharedWorkspace(), iconForContentType: &*kind] };
        cache.borrow_mut().insert(key, image.clone());
        image
    })
}

/// The menu-bar icon for a state ("idle", "downloading", …), as a template
/// image that follows the menu bar's appearance.
pub fn tray_image(state: &str) -> Retained<NSImage> {
    TRAY.with(|cache| {
        if let Some(i) = cache.borrow().get(state) {
            return i.clone();
        }
        let bundled = resource(&format!("tray-{state}Template.png")).and_then(|p| unsafe { NSImage::initWithContentsOfFile(NSImage::alloc(), &ns(&p.to_string_lossy())) });
        let image = match bundled {
            Some(i) => {
                unsafe { i.setSize(NSSize::new(18.0, 18.0)) };
                i
            }
            None => crate::kit::symbol("arrow.down.circle", odm_ui::APP_NAME).unwrap_or_else(|| unsafe { NSImage::new() }),
        };
        unsafe { image.setTemplate(true) };
        cache.borrow_mut().insert(state.to_string(), image.clone());
        image
    })
}
