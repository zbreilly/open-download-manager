//! Keep the system from idle-sleeping while downloads run, via a logind
//! sleep/idle inhibitor (released automatically if the process dies).

pub struct Awake {
    _private: (),
}

impl Awake {
    pub fn new() -> Self {
        Awake { _private: () }
    }

    /// Hold (true) or release (false) the inhibitor. Cheap to call often.
    pub fn set(&self, on: bool) {
        let _ = on;
        // STUB
    }
}

impl Default for Awake {
    fn default() -> Self {
        Self::new()
    }
}
