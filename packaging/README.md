# lmdv packaging

Recipes for shipping lmdv outside the development checkout: desktop metadata,
an AppImage builder and a Windows NSIS installer. Nothing here changes the
application code; the per-user development installer is described where its
files overlap with these recipes.

**Status:** `.github/workflows/release.yml` builds and publishes the Linux AppImage,
raw Linux binary, Windows NSIS installer and raw Windows binary on `v*` tags, plus
a `SHA256SUMS` file and its detached `SHA256SUMS.asc` signature. The AppStream
component id is `io.github.leandromarceddu.lmdv` (metainfo-only; runtime and data
root stay `lmdv`/`md.lmdv`). See the end of this file.

## Files

| Path | What it is |
|---|---|
| `lmdv.desktop` | System desktop entry (`Exec=lmdv %f`, `Icon=lmdv`, three MIME types). |
| `lmdv.metainfo.xml` | AppStream metadata, component id `md.lmdv`, launchable `lmdv.desktop`. |
| `lmdv.xml` | shared-mime-info package: aliases, extensions and glob weights for `text/markdown`. |
| `../assets/lmdv.svg` | Application icon, consumed by the AppImage and the per-user installer. |
| `../scripts/package-appimage.sh` | Builds/stages the AppDir and emits `dist/lmdv-<version>-<arch>.AppImage`. |
| `windows/lmdv.nsi` | NSIS installer: per-user install, Start Menu entry, optional desktop shortcut and default `.md` association. |
| `../.github/workflows/release.yml` | Tag-driven Linux AppImage + raw binary and Windows NSIS release, with `SHA256SUMS`. |
| `test-install-md-handler.sh` | Temp-HOME end-to-end test: template creation/uninstall/re-run, localized names, `$HOME` guard, icon fallback. |
| `test-release-checksums.sh` | Parses the release workflow and runs its checksum step on a fake `dist/`; asserts exactly `dist/*` is covered. |

## GitHub release

Push a `v*` tag. `.github/workflows/release.yml` builds and publishes:

