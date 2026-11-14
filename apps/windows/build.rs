fn main() {
    // Only when building on Windows for Windows; type-checking for Windows
    // from another OS skips both steps.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Stage a private Windows App SDK runtime beside the executable, so
        // the app runs on a clean Windows 10/11 with nothing else installed.
        windows_reactor_setup::as_self_contained();
        // the .exe's icon
        println!("cargo::rerun-if-changed=app.rc");
        let _ = embed_resource::compile("app.rc", embed_resource::NONE);
    }
}
