//! Runs the P1-02 shell outside the `lmdv` binary, for snapshots and manual
//! checks: `cargo run -p lmdv-app --example shell -- --scene tabs`.

fn main() {
    let cwd = std::env::current_dir().unwrap_or_default();
    let args = std::iter::once("lmdv".to_owned()).chain(rune::extra_args().iter().cloned());
    let launch = lmdv_io::parse_args(args, cwd);
    lmdv_app::run(launch);
}
