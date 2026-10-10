//! What the app asks of Windows itself: one instance per user, waking the
//! UI thread from other threads, launch at sign-in, staying awake, power
//! actions, opening files, the clipboard, pickers and notifications.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Mutex;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::{ReadFile, PIPE_ACCESS_INBOUND};
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::System::Pipes::*;
use windows_sys::Win32::System::Power::{SetSuspendState, SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const APP_ID: &str = "app.opendm.desktop";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// `CF_UNICODETEXT`
const UNICODE_TEXT: u32 = 13;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn pipe_name() -> String {
    let user: String = std::env::var("USERNAME").unwrap_or_default().chars().filter(|c| c.is_alphanumeric()).collect();
    format!(r"\\.\pipe\OpenDownloadManager.Open.{user}")
}

// ------------------------------------------------------- startup failures

/// Where the reason of a failed start is written.
pub fn error_log() -> std::path::PathBuf {
    crate::engine::data_dir().join("startup-error.log")
}

/// The app could not start (or hit an internal error): keep the reason in
/// [`error_log`] and tell the user. `ODM_NO_DIALOG` (set by automated runs)
/// skips the message box.
pub fn startup_failure(message: &str) {
    let log = error_log();
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&log, message);
    if std::env::var_os("ODM_NO_DIALOG").is_none() {
        let text = wide(&format!("Open Download Manager could not start.\n\n{message}"));
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), wide(odm_ui::APP_NAME).as_ptr(), MB_OK | MB_ICONERROR) };
    }
}

// ------------------------------------------------------------ one instance

/// Become the app's one instance. Returns false when another instance was
/// running; it has been handed `args` (argv without the program name), so
/// exit. Later launches reach this one as [`Posted::Args`].
pub fn single_instance(args: &[String]) -> bool {
    // the handle is kept for the life of the process on purpose
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 1, wide(r"Local\OpenDownloadManager.Instance").as_ptr()) };
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if mutex.is_null() || !already {
        let _ = std::thread::Builder::new().name("odm-instance".into()).spawn(listen);
        return true;
    }
    // the first instance may still be starting its listener
    for _ in 0..30 {
        if let Ok(mut pipe) = std::fs::OpenOptions::new().write(true).open(pipe_name()) {
            let _ = pipe.write_all(args.join("\n").as_bytes());
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

fn listen() {
    let name = wide(&pipe_name());
    loop {
        // inbound only; the default security lets only this user write
        let pipe = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                0,
                4096,
                0,
                std::ptr::null(),
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            std::thread::sleep(std::time::Duration::from_secs(1));
            continue;
        }
        let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if connected {
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let mut got = 0u32;
                let ok = unsafe { ReadFile(pipe, buf.as_mut_ptr(), buf.len() as u32, &mut got, std::ptr::null_mut()) };
                if ok == 0 || got == 0 || data.len() > 1 << 20 {
                    break;
                }
                data.extend_from_slice(&buf[..got as usize]);
            }
            let text = String::from_utf8_lossy(&data);
            post(Posted::Args(text.lines().map(String::from).collect()));
        }
        unsafe {
            DisconnectNamedPipe(pipe);
            CloseHandle(pipe);
        }
    }
}

// ----------------------------------------------- waking the UI thread

/// Something another thread hands to the UI thread.
pub enum Posted {
    /// an engine event: (name, JSON)
    Engine(String, String),
    /// the arguments of a later launch
    Args(Vec<String>),
    /// something went wrong off the UI thread: tell the user
    Error(String),
    /// once a second
    Tick,
}

const WM_ODM_WAKE: u32 = WM_APP + 41;
static QUEUE: Mutex<VecDeque<Posted>> = Mutex::new(VecDeque::new());
static PUMP_WINDOW: AtomicIsize = AtomicIsize::new(0);

thread_local! {
    static SINK: RefCell<Option<Box<dyn Fn(Posted)>>> = const { RefCell::new(None) };
}

/// Hand `p` to the UI thread (from any thread).
pub fn post(p: Posted) {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push_back(p);
    let hwnd = PUMP_WINDOW.load(Ordering::SeqCst);
    if hwnd != 0 {
        unsafe { PostMessageW(hwnd as HWND, WM_ODM_WAKE, 0, 0) };
    }
}

fn drain() {
    loop {
        // taken one at a time so the sink runs without the lock
        let next = QUEUE.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
        let Some(p) = next else { break };
        SINK.with(|s| {
            if let Some(sink) = s.borrow().as_ref() {
                sink(p);
            }
        });
    }
}

