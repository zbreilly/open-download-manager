//! One running instance per user session. A second launch (e.g. clicking an
//! `odm://` link or the launcher again) forwards its arguments to the first
//! one and exits.
//!
//! Implemented over D-Bus: the primary instance owns the session-bus name
//! [`crate::APP_ID`] and serves `app.opendm.Desktop.Open(as args)`.
use zbus::blocking::Connection;
use zbus::fdo::{RequestNameFlags, RequestNameReply};

const PATH: &str = "/app/opendm/Desktop";

/// Keeps the primary instance registered while alive.
pub struct Instance {
    _conn: Option<Connection>,
}

pub enum Acquired {
    /// this process is the primary instance; keep `Instance` alive
    Primary(Instance),
    /// another instance was running and received `args`; exit now
    Forwarded,
}

struct Service {
    on_args: Box<dyn Fn(Vec<String>) + Send + Sync>,
}

#[zbus::interface(name = "app.opendm.Desktop")]
impl Service {
    fn open(&self, args: Vec<String>) {
        (self.on_args)(args);
    }
}

/// Become the primary instance, or forward `args` (argv without the program
/// name) to the running one. `on_args` is called (on a background thread)
/// with the arguments of every later launch.
pub fn acquire(args: Vec<String>, on_args: impl Fn(Vec<String>) + Send + Sync + 'static) -> Result<Acquired, String> {
    // without a session bus there is nothing to coordinate through
    let Ok(conn) = Connection::session() else {
        return Ok(Acquired::Primary(Instance { _conn: None }));
    };
    conn.object_server()
        .at(PATH, Service { on_args: Box::new(on_args) })
        .map_err(|e| format!("d-bus: {e}"))?;
    let taken = match conn.request_name_with_flags(crate::APP_ID, RequestNameFlags::DoNotQueue.into()) {
        Ok(RequestNameReply::PrimaryOwner) | Ok(RequestNameReply::AlreadyOwner) => false,
        Ok(_) => true,
        Err(zbus::Error::NameTaken) => true,
        Err(e) => return Err(format!("d-bus: {e}")),
    };
    if !taken {
        return Ok(Acquired::Primary(Instance { _conn: Some(conn) }));
    }
    conn.call_method(Some(crate::APP_ID), PATH, Some(crate::APP_ID), "Open", &(args,))
        .map_err(|e| format!("could not reach the running app: {e}"))?;
    Ok(Acquired::Forwarded)
}
