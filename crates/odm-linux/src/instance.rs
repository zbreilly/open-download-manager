//! One running instance per user session. A second launch (e.g. clicking an
//! `odm://` link or the launcher again) forwards its arguments to the first
//! one and exits.
//!
//! Implemented over D-Bus: the primary instance owns the session-bus name
//! [`crate::APP_ID`] and serves `app.opendm.Desktop.Open(as args)`.

/// Keeps the primary instance registered while alive.
pub struct Instance {
    _private: (),
}

pub enum Acquired {
    /// this process is the primary instance; keep `Instance` alive
    Primary(Instance),
    /// another instance was running and received `args`; exit now
    Forwarded,
}

/// Become the primary instance, or forward `args` (argv without the program
/// name) to the running one. `on_args` is called (on a background thread)
/// with the arguments of every later launch.
pub fn acquire(args: Vec<String>, on_args: impl Fn(Vec<String>) + Send + Sync + 'static) -> Result<Acquired, String> {
    let _ = (args, on_args);
    // STUB: replaced by the D-Bus implementation
    Ok(Acquired::Primary(Instance { _private: () }))
}