unsafe extern "system" fn pump_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ODM_WAKE => {
            drain();
            0
        }
        WM_TIMER => {
            SINK.with(|s| {
                if let Some(sink) = s.borrow().as_ref() {
                    sink(Posted::Tick);
                }
            });
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Call on the UI thread, once: `sink` then receives everything posted
/// (and a tick every second) on this thread, inside its message loop.
pub fn install_pump(sink: impl Fn(Posted) + 'static) {
    SINK.with(|s| *s.borrow_mut() = Some(Box::new(sink)));
    let class = wide("OpenDownloadManager.Pump");
    unsafe {
        let wc = WNDCLASSW { lpfnWndProc: Some(pump_proc), lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        // a message-only window: never shown, only receives our messages
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        );
        PUMP_WINDOW.store(hwnd as isize, Ordering::SeqCst);
        SetTimer(hwnd, 1, 1000, None);
        // events that arrived before the window existed
        PostMessageW(hwnd, WM_ODM_WAKE, 0, 0);
    }
}

/// Can a notification-area icon be added right now? Not while Explorer is
/// restarting, and not in sessions without a working shell. Tried for real
/// (add, then remove, a throwaway icon), because a taskbar window existing
/// doesn't mean it accepts icons, and a failed add would end the app.
pub fn tray_available() -> bool {
    use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIF_MESSAGE, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW};
    let hwnd = PUMP_WINDOW.load(Ordering::SeqCst) as HWND;
    if hwnd.is_null() || unsafe { FindWindowW(wide("Shell_TrayWnd").as_ptr(), std::ptr::null()) }.is_null() {
        return false;
    }
    unsafe {
        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = 0x0D4D;
        data.uFlags = NIF_MESSAGE;
        data.uCallbackMessage = WM_APP + 42;
        let added = Shell_NotifyIconW(NIM_ADD, &data) != 0;
        Shell_NotifyIconW(NIM_DELETE, &data);
        added
    }
}

// ------------------------------------------------------------ context menu

/// One entry of a context menu.
pub enum PopupItem {
    Item { key: String, label: String, enabled: bool },
    Separator,
    Submenu { label: String, items: Vec<PopupItem> },
}

impl PopupItem {
    pub fn item(key: impl Into<String>, label: impl Into<String>, enabled: bool) -> Self {
        PopupItem::Item { key: key.into(), label: label.into(), enabled }
    }
}

/// Fill a Win32 menu; `keys[id - 1]` is the key of the item with that id.
unsafe fn fill_menu(menu: HMENU, items: &[PopupItem], keys: &mut Vec<String>) {
    for item in items {
        match item {
            PopupItem::Separator => {
                unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null()) };
            }
            PopupItem::Item { key, label, enabled } => {
                keys.push(key.clone());
                let flags = if *enabled { MF_STRING } else { MF_STRING | MF_GRAYED };
                unsafe { AppendMenuW(menu, flags, keys.len(), wide(label).as_ptr()) };
            }
            PopupItem::Submenu { label, items } => unsafe {
                let sub = CreatePopupMenu();
                fill_menu(sub, items, keys);
                // the parent menu owns (and destroys) the submenu
                AppendMenuW(menu, MF_POPUP, sub as usize, wide(label).as_ptr());
            },
        }
    }
}

/// Show a context menu at the pointer and wait for the choice: the key of
/// the chosen item, or None. `window` is the window the menu belongs to.
pub fn popup_menu(window: *mut c_void, items: &[PopupItem]) -> Option<String> {
    let mut keys = vec![];
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return None;
        }
        fill_menu(menu, items, &mut keys);
        let mut at = POINT { x: 0, y: 0 };
        GetCursorPos(&mut at);
        // the menu closes on a click elsewhere only when its owner is in front
        SetForegroundWindow(window as HWND);
        let chosen = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD, at.x, at.y, 0, window as HWND, std::ptr::null());
        DestroyMenu(menu);
        keys.get((chosen as usize).wrapping_sub(1)).cloned()
    }
}

// -------------------------------------------------------- sign-in, power

fn exe_command() -> Option<String> {
    Some(format!("\"{}\" --minimized", std::env::current_exe().ok()?.display()))
}

pub fn autostart_enabled() -> bool {
    windows_registry::CURRENT_USER.open(RUN_KEY).and_then(|k| k.get_string(odm_ui::APP_NAME)).is_ok()
}

/// Add or remove the "start when I sign in" entry.
pub fn set_autostart(on: bool) -> Result<(), String> {
    let key = windows_registry::CURRENT_USER.create(RUN_KEY).map_err(|e| format!("Could not change the startup setting: {e}"))?;
    if on {
        let command = exe_command().ok_or("Could not find the app's own path.")?;
        key.set_string(odm_ui::APP_NAME, &command).map_err(|e| format!("Could not change the startup setting: {e}"))
    } else {
        match key.remove_value(odm_ui::APP_NAME) {
            Ok(()) => Ok(()),
            Err(_) if !autostart_enabled() => Ok(()),
            Err(e) => Err(format!("Could not change the startup setting: {e}")),
        }
    }
}

static AWAKE: AtomicU32 = AtomicU32::new(0);

/// Keep the system from idle-sleeping while downloads run. Call on the UI
/// thread (the state belongs to the calling thread).
pub fn keep_awake(on: bool) {
    if AWAKE.swap(on as u32, Ordering::SeqCst) != on as u32 {
        unsafe { SetThreadExecutionState(if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS }) };
    }
}

