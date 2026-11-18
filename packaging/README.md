# lmdv packaging

Recipes for shipping lmdv outside the development checkout: desktop metadata,
an AppImage builder and a Windows NSIS installer. Nothing here changes the
application code or the existing per-user installer.

**Status:** `.github/workflows/release.yml` builds and publishes the Linux AppImage,
raw Linux binary and Windows NSIS installer on `v*` tags. There are no signed
checksums or reproducible-build claims yet; the blockers are listed at the end.

## Files

| Path | What it is |
|---|---|
| `lmdv.desktop` | System desktop entry (`Exec=lmdv %f`, `Icon=lmdv`, three MIME types). |
| `lmdv.metainfo.xml` | AppStream metadata, component id `md.lmdv`, launchable `lmdv.desktop`. |
| `lmdv.xml` | shared-mime-info package: aliases, extensions and glob weights for `text/markdown`. |
| `../assets/lmdv.svg` | Application icon, consumed by the AppImage. |
| `../scripts/package-appimage.sh` | Builds/stages the AppDir and emits `dist/lmdv-<version>-<arch>.AppImage`. |
| `windows/lmdv.nsi` | NSIS installer: per-user install, Start Menu entry, optional desktop shortcut and default `.md` association. |
| `../.github/workflows/release.yml` | Tag-driven Linux AppImage + raw binary and Windows NSIS release. |

## GitHub release

Push a `v*` tag. `.github/workflows/release.yml` builds and publishes:

- Linux: `dist/lmdv-<version>-<arch>.AppImage` and the raw `lmdv-<version>-x86_64` binary.
- Windows: `dist/lmdv-<version>-setup.exe`, built with `packaging/windows/lmdv.nsi`.

The NSIS installer associates `.md` at the user level and registers lmdv in Open with /
Default apps. On Windows 10/11 an existing per-user `UserChoice` still wins until the
user confirms lmdv in Settings > Default apps; the installer never deletes that key.

## Identity: `lmdv` vs `md.lmdv`

Three different names exist and must not be confused:

| Name | Where it comes from | Used for |
|---|---|---|
| `lmdv` | `APP_ID` in `crates/lmdv-app/src/shared.rs`; command name | Wayland `app_id`, X11 `WM_CLASS`, desktop file basename, icon name |
| `md.lmdv` | `APP_DIR` in `crates/lmdv-io/src/dirs.rs` | XDG config/data/cache/state root |
| `md.lmdv` | this packaging | AppStream component id |

Packages install `lmdv.desktop` + `lmdv.svg` so that the window identity matches the
desktop file on native Wayland. The AppStream id is `md.lmdv` as the stable
project identity, with `<launchable type="desktop-id">lmdv.desktop</launchable>`
pointing at the installed entry.

Consequences, reported rather than hidden:

* `appstreamcli validate` warns `cid-desktopapp-is-not-rdns` (two components) and
  `url-homepage-missing` (no homepage by decision). Exit code 3; no hard errors.
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

| | `install-md-handler.sh` | release packages |
|---|---|---|
| Scope | `$HOME` only, no sudo | AppImage (read-only mount) / per-user Windows install |
| Exec | absolute path to the `~/.local/bin/lmdv` launcher | `lmdv %f` (AppImage) / `lmdv.exe "%1"` (NSIS) |
| Binary selection | picks `target/debug` vs `target/release` at launch | packaged binary only |
| Owner marker | `X-LMDV-Managed=true`, launcher marker | image/package ownership, no marker |
| Refresh | calls `update-*` itself | re-download the AppImage / run the installer |
| Uninstall | restores the previous default handler | delete the image + desktop bits / NSIS uninstaller |

The per-user installer and the AppImage integration both write to `~/.local/share`, so
they share one desktop id, icon name and MIME type list and Open With shows one entry.
A per-user registration therefore replaces the image's integration files; after
integrating the AppImage, remove the development registration once if you no longer want
it:

```bash
scripts/install-md-handler.sh --uninstall   # restores the previous handler, leaves the image alone
```

Integrating the AppImage does **not** change the default handler. The per-user installer
does call `xdg-mime default` (docs/LINUX_INTEGRATION.md §3), so a dev install that is later
followed by an AppImage integration keeps its association until the user changes it.

## AppImage

