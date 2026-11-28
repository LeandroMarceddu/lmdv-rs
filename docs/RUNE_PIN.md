# Rune pin

- Repository: https://github.com/anomalyco/rune
- Snapshot commit: `86df731b7171f52c9918f288784dda6ed0327289`
- Branch: `dev`
- Snapshot date: 2026-11-28
- Upstream head checked 2026-11-28: `86df731b7171f52c9918f288784dda6ed0327289`
  (same as the pin; includes the #2745 kit hooks and #2744 core input/
  clipboard/field-menu/multi-caret APIs)
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
