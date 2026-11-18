# lmdv packaging

Recipes for shipping lmdv outside the development checkout: desktop metadata,
an AppImage builder, a Flatpak manifest and an AUR package. Nothing here changes the
application code or the existing per-user installer.

**Status:** `.github/workflows/release.yml` builds and publishes the Linux AppImage,
raw Linux binary and Windows NSIS installer on `v*` tags. Flatpak and AUR remain
recipes with placeholder source values; there are no signed checksums or
reproducible-build claims yet. The blockers are listed at the end.

## Files

| Path | What it is |
|---|---|
| `lmdv.desktop` | System desktop entry (`Exec=lmdv %f`, `Icon=lmdv`, three MIME types). |
| `lmdv.metainfo.xml` | AppStream metadata, component id `md.lmdv`, launchable `lmdv.desktop`. |
| `lmdv.xml` | shared-mime-info package: aliases, extensions and glob weights for `text/markdown`. |
| `../assets/lmdv.svg` | Application icon, consumed by every recipe (owned by the icon task). |
| `../scripts/package-appimage.sh` | Builds/stages the AppDir and emits `dist/lmdv-<version>-<arch>.AppImage`. |
| `flatpak/md.lmdv.json` | Flatpak manifest (freedesktop 25.08 + Rust SDK extension). |
| `flatpak/README.md` | Flatpak build, permissions and portal notes. |
| `aur/PKGBUILD`, `aur/.SRCINFO` | `pkgname=lmdv` source package for Arch/CachyOS. |
| `aur/README.md` | AUR build, `lmdv-bin` variant and validation notes. |
| `windows/lmdv.nsi` | NSIS installer: per-user install, Start Menu entry, optional desktop shortcut and default `.md` association. |
| `../.github/workflows/release.yml` | Tag-driven Linux AppImage + raw binary and Windows NSIS release. |

## GitHub release

Push a `v*` tag. `.github/workflows/release.yml` builds and publishes:

- Linux: `dist/lmdv-<version>-<arch>.AppImage` and the raw `lmdv-<version>-x86_64` binary.
- Windows: `dist/lmdv-<version>-setup.exe`, built with `packaging/windows/lmdv.nsi`.

The NSIS installer associates `.md` at the user level and registers lmdv in Open with /
Default apps. On Windows 10/11 an existing per-user `UserChoice` still wins until the
user confirms lmdv in Settings > Default apps; the installer never deletes that key.

No Flatpak/AUR/deb/rpm artifacts are built by the release workflow.

## Identity: `lmdv` vs `md.lmdv`

Three different names exist and must not be confused:

| Name | Where it comes from | Used for |
|---|---|---|
| `lmdv` | `APP_ID` in `crates/lmdv-app/src/shared.rs`; command name | Wayland `app_id`, X11 `WM_CLASS`, desktop file basename, icon name |
| `md.lmdv` | `APP_DIR` in `crates/lmdv-io/src/dirs.rs` | XDG config/data/cache/state root |
| `md.lmdv` | this packaging | AppStream component id (and all Flatpak names) |

Packages install `lmdv.desktop` + `lmdv.svg` so that the window identity matches the
desktop file on native Wayland. The AppStream id is `md.lmdv` as the stable
project identity, with `<launchable type="desktop-id">lmdv.desktop</launchable>`
pointing at the installed entry.

Consequences, reported rather than hidden:

* `appstreamcli validate` warns `cid-desktopapp-is-not-rdns` (two components) and
  `url-homepage-missing` (no homepage by decision). Exit code 3; no hard errors.
* Flatpak requires the desktop file and icon to be named after the app id, so the
  Flatpak build installs `md.lmdv.desktop` / `md.lmdv.svg` and rewrites `Icon=` to
  `md.lmdv`. The Flatpak and system packages therefore use different desktop ids;
  this is legal (Flatpak exports its own) but means the final reverse-DNS decision
  should change `APP_ID`, the desktop file, the icon and the metainfo id together.
* `desktop-file-validate` emits an informational hint (exit 0) that
  `Categories=Office;Viewer;Utility;` has more than one main category. That is the
  requested category set; the app appears under both Office and Utilities.