- Linux: `dist/lmdv-<version>-<arch>.AppImage` and the raw `lmdv-<version>-x86_64` binary.
- Windows: `dist/lmdv-<version>-installer.exe` (NSIS) and the raw `dist/lmdv-<version>-windows-x86_64.exe`, built with `packaging/windows/lmdv.nsi`.
- `dist/SHA256SUMS`: one `sha256sum` row for every other published file, sorted
  by name (the four assets above plus `lmdv-release-key.asc`; never the staged
  AppDir, the checksum files themselves or GitHub's generated source archives).
- `dist/SHA256SUMS.asc`: the detached PGP signature over `SHA256SUMS`.
- `dist/lmdv-release-key.asc`: the public release key.

The checksum step runs after both artifact uploads are merged into `dist/` and
before the release is created. It fails the release when its rows and `dist/*`
disagree. Verify a download with:

```bash
gpg --import lmdv-release-key.asc
gpg --verify SHA256SUMS.asc SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
```

The release key fingerprint is
`DD029B151E6A78B4D2357B17FEFE77EDE1F2E862` (Ed25519, sign-only, expires
2028-11-24). The private key and passphrase live only in the repository's
GitHub Actions secrets; the public key is committed and published.

Release notes come from `docs/releases/<tag>.md` when that file exists; otherwise
GitHub generates them. Write the notes with the version bump, before tagging.

The NSIS installer associates `.md` at the user level and registers lmdv in Open with /
Default apps. On Windows 10/11 an existing per-user `UserChoice` still wins until the
user confirms lmdv in Settings > Default apps; the installer never deletes that key.

## Identity: `lmdv` vs `md.lmdv`

Three different names exist and must not be confused:

| Name | Where it comes from | Used for |
|---|---|---|
| `lmdv` | `APP_ID` in `crates/lmdv-app/src/shared.rs`; command name | Wayland `app_id`, X11 `WM_CLASS`, desktop file basename, icon name |
| `md.lmdv` | `APP_DIR` in `crates/lmdv-io/src/dirs.rs` | XDG config/data/cache/state root |
| `io.github.leandromarceddu.lmdv` | `packaging/lmdv.metainfo.xml` | AppStream component id (metainfo-only) |

Packages install `lmdv.desktop` + `lmdv.svg` so that the window identity matches the
desktop file on native Wayland. The AppStream id is
`io.github.leandromarceddu.lmdv`, with
`<launchable type="desktop-id">lmdv.desktop</launchable>` bridging it to the
installed entry; the runtime/data ids stay `lmdv`/`md.lmdv`.

Consequences, reported rather than hidden:

* `appstreamcli validate` now only warns `url-homepage-missing` (no homepage by
  decision); the rDNS warning is gone.
* `desktop-file-validate` emits an informational hint (exit 0) that
  `Categories=Office;Viewer;Utility;` has more than one main category. That is the
  requested category set; the app appears under both Office and Utilities.

Exec uses `%f`, not `%F`: the CLI parses only the first positional
(`docs/LINUX_INTEGRATION.md` §2.2), so `%F` would advertise multi-file selection and
silently drop files. Changing it is a one-character edit if the CLI grows multi-file
support.

## Relationship to `scripts/install-md-handler.sh`

The per-user installer remains the development path. The two layers are
deliberately separate:

| | `install-md-handler.sh` | release packages |
|---|---|---|
| Scope | `$HOME` only, no sudo | AppImage (read-only mount) / per-user Windows install |
| Exec | absolute path to the `~/.local/bin/lmdv` launcher | `lmdv %f` (AppImage) / `lmdv.exe "%1"` (NSIS) |
| Binary selection | picks `target/debug` vs `target/release` at launch | packaged binary only |
| New template | writes an empty Markdown template into the XDG Templates folder (below) | not installed by the image |
| Owner marker | `X-LMDV-Managed=true`, launcher marker | image/package ownership, no marker |
| Refresh | calls `update-*` itself | re-download the AppImage / run the installer |
| Uninstall | restores the previous default handler, removes only the empty template | delete the image + desktop bits / NSIS uninstaller |

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

## New → Markdown document template (S-08)

`scripts/install-md-handler.sh` writes one **empty** file into the user's XDG
Templates folder so file managers can offer **New → Markdown document**, the
Linux substitute for Explorer's New menu (`docs/BEHAVIOR_SPEC.md` S-08 and §3.19):

| | |
|---|---|
| Default path | the XDG Templates folder (`xdg-user-dir TEMPLATES`, usually `~/Templates`, localized: `~/Vorlagen`, `~/Modelli`, …) |
| File | `Markdown document.md`, empty, mode 0644 |
| Localized name | `Markdown-Dokument.md` (de), `Documento Markdown.md` (it/es), `Document Markdown.md` (fr), `Markdown 文档.md` (zh-CN) from `$LANGUAGE`/`$LC_ALL`/`$LC_MESSAGES`/`$LANG`; English otherwise |
| Opt out | `--no-template` |
| Custom folder | `--template-dir DIR` (refuses `$HOME` itself) |
| Record | `$XDG_STATE_HOME/lmdv/install/template.tsv` (header + absolute path) |
| Re-run | replaces the empty template, never a non-empty one unless `--force`; a changed locale renames it |
| Uninstall | removes it only while it is still empty; an edited template is left in place (warning) and the record is dropped, so `--force` is needed to manage it again |
| `--purge` | also removes the state directory |

**Desktop Environment caveat.** The entry exists only where the file manager
reads the XDG Templates folder:

| File manager | New → entry |
|---|---|
| GNOME Files (Nautilus), Nemo, Caja, Thunar | yes, from `$XDG_TEMPLATES_DIR` |
| Dolphin / KDE Frameworks **6.7+** (Sep 2024) | yes, plain files in `XDG_TEMPLATES_DIR` |
| Dolphin before KDE Frameworks 6.7 (including Plasma 5/KF5) | no plain-file entry; it reads only `.desktop` templates from `~/.local/share/templates`, which this installer does not write |
| Other file managers | depends on the file manager; nothing in the app can force an entry |

The Templates folder name is localized, so the installer resolves it instead of
assuming `~/Templates`. KDE 6.7 had a bug where a misconfigured
`XDG_TEMPLATES_DIR="$HOME/"` listed the whole home folder in Create New; the
installer prints a warning and skips the template rather than writing into
`$HOME`. The Windows row's localized display name is approximated by the
install-time locale: a later language change does not rename an installed
template until the installer is re-run. Re-run the installer after changing
the interface language if the localized name matters.

The template is intentionally empty (the spec's "creating empty .md"); the price
is that ownership cannot be a comment inside the file, hence the state record
and the empty-only deletion rule.

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
scripts/package-appimage.sh                    # cargo build --release -p lmdv + dist/lmdv-0.9.0-x86_64.AppImage
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
  ./lmdv-0.9.0-x86_64.AppImage --appimage-extract
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
bash -n scripts/package-appimage.sh scripts/install-md-handler.sh
shellcheck scripts/package-appimage.sh scripts/install-md-handler.sh
scripts/package-appimage.sh --dry-run

desktop-file-validate packaging/lmdv.desktop
appstreamcli validate --no-net packaging/lmdv.metainfo.xml
xmllint --noout packaging/lmdv.metainfo.xml packaging/lmdv.xml assets/lmdv.svg

packaging/test-install-md-handler.sh      # temp HOME; template + icon fallback
packaging/test-release-checksums.sh       # workflow YAML + the SHA256SUMS step
```

Both packaging tests write only under a fresh `mktemp` directory in
`/tmp/opencode` and clean it up on success (`--keep` or `LMDV_TEST_KEEP=1`
keeps it for evidence). `test-release-checksums.sh` needs PyYAML
(`python3 -m pip install --user pyyaml`) and prints `SKIP` without it.

The NSIS installer is compiled by the release workflow on `windows-latest`.

Staged-tree check (catches a metainfo launchable that does not resolve):

```bash
scripts/package-appimage.sh --binary ./target/release/lmdv --appdir-only
appstreamcli validate-tree --no-net dist/AppDir
```

## Project identity: rDNS decision (metainfo-only)

The AppStream component id is `io.github.leandromarceddu.lmdv` and the developer
id is `io.github.leandromarceddu`. This is the reverse-DNS id of the public
repository and clears `cid-desktopapp-is-not-rdns` with no runtime or data
migration.

Unchanged on purpose:

* **Runtime identity** stays `lmdv` (`APP_ID`, `lmdv.desktop`, `lmdv.svg`,
  `StartupWMClass`, installer desktop/icon names, Settings → Make default).
* **Data root** stays `md.lmdv` (`~/.config/md.lmdv`, drafts, cache, state).
  Renaming it would need a migration; the AppStream id does not require it.

`<launchable type="desktop-id">lmdv.desktop</launchable>` bridges the AppStream
id to the installed entry. A full runtime rename and a data-root migration remain
possible later; they are separate changes.

## What remains for a signed, reproducible release

1. **Release metadata.** No project homepage will be provided, so keep the
   `url-homepage-missing` warning as accepted. Add `<screenshots>` if a channel needs
   them.
2. **Signing.** `SHA256SUMS.asc` is produced by the release workflow with the
   dedicated key; its public half and fingerprint are published. Windows
   Authenticode and an AppImage signature still need platform tooling/certs.
3. **First signed release.** Confirm the `Sign SHA256SUMS` step on the next tag
   and verify the downloaded `.asc` before calling the release signed.
4. **Reproducibility.** The release build is not bit-reproducible yet: Cargo embeds
   absolute paths. Set `SOURCE_DATE_EPOCH`, use `--remap-path-prefix`, and pin the
   AppImage/NSIS tool versions before claiming reproducible builds.
5. **AppImage build.** Build once with a release binary, check `ldd` coverage with
   `--appimage-extract`, test on a clean VM without Rust/GTK dev packages, and embed
   zsync `--update-info` at release time.