The release workflow builds the image with `scripts/package-appimage.sh`. Layout expected
by AppImage tooling:

```
AppDir/
  AppRun                      # execs usr/bin/lmdv "$@"
  usr/bin/lmdv
  usr/share/applications/lmdv.desktop
  usr/share/icons/hicolor/scalable/apps/lmdv.svg
  usr/share/mime/packages/lmdv.xml
  usr/lib/…                   # if not fully static
```

Run it locally with:

```bash
scripts/package-appimage.sh --dry-run          # print the exact plan, write nothing
scripts/package-appimage.sh                    # cargo build --release -p lmdv + dist/lmdv-0.5.0-x86_64.AppImage
scripts/package-appimage.sh --no-build         # reuse target/release/lmdv
scripts/package-appimage.sh --binary ./target/debug/lmdv --appdir-only \
    --out-dir /tmp/lmdv-appimage               # stage + validate, no image (fast smoke test)
```

* `linuxdeploy` **with its appimage output plugin** — on Arch/CachyOS:
  `sudo pacman -S --needed linuxdeploy-appimage`. Elsewhere use the linuxdeploy
  AppImage. `appimagetool` works as a fallback (`APPIMAGETOOL=/path/to/appimagetool`),
  but it does not bundle shared libraries.
* Optional but recommended for a fully offline run: an AppImage runtime file with
  `--runtime-file` (also `RUNTIME_FILE=...`).
* The desktop file **inside** the image should use `Exec=lmdv %f`; AppImage integration
  tools rewrite it to an absolute path to the `.AppImage` when they install it.
* AppImage does not auto-integrate. Options: `AppImageLauncher` or `appimaged` moves/copies
  the file and installs the desktop entry/icon; otherwise:
  ```bash
  ./lmdv-0.5.0-x86_64.AppImage --appimage-extract
  # copy squashfs-root/usr/share/applications/lmdv.desktop to ~/.local/share/applications,
  # rewrite Exec to the absolute .AppImage path, copy the icon, then register as in §3
  ```
* No sandbox: `%f` paths work, and the Unix socket works normally.
* Portable mode: marker next to the `.AppImage`, **not** inside (the squashfs is read-only).
* Updates: manual re-download, or AppImageUpdate if the image is built with zsync. Do not
  build a self-updater into the app.
* Uninstall: remove the `.AppImage`, the desktop file, icon, MIME package; re-run
  `update-*`; restore the handler.

`dist/` is build output and is not committed; the root `.gitignore` ignores `/dist/`.

## Validation

Run from the repository root:

```bash
bash -n scripts/package-appimage.sh
shellcheck scripts/package-appimage.sh
scripts/package-appimage.sh --dry-run

desktop-file-validate packaging/lmdv.desktop
appstreamcli validate --no-net packaging/lmdv.metainfo.xml
xmllint --noout packaging/lmdv.metainfo.xml packaging/lmdv.xml assets/lmdv.svg
```

The NSIS installer is compiled by the release workflow on `windows-latest`.

Staged-tree check (catches a metainfo launchable that does not resolve):

```bash
scripts/package-appimage.sh --binary ./target/release/lmdv --appdir-only
appstreamcli validate-tree --no-net dist/AppDir
```

## What remains for a signed, reproducible release

1. **Project identity decision.** Pick a three-component reverse-DNS id (for example
   `md.lmdv.App`), then change `APP_ID`, `lmdv.desktop`/`lmdv.svg` and the metainfo id
   together. This clears the AppStream rDNS warning.
2. **Release metadata.** No project homepage will be provided, so keep the
   `url-homepage-missing` warning as accepted. Add `<screenshots>` if a channel needs
   them.
3. **Release artifacts.** Tagged source tarball containing `vendor/rune/`, the raw binary
   and AppImage, the NSIS installer, signed checksums and a PGP-signed git tag.
4. **Reproducibility.** The release build is not bit-reproducible yet: Cargo embeds
   absolute paths. Set `SOURCE_DATE_EPOCH`, use `--remap-path-prefix`, and pin the
   AppImage/NSIS tool versions before claiming reproducible builds.
5. **AppImage build.** Build once with a release binary, check `ldd` coverage with
   `--appimage-extract`, test on a clean VM without Rust/GTK dev packages, and embed
   zsync `--update-info` at release time.
