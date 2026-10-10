fn main() {
    // Stage a private Windows App SDK runtime beside the executable, so the
    // app runs on a clean Windows 10/11 without any runtime installed.
    // (Skipped when only type-checking for Windows from another OS.)
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_reactor_setup::as_self_contained();
    }
}
