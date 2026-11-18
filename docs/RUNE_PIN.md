# Rune pin

- Repository: https://github.com/anomalyco/rune
- Snapshot commit: `cca5df6fafb4721d96fe105f126ebfd2ec351bb6`
- Branch: `dev`
- Snapshot date: 2026-11-17
- License: MIT
- Toolchain pinned by upstream: Rust `1.99.1` (`rust-toolchain.toml`)

The full Rune workspace is vendored in [`vendor/rune/`](../vendor/rune/). This
file is the pin the vendor scripts read; `vendor/rune/UPSTREAM.md` records what
was actually vendored. Do not re-point the pin without re-vendoring, re-applying
the numbered patches and rerunning the goldens.

## Pin policy

- Pin the **rev**, not a branch. Upstream promises `dev` is never rewritten, but a rev makes builds reproducible.
- Rune is pre-release and does not accept pull requests. Keep patches in `vendor/rune/patches/`; report core bugs upstream as issues.
- When bumping the rev, re-read the changelog, run the golden snapshots, and update this file plus the vendored tree.
- Record any further divergence in `docs/DECISIONS.md`.

## Linux build dependencies (CachyOS/Arch)

```bash
sudo pacman -S --needed libxkbcommon libxkbcommon-x11 wayland mesa-vulkan-drivers
rustup toolchain install 1.99.1
```

The app itself is tested on Wayland and X11. Headless `--snap` runs do not need a display or `libxkbcommon-x11`.