Exec uses `%f`, not `%F`: the CLI parses only the first positional
(`docs/LINUX_INTEGRATION.md` §2.2), so `%F` would advertise multi-file selection and
silently drop files. Changing it is a one-character edit if the CLI grows multi-file
support.

## Relationship to `scripts/install-md-handler.sh`

The per-user installer is **unchanged** and remains the development path. The two layers
are deliberately separate:

| | `install-md-handler.sh` | this directory |
|---|---|---|
| Scope | `$HOME` only, no sudo | `/usr` (pacman), read-only AppDir, Flatpak sandbox |
| Exec | absolute path to the `~/.local/bin/lmdv` launcher | `lmdv %f` (resolved by `PATH` / AppImage integration / `flatpak run`) |
| Binary selection | picks `target/debug` vs `target/release` at launch | packaged binary only |
| Owner marker | `X-LMDV-Managed=true`, launcher marker | package ownership, no marker |
| Refresh | calls `update-*` itself | pacman/Flatpak hooks and exports |
| Uninstall | restores the previous default handler | package removal; the user's `mimeapps.list` default may remain |

The per-user installer and the system packages write the same desktop id, icon name and
MIME type list, so “Open With” shows one entry, not two. Because `~/.local/share`
precedes `/usr/share` in `XDG_DATA_DIRS`, a per-user registration **shadows** a packaged
one. After installing a package, remove the development registration once if you no
longer want it:

```bash
scripts/install-md-handler.sh --uninstall   # restores the previous handler, leaves /usr alone
```

Installing a package does **not** change the default handler. The per-user installer
does call `xdg-mime default` (docs/LINUX_INTEGRATION.md §3), so a dev install that is later
followed by a package keeps its association until the user changes it; packages leave the
choice to the user.

## AppImage

```bash
scripts/package-appimage.sh --dry-run          # print the exact plan, write nothing
scripts/package-appimage.sh                    # cargo build --release -p lmdv + dist/lmdv-0.5.0-x86_64.AppImage
scripts/package-appimage.sh --no-build         # reuse target/release/lmdv
scripts/package-appimage.sh --binary ./target/debug/lmdv --appdir-only \
    --out-dir /tmp/lmdv-appimage               # stage + validate, no image (fast smoke test)
```

Prerequisites:

* A release binary: built by the script (`cargo build --release -p lmdv --locked`) or
  supplied with `--binary` / reused with `--no-build`.
* `linuxdeploy` **with its appimage output plugin** — on Arch/CachyOS:
  `sudo pacman -S --needed linuxdeploy-appimage`. Elsewhere use the linuxdeploy
  AppImage. `appimagetool` works as a fallback (`APPIMAGETOOL=/path/to/appimagetool`),
  but then shared libraries are not bundled.
* Optional but recommended for a fully offline run: an AppImage runtime file with
  `--runtime-file` (or `RUNTIME_FILE`). linuxdeploy's plugin otherwise fetches the
  runtime once and caches it; the script itself never downloads anything.

What goes into the image:

```
AppDir/
  AppRun                                   # execs usr/bin/lmdv, sets PATH
  usr/bin/lmdv
  usr/share/applications/lmdv.desktop
  usr/share/icons/hicolor/scalable/apps/lmdv.svg
  usr/share/mime/packages/lmdv.xml
  usr/share/metainfo/md.lmdv.metainfo.xml
```

The staged metadata is validated by `desktop-file-validate`, `xmllint` and (for
information) `appstreamcli validate` before the image is built. The AppDir copy of the
desktop file has `TryExec` removed: integration tools rewrite `Exec` to the absolute
`.AppImage` path but usually leave `TryExec=lmdv`, which is not on `PATH` and would make
launchers hide the entry.

AppImage has **no sandbox**: `%f` paths work, the Unix single-instance socket works, and
there is no portal indirection. It also does **not** integrate itself. Use
AppImageLauncher/`appimaged`, or extract and install the desktop file with `Exec` rewritten
to the absolute image path (`docs/LINUX_INTEGRATION.md` §7.4). Portable mode uses a marker
file *next to* the `.AppImage`, never inside the read-only squashfs. The script does not
build a self-updater; pass `--update-info` to embed zsync update information for
AppImageUpdate.

