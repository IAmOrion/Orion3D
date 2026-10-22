// Embeds the app icon and version info in the Windows .exe.
fn main() {
    println!("cargo:rerun-if-changed=assets/orion3d.ico");
    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("assets/orion3d.ico");
            res.set("ProductName", "Orion3D");
            res.set("FileDescription", "Orion3D");
            if let Err(e) = res.compile() {
                println!("cargo:warning=Could not embed the app icon: {e}");
            }
        }
    }
}
