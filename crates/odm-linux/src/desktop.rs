//! Which desktop is running, to pick the matching front end.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    Plasma,
    Gnome,
    /// anything else (Xfce, Cinnamon, Sway, ...)
    Other,
}

/// From `XDG_CURRENT_DESKTOP` (a colon-separated list, e.g. "KDE" or
/// "ubuntu:GNOME").
pub fn current() -> Desktop {
    from_xdg(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default())
}

pub fn from_xdg(v: &str) -> Desktop {
    let parts: Vec<String> = v.split(':').map(|s| s.trim().to_ascii_lowercase()).collect();
    if parts.iter().any(|p| p == "kde" || p == "plasma") {
        Desktop::Plasma
    } else if parts.iter().any(|p| p == "gnome" || p == "unity" || p == "pantheon" || p == "budgie") {
        Desktop::Gnome
    } else {
        Desktop::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects() {
        assert_eq!(from_xdg("KDE"), Desktop::Plasma);
        assert_eq!(from_xdg("ubuntu:GNOME"), Desktop::Gnome);
        assert_eq!(from_xdg("XFCE"), Desktop::Other);
        assert_eq!(from_xdg(""), Desktop::Other);
    }
}