`dist/` is build output and is not committed; the root `.gitignore` ignores `/dist/`.

## Flatpak

See `flatpak/README.md`. Short version: generate `packaging/flatpak/cargo-sources.json`
from `Cargo.lock` with `flatpak-cargo-generator` (Flatpak builds have no network), then

```bash
flatpak-builder --user --install --force-clean build-dir packaging/flatpak/md.lmdv.json
flatpak run md.lmdv
```

The sandbox is Wayland/X11 + GPU + `--filesystem=home:ro`; saving relies on the document
portal, with `--filesystem=xdg-documents` or `home` as documented alternatives.

## AUR

See `aur/README.md`. Short version:

```bash
cd packaging/aur
updpkgsums
makepkg --printsrcinfo > .SRCINFO
makepkg -si
```

`pkgname=lmdv`, `arch=('x86_64' 'aarch64')`, Rustup brings the Rune-pinned 1.99.1
toolchain, and the package installs the binary plus desktop/metainfo/icon/MIME files.
The `lmdv-bin` variant is documented in the AUR README.

## Validation

Run from the repository root:

```bash
bash -n scripts/package-appimage.sh
shellcheck scripts/package-appimage.sh
scripts/package-appimage.sh --dry-run

desktop-file-validate packaging/lmdv.desktop
appstreamcli validate --no-net packaging/lmdv.metainfo.xml
xmllint --noout packaging/lmdv.metainfo.xml packaging/lmdv.xml assets/lmdv.svg
python3 -m json.tool packaging/flatpak/md.lmdv.json

namcap packaging/aur/PKGBUILD
(cd packaging/aur && makepkg --printsrcinfo | diff -u .SRCINFO -)
```

Staged-tree check (catches a metainfo launchable that does not resolve):

```bash
scripts/package-appimage.sh --binary ./target/release/lmdv --appdir-only
appstreamcli validate-tree --no-net dist/AppDir
```

## What remains for a signed, reproducible release

1. **Project identity decision.** Pick a three-component reverse-DNS id (for example
   `md.lmdv.App`), then change `APP_ID`, `lmdv.desktop`/`lmdv.svg`, the metainfo id and
   the Flatpak app id together. This clears the AppStream rDNS warning and the
   Flatpak/Wayland icon mismatch.
2. **Release source and metadata.** No project homepage will be provided, so keep the
   `url-homepage-missing` warning as accepted and use `url=` in the PKGBUILD only for
   the release tarball base, not as a homepage. Add `<screenshots>` if a channel needs
   them; Flathub's metadata requirements may not be satisfiable without a homepage.
3. **Release artifacts.** Tagged source tarball containing `vendor/rune/`, prebuilt
   `lmdv-<version>-<arch>.tar.gz` for `lmdv-bin`, signed checksums (`sha256sums` filled
   by `updpkgsums`, `validpgpkeys` for the AUR), and a PGP-signed git tag.
4. **Reproducibility.** The release build is not bit-reproducible yet: Cargo embeds
   absolute paths, and the Flatpak manifest relies on the SDK's rust-stable version.
   Set `SOURCE_DATE_EPOCH`, use `--remap-path-prefix`, and pin the SDK extension
   version before claiming reproducible builds.
5. **Flatpak build.** Generate `cargo-sources.json`, build on a machine with
   `org.freedesktop.Platform//25.08`, then run `flatpak-builder-lint` and verify the
   exported desktop entry and icon, plus `text/markdown` resolution on the host.
6. **AppImage build.** Build once with a release binary, check `ldd` coverage with
   `--appimage-extract`, test on a clean VM without Rust/GTK dev packages, and embed
   zsync `--update-info` at release time.
7. **Manifest hygiene.** The root `.gitignore` covers `/dist/` and
   `packaging/flatpak/cargo-sources.json` (the generated file is reproducible but large).
