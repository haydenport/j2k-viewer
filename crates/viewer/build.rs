//! Windows only: embed the application icon and version information into `viewer.exe`, so Explorer,
//! the taskbar and the installer show the viewer's own icon instead of the generic one.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    // Decided by the *target*, not the host, so Linux builds never touch the resource compiler.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/icon.ico");
    res.set("ProductName", "j2k viewer");
    res.set("FileDescription", "JPEG 2000 / HTJ2K image viewer");
    res.set("OriginalFilename", "viewer.exe");
    res.set("InternalName", "viewer");
    res.set("LegalCopyright", "Copyright (c) 2026 haydenport");
    if let Err(e) = res.compile() {
        // Without the icon the program still works; do not fail the whole build over it.
        println!("cargo:warning=could not embed the Windows icon/version resource: {e}");
    }
}
