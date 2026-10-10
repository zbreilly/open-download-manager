//! Desktop notifications (org.freedesktop.Notifications), with action
//! buttons where the server supports them.
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::OnceLock;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

type Callback = Box<dyn Fn(String) + Send>;

#[derive(Default)]
struct State {
    /// our id → the server's id, to replace an earlier notification
    by_key: HashMap<String, u32>,
    callbacks: HashMap<u32, Callback>,
}

struct Server {
    conn: Connection,
    state: Mutex<State>,
    actions: bool,
}

/// One connection and one signal listener for the life of the process.
fn server() -> Option<&'static Server> {
    static SERVER: OnceLock<Option<Server>> = OnceLock::new();
    SERVER
        .get_or_init(|| {
            let conn = Connection::session().ok()?;
            let caps: Vec<String> = conn
                .call_method(Some(DEST), PATH, Some(DEST), "GetCapabilities", &())
                .ok()
                .and_then(|r| r.body().deserialize().ok())
                .unwrap_or_default();
            let s = Server { conn, state: Mutex::new(State::default()), actions: caps.iter().any(|c| c == "actions") };
            std::thread::Builder::new().name("odm-notify".into()).spawn(listen).ok()?;
            Some(s)
        })
        .as_ref()
}

fn listen() {
    let Some(s) = server() else { return };
    let Ok(proxy) = zbus::blocking::Proxy::new(&s.conn, DEST, PATH, DEST) else { return };
    let Ok(signals) = proxy.receive_all_signals() else { return };
    for msg in signals {
        let header = msg.header();
        let Some(member) = header.member() else { continue };
        match member.as_str() {
            "ActionInvoked" => {
                let Ok((id, action)) = msg.body().deserialize::<(u32, String)>() else { continue };
                // taken out so the callback runs without the lock
                let cb = s.state.lock().callbacks.remove(&id);
                if let Some(cb) = cb {
                    cb(action);
                }
            }
            "NotificationClosed" => {
                let Ok((id, _reason)) = msg.body().deserialize::<(u32, u32)>() else { continue };
                let mut st = s.state.lock();
                st.callbacks.remove(&id);
                st.by_key.retain(|_, v| *v != id);
            }
            _ => {}
        }
    }
}

/// Show a notification. `actions` are (id, label) pairs; `on_action(id)`
/// runs on a background thread when one is clicked ("default" when the
/// notification body itself is clicked). `id` groups/replaces notifications
/// for the same download.
pub fn show(id: &str, title: &str, body: &str, actions: &[(&str, &str)], on_action: impl Fn(String) + Send + 'static) {
    let (id, title, body) = (id.to_string(), title.to_string(), body.to_string());
    let actions: Vec<String> = actions.iter().flat_map(|(k, l)| [k.to_string(), l.to_string()]).collect();
    // never make the caller (often the GUI or events thread) wait for the bus
    let _ = std::thread::Builder::new().name("odm-notify-send".into()).spawn(move || {
        let Some(s) = server() else { return };
        let replaces = s.state.lock().by_key.get(&id).copied().unwrap_or(0);
        let mut list: Vec<&str> = vec![];
        if s.actions {
            list.extend(["default", "Open"]);
            list.extend(actions.iter().map(String::as_str));
        }
        let mut hints: HashMap<&str, Value> = HashMap::new();
        hints.insert("desktop-entry", Value::from(crate::APP_ID));
        let sent = s.conn.call_method(
            Some(DEST),
            PATH,
            Some(DEST),
            "Notify",
            &(crate::APP_NAME, replaces, crate::APP_ID, title.as_str(), body.as_str(), list, hints, -1i32),
        );
        let Ok(nid) = sent.and_then(|r| r.body().deserialize::<u32>()) else { return };
        let mut st = s.state.lock();
        st.by_key.insert(id, nid);
        st.callbacks.insert(nid, Box::new(on_action));
    });
}
