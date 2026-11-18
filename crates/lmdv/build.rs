//! Embeds the Windows executable icon and version metadata.
//!
//! Explorer, shortcuts and the taskbar read the `.ico` resource; the runtime
//! window icon is set from `assets/lmdv-512.png` in `main.rs`.

fn main() {
    #[cfg(windows)]
    embed_windows_resources();
}

#[cfg(windows)]
fn embed_windows_resources() {
    use std::path::PathBuf;

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let icon = root.join("assets/lmdv.ico");
    if !icon.is_file() {
        panic!("missing Windows icon: {}", icon.display());
    }
    let mut res = winres::WindowsResource::new();
    res.set_icon(icon.to_str().expect("a UTF-8 icon path"));
    res.set("ProductName", "lmdv");
    res.set("FileDescription", "lmdv - native Markdown reader");
    res.set("LegalCopyright", "MIT");
    res.set("OriginalFilename", "lmdv.exe");
    if let Err(error) = res.compile() {
        panic!("could not embed Windows resources: {error}");
    }
}
