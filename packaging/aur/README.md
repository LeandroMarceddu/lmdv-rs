# AUR package for lmdv

**Status:** recipe only. `makepkg` is installed on the development machine and
`.SRCINFO` was generated with it, but a full `makepkg` run needs the (not yet existing)
release tarball and downloads the pinned Rust toolchain. Treat `url=`, `source=` and
`sha256sums=` as release placeholders.

## Files

| File | Purpose |
|---|---|
| `PKGBUILD` | Source build (`pkgname=lmdv`), `x86_64` and `aarch64`. |
| `.SRCINFO` | Generated metadata for the AUR web interface. |

## Building

```bash
# in a clean checkout of the package directory
cd packaging/aur
updpkgsums              # once source=() points at a real release tarball
makepkg --printsrcinfo > .SRCINFO
makepkg -si             # build and install the package
makepkg -sr             # build in a clean chroot (recommended before uploading)
```

The PKGBUILD installs the Rune-pinned toolchain with Rustup during `prepare()` because
`Cargo.toml` requires Rust 1.99 and Rune pins the exact channel in
`rust-toolchain.toml`. That is a build-time network access; it is not a runtime
dependency, and the installed system never needs Rust.

`check()` runs the workspace test suite. It is heavy (the renderer, exporter and app
tests compile Typst/Rune test binaries); use `makepkg --nocheck` while iterating. GUI-only
tests are skipped unless `RUNE_GUI_TESTS=1` is set.

`sha256sums=('SKIP')` is intentional in the placeholder: run `updpkgsums` before
uploading. Once release tarballs are signed, add the maintainer key to `validpgpkeys`.

## What the package installs

```
/usr/bin/lmdv
/usr/share/applications/lmdv.desktop
/usr/share/icons/hicolor/scalable/apps/lmdv.svg
/usr/share/metainfo/md.lmdv.metainfo.xml
/usr/share/mime/packages/lmdv.xml
/usr/share/doc/lmdv/README.md
```

Arch's `shared-mime-info`, `desktop-file-utils` and `gtk-update-icon-cache` hooks refresh
the `/usr` caches when the package is installed or removed, so there is **no** `.INSTALL`
file (running the tools again would only be noise). The package does not set itself as
the default handler; that stays a user choice (`xdg-mime default lmdv.desktop
text/markdown`). The per-user installer *does* set it, which is why the dev registration
must be uninstalled before the packaged entry becomes the active handler.

No `options=(!strip)`: the release binary needs no debug symbols at runtime. Add it (plus
`debug`) only if you need symbolized panic backtraces.

## `lmdv-bin` variant

If release tarballs carry a prebuilt binary, copy this PKGBUILD and replace the build
half:

```bash
pkgname=lmdv-bin
depends=('glibc' 'gcc-libs' 'libxkbcommon' 'wayland' 'vulkan-icd-loader'
         'shared-mime-info' 'desktop-file-utils' 'hicolor-icon-theme')
makedepends=()
source=("$pkgname-$pkgver.tar.gz::$url/releases/download/v$pkgver/lmdv-$pkgver-$CARCH.tar.gz")
sha256sums=('REPLACE_ME')

package() {
  cd "$pkgname-$pkgver"
  install -Dm755 lmdv "$pkgdir/usr/bin/lmdv"
  install -Dm644 lmdv.desktop "$pkgdir/usr/share/applications/lmdv.desktop"
  install -Dm644 lmdv.svg "$pkgdir/usr/share/icons/hicolor/scalable/apps/lmdv.svg"
  install -Dm644 md.lmdv.metainfo.xml "$pkgdir/usr/share/metainfo/md.lmdv.metainfo.xml"
  install -Dm644 lmdv.xml "$pkgdir/usr/share/mime/packages/lmdv.xml"
}
```

The prebuilt tarball must be built on the matching architecture (`$CARCH`), not
cross-compiled against a newer glibc.

## Validation

```bash
namcap PKGBUILD
makepkg --printsrcinfo > .SRCINFO && git diff --exit-code .SRCINFO
```

`namcap` on the PKGBUILD checks dependency hygiene; run it on the built package
(``namcap lmdv-0.5.0-1-x86_64.pkg.tar.zst``) to catch missing shared-library or data
dependencies.

## Relationship to the per-user installer

`scripts/install-md-handler.sh` is unchanged and remains the development path: it writes
into `~/.local/share` and points a launcher at `target/{debug,release}`. This package
writes into `/usr` and is not aware of the launcher.

If both are present, XDG precedence makes the **per-user** `lmdv.desktop` win, so the
system package appears installed but double-click still starts the development build.
After installing the package, remove the per-user registration once:

```bash
scripts/install-md-handler.sh --uninstall
```

The uninstaller restores the handler that was default before the dev install, if it is
still ours; it never touches `/usr`.

## Known packaging findings

* `appstreamcli validate` warns `cid-desktopapp-is-not-rdns` for the `md.lmdv`
  component id and `url-homepage-missing` (no homepage by decision). Both are tracked
  in `packaging/README.md`; the id decision also affects `APP_ID` in the app.
* `desktop-file-validate` emits a hint (exit 0) that
  `Categories=Office;Viewer;Utility;` contains more than one main category. The entry
  then appears in both the Office and Utility menus by design.
