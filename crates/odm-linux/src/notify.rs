//! Desktop notifications (org.freedesktop.Notifications), with action
//! buttons where the server supports them.

/// Show a notification. `actions` are (id, label) pairs; `on_action(id)`
/// runs on a background thread when one is clicked ("default" when the
/// notification body itself is clicked). `id` groups/replaces notifications
/// for the same download.
pub fn show(id: &str, title: &str, body: &str, actions: &[(&str, &str)], on_action: impl Fn(String) + Send + 'static) {
    let _ = (id, title, body, actions, on_action);
    // STUB
}
