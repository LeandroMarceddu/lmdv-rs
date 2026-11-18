#![cfg_attr(windows, windows_subsystem = "windows")]

//! lmdv — the native Markdown viewer/editor binary.

fn main() {
    // Windows: the window/taskbar icon comes from the same 512 px PNG as the
    // Linux/shortcut icon. Rune reads it before the first window; the .ico
    // resource embedded by build.rs covers Explorer and shortcuts.
    #[cfg(windows)]
    if let Err(error) = rune::set_app_icon(include_bytes!("../../../assets/lmdv-512.png").to_vec()) {
        eprintln!("lmdv: could not set the window icon: {error}");
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    // Rune consumes its own `--snap`/`--tree`/`--scene`/`--size`/... flags and
    // leaves the app's arguments in `extra_args`. Only those are the original
    // CLI grammar; parsing `std::env::args()` directly would turn Rune's flags
    // into a launch path.
    let args = std::iter::once("lmdv".to_owned()).chain(rune::extra_args().iter().cloned());
    let launch = lmdv_io::parse_args(args, cwd);
    lmdv_app::run(launch);
}