pub fn sleep() -> Result<(), String> {
    if !unsafe { SetSuspendState(false, false, false) } {
        return Err("Windows refused to sleep.".into());
    }
    Ok(())
}

pub fn shut_down() -> Result<(), String> {
    std::process::Command::new("shutdown.exe").args(["/s", "/t", "0"]).spawn().map(|_| ()).map_err(|e| format!("Could not shut down: {e}"))
}

// ------------------------------------------------------------ files, links

fn shell_open(target: &str, parameters: Option<&str>) -> Result<(), String> {
    let target_w = wide(target);
    let params_w = parameters.map(wide);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            wide("open").as_ptr(),
            target_w.as_ptr(),
            params_w.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute reports success as a value above 32
    if result as isize > 32 {
        Ok(())
    } else {
        Err(format!("Could not open {target}."))
    }
}

/// Open a file or folder with its default app.
pub fn open_path(p: &Path) -> Result<(), String> {
    if !p.exists() {
        return Err("The file is no longer there. It may have been moved or deleted.".into());
    }
    shell_open(&p.to_string_lossy(), None)
}

/// Show a file selected in File Explorer.
pub fn reveal(p: &Path) -> Result<(), String> {
    if p.is_dir() || !p.exists() {
        let dir = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
        return shell_open(&dir.to_string_lossy(), None);
    }
    shell_open("explorer.exe", Some(&format!("/select,\"{}\"", p.display())))
}

/// Show a download: the finished file, else its part file, else the folder.
pub fn reveal_download(d: &serde_json::Value) -> Result<(), String> {
    let full = Path::new(odm_ui::s(d, "saveDir")).join(odm_ui::s(d, "fileName"));
    let part = std::path::PathBuf::from(format!("{}.odmpart", full.display()));
    if full.exists() {
        reveal(&full)
    } else if part.exists() {
        reveal(&part)
    } else {
        reveal(Path::new(odm_ui::s(d, "saveDir")))
    }
}

/// The folder with the bundled browser extension, if it is there.
pub fn extension_dir() -> Option<std::path::PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.join("extension");
    dir.join("chromium").is_dir().then_some(dir)
}

/// A file that ships beside the executable (icons).
pub fn asset(name: &str) -> String {
    let dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)).unwrap_or_default();
    dir.join("Assets").join(name).to_string_lossy().into_owned()
}

// ---------------------------------------------------------------- clipboard

/// Changes whenever the clipboard's contents do.
pub fn clipboard_sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

pub fn clipboard_text() -> Option<String> {
    unsafe {
        if IsClipboardFormatAvailable(UNICODE_TEXT) == 0 || OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let handle = GetClipboardData(UNICODE_TEXT);
        let mut text = None;
        if !handle.is_null() {
            let data = GlobalLock(handle as *mut c_void) as *const u16;
            if !data.is_null() {
                let mut len = 0;
                // bounded: nothing we care about is longer than this
                while len < 1 << 16 && *data.add(len) != 0 {
                    len += 1;
                }
                text = Some(String::from_utf16_lossy(std::slice::from_raw_parts(data, len)));
                GlobalUnlock(handle as *mut c_void);
            }
        }
        CloseClipboard();
        text
    }
}

pub fn set_clipboard_text(text: &str) {
    let data = wide(text);
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return;
        }
        EmptyClipboard();
        let handle = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2);
        if !handle.is_null() {
            let target = GlobalLock(handle) as *mut u16;
            if !target.is_null() {
                std::ptr::copy_nonoverlapping(data.as_ptr(), target, data.len());
                GlobalUnlock(handle);
                // the clipboard owns the memory from here on
                SetClipboardData(UNICODE_TEXT, handle as HANDLE);
            }
        }
        CloseClipboard();
    }
}

// ------------------------------------------------------------------ pickers

pub fn pick_folder(start: &str) -> Option<String> {
    let mut dialog = rfd::FileDialog::new().set_title("Choose a Folder");
    if Path::new(start).is_dir() {
        dialog = dialog.set_directory(start);
    }
    dialog.pick_folder().map(|p| p.to_string_lossy().into_owned())
}

pub fn pick_text_file() -> Option<String> {
    rfd::FileDialog::new()
        .set_title("Import Link List")
        .add_filter("Text files", &["txt", "csv"])
        .add_filter("All files", &["*"])
        .pick_file()
        .map(|p| p.to_string_lossy().into_owned())
}

pub fn pick_save_file(name: &str) -> Option<String> {
    rfd::FileDialog::new()
        .set_title("Export Link List")
        .set_file_name(name)
        .add_filter("Text files", &["txt"])
        .save_file()
        .map(|p| p.to_string_lossy().into_owned())
}

// ------------------------------------------------------------ notifications

/// Show a toast. Needs the Start menu shortcut the installer creates (it
/// carries the app's id); without it Windows drops the toast, which is all
/// that happens.
pub fn notify(title: &str, body: &str) {
    let _ = tauri_winrt_notification::Toast::new(APP_ID).title(title).text1(body).show();
}
