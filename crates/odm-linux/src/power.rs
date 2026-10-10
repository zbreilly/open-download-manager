//! "When the queue finishes": sleep or shut down, through logind (polkit
//! decides; a normal desktop user is allowed). The 60-second countdown with
//! a Cancel button is the front end's job; call this when it runs out.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Sleep,
    Shutdown,
}

impl PowerAction {
    /// From the queue's `afterAction` value ("sleep", "shutdown"; others: None).
    pub fn from_setting(s: &str) -> Option<Self> {
        match s {
            "sleep" => Some(Self::Sleep),
            "shutdown" => Some(Self::Shutdown),
            _ => None,
        }
    }
}

pub fn run(action: PowerAction) -> Result<(), String> {
    let method = match action {
        PowerAction::Sleep => "Suspend",
        PowerAction::Shutdown => "PowerOff",
    };
    let conn = zbus::blocking::Connection::system().map_err(|e| format!("system bus: {e}"))?;
    conn.call_method(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1",
        Some("org.freedesktop.login1.Manager"),
        method,
        // interactive = false: never pop up a password prompt
        &(false,),
    )
    .map(|_| ())
    .map_err(|e| format!("{method}: {e}"))
}
