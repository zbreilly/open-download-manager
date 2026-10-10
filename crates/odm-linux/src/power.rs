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
    let _ = action;
    Err("not implemented yet".into()) // STUB
}
