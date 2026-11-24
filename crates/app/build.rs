//! Compiles the Slint UI (`ui/`, edition and entry file in `ui/slint-project.json`) and, on
//! Windows, embeds the icon, the version information and the manifest into the executable.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    slint_build::compile("ui")?;
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/icon.ico")
            .set_manifest_file("assets/app.manifest")
            .set("ProductName", "Antigravity Account Switcher")
            .set("FileDescription", "Antigravity Account Switcher")
            .set("OriginalFilename", "AntigravityAccountSwitcher.exe")
            .set("LegalCopyright", "Copyright (c) 2026 DrS. MIT License.");
        resource.compile()?;
    }
    Ok(())
}
