fn main() {
    // tauri.conf.json bundles ../extension/dist as a resource; on a fresh
    // checkout (before extension/build.sh ran) create it empty so that
    // `cargo check` / `cargo build` work. Release builds run build.sh first.
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../extension/dist");
    if !dist.exists() {
        let _ = std::fs::create_dir_all(&dist);
    }
    tauri_build::build()
}
