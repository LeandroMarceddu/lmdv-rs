# Rune pin

- Repository: https://github.com/anomalyco/rune
- Snapshot commit: `07bf8c5d3b04d2291f5493392aee8e0694096ce5`
- Branch: `dev`
- Snapshot date: 2026-11-28
- Upstream head checked 2026-11-28: `07bf8c5d3b04d2291f5493392aee8e0694096ce5`
  (same as the pin; `dev` is the default branch and there is no `main`;
  this rev includes the rest/heap-trim fixes and the inline-image/a11y/
  highlight/Mermaid-SVG features that retired all four local patches)
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
