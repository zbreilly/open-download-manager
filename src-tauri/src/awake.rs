//! Keep the system from sleeping while downloads are active.
//!
//! * Windows: `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`
//!   on a dedicated thread (the state belongs to the calling thread).
//! * Linux: a `systemd-inhibit --what=sleep --mode=block` child process.
//! * macOS: a `caffeinate -i` child process.
//!
//! The helper processes watch this process and exit on their own if the app
//! dies without releasing them. Missing tools are ignored.

use parking_lot::Mutex;

pub struct Awake {
    on: Mutex<bool>,
    #[cfg(unix)]
    child: Mutex<Option<std::process::Child>>,
    #[cfg(windows)]
    tx: Option<std::sync::mpsc::Sender<bool>>,
}

impl Default for Awake {
    fn default() -> Self {
        Self::new()
    }
}

impl Awake {
    pub fn new() -> Self {
        Self {
            on: Mutex::new(false),
            #[cfg(unix)]
            child: Mutex::new(None),
            #[cfg(windows)]
            tx: win::spawn_worker(),
        }
    }

    /// Hold (true) or release (false) the keep-awake request. Cheap to call
    /// repeatedly with the same value.
    pub fn set(&self, want: bool) {
        let mut on = self.on.lock();
        if *on == want {
            // reap a helper that could not hold the lock (no logind, ...)
            #[cfg(unix)]
            if want {
                let mut child = self.child.lock();
                if matches!(child.as_mut().map(|c| c.try_wait()), Some(Ok(Some(_)))) {
                    *child = None;
                }
            }
            return;
        }
        *on = want;
        #[cfg(windows)]
        if let Some(tx) = &self.tx {
            let _ = tx.send(want);
        }
        #[cfg(unix)]
        {
            let mut child = self.child.lock();
            if let Some(c) = child.take() {
                kill_group(c);
            }
            if want {
                *child = spawn_inhibitor();
            }
        }
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        self.set(false);
    }
}

#[cfg(unix)]
fn kill_group(mut c: std::process::Child) {
    // the helper runs in its own process group: take its children too
    let pid = c.id() as i32;
    // SAFETY: plain kill(2) on a process group we created
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    let _ = c.kill();
    let _ = c.wait();
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_inhibitor() -> Option<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let me = std::process::id().to_string();
    // the inhibitor lives as long as its child; the child exits by itself
    // when this process is gone, so a crash never leaves the lock behind
    let r = std::process::Command::new("systemd-inhibit")
        .args([
            "--what=sleep",
            "--who=Open Download Manager",
            "--why=Downloads in progress",
            "--mode=block",
            "sh",
            "-c",
            "while kill -0 \"$0\" 2>/dev/null; do sleep 10; done",
            &me,
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn();
    match r {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::debug!("keep-awake unavailable (systemd-inhibit): {e}");
            None
        }
    }
}

#[cfg(target_os = "macos")]
fn spawn_inhibitor() -> Option<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let me = std::process::id().to_string();
    let r = std::process::Command::new("/usr/bin/caffeinate")
        .args(["-i", "-w", &me])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn();
    match r {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::debug!("keep-awake unavailable (caffeinate): {e}");
            None
        }
    }
}

#[cfg(windows)]
mod win {
    use std::sync::mpsc;
    use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};

    pub fn spawn_worker() -> Option<mpsc::Sender<bool>> {
        let (tx, rx) = mpsc::channel::<bool>();
        std::thread::Builder::new()
            .name("odm-awake".into())
            .spawn(move || {
                for on in rx {
                    let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS };
                    // SAFETY: no pointers involved; affects this thread only
                    unsafe {
                        SetThreadExecutionState(flags);
                    }
                }
            })
            .ok()?;
        Some(tx)
    }
}
