fn main() {
    slint_build::compile("ui/app.slint").expect("Slint build failed");
    // Windows: Aimdot's icon + version details in aimdot-settings.exe (Explorer, taskbar, Properties > Details),
    // like Echo's build.rs. An exe without them also looks more suspicious to antivirus heuristics.
    println!("cargo:rerun-if-changed=../assets/icons/aimdot.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let ver = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
        let n: Vec<u64> = ver.split(['.', '-', '+']).take(3).map(|x| x.parse().unwrap_or(0)).collect();
        let packed = (n.first().copied().unwrap_or(0) << 48) | (n.get(1).copied().unwrap_or(0) << 32) | (n.get(2).copied().unwrap_or(0) << 16);
        let mut r = winresource::WindowsResource::new();
        r.set_icon("../assets/icons/aimdot.ico")
            .set("ProductName", "Aimdot")
            .set("FileDescription", "Aimdot settings")
            .set("CompanyName", "Resister0")
            .set("LegalCopyright", "\u{a9} 2026 Resister0")
            .set("OriginalFilename", "aimdot-settings.exe")
            .set("InternalName", "aimdot-settings")
            .set("FileVersion", &ver)
            .set("ProductVersion", &ver)
            .set_version_info(winresource::VersionInfo::FILEVERSION, packed)
            .set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed);
        if let Err(e) = r.compile() {
            // no resource compiler (rc.exe / windres) found: still build, just without the icon / details
            println!("cargo:warning=exe icon / version info skipped: {e}");
        }
    }
}
