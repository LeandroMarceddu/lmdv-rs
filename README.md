# lmdv

A native Linux-first Markdown viewer and editor, written in Rust with [Rune](https://github.com/anomalyco/rune). It runs as a native GPU app with no webview.

**Status:** P0–P5 complete; P6 and P7 in progress. Linux first, with Windows compilation verified. See [`docs/STATUS.md`](docs/STATUS.md) for the current phase and open items.

CI runs the full workspace gate on Linux for every push to `main` and every pull request. Windows is compile-checked with `cargo check --workspace --all-targets` and must keep building; its runtime integration is not claimed yet.

## What it will do

- Open `.md` files by double-click, drag-and-drop, command line, or a folder browser
- Read GitHub-flavored Markdown: tables, task lists, footnotes, callouts, front matter, code highlighting, native Mermaid, math, images, video/audio (external player on Linux)
- Remember reading positions, recent files, themes and text settings
- Edit with a source editor and live preview; explicit save, crash-recovery drafts, file watching and conflict guards
- Find/replace, command palette, outline, folder search, table preview and filtering
- Export to self-contained HTML, DOCX and PDF; print; copy rich text
- Six interface languages, accessible tree, four themes (light, sepia, dark, black)
- Work offline; no accounts, ads, telemetry or network calls

## Planning documents

| File | Purpose |
|---|---|
| [`PLAN.md`](PLAN.md) | P0–P7 phases, gates, first tasks |
| [`AGENTS.md`](AGENTS.md) | Operating manual for agents/contributors |
| [`docs/BEHAVIOR_SPEC.md`](docs/BEHAVIOR_SPEC.md) | 342-row authoritative behavior spec |
| [`docs/PARITY_CHECKLIST.md`](docs/PARITY_CHECKLIST.md) | Generated implementation checklist |
| [`docs/FEATURE_PARITY.md`](docs/FEATURE_PARITY.md) | Rune mapping, phases, substitutes/gaps |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Condensed design; [`ARCHITECTURE_DETAIL.md`](docs/ARCHITECTURE_DETAIL.md) for the long form |
| [`docs/TESTING.md`](docs/TESTING.md) | Fixtures, scenes, goldens, a11y, performance |
| [`docs/RISKS_AND_SPIKES.md`](docs/RISKS_AND_SPIKES.md) | Risk register and spike specs |
| [`docs/LINUX_INTEGRATION.md`](docs/LINUX_INTEGRATION.md) | MIME, desktop entry, launcher, socket, packaging |
| [`docs/DECISIONS.md`](docs/DECISIONS.md) | ADR log |

## Releases

Tags `v*` build a Linux AppImage, a raw Linux binary and a Windows NSIS installer through [`.github/workflows/release.yml`](.github/workflows/release.yml).

## Linux prerequisites (CachyOS/Arch)

```bash
sudo pacman -S --needed libxkbcommon libxkbcommon-x11 wayland mesa-vulkan-drivers
rustup toolchain install 1.99.1
rustup component add rustfmt clippy --toolchain 1.99.1
```

Rune pins Rust 1.99.1. Wayland and X11 are both targeted; NVIDIA/Vulkan is the tested GPU path.

## Opening `.md` files

`scripts/install-md-handler.sh` registers the current build as your per-user Markdown handler. It installs a stable launcher that follows whichever of `target/debug/lmdv` or `target/release/lmdv` is newer, so rebuilding is picked up without re-running it.

```bash
./scripts/install-md-handler.sh --dev        # prefer target/debug
./scripts/install-md-handler.sh --release    # prefer target/release
./scripts/install-md-handler.sh              # newest of the two
./scripts/install-md-handler.sh --uninstall  # restore the previous handler
```

Useful flags: `--binary PATH`, `--no-wrapper`, `--no-mime`, `--dry-run`, `--purge`, `--force`, `--help`.

Double-clicking a `.md` opens it in the running instance when one exists. The CLI is `lmdv [--new-window] [file] [--line N]` (also `-l N`, `--line=N`, `--`).

If the handler does not appear, run `update-desktop-database ~/.local/share/applications`, then `gio open file.md`; on Plasma, log out/in or run `kbuildsycoca6`. See [`docs/LINUX_INTEGRATION.md`](docs/LINUX_INTEGRATION.md) for the full test plan.

## License / provenance

Personal project. Rune is MIT; its license is kept in [`vendor/rune/`](vendor/rune/). Re-check attribution if this is ever published.

Heavily inspired by Leggimi
