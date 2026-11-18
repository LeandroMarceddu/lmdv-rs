# Flatpak recipe for lmdv

**Status:** recipe only. `flatpak`/`flatpak-builder` are not installed on the development
machine, so this manifest has not been built end-to-end yet. The manifest parses as JSON
and follows the freedesktop runtime conventions; the build needs one generated file
(`cargo-sources.json`) and a runtime decision (see “Identity” below).

## Files

| File | Purpose |
|---|---|
| `md.lmdv.json` | Flatpak manifest: `org.freedesktop.Platform//25.08`, Rust SDK extension, simple `cargo build --release` module, Wayland/X11/GPU finish-args. |
| `cargo-sources.json` | **Generated, not committed.** Offline cargo sources for the module. |

## Prerequisites

```bash
sudo pacman -S --needed flatpak flatpak-builder
flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
flatpak install flathub org.freedesktop.Platform//25.08 org.freedesktop.Sdk//25.08 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08
```

The Rust SDK extension must provide a compiler at least as new as the workspace MSRV
(`Cargo.toml`: `rust-version = "1.99"`). The repository's `rust-toolchain.toml` is a
rustup file and is ignored inside the Flatpak build.

## Why `cargo-sources.json`

`flatpak-builder` builds **without network access**. `cargo build` therefore needs every
crates.io dependency vendored up front. Generate the file from `Cargo.lock` with
[`flatpak-cargo-generator`](https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo)
and place it next to this manifest:

```bash
# one-time: get the generator (poetry v2 + its deps are required by the script)
git clone https://github.com/flatpak/flatpak-builder-tools.git
cd flatpak-builder-tools/cargo
poetry install
poetry env activate

# from the repository root
python3 flatpak-builder-tools/cargo/flatpak-cargo-generator.py \
    Cargo.lock -o packaging/flatpak/cargo-sources.json
```

The generated JSON contains the vendored crates **and** a `cargo/config.toml` source
entry; the module copies that config into `.cargo/config.toml`, which is what makes the
path dependencies under `vendor/rune/` and the `[patch.crates-io]` entries resolve
offline. Regenerate it whenever `Cargo.lock` changes; `md.lmdv.json` fails with a
"source file not found" error until the file exists.

Flathub submissions use a `git`/`archive` source at the release tag instead of the local
`dir` source. The local source carries `"skip": [".git", "target", "dist"]` so a developer
checkout does not copy a multi-gigabyte `target/` into the build sandbox.

## Build, run, install

```bash
# from the repository root
flatpak-builder --user --install --force-clean build-dir packaging/flatpak/md.lmdv.json
flatpak run md.lmdv
flatpak run md.lmdv -- tests/fixtures/test-features.md

flatpak-builder --run build-dir packaging/flatpak/md.lmdv.json lmdv --version
flatpak build-export repo build-dir          # local OSTree repository
flatpak uninstall --delete-data md.lmdv
```

Exported integration files appear under
`~/.local/share/flatpak/exports/share/applications/md.lmdv.desktop` and
`.../icons/hicolor/scalable/apps/md.lmdv.svg`; that directory is on `XDG_DATA_DIRS`, so
“Open With” picks the app up without further registration.

## Sandbox permissions

| finish-arg | Why |
|---|---|
| `--socket=wayland`, `--socket=fallback-x11`, `--share=ipc` | Native Wayland, X11/XWayland fallback, XShm. |
| `--device=dri` | Vulkan/GPU rendering through the runtime's driver extension (NVIDIA needs the matching `org.freedesktop.Platform.GL.nvidia-*` extension on the host). |
| `--filesystem=home:ro` | A viewer must read an arbitrary `~/…` path passed by the desktop entry or the CLI. Read-only is the minimum that still makes `flatpak run md.lmdv ~/notes/a.md` work. |

No D-Bus permission is declared: Flatpak proxies the session bus's portal interfaces
(`org.freedesktop.portal.*`), so file choosers and the document portal are reachable
without `--talk-name`.

**Document portal and saving.** With `home:ro`, edits cannot be saved next to the
document by plain path. Documents delivered through the portal arrive as
`/run/user/$UID/doc/<id>/<name>` and carry the portal's read/write grant, so Save works
for those. Two alternatives, in order of preference:

1. Keep `home:ro` and rely on the portal: opening from the file manager (file
   forwarding, `flatpak run --file-forwarding`) and Save-As through the file chooser work;
   a plain CLI path outside the portal is read-only and must fail with a visible error.
2. Grant write access where documents live, e.g. `--filesystem=xdg-documents` (or
   `--filesystem=home`), if saving in place from a CLI path is a required workflow.

The app must tolerate both cases: the portal path is the document identity for the
session and must not be mapped back to a host path (`docs/LINUX_INTEGRATION.md` §7.5).

## Identity

* `app-id`, desktop file, icon and metainfo must agree: `md.lmdv`,
  `md.lmdv.desktop`, `md.lmdv.svg`, `md.lmdv.metainfo.xml`.
* The manifest copies `packaging/lmdv.desktop` to `md.lmdv.desktop`, rewrites
  `Icon=lmdv` to `Icon=md.lmdv` (the exported icon name must match the app id), installs
  the icon under both names, and drops `TryExec`: on the host `lmdv` is not a command,
  and launchers that enforce `TryExec` would hide the exported entry.
* The app currently reports the window id **`lmdv`** (`crates/lmdv-app/src/shared.rs`,
  `APP_ID`), which matches the system desktop file `lmdv.desktop` used by the AUR package
  and the per-user installer. Flatpak requires the desktop file to be named after the
  app id, so the exported entry is `md.lmdv.desktop`; on native Wayland the compositor
  may therefore show a second taskbar icon until `APP_ID` becomes `md.lmdv`.
  `StartupWMClass=lmdv` still groups the X11/XWayland window.
* AppStream (`appstreamcli validate`) warns `cid-desktopapp-is-not-rdns` for the
  two-component id `md.lmdv`. Flathub's linter also prefers at least three components.
  Picking the final identity (for example `md.lmdv.App`) is a project decision that must
  change `APP_ID`, the desktop file, the icon name, the metainfo id and this manifest
  together. See `../README.md`.

## Single instance

The app's normal single-instance rendezvous is a Unix socket in `$XDG_RUNTIME_DIR`
(`docs/LINUX_INTEGRATION.md` §5). Inside Flatpak that directory is shared per user, so
two `flatpak run` instances *may* find each other, but the design document requires a
D-Bus implementation (`org.freedesktop.Application`) for Flatpak. Until that lands,
expect either a working socket or a degraded second instance; the app's fallback is
“run without single instance”, never a crash.

## Runtime behavior

* No telemetry and no runtime network access; nothing in the sandbox is granted
  `--share=network`.
* PDF export needs a proportional and a monospace font. Fonts come from the runtime
  and the host font extension; if `fontdb` finds none, export falls back to Typst's
  bundled serif (recorded in the app's export notice).
* Math fonts are embedded in `lmdv-math` (KaTeX), so math renders without system fonts.
