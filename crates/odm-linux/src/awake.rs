//! Keep the system from idle-sleeping while downloads run, via a logind
//! sleep/idle inhibitor (released automatically if the process dies).
use parking_lot::Mutex;
use zbus::zvariant::OwnedFd;

pub struct Awake {
    /// the inhibitor lock: logind releases it when the fd is closed
    held: Mutex<Option<OwnedFd>>,
}

fn inhibit(what: &str) -> Result<OwnedFd, zbus::Error> {
    let conn = zbus::blocking::Connection::system()?;
    let reply = conn.call_method(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1",
        Some("org.freedesktop.login1.Manager"),
        "Inhibit",
        &(what, crate::APP_NAME, "Downloading files", "block"),
    )?;
    reply.body().deserialize::<OwnedFd>()
}

impl Awake {
    pub fn new() -> Self {
        Awake { held: Mutex::new(None) }
    }

    /// Hold (true) or release (false) the inhibitor. Cheap to call often.
    pub fn set(&self, on: bool) {
        let mut held = self.held.lock();
        if !on {
            *held = None;
        } else if held.is_none() {
            // blocking sleep may need more rights than blocking idle
            *held = inhibit("sleep:idle").or_else(|_| inhibit("idle")).ok();
        }
    }
}

impl Default for Awake {
    fn default() -> Self {
        Self::new()
    }
}
