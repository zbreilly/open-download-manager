fn main() {
    // Only when building on Windows for Windows; type-checking for Windows
    // from another OS skips these steps.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Stage a private Windows App SDK runtime beside the executable, so
        // the app runs on a clean Windows 10/11 with nothing else installed.
        windows_reactor_setup::as_self_contained();
        // the .exe's icon
        println!("cargo::rerun-if-changed=app.rc");
        let _ = embed_resource::compile("app.rc", embed_resource::NONE);
        // The .exe's product name and version (Properties > Details). A code
        // signing service checks them before it signs the file, so a build
        // without them fails here instead of at release time.
        let rc = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("odm-version.rc");
        std::fs::write(&rc, version_rc(env!("CARGO_PKG_VERSION"))).expect("write the version resource");
        embed_resource::compile(&rc, embed_resource::NONE)
            .manifest_required()
            .expect("compile the version resource");
    }
}

/// A VERSIONINFO resource for `version` ("1.2.3", or "1.2.3-beta.1": the
/// numbers before any suffix give the numeric version, the text is kept whole).
#[cfg_attr(not(windows), allow(dead_code))]
fn version_rc(version: &str) -> String {
    let mut n = version
        .split(['-', '+'])
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse::<u16>().unwrap_or(0));
    let (major, minor, patch) = (n.next().unwrap_or(0), n.next().unwrap_or(0), n.next().unwrap_or(0));
    // the version text goes inside a quoted string
    let text: String = version.chars().filter(|c| c.is_ascii_alphanumeric() || ".-+".contains(*c)).collect();
    format!(
        r#"1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "Open Download Manager"
            VALUE "FileDescription", "Open Download Manager"
            VALUE "FileVersion", "{text}"
            VALUE "InternalName", "OpenDownloadManager"
            VALUE "LegalCopyright", "Open Download Manager contributors. AGPL-3.0-only."
            VALUE "OriginalFilename", "OpenDownloadManager.exe"
            VALUE "ProductName", "Open Download Manager"
            VALUE "ProductVersion", "{text}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    )
}
