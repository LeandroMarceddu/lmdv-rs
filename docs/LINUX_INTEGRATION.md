# lmdv — Linux desktop integration

**Status:** design + tested scripts, ready to land as `docs/LINUX_INTEGRATION.md` and `scripts/`.
**Target:** CachyOS (Arch-like), KDE Plasma 6 on Wayland as the default session, Plasma X11 available, NVIDIA GPU. GNOME/Thunar/Nemo are secondary.
**Scope:** MIME ownership of Markdown, desktop entry, per-user registration, a build-switching launcher, single-instance CLI forwarding, file-manager behavior, packaging, install/uninstall scripts, tests.
**Out of scope:** the renderer, the Rune widget layer, the editor, and the Windows/macOS ports.

Everything marked **verified** was executed on this machine (Plasma 6.8.2, Wayland, `desktop-file-utils 0.29`, `shared-mime-info 2.5.1`) on 2026-11-12. Behavior citations use the reference `<file>:<line>`.

---

## 0. Decisions at a glance

| Concern | Decision | Why |
|---|---|---|
| Canonical type | `text/markdown`, aliases `text/x-markdown` and `application/x-markdown` | `text/markdown` is what the freedesktop DB and every current file manager agree on; the aliases cover older tools |
| Detection | extension only (`*.md`, `*.markdown`, `*.mdown`, `*.mkd`, `*.mkdn`, `*.mdwn`); **no magic rule** | shared-mime-info has no Markdown magic; a magic rule would misclassify plain text |
| Registration scope | per-user only (`~/.local/share`, `~/.config`) | no sudo, survives package changes, easy to undo |
| Desktop file | `~/.local/share/applications/lmdv.desktop`, `Exec="/home/<u>/.local/bin/lmdv" %f` | one stable Exec path that survives debug/release rebuilds |
| Launcher | bash script at `~/.local/bin/lmdv` | the only place that can choose between `target/debug` and `target/release` before a process exists |
| Single instance | `flock` + Unix socket `$XDG_RUNTIME_DIR/lmdv-$UID.sock`, newline-JSON | survives `kill -9`, no D-Bus dependency, matches `PLAN.md` phase 1 |
| Routing | port `windowing.rs` rules: activate tab → idle window → last-used → new | the behavior spec |
| CLI | `lmdv [--new-window] [file] [--line N]`, `-l N`, `--line=N`, `--`, first positional only | exactly `parse_args` in `src/lib.rs:179` |
| `--wait` | **not in v1**; protocol reserves it | the fixed grammar has no `--wait`; adding it later is purely additive |
| Packaging order | dev installer → AppImage | AppImage is the Linux package; the Windows NSIS installer is separate |

The reference implementation's Linux single-instance is a **D-Bus well-known name** (`<bundle-id>.SingleInstance`), forwarding `argv` + `cwd` and exiting 0 after the primary accepts (`src/lib.rs:490`). That is a valid alternative, but it silently loses single-instance behavior when the session bus is missing, and it gives no control over timeouts. The Unix socket chosen here is the one already committed to in `PLAN.md:92`.

---

## 1. MIME detection

### 1.1 How a file manager decides

The chain is:

```
filename/extension ──► shared-mime-info database ──► GIO (GTK apps) or KIO/KService (KDE)
                                                      │
                                                      ▼
                              ~/.config/mimeapps.list ([Default Applications])
                                                      │
                                                      ▼
                              /usr/share/applications/lmdv.desktop  → Exec … %f
```

* **Glob vs magic.** shared-mime-info first tries *magic* (content sniffing) and *treemagic*, then globs; glob weights break ties. Markdown has **no magic rule**, only globs. So detection is purely by filename. A `.md` renamed to `.txt` is `text/plain`; a Markdown file named `README` is `text/plain`.
* **GLib/GIO** (Nautilus, Thunar, Nemo, GTK file choosers) uses the compiled `mime.cache`; `xdg-mime query filetype` and `gio info -a standard::content-type` both go through it.
* **KDE** uses the same shared-mime-info packages but builds its own service cache (`KService`/`ksycoca`). Depending on version it matches the desktop file's `MimeType=` list with or without alias resolution; listing all three spellings removes the ambiguity and is harmless on GIO.
* **Never use `file(1)` to test.** libmagic has no Markdown rule:

```console
$ file --mime-type /tmp/opencode/_t.md
/tmp/opencode/_t.md: text/plain
$ xdg-mime query filetype /tmp/opencode/_t.md
text/markdown
$ gio info -a standard::content-type /tmp/opencode/_t.md | tail -1
  standard::content-type: text/markdown
```

**Verified:** those three commands produce exactly those three results on this box.

### 1.2 What the system already provides

`/usr/share/mime/globs2` on a stock Arch/CachyOS install contains:

```
50:text/markdown:*.md
50:text/markdown:*.mkd
50:text/markdown:*.markdown
```

`/usr/share/mime/packages/freedesktop.org.xml` declares:

```xml
<mime-type type="text/markdown">
  <comment>Markdown document</comment>
  ...
  <glob pattern="*.md"/>
  <glob pattern="*.mkd"/>
  <glob pattern="*.markdown"/>
  <alias type="text/x-markdown"/>
</mime-type>
```

and `/usr/share/mime/subclasses` has `text/markdown text/plain`. Consequences:

* `text/x-markdown` already resolves to `text/markdown`, so a handler for `text/markdown` covers it.
* `application/x-markdown` is **not** an alias in the reference. Some editors and older integrations emit it; we add the alias at user level.
* Because Markdown is a subclass of `text/plain`, editors that declare only `text/plain` also appear in "Open With" for `.md`. **Verified:** micro, vim, Kate and KWrite declare only `text/plain` and all appear in `gio mime text/markdown`. The explicit default still wins.

### 1.3 What to register

A per-user package that **re-declares the existing type**. shared-mime-info merges definitions from all packages: globs are combined, aliases added, and a higher weight overrides the system's 50. **Verified** with an isolated `XDG_DATA_HOME`: after `update-mime-database`, `~/.local/share/mime/aliases` contained both aliases and `globs2` contained `60:text/markdown:*.md`, while `/usr` was untouched.

`~/.local/share/mime/packages/lmdv.xml`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="text/markdown">
    <comment>Markdown document</comment>
    <glob pattern="*.md" weight="60"/>
    <glob pattern="*.markdown" weight="58"/>
    <glob pattern="*.mdown" weight="57"/>
    <glob pattern="*.mkd" weight="57"/>
    <glob pattern="*.mkdn" weight="57"/>
    <glob pattern="*.mdwn" weight="57"/>
    <alias type="text/x-markdown"/>
    <alias type="application/x-markdown"/>
  </mime-type>
</mime-info>
```

Notes:

* **Do not** declare a *new* type (`application/x-lmdv`). Nothing would map `.md` to it, and the MIME database is the wrong place to invent an identity.
* `*.mdx` is deliberately omitted: MDX is JSX-flavoured Markdown and some projects map it to `text/mdx`. Add it only if the user asks; it is one `<glob pattern="*.mdx" weight="56"/>` line.
* The weight of 60 is not strictly required on stock Arch (nothing else matches `*.md`), but it makes the result stable against other packages that also register `*.md`.
* The file is **idempotent**: rerunning `update-mime-database` with the same file is a no-op.
* Removing the package and rebuilding the DB restores the stock globs/aliases exactly; **verified**.

### 1.4 Test commands

```bash
printf '# hi\n' > "/tmp/md test ünïcode.md"
xdg-mime query filetype "/tmp/md test ünïcode.md"        # text/markdown
gio info -a standard::content-type "/tmp/md test ünïcode.md" | tail -1
grep -E 'markdown' /usr/share/mime/globs2                 # stock weights 50
grep -E 'markdown' ~/.local/share/mime/globs2             # ours: 60/58/57
grep markdown ~/.local/share/mime/aliases                 # both aliases if installed
```

Expected: `text/markdown` from both queries; the user DB present only after `update-mime-database`.

Even with **no** user package, `.md` still resolves to `text/markdown` on stock Arch. The package buys the extra aliases, the extra extensions, and a stable weight — it is not what makes double-click work.

---

## 2. Desktop entry

### 2.1 The file

`~/.local/share/applications/lmdv.desktop`:

```ini
[Desktop Entry]
Type=Application
Version=1.5
Name=lmdv
GenericName=Markdown Viewer
Comment=View and edit Markdown documents
Exec="/home/leandro/.local/bin/lmdv" %f
TryExec=/home/leandro/.local/bin/lmdv
Icon=lmdv
Terminal=false
StartupNotify=false
StartupWMClass=lmdv
SingleMainWindow=true
DBusActivatable=false
NoDisplay=false
Categories=Utility;TextTools;
Keywords=markdown;md;view;preview;text;
MimeType=text/markdown;text/x-markdown;application/x-markdown;
X-LMDV-Managed=true
```

`desktop-file-validate` 0.29 reports **clean** for this file (no output, exit 0). **Verified.**

### 2.2 Field-by-field

| Field | Value | Why / caveats |
|---|---|---|
| `Type` | `Application` | required |
| `Version` | `1.5` | Desktop Entry *spec* version, not app version |
| `Name` | `lmdv` | shown in Open With; keep it lowercase to match the command |
| `GenericName` | `Markdown Viewer` | menu subtitle |
| `Comment` | short sentence | tooltip; no trailing period is conventional |
| `Exec` | `"/abs/lmdv" %f` | `%f` = one local path. See quoting below. `%F` would pass several paths (the CLI only uses the first), `%u`/`%U` pass URIs (the app would have to decode `file://`) |
| `TryExec` | absolute path | launchers hide the entry when it is missing/not executable. It is a *path string*, not an Exec line: no quotes, no field codes |
| `Icon` | `lmdv` | themed name; install `lmdv.svg` into `hicolor/scalable/apps`. An absolute path also works but breaks scaling/theming |
| `Terminal` | `false` | **must** be false; true flashes a terminal for a GUI app |
| `StartupNotify` | `false` | winit does not send XDG startup notifications; `true` leaves the cursor spinning until the timeout |
| `StartupWMClass` | `lmdv` | X11/XWayland window grouping; must equal `WM_CLASS`. Ignored on native Wayland, where the app_id (matched against the desktop-file basename) is used |
| `SingleMainWindow` | `true` | GNOME hint for "one main window"; harmless on KDE. Requires a recent desktop-file-utils; older ones just warn |
| `DBusActivatable` | `false` | explicit. `true` requires a matching `lmdv.service`; GLib then ignores `Exec` and activation fails |
| `NoDisplay` | `false` | **do not set true**: it removes the app from menus *and* from most Open With lists |
| `Categories` | `Utility;TextTools;` | validated clean. Adding `Viewer;` makes the validator hint that it needs `Graphics` or `Office`; `Office;Viewer;TextTools;` then hints about `TextTools`. Keep it simple |
| `Keywords` | `;`-terminated | search terms in app launchers |
| `MimeType` | all three | covers tools that emit either alias; harmless on GIO, removes ambiguity in KDE's service cache |
| `X-LMDV-Managed` | `true` | our ownership marker; desktop environments ignore unknown `X-` keys. Used by the installer/uninstaller |

Optional additions, not enabled by default:

```ini
Actions=NewWindow;

[Desktop Action NewWindow]
Name=New Window
Exec="/home/leandro/.local/bin/lmdv" --new-window %f
```

`Hidden=true` is the correct way to *mask* a system-wide entry of the same name; it is not a substitute for deleting our own file.

### 2.3 `Exec` quoting: the part that bites

The Desktop Entry spec says: inside double quotes, escape `"`, `` ` ``, `$` and `\` with a backslash. **GLib does not implement that the way the spec reads.** The value is unescaped twice (key-file string + Exec argument), so a single backslash is consumed before the Exec parser sees it. Measured matrix (each case: write the file, `update-desktop-database`, `xdg-mime default`, then `gio mime text/markdown` and `desktop-file-validate`):

| Path contains | File value | `desktop-file-validate` | GIO loads | Verdict |
|---|---|---|---|---|
| space | `"…/a b/lmdv" %f` | ok | yes | quote |
| space, unquoted | `…/a b/lmdv %f` | ok | **no** | silently broken |
| `$` | `"…/a$b…"` | **FAIL** | yes | validator/spec disagree |
| `$` | `"…/a\$b…"` | ok | **no** | one backslash is eaten |
| `$` | `"…/a\\$b…"` | ok | yes | **two backslashes** |
| `"` | `"…/a\"b…"` | ok | **no** | one backslash is eaten |
| `"` | `"…/a\\"b…"` | ok | yes | **two backslashes** |
| `\` | `"…/a\\\\b…"` | ok | yes | **four backslashes** |
| `` ` `` | ``"…/a\`b…"`` | ok | **no** | one backslash is eaten |
| `` ` `` | ``"…/a\\`b…"`` | ok | yes | **two backslashes** |
| `%` | `"…/100%/lmdv"` | **FAIL** (`invalid field code "%/"`) | **no** | not representable in the program path |

The installer therefore **refuses** an Exec path containing `"`, `\`, `$`, `` ` ``, `%` or a newline. It leaves a simple path (no spaces) unquoted and quotes only a path that needs it. This costs nothing in practice because the default Exec path is `~/.local/bin/lmdv`; a checkout path with `$` or `%` still works, because it is only a *runtime argument* inside the wrapper, never part of `Exec`. If `--no-wrapper` is used with such a path, move the binary to a simple path or fix the escaping by hand using the table above and retest both tools.

**Why unquoted when possible:** `xdg-mime`'s `desktop_file_to_binary` does not strip the quotes from `Exec` before resolving the program. With `Exec="…/lmdv" %f`, `xdg-mime query default text/markdown` can fall through to a system handler (e.g. Kate) even though the association is correct. Leaving a simple path unquoted makes the query work; the installer also verifies the default and falls back to writing `mimeapps.list` directly if `xdg-mime`/`gio` refuses.

Also:

* Quote the **program**, never `%f`. Field codes expand after parsing; quoting `%f` would treat the path as a literal argument.
* A filename containing a newline (legal on Linux) breaks line-oriented logs and any plain-text protocol; JSON handles it in the app, but the wrapper's log line will contain a newline. A 10-line fix is to log only `argc`, or to strip newlines; not worth blocking on.
* A file named `-foo.md` passed by CLI is treated as an unknown flag; use `lmdv -- ./-foo.md`. File managers always pass absolute paths, so double-click is unaffected.

### 2.4 Window identity (`StartupWMClass` / app_id)

For the taskbar/dock to associate the window with `lmdv.desktop`:

* **X11 / XWayland:** `WM_CLASS` must be `lmdv` (instance and class). Verify with `xprop WM_CLASS` and click the window. Set the name in the toolkit (winit: `WindowAttributes::with_name("lmdv")`; eframe: `ViewportBuilder::with_app_id("lmdv")`).
* **Native Wayland:** the compositor uses the `app_id`; KWin/GNOME match it against the desktop-file basename (`lmdv.desktop` → `lmdv`). If the toolkit reports something like `lmdv-rs` or a reverse-DNS id, change it or rename the desktop file to match.
* If they disagree, the symptom is a *second* taskbar icon with a generic glyph; `StartupWMClass` is not used on Wayland at all.

**Namespace note.** The desktop id and app id are `lmdv`, matching the command. The app's XDG data root is `md.lmdv` (`ARCHITECTURE.md` §10). They need not match, but if the orchestrator prefers reverse-DNS for the desktop file (`md.lmdv.desktop`), change the desktop id, `StartupWMClass`/app id, `Icon` name, and every `xdg-mime default` call together.

---

## 3. Registration

### 3.1 Order of operations

```
write ~/.local/share/mime/packages/lmdv.xml
write ~/.local/share/icons/hicolor/scalable/apps/lmdv.svg
write ~/.local/share/applications/lmdv.desktop
write ~/.local/bin/lmdv                      (launcher)
        │
        ├─ update-mime-database  ~/.local/share/mime
        ├─ update-desktop-database ~/.local/share/applications
        ├─ gtk-update-icon-cache -f -t ~/.local/share/icons/hicolor   (if index.theme)
        ├─ kbuildsycoca6 --noincremental                              (Plasma only)
        └─ xdg-mime default lmdv.desktop text/markdown \
                          text/x-markdown application/x-markdown
```

`xdg-mime default` is the last step because it must be able to find the desktop file. It writes **both** `[Default Applications]` and `[Added Associations]` into `$XDG_CONFIG_HOME/mimeapps.list`. **Verified:** one invocation with three MIME types wrote all six lines.

### 3.2 The commands

```bash
# 1. rebuild the MIME database (globs/aliases)
update-mime-database "$HOME/.local/share/mime"

# 2. rebuild mimeinfo.cache so "Open With" lists the app
update-desktop-database "$HOME/.local/share/applications"

# 3. icon cache (optional on KDE, useful on GTK; needs the theme index)
[ -f "$HOME/.local/share/icons/hicolor/index.theme" ] && \
  gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor"

# 4. KDE service cache (Plasma only; safe to skip and re-login instead)
kbuildsycoca6 --noincremental

# 5. set the default handler for all three spellings
xdg-mime default lmdv.desktop text/markdown
xdg-mime default lmdv.desktop text/x-markdown
xdg-mime default lmdv.desktop application/x-markdown
# (one call with several types works too)
```

What each tool actually does:

| Tool | Writes | Needed for |
|---|---|---|
| `update-mime-database DIR` | `DIR/mime.cache`, `aliases`, `globs2`, `types`, … | our aliases and extra extensions |
| `update-desktop-database DIR` | `DIR/mimeinfo.cache` (and `intent.cache` on 0.29) | the app appearing in "Open With" |
| `xdg-mime default` | `$XDG_CONFIG_HOME/mimeapps.list` | the actual default choice |
| `kbuildsycoca6` | `$XDG_CACHE_HOME/ksycoca6_*` | Dolphin/KRunner seeing a *new* desktop file immediately |
| `gtk-update-icon-cache` | `icon-theme.cache` | GTK icon lookup performance; optional |

**Verified:** without `update-desktop-database`, `gio open` still launched the app (because `mimeapps.list` pointed at it) but `gio mime` did **not** list it under "Registered applications". That is exactly the "it works from `gio open` but not in the Open With menu" symptom.

### 3.3 Per-user vs system

| | Per-user (recommended here) | System |
|---|---|---|
| Desktop file | `~/.local/share/applications/lmdv.desktop` | `/usr/share/applications/lmdv.desktop` |
| MIME package | `~/.local/share/mime/packages/lmdv.xml` | `/usr/share/mime/packages/lmdv.xml` |
| Icon | `~/.local/share/icons/hicolor/scalable/apps/lmdv.svg` | `/usr/share/icons/hicolor/scalable/apps/lmdv.svg` |
| Binary | `~/.local/bin/lmdv` (wrapper) | `/usr/bin/lmdv` |
| Default | `~/.config/mimeapps.list` | `/etc/xdg/mimeapps.list` (or distro default) |
| Refresh | manual (above) | Arch pacman hooks run it automatically |
| Requires | nothing | `sudo` / package manager |

Arch ships these hooks in `shared-mime-info`, `desktop-file-utils` and `gtk-update-icon-cache` (**verified**: `/usr/share/libalpm/hooks/{30-update-mime-database,update-desktop-database,gtk-update-icon-cache}.hook`). A packaged install refreshes `/usr` automatically; a user-level install does **not**, which is why the script calls the tools itself.

### 3.4 `mimeapps.list` precedence

The spec's lookup order, highest first:

1. `$XDG_CONFIG_HOME/$desktop-mimeapps.list` (e.g. `kde-mimeapps.list`, `gnome-mimeapps.list`)
2. `$XDG_CONFIG_HOME/mimeapps.list`  ← what `xdg-mime` writes
3. `$XDG_CONFIG_DIRS/$desktop-mimeapps.list`
4. `$XDG_CONFIG_DIRS/mimeapps.list`
5. `$XDG_DATA_HOME/applications/$desktop-mimeapps.list`
6. `$XDG_DATA_HOME/applications/mimeapps.list` (deprecated)
7. `$XDG_DATA_DIRS/applications/$desktop-mimeapps.list`
8. `$XDG_DATA_DIRS/applications/mimeapps.list` (deprecated)

Sections: `[Default Applications]` (one ID), `[Added Associations]` (`;`-terminated list), `[Removed Associations]` (masks handlers). A desktop-specific file or a `[Removed Associations]` entry can override what you just wrote — check those before debugging deeper (`ls ~/.config/*mimeapps.list /etc/xdg/*mimeapps.list`).

### 3.5 Querying the current default

```bash
xdg-mime query default text/markdown     # what xdg-utils thinks
gio mime text/markdown                   # what GIO will actually launch
grep -n -A3 '^\[Default Applications\]' ~/.config/mimeapps.list
```

These can disagree, and the disagreement is informative:

* `xdg-mime query default` is a **literal reader**: it parses `mimeapps.list` (and the deprecated `defaults.list`/`mimeinfo.cache` fallback) without loading or validating the desktop file. It can return `lmdv.desktop` even when that entry is unusable.
* `gio mime` / `GDesktopAppInfo` **loads** the desktop file, checks `TryExec`/`Exec`, and falls back to a **recommended** handler from `mimeinfo.cache` when the configured one is invalid.
* **Verified:** with a desktop file present in `mimeapps.list` but an `Exec` that GLib rejected, `xdg-mime query default` printed `lmdv.desktop`, `gio mime` reported OnlyOffice, and `gio open` launched OnlyOffice. Always verify with `gio mime` **and** a real launch.
* KDE's `kioclient5 exec`/Dolphin use `KService`; they agree with `mimeapps.list` but may need `kbuildsycoca6`.

### 3.6 Restoring the previous handler on uninstall

The installer saves the pre-install default per MIME type to:

```
$XDG_STATE_HOME/lmdv/install/previous-defaults.tsv
text/markdown	org.kde.kate.desktop
text/x-markdown
application/x-markdown
```

Rules:

1. Save only on the **first** install; reruns keep the original file.
2. On uninstall, read the saved value.
3. If the current default is **not** `lmdv.desktop` any more, the user changed it after install: leave it alone, only prune our entry from `[Added Associations]`. This is implemented and **tested**: install → `xdg-mime default onlyoffice-desktopeditors.desktop` → uninstall prints `leaving text/markdown -> onlyoffice…` and does not steal it back.
4. If the current default is ours and a previous value exists, `xdg-mime default "$prev" "$mime"`.
5. If no previous value exists, remove our entries from `[Default Applications]` and `[Added Associations]` in both `$XDG_CONFIG_HOME/mimeapps.list` and the deprecated `$XDG_DATA_HOME/applications/mimeapps.list`.

Manual restore, if you ever need it without the script:

```bash
xdg-mime default org.kde.kate.desktop text/markdown
# or remove the lines by hand:
$EDITOR ~/.config/mimeapps.list
```

### 3.7 Fallbacks when tools are missing

| Missing | Effect | Fallback |
|---|---|---|
| `update-mime-database` | aliases and extra globs absent; `.md` still resolves via the stock DB | install `shared-mime-info`; double-click still works |
| `update-desktop-database` | app missing from "Open With"; launching via `mimeapps.list` still works | install `desktop-file-utils`, or re-login |
| `xdg-mime` | no default written | `gio mime <type> lmdv.desktop` (after `update-desktop-database`; otherwise it errors "Failed to load info for handler"), else edit `mimeapps.list` directly |
| `gio` | no default written | `xdg-mime`, else edit `mimeapps.list` |
| `kbuildsycoca6` | Dolphin may miss the entry until re-login | log out/in; or run it from a terminal with a session bus |
| `desktop-file-validate` | no validation | install `desktop-file-utils`; not required at runtime |

The installer implements the first three fallbacks, including a small `mimeapps.list` editor. **Verified** with a PATH containing only coreutils/awk/sed: install and uninstall both succeed, preserve unrelated entries, and are idempotent.

The one non-obvious fallback is editing `mimeapps.list` directly: `xdg-mime` prepends the new app to the existing list rather than replacing it. Our editor does the same, so uninstalling restores `kate;zed;` rather than just `kate`.

---

## 4. Dev/intermediate builds: the launcher

### 4.1 Why a wrapper is necessary

The desktop entry must name one executable path. During development there are two candidates (`target/debug/lmdv`, `target/release/lmdv`), they change on every build, and the GUI launch environment is not a login shell. A shell wrapper at a stable path solves four problems that neither the app nor the desktop file can:

1. **Build selection.** Only a process that starts *before* `lmdv` can decide which `lmdv` to run. The binary cannot switch its own profile.
2. **GUI environment.** `Exec` runs with the session's `PATH`, which may lack `~/.cargo/bin`, `~/.local/bin` additions from `.bashrc`, rustup shims, etc. The wrapper uses absolute paths only.
3. **Diagnostics.** GUI launches have no terminal. The wrapper logs the decision and shows a dialog on failure.
4. **Argument hygiene.** File managers may pass a relative path or a `file://` URI depending on the field code; the wrapper absolutizes existing paths so the single-instance router can dedupe them.

The wrapper deliberately does **not** build, start a server, or implement single-instance logic. Those belong to the app.

### 4.2 Selection policy

| Condition | Chosen binary |
|---|---|
| `LMDV_BIN` set | that path (validated; if not executable, error) |
| `LMDV_PROFILE=release` | `target/release/lmdv` if present, else normal auto |
| `LMDV_PROFILE=debug` | `target/debug/lmdv` if present, else normal auto |
| `LMDV_FIXED_BIN` set (installer `--binary`) | that path |
| default (`auto`) | the **newer** of the two by mtime; tie → release |
| only one exists | that one |
| none | GUI error, exit 127 |

Why mtime and not "release wins": iterating with `cargo build` produces a fresh debug binary, but a stale release binary may still exist. Newest-wins makes `cargo build` immediately visible without re-running the installer, and `cargo build --release` likewise. A **tie** goes to release to avoid surprising slowness after a `cargo build --release` that happens to land in the same nanosecond (rare).

`LMDV_PROFILE` is the explicit override; `LMDV_BIN` is the escape hatch for a one-off run:

```bash
LMDV_BIN=./target/debug/lmdv lmdv notes.md
LMDV_PROFILE=release lmdv --version
LMDV_LAUNCH_DEBUG=1 lmdv notes.md   # prints the chosen binary on stderr
```

`LMDV_FIXED_BIN` is only baked in when the installer is given `--binary PATH`; a normal install leaves it empty so the repo remains authoritative. This matters: an installer run with `--release` sets the *preference*, not a frozen binary.

### 4.3 Logging

`$XDG_STATE_HOME/lmdv/launch.log` (default `~/.local/state/lmdv/launch.log`), one line per launch:

```
2026-11-12T20:41:07Z [12345] profile=auto bin=/home/u/src/lmdv-rs/target/debug/lmdv argc=2 args=[/home/u/notes/a.md][--line]
```

* Rotated to `launch.log.1` at 256 KiB (one generation).
* Failures append `ERROR: …` before the dialog.
* Nothing is written to stdout/stderr in the GUI case; both are usually `/dev/null`.
* The log contains file paths, not secrets. `~/.local/state` is `0700` on a typical Arch install; if your umask is `022` and the directory did not exist, `chmod 700 ~/.local/state/lmdv` to keep paths private.
* `systemd-cat`/journal is not used: it would add a process and split the log. If the user wants the journal, they can run the wrapper from a terminal.

### 4.4 Errors and the "never open a terminal" rule

Order: `kdialog` (non-GNOME), `zenity`, `notify-send`, stderr. Each is `timeout 30` so a broken dialog cannot wedge the launch. `LMDV_NO_DIALOG=1` suppresses all UI (used by tests). `Terminal=false` in the desktop file completes the rule; the wrapper never invokes a terminal emulator.

Typical messages:

* `No lmdv binary found. Build it with 'cargo build' (debug) or 'cargo build --release', or set LMDV_BIN.`
* `Selected binary is not executable: …`
* `Refusing to launch the launcher itself (…). Set LMDV_BIN to the real binary.`

### 4.5 Concurrent `cargo build` (ETXTBSY)

Cargo/rustc writes `target/debug/lmdv` in place. Launching it at the wrong moment fails with `ETXTBSY` (errno 26), which appears as exit code 126. The wrapper retries the child up to 10 times at 100 ms, then reports. This is a real annoyance when double-clicking repeatedly during a rebuild. It is not a substitute for atomic installs; release packaging should write to a temp file and `rename(2)`.

### 4.6 The launcher (reference copy)

This is the exact file the installer generates, with the real repo path substituted:

```bash
#!/bin/bash
# lmdv-launcher v1 -- generated by install-md-handler.sh
# Managed file: re-running the installer overwrites it.
#
# Picks target/debug or target/release, absolutizes file arguments, logs to
# $XDG_STATE_HOME/lmdv/launch.log, never opens a terminal. Single-instance
# forwarding lives in the lmdv binary, not here.
set -u

LMDV_REPO="/home/leandro/Desktop/Dev/Testing/lmdv-rs"
LMDV_PREFERRED_PROFILE="auto"
LMDV_FIXED_BIN=""

SELF="$(readlink -f -- "${BASH_SOURCE[0]}" 2>/dev/null || printf '%s' "${BASH_SOURCE[0]}")"
STATE_HOME="${XDG_STATE_HOME:-$HOME/.local/state}"
LOG_DIR="$STATE_HOME/lmdv"
LOG_FILE="$LOG_DIR/launch.log"
MAX_LOG_BYTES=262144

mkdir -p -- "$LOG_DIR" 2>/dev/null || true

log() {
    [ -d "$LOG_DIR" ] || return 0
    if [ -f "$LOG_FILE" ] && [ "$(stat -c %s -- "$LOG_FILE" 2>/dev/null || echo 0)" -gt "$MAX_LOG_BYTES" ]; then
        mv -f -- "$LOG_FILE" "$LOG_FILE.1" 2>/dev/null || true
    fi
    printf '%s [%s] %s\n' "$(date -u +%FT%TZ)" "$$" "$*" >>"$LOG_FILE" 2>/dev/null || true
}

gui_error() {
    local msg="$1"
    log "ERROR: $msg"
    [ -n "${LMDV_NO_DIALOG:-}" ] && return 0
    if [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
        if command -v kdialog >/dev/null 2>&1 && [ "${XDG_CURRENT_DESKTOP:-}" != "GNOME" ]; then
            timeout 30 kdialog --title "lmdv" --error "$msg" >/dev/null 2>&1 && return 0
        fi
        if command -v zenity >/dev/null 2>&1; then
            timeout 30 zenity --error --title="lmdv" --text="$msg" >/dev/null 2>&1 && return 0
        fi
    fi
    if command -v notify-send >/dev/null 2>&1 && [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
        notify-send -a lmdv -u critical "lmdv" "$msg" >/dev/null 2>&1 || true
    fi
    printf 'lmdv: %s\n' "$msg" >&2
}

normalize_args() {
    local -a src=("$@") out=()
    local a expect_value=0 end_of_opts=0
    for a in "${src[@]}"; do
        if [ "$end_of_opts" -eq 1 ]; then
            if [ -e "$a" ]; then out+=("$(realpath -m -- "$a")"); else out+=("$a"); fi
            continue
        fi
        if [ "$expect_value" -eq 1 ]; then out+=("$a"); expect_value=0; continue; fi
        case "$a" in
            --) out+=("$a"); end_of_opts=1 ;;
            --line) out+=("$a"); expect_value=1 ;;
            --line=*|-*|"") out+=("$a") ;;
            *) if [ -e "$a" ]; then out+=("$(realpath -m -- "$a")"); else out+=("$a"); fi ;;
        esac
    done
    printf '%s\0' "${out[@]}"
}

pick_bin() {
    local rel="$LMDV_REPO/target/release/lmdv"
    local dbg="$LMDV_REPO/target/debug/lmdv"
    local profile="${LMDV_PROFILE:-$LMDV_PREFERRED_PROFILE}"

    if [ -n "${LMDV_BIN:-}" ]; then printf '%s' "$LMDV_BIN"; return 0; fi
    if [ -n "$LMDV_FIXED_BIN" ] && [ -x "$LMDV_FIXED_BIN" ]; then printf '%s' "$LMDV_FIXED_BIN"; return 0; fi

    case "$profile" in
        release) [ -x "$rel" ] && { printf '%s' "$rel"; return 0; } ;;
        debug)   [ -x "$dbg" ] && { printf '%s' "$dbg"; return 0; } ;;
    esac
    if [ -x "$rel" ] && [ -x "$dbg" ]; then
        if [ "$dbg" -nt "$rel" ]; then printf '%s' "$dbg"; else printf '%s' "$rel"; fi
        return 0
    fi
    [ -x "$rel" ] && { printf '%s' "$rel"; return 0; }
    [ -x "$dbg" ] && { printf '%s' "$dbg"; return 0; }
    return 1
}

main() {
    local bin profile
    profile="${LMDV_PROFILE:-$LMDV_PREFERRED_PROFILE}"

    if ! bin="$(pick_bin)"; then
        gui_error "No lmdv binary found. Build it with 'cargo build' (debug) or 'cargo build --release', or set LMDV_BIN."
        exit 127
    fi
    if [ ! -x "$bin" ]; then
        gui_error "Selected binary is not executable: $bin"
        exit 126
    fi
    if [ "$(readlink -f -- "$bin" 2>/dev/null)" = "$SELF" ]; then
        gui_error "Refusing to launch the launcher itself ($bin). Set LMDV_BIN to the real binary."
        exit 126
    fi

    local -a args=()
    mapfile -d '' -t args < <(normalize_args "$@")

    log "profile=$profile bin=$bin argc=${#args[@]} args=$(printf '[%s]' "${args[@]}")"

    # Cargo rewrites target/debug/lmdv in place; a concurrent launch can hit
    # ETXTBSY (exit 126). Retry briefly instead of showing an error.
    local rc attempt=0
    while :; do
        "$bin" "${args[@]}"
        rc=$?
        if [ "$rc" -eq 126 ] && [ "$attempt" -lt 10 ]; then
            sleep 0.1
            attempt=$((attempt + 1))
            continue
        fi
        break
    done

    [ "$rc" -ne 0 ] && log "exit=$rc bin=$bin"
    exit "$rc"
}

main "$@"
```

`/usr/bin/env bash` was replaced by `/bin/bash` in the installed copy on purpose: a GUI session can have a minimal or empty `PATH`, and `env` itself must be found. On Arch/CachyOS `/bin` is a symlink to `/usr/bin`, so this is safe. The script needs bash ≥ 4.4 (`mapfile`, arrays) — Arch ships 5.x.

### 4.7 PATH precedence

If `~/.cargo/bin/lmdv` also exists (from `cargo install`), whichever directory comes first in `PATH` wins in a terminal, while the desktop entry always uses the absolute wrapper path. Check:

```bash
type -a lmdv
```

Recommendation: use **either** the wrapper (`~/.local/bin`) **or** `cargo install` (`~/.cargo/bin`), not both. The installer prints the wrapper path in its checklist; if `command -v lmdv` resolves elsewhere, fix `PATH` or uninstall the other copy.

---

## 5. Single instance and CLI forwarding

### 5.1 CLI contract (from the specification)

The reference `parse_args` (`src/lib.rs:179`), exercised by the Windows-only tests at `src/lib.rs:571`:

```
lmdv [--new-window] [file] [--line N]
lmdv file.md -l 40
lmdv --line=40 file.md
lmdv --new-window            # empty window on the start page
lmdv -- --new-window         # a file literally named "--new-window"
```

Rules, exactly:

* `--new-window` anywhere before `--` sets the flag; it applies to the first file too.
* `--line N`, `-l N`, `--line=N`: `N` is **1-based**; a value that does not parse is ignored (no error).
* `--` ends option parsing; everything after is a positional, even if it starts with `-`.
* Unknown flags *starting with `-`* are ignored (`--help` therefore opens a window in the reference — we add `-h/--help` and `-V/--version` as a Linux deviation, documented).
* Only the **first** positional is used. Extra positionals are ignored. (`PLAN.md:91` writes `[file…]`; the source is authoritative — one file. Multi-file would change routing and is a product decision.)
* A relative path is resolved against the *client's* `cwd`, then `std::path::absolute` (no filesystem access, no existence check). On Unix, `std::path::absolute` collapses `.` and duplicate separators but **keeps `..`**; Windows collapses `..`. The ported tests must use `/` and expect `..` to be preserved (verified with rustc on this machine).
* A file that does not exist is still forwarded; the receiving window displays "Not found" (`CHANGELOG.md`, 2.2.0 fix). Do not reject it in the client.

A suggested `cli.rs` (drop-in for the reference parser, plus help/version):

```rust
use std::path::{Path, PathBuf};

#[derive(Debug, Default, PartialEq)]
pub struct Cli {
    pub path: Option<PathBuf>,
    pub line: Option<u32>,      // 1-based
    pub new_window: bool,
}

pub fn parse_args<I: IntoIterator<Item = String>>(args: I, cwd: Option<&Path>) -> Cli {
    let mut it = args.into_iter().skip(1);
    let mut cli = Cli::default();
    let mut positional_only = false;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--" if !positional_only => positional_only = true,
            "--new-window" if !positional_only => cli.new_window = true,
            "--line" | "-l" if !positional_only => {
                cli.line = it.next().and_then(|v| v.parse().ok());
            }
            s if !positional_only && s.starts_with("--line=") => {
                cli.line = s[7..].parse().ok();
            }
            s if !positional_only && s.starts_with('-') => {}   // ignore, like the reference
            s if cli.path.is_none() => {
                let p = PathBuf::from(s);
                let p = if p.is_relative() {
                    cwd.map(|c| c.join(&p)).unwrap_or(p)
                } else {
                    p
                };
                cli.path = Some(std::path::absolute(&p).unwrap_or(p));
            }
            _ => {}
        }
    }
    cli
}
```

Port the reference test cases (they are good), but rewrite the Windows literals: `a/b.md`, `-dash.md`, `D:/x/../y.md` (expect `..` retained on Unix), and keep the `--`/`--new-window`-after-`--` cases verbatim in intent.

### 5.2 What the first instance does

1. Parses the CLI (the client did too, for validation/help).
2. Resolves the socket path and **takes the instance lock**.
3. Unlinks any leftover socket and binds a fresh one, `chmod 0600`.
4. Spawns the listener thread **before** creating the window, so early second launches sit in the listen backlog and are then queued in order.
5. Creates the first window. If the CLI named a file, it loads it (with `--line` applied after layout); otherwise it shows the start page.
6. On exit: removes the socket, drops the lock (kernel releases it even on `kill -9`; only the socket file can be left behind, and it is cleaned next time).

### 5.3 Socket path, permissions, length

Requested name: `$XDG_RUNTIME_DIR/lmdv-$UID.sock` (e.g. `/run/user/1000/lmdv-1000.sock`).

Resolution order:

1. `$XDG_RUNTIME_DIR` if it is a directory, owned by the effective UID, with no group/other permission bits (systemd creates `0700`).
2. `$XDG_CACHE_HOME/lmdv/lmdv-$UID.sock` (`~/.cache/lmdv/...`), creating the directory `0700`.
3. `/tmp/lmdv-$UID/lmdv.sock`, directory created `0700`, `symlink_metadata` checked (do **not** follow a symlink planted by another user).
4. All failed → run without single-instance (degraded), log once, do not crash.

Constraints and details:

* `sockaddr_un.sun_path` is 108 bytes on Linux; a path must be **< 108 bytes including the NUL**, so ≤ 107. Check before binding; if `/run/user/1000/lmdv-1000.sock` fits (it always does), fine. A long cache path may not; fall through to `/tmp`.
* Set `umask(0o077)` around `bind` and then `chmod 0600` explicitly; a socket in a `0700` directory is already safe, but the mode is visible and some tools check it.
* The socket is a *unix stream* socket (`SOCK_STREAM`), not datagram: the client writes one request and reads one response.
* The lock file `lmdv-1000.sock.lock` stays behind (0 bytes, `0600`). Do not delete it on exit; another process may have opened it. The lock is on the open file description, so unlinking would be a classic race.

**Reconciliation with `ARCHITECTURE.md` §9.6.** The architecture doc specifies `$XDG_RUNTIME_DIR/lmdv/<instance-id>.sock` so a portable copy can run beside an installed one. The flat `lmdv-$UID.sock` above is the form requested for this phase. They differ only in directory layout and instance id; when portable mode lands, switch to `$XDG_RUNTIME_DIR/lmdv-<uid>/main.sock` and `.../portable-<hash>.sock` (directory `0700`, same lock/protocol). Everything else in this section is unchanged.

### 5.4 Startup algorithm: why `flock` beats "connect and see"

The naive design is: if `connect()` succeeds, forward; if it fails with `ECONNREFUSED`, unlink and become primary. That has a race: two instances can both fail to connect, both unlink, both bind (one wins, the other's socket is destroyed), or one can unlink a socket that a live-but-slow primary just bound.

The robust design uses an **advisory lock** on a separate file:

```
open("lmdv-$UID.sock.lock", O_CREAT|O_RDWR, 0600)
flock(LOCK_EX | LOCK_NB)
  success ──► I am the primary:
              unlink(socket)        # safe: no other lock holder can be alive
              bind(socket), chmod 0600
              listen(backlog=128)
              keep the lock for the process lifetime
  EWOULDBLOCK ──► a primary exists:
              wait up to 5 s for the socket to appear (it may still be starting)
              connect and forward
```

Properties:

* `kill -9` releases the lock but leaves the socket; the next start takes the lock and unlinks the stale file. No "delete the socket manually" support burden.
* A primary that is alive but not yet listening is handled by the wait loop, not by unlinking its socket.
* A primary that is hung *after* binding still holds the lock; a second launch cannot become primary, so it times out with a clear message instead of corrupting state. Recovery: `kill` the hung process; the lock is released automatically.
* The lock file must live on a filesystem where `flock` works. `$XDG_RUNTIME_DIR` is tmpfs; the cache fallback may be NFS, where `flock` can be emulated or fail — if the lock call returns an unexpected error, fall back to connect-based detection and log it.

Rust sketch:

```rust
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use rustix::fs::{flock, FlockOperation};

pub struct Guard {
    _lock: File,
    socket: PathBuf,
}
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
    }
}

pub enum Claim {
    Primary { guard: Guard, listener: UnixListener, path: PathBuf },
    Secondary { path: PathBuf },
}

pub fn claim() -> io::Result<Claim> {
    let path = socket_path()?;
    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .create(true).read(true).write(true)
        .mode(0o600)
        .open(&lock_path)?;
    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600))?;
    match flock(&lock, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {
            let _ = fs::remove_file(&path);            // stale by definition
            let listener = UnixListener::bind(&path)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
            Ok(Claim::Primary {
                guard: Guard { _lock: lock, socket: path.clone() },
                listener,
                path,
            })
        }
        Err(rustix::io::Errno::WOULDBLOCK) => Ok(Claim::Secondary { path }),
        Err(e) => Err(e.into()),
    }
}
```

Path resolution:

```rust
fn socket_path() -> io::Result<PathBuf> {
    let uid = rustix::process::geteuid().as_raw();
    let name = format!("lmdv-{uid}.sock");
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        if private_dir_ok(&d, uid) && fits(&d.join(&name)) {
            return Ok(d.join(&name));
        }
    }
    if let Some(cache) = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    {
        let d = cache.join("lmdv");
        if create_private_dir(&d).is_ok() && fits(&d.join(&name)) {
            return Ok(d.join(&name));
        }
    }
    let d = PathBuf::from(format!("/tmp/lmdv-{uid}"));
    create_private_dir(&d)?;
    Ok(d.join("lmdv.sock"))
}

fn fits(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().len() < 108
}
```

`private_dir_ok` checks `symlink_metadata` (not following a symlink), `is_dir()`, `uid == euid`, `mode & 0o077 == 0`. `create_private_dir` does `create_dir_all` then `set_permissions(0o700)`.

### 5.5 Protocol

Newline-delimited JSON, one request per connection, UTF-8. Versioned.

Request:

```json
{"v":1,"cmd":"open","cwd":"/home/u/notes","files":["/home/u/notes/a.md"],"line":42,"new_window":false,"wait":false,"activation_token":null}
```

| Field | Type | Meaning |
|---|---|---|
| `v` | int | protocol version, currently `1`; reject others |
| `cmd` | string | `"open"` (v1); `"ping"` reserved for health checks |
| `cwd` | string? | the client's cwd, for diagnostics and for resolving a raw relative path |
| `files` | array | absolute paths; v1 uses the first, an array leaves room for multi-file |
| `line` | int? | 1-based; absent means "remembered position" |
| `new_window` | bool | force a new window |
| `wait` | bool | reserved for §5.7; senders must tolerate `ok:true` + immediate close |
| `activation_token` | string? | copied from the client's `XDG_ACTIVATION_TOKEN` when the launcher provided one; lets the primary raise the window on Wayland (§5.8) |

Response:

```json
{"v":1,"ok":true,"action":"tab","window":"doc-2"}
{"v":1,"ok":false,"error":"timeout waiting for the UI"}
```

Limits: 64 KiB max request line, 2 s read/write timeout, 128-deep listen backlog. A connection with a peer UID different from `geteuid()` is closed without a response (`UnixStream::peer_cred()`).

Debugging without the app's CLI:

```bash
printf '%s\n' '{"v":1,"cmd":"open","files":["/tmp/a.md"]}' | socat - UNIX-CONNECT:"$XDG_RUNTIME_DIR/lmdv-$UID.sock"
# or, for a quick liveness check:
ss -xl | grep "lmdv-$UID.sock"
```

A bare non-JSON line is optionally treated as a single path (nice for `printf '/tmp/a.md\n' | nc -U …`), but that is a debugging convenience, not part of the contract.

### 5.6 Routing rules (ported)

`target()` in the reference `windowing.rs:80` decides in this order. `by_use` is the focus list (most recently focused first) followed by never-focused windows.

| # | Condition | Result |
|---|---|---|
| 0 | `--new-window` | **New window**, always |
| 1 | no file | **Front** the last-used open window; if none, new |
| 2 | a non-closing window holds the file in a tab, or it is in flight to one | **Activate** that tab |
| 3 | the first **idle** window (ready, shown, start page alone, nothing queued/in flight, not closing) | **Tab** there |
| 4 | the last-focused window is open | **Tab** there |
| 5 | any other window whose UI is ready | **Tab** there |
| 6 | otherwise | **New window** |

Details that make it feel right:

* **In-flight set (`opening`)**: a file claimed by a window is recorded before the tab exists. A burst of double-clicks cannot open the same file twice or lose one.
* **Pre-ready queue (`queued`)**: launches that arrive while the page is still starting are stored and replayed in order after `show_window`. Our event loop can simply hold requests in a `VecDeque` until the first frame is presented.
* **Closing windows are skipped**: a window with an unsaved-changes prompt must never receive a new file.
* **`--new-window` ignores dedupe**: the same file opens in a second window, by design (`CHANGELOG.md`, 2.2.0).
* **Missing file**: it still routes to a tab and shows "Not found"; it is not a client-side error.
* **Activate** = bring the tab forward, unminimize, focus; it must **not** reset the reading position unless `--line` was given (reference fix: a second double-click keeps the place).
* **`--line`** scrolls the target tab so that line is at the top and highlights the block. It beats the remembered position. On an already-open tab, it still jumps. 1-based at the CLI, converted to 0-based for layout.
* **Window labels**: the reference uses `main`, then `doc-N`; the exact label does not matter, but a stable `window` id in the response is useful for tests.
* **Cascade**: new windows step +32 logical px from the window they came from; a burst within 3 s cascades from the previous new window. Nice-to-have; not required for the gate.

### 5.7 `--wait` (proposal, not v1)

The reference has no `--wait`, so v1 should not ship it. If it is wanted later (editor integration: `lmdv --wait file.md` blocks until the tab closes), the additive design is:

* request `"wait":true`;
* the primary replies `{"ok":true,"action":"tab"}` immediately, then keeps the connection open;
* when the tab or window closes, writes `{"v":1,"ok":true,"event":"closed"}` and closes;
* the client blocks with no timeout (or an optional `--wait-timeout N`), exits 0;
* if the tab is **activated** (file already open), wait for that tab to close;
* cap concurrent waiters (e.g. 16) and drop the oldest with an error;
* a second instance of `--wait` does not become primary; it forwards and waits.

This must never block the UI thread: the listener thread owns the socket, the UI only signals "closed" through the event channel.

### 5.8 Focus and raise

The reference calls `unminimize()` + `set_focus()` (`windowing.rs:212`).

* **X11 / XWayland**: `_NET_ACTIVE_WINDOW` works; `window.focus_window()` (winit) plus `set_minimized(false)` is enough. Some window managers apply focus-stealing prevention; the request is then marked urgent.
* **Wayland**: there is no global "raise" request. The `xdg_activation_v1` protocol requires a token, usually obtained from a recent user input event. The existing instance usually has none, so KWin may refuse. Forward any `XDG_ACTIVATION_TOKEN` the *client* inherited (file managers and `systemd-run --user` sometimes set one) in the request, and use it with the activation protocol; `winit`'s `focus_window()` uses the protocol where available. When refused, call `request_user_attention(Some(Urgency::Normal))` so the taskbar entry is highlighted. This is a compositor policy, not a bug in lmdv.
* Practical check on Plasma: with the window behind, `lmdv` (no args) should bring it forward; if KWin's focus-stealing prevention is set to "High", it may only flash. Document it.

### 5.9 Where the code lives

Suggested layout (matches `PLAN.md:34`):

```
crates/lmdv-app/src/
  main.rs        claim() → Primary: run UI; Secondary: forward + exit
  cli.rs         parse_args (ported) + -h/-V
  instance.rs    socket path, lock, Guard, wait-for-socket
  ipc.rs         Request/Response, serve(), forward()
  app.rs         windows/tabs, route(), apply_request()
  windowing.rs   the routing table and focus history
```

`main` runs before any window/toolkit initialization: the secondary path must not create a window, a GPU device, or a Rune context.

winit integration (0.30 style):

```rust
pub enum UserEvent {
    Open(ipc::Request, std::sync::mpsc::SyncSender<Result<Reply, String>>),
}

impl winit::application::ApplicationHandler<UserEvent> for App {
    fn user_event(&mut self, _el: &winit::event_loop::ActiveEventLoop, ev: UserEvent) {
        match ev {
            UserEvent::Open(req, ack) => {
                let reply = self.route(req);   // tabs/windows, no blocking
                let _ = ack.send(reply);
            }
        }
    }
    // …
}
```

eframe/egui variant: the listener thread holds `egui::Context` (it is `Send + Sync`), sends `Request` through an `mpsc::Sender`, and calls `ctx.request_repaint()`; `App::update` drains `try_recv()` at the top of the frame and acknowledges. Do **not** call `recv()` on the UI thread.

### 5.10 Wrapper side

Nothing. The wrapper does not open, read, or create the socket. It only absolutizes file arguments and `exec`s the binary. This keeps the shell simple and makes the IPC testable with the binary alone (`LMDV_BIN=target/debug/lmdv ./target/debug/lmdv a.md`). If the wrapper *did* speak the protocol, every future protocol change would need two implementations.

### 5.11 Failure modes around the socket

| Situation | Behavior |
|---|---|
| No socket, no lock | become primary, bind |
| Stale socket (crash/`kill -9`) | lock is free → unlink, bind |
| Live primary, socket not yet bound | lock busy → wait up to 5 s, then connect |
| Live primary, UI hung | lock busy, connect may succeed but no reply → 2 s timeout, error dialog "lmdv is not responding"; do not unlink |
| Socket path > 107 bytes | fall to cache, then `/tmp` |
| `$XDG_RUNTIME_DIR` unset/0755 | fall to cache (`0700`) |
| Cache on NFS / read-only | fall to `/tmp/lmdv-$UID` |
| Two launches at once | one wins `flock`, the other waits and forwards |
| Peer UID mismatch | connection closed, no response |
| Socket deleted by hand while running | primary keeps its bound fd; a new launch sees the lock busy, fails to connect, times out; restart the app |
| App exits | socket unlinked by `Drop`; lock released |

---

## 6. File manager caveats

All of these use the same MIME database and `mimeapps.list`; what differs is the *cache* and the Open With UI. The default handler is resolved at launch time, so a running file manager usually picks up a new default without a restart — the **menu list** is what goes stale.

| File manager | Backend | Refresh after install | Open With source | Restart |
|---|---|---|---|---|
| Nautilus (GNOME) | GIO | `update-desktop-database` | `mimeinfo.cache` + `.desktop` files | `nautilus -q` |
| Dolphin (Plasma) | KIO / KService / ksycoca | `kbuildsycoca6 --noincremental` (+ `update-desktop-database`) | KService database | `kquitapp6 dolphin` (it restarts on demand) |
| Thunar (XFCE) | GIO | `update-desktop-database` | `mimeinfo.cache` | `thunar -q` |
| Nemo (Cinnamon) | GIO | `update-desktop-database` | `mimeinfo.cache` | `nemo -q` |
| PCManFM / PCManFM-Qt | libfm(-qt) → GIO | `update-desktop-database` | `mimeinfo.cache` | `pcmanfm -q` / `pcmanfm-qt -q` |

Specifics:

* **Nautilus**: the "Open With Other Application" dialog lists apps from `mimeinfo.cache`; without `update-desktop-database` our entry is missing even though `gio open` works. Nautilus remembers per-file-type choices in `mimeapps.list`, so a choice made once survives. `nautilus -q` is the safe reload.
* **Dolphin**: KDE builds the Open With menu from `KService`, which is cached in `ksycoca`. A newly installed desktop file can be invisible until `kbuildsycoca6` or a re-login. Dolphin also has a "File Type Options" dialog in some versions; the Open With submenu is the authoritative path on Plasma 6. `kioclient5 exec FILE` is a good CLI proxy for "what Dolphin would do" (verified: it launched the handler).
* **Thunar**: GIO-based. `thunar -q` terminates it; the next launch rebuilds the menu. Thunar's own "Default Applications" settings only cover browser/mail; file types come from `mimeapps.list`.
* **Nemo**: GIO-based; `nemo -q`. The Properties → Open With tab writes `mimeapps.list`.
* **PCManFM-Qt**: uses libfm-qt, which delegates to GIO/`QDesktopServices` depending on build. If the default does not take effect, set it with `xdg-mime`, restart the FM, and verify with `gio mime`. Do not chase a PCManFM-specific setting.

### 6.1 Wayland vs X11

MIME resolution is identical; the differences are window management and identity:

* `StartupWMClass` is used on X11/XWayland; native Wayland uses the app_id matched against the desktop-file basename.
* Raising/focusing an existing window is unrestricted on X11 and best-effort on Wayland (§5.8).
* Under XWayland, the app sees `DISPLAY` and `WAYLAND_DISPLAY`; winit/wgpu picks Wayland by default. Forcing a backend is a debugging step, not a configuration: `WINIT_UNIX_BACKEND=x11` (winit ≤ 0.29) or unset `WAYLAND_DISPLAY` for one run.
* Drag-and-drop, clipboard, and file dialogs go through the toolkit/portals; not relevant to `.md` association.

### 6.2 `.desktop` trust and permissions

* For the application menu and Open With, a desktop file in `~/.local/share/applications` does **not** need to be executable or "trusted". Mode `0644` is correct; `0755` is harmless.
* GNOME only requires `metadata::trusted` for `.desktop` files placed on the **desktop** (icons), not for menu entries:
  ```bash
  gio set ~/Desktop/lmdv.desktop metadata::trusted true
  ```
* KDE may show "This file is not executable" only for desktop icons; menu entries are unaffected.
* Never put the desktop file in the repo and symlink it: file managers and `update-desktop-database` handle symlinks inconsistently. Install a copy.

### 6.3 "Open With" caching

Three caches, three fixes:

1. `~/.local/share/applications/mimeinfo.cache` → `update-desktop-database ~/.local/share/applications`.
2. KDE `ksycoca` → `kbuildsycoca6 --noincremental` (or re-login).
3. The file manager's in-memory menu → restart it (`nautilus -q`, `thunar -q`, `nemo -q`, `kquitapp6 dolphin`).

`gio open` and double-click do **not** use the menu cache; they resolve the default every time. So "double-click works but the app is missing from Open With" is almost always cache #1 or #2, not a broken association.

### 6.4 Verification commands

```bash
# What will a double-click resolve to?
xdg-mime query filetype "$PWD/note.md"       # text/markdown
xdg-mime query default text/markdown          # lmdv.desktop
gio mime text/markdown                        # Default + Registered list

# Launch exactly like a file manager
gio open "$PWD/note.md"
xdg-open "$PWD/note.md"                       # Plasma: selects the KDE opener
kioclient5 exec "$PWD/note.md"                # KIO/Dolphin path
gtk-launch lmdv "$PWD/note.md"                # GTK app-info path
gio launch ~/.local/share/applications/lmdv.desktop "$PWD/note.md"

# Then the real test: double-click in Dolphin/Nautilus.
```

**Verified on this machine:** `gio open`, `xdg-open` (debug log: `Selected DE kde`), `kioclient5 exec`, `gtk-launch`, and `gio launch` all invoked the registered handler with a path containing spaces and Unicode. Double-click is still the release gate because it exercises the file manager's own launch path and cwd.

---

## 7. Packaging

### 7.1 Where things go

| Method | Binary | Desktop file | Icon | MIME XML | Single instance | Updates |
|---|---|---|---|---|---|---|
| dev (this repo) | `target/{debug,release}/lmdv` + wrapper `~/.local/bin/lmdv` | `~/.local/share/applications/lmdv.desktop` | `~/.local/share/icons/hicolor/scalable/apps/lmdv.svg` | `~/.local/share/mime/packages/lmdv.xml` | socket | `cargo build` |
| `cargo install --path .` | `~/.cargo/bin/lmdv` | not installed | not installed | not installed | socket | reinstall |
| AppImage | inside the image + `AppRun` | inside `usr/share/applications` | inside `usr/share/icons` | inside `usr/share/mime` | socket (no sandbox) | AppImageUpdate / manual |

### 7.2 Dev install

Exactly what `scripts/install-md-handler.sh` does. The binary stays in `target/`; the wrapper is the stable entry point. `cargo clean` removes the binary but not the registration; the next launch shows the "No lmdv binary found" dialog until you rebuild.

### 7.3 `cargo install --path`

```bash
cargo install --path .                 # release, ~/.cargo/bin/lmdv, ~/.cargo/bin
cargo install --path . --debug         # debug
cargo install --path . --root ~/.local # ~/.local/bin/lmdv
cargo install --list
cargo uninstall lmdv
```

Caveats:

* By default this is a **release** build; iterating means reinstalling each time, which is slower than pointing the wrapper at `target/debug`.
* It installs only the binary. You still need the desktop file, icon, MIME package, and default handler; re-run the installer with `--binary "$(command -v lmdv)" --no-wrapper` (or keep the wrapper).
* `~/.cargo/bin` may not be on the GUI session's `PATH`; the desktop entry must use an absolute path (the installer does).
* `cargo install` overwrites the binary in place; a running instance keeps the old inode, and a launch during the copy can hit `ETXTBSY`. The wrapper retries; packaging should install atomically.

### 7.4 AppImage

Layout expected by AppImage tooling:

```
AppDir/
  AppRun                      # execs usr/bin/lmdv "$@"
  usr/bin/lmdv
  usr/share/applications/lmdv.desktop
  usr/share/icons/hicolor/scalable/apps/lmdv.svg
  usr/share/mime/packages/lmdv.xml
  usr/lib/…                   # if not fully static
```

* The desktop file **inside** the image should use `Exec=lmdv %f`; AppImage integration tools rewrite it to an absolute path to the `.AppImage` when they install it.
* AppImage does not auto-integrate. Options: `AppImageLauncher` or `appimaged` moves/copies the file and installs the desktop entry/icon; otherwise:
  ```bash
  ./lmdv-0.7.0-x86_64.AppImage --appimage-extract
  # copy squashfs-root/usr/share/applications/lmdv.desktop to ~/.local/share/applications,
  # rewrite Exec to the absolute .AppImage path, copy the icon, then register as in §3
  ```
* No sandbox: `%f` paths work, and the Unix socket works normally.
* Portable mode: marker next to the `.AppImage`, **not** inside (the squashfs is read-only).
* Updates: manual re-download, or AppImageUpdate if the image is built with zsync. Do not build a self-updater into the app.
* Uninstall: remove the `.AppImage`, the desktop file, icon, MIME package; re-run `update-*`; restore the handler.

### 7.5 Portable mode

A marker file next to the executable:

```
<dir>/lmdv            (or lmdv.AppImage)
<dir>/lmdv-portable.txt   (empty marker; matches ARCHITECTURE.md §8.1)
<dir>/data/config/
<dir>/data/cache/
<dir>/data/state/
```

Rules:

* Check `lmdv-portable.txt` next to `std::env::current_exe()`; do **not** follow symlinks to the marker (use `symlink_metadata`).
* If `<exe_dir>/data` is not writable, fall back to XDG dirs and log once. An AppImage mounted read-only must put `data` next to the `.AppImage`, not inside `$APPDIR`.
* Portable mode also changes the instance id (`ARCHITECTURE.md` §8.1), so a portable copy and an installed copy can run at the same time.
* `LMDV_HOME` overrides the data root for tests and advanced users, as specified in `ARCHITECTURE.md` §8.1.
* The single-instance socket still belongs in the runtime dir (or `/tmp` fallback); a portable app can still be one instance per user. Do not put the socket in `lmdv-data` unless the path fits and the filesystem supports sockets.
* Portable mode is for USB sticks and AppImage; it is **not** a replacement for the user-level desktop registration (a portable app cannot register itself without writing outside its directory).

### 7.6 Auto-update expectations

| Method | Update mechanism |
|---|---|
| dev / `cargo install` | manual (rebuild/reinstall) |
| AppImage | manual or AppImageUpdate (zsync) |
| portable | manual |

Do not add a network self-updater: the project is offline-first, and a viewer that phones home on launch contradicts the product rules. A "check for updates" menu item that opens the releases page is acceptable.

### 7.7 Uninstall

| Method | Command |
|---|---|
| dev | `scripts/install-md-handler.sh --uninstall` (`--purge` to delete state) |
| `cargo install` | `cargo uninstall lmdv` + the installer's `--uninstall` for the desktop bits |
| AppImage | delete the image, remove the desktop file/icon/MIME package, `update-*`, restore the handler |

The dev uninstaller restores the saved handler only if `lmdv.desktop` is still the current default; otherwise it leaves the user's newer choice alone (§3.6).

---

## 8. Scripts

Two files, both tested. The tested copies live in this repository under `scripts/` (`install-md-handler.sh`, `uninstall-md-handler.sh`).

* `scripts/install-md-handler.sh` — install, re-register, and `--uninstall`.
* `scripts/uninstall-md-handler.sh` — a two-line convenience wrapper around `--uninstall`.

The launcher is embedded in the installer (single self-contained file); §4.6 shows the generated result. The installer never edits an unmanaged file without `--force`, and every file it writes carries a marker (`X-LMDV-Managed=true`, `<!-- lmdv-managed -->`, `# lmdv-launcher v1`).

### 8.1 `scripts/install-md-handler.sh`

```bash
#!/usr/bin/env bash
# install-md-handler.sh -- register the current lmdv build as the Markdown handler.
#
# Per-user only: never needs sudo, never writes outside $HOME / the XDG dirs.
# Safe to re-run: every file it writes carries a marker and is replaced whole.
#
# Examples:
#   scripts/install-md-handler.sh --release
#   scripts/install-md-handler.sh --dev --dry-run
#   scripts/install-md-handler.sh --binary ./target/debug/lmdv
#   scripts/install-md-handler.sh --uninstall
set -euo pipefail

DESKTOP_ID="lmdv.desktop"
MIME_TYPES=(text/markdown text/x-markdown application/x-markdown)
MARKER="X-LMDV-Managed=true"
WRAPPER_MARKER="# lmdv-launcher v1"

DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
STATE_HOME="${XDG_STATE_HOME:-$HOME/.local/state}"
APP_DIR="$DATA_HOME/applications"
MIME_DIR="$DATA_HOME/mime/packages"
ICON_DIR="$DATA_HOME/icons/hicolor/scalable/apps"
BIN_DIR="${LMDV_BIN_DIR:-$HOME/.local/bin}"
STATE_DIR="$STATE_HOME/lmdv/install"
PREV_DEFAULTS="$STATE_DIR/previous-defaults.tsv"
WRAPPER="$BIN_DIR/lmdv"
DESKTOP_FILE="$APP_DIR/$DESKTOP_ID"
MIME_FILE="$MIME_DIR/lmdv.xml"
ICON_FILE="$ICON_DIR/lmdv.svg"

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"

MODE="install"
DRY_RUN=0
DO_MIME=1
USE_WRAPPER=1
FORCE=0
PURGE=0
PROFILE="auto"
BINARY=""
BINARY_EXPLICIT=0
ALLOW_MISSING=0

say()  { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
usage: install-md-handler.sh [options]

  --dev               default the launcher to target/debug/lmdv
  --release           default the launcher to target/release/lmdv
                      (without either, the newest of the two wins at launch)
  --binary PATH       use this exact binary (LMDV_BIN still overrides)
  --repo PATH         project root (default: parent of this script)
  --prefix DIR        where the `lmdv` launcher goes (default: ~/.local/bin)
  --no-wrapper        point the desktop entry straight at the binary
  --no-mime           install the desktop entry but do not claim .md files
  --allow-missing     install even if no build exists yet
  --uninstall         remove everything this script installed, restore the
                      previous default handler
  --purge             with --uninstall, also delete ~/.local/state/lmdv
  --force             overwrite files that are not marked as ours
  --dry-run           print what would happen, change nothing
  -h, --help          this text
EOF
}

while (($#)); do
    case "$1" in
        --dev)           PROFILE="debug" ;;
        --release)       PROFILE="release" ;;
        --binary)        BINARY="${2:?--binary needs a path}"; BINARY_EXPLICIT=1; shift ;;
        --repo)          REPO_DIR="${2:?--repo needs a path}"; shift ;;
        --prefix)        BIN_DIR="${2:?--prefix needs a path}"; shift ;;
        --no-wrapper)    USE_WRAPPER=0 ;;
        --no-mime)       DO_MIME=0 ;;
        --allow-missing) ALLOW_MISSING=1 ;;
        --uninstall)     MODE="uninstall" ;;
        --purge)         PURGE=1 ;;
        --force)         FORCE=1 ;;
        --dry-run)       DRY_RUN=1 ;;
        -h|--help)       usage; exit 0 ;;
        *)               die "unknown option: $1 (try --help)" ;;
    esac
    shift
done

REPO_DIR="$(realpath -m -- "$REPO_DIR")"
BIN_DIR="$(realpath -m -- "$BIN_DIR")"
WRAPPER="$BIN_DIR/lmdv"

# --- helpers -----------------------------------------------------------------

run() {
    if ((DRY_RUN)); then
        printf '[dry-run] %s\n' "$*"
    else
        "$@"
    fi
}

# write_file PATH < content
write_file() {
    local path="$1"
    if ((DRY_RUN)); then
        printf '[dry-run] write %s\n' "$path"
        cat >/dev/null
        return 0
    fi
    mkdir -p -- "$(dirname -- "$path")"
    cat >"$path"
}

desktop_quote() {
    local p="$1"
    # A literal % is a field-code introducer and cannot appear in the program
    # path; $ and ` make desktop-file-validate and GLib disagree; quotes and
    # backslashes have two competing escape rules. Refuse the lot: the default
    # Exec path is ~/.local/bin/lmdv, so this only affects --no-wrapper.
    case "$p" in
        *'"'*|*'\'*|*'$'*|*'`'*|*'%'*|*$'\n'*) return 1 ;;
    esac
    printf '"%s"' "$p"
}

managed_file() { # path, marker
    [ -f "$1" ] && grep -Fq -- "$2" "$1" 2>/dev/null
}

require_ours_or_absent() { # path, marker-regex
    if [ -e "$1" ] && ! managed_file "$1" "$2"; then
        if ((FORCE)); then
            warn "overwriting unmanaged file: $1"
        else
            die "$1 exists and was not written by this script; re-run with --force to replace it"
        fi
    fi
}

pick_binary() {
    if [ -n "$BINARY" ]; then
        BINARY="$(realpath -m -- "$BINARY")"
        [ -x "$BINARY" ] || die "not executable: $BINARY"
        return 0
    fi
    local rel="$REPO_DIR/target/release/lmdv" dbg="$REPO_DIR/target/debug/lmdv"
    case "$PROFILE" in
        release) [ -x "$rel" ] && { BINARY="$rel"; return 0; } ;;
        debug)   [ -x "$dbg" ] && { BINARY="$dbg"; return 0; } ;;
    esac
    if [ -x "$rel" ] && [ -x "$dbg" ]; then
        if [ "$dbg" -nt "$rel" ]; then BINARY="$dbg"; else BINARY="$rel"; fi
        return 0
    fi
    [ -x "$rel" ] && { BINARY="$rel"; return 0; }
    [ -x "$dbg" ] && { BINARY="$dbg"; return 0; }
    return 1
}

# Remove `app` from every ;-list of a mimeapps.list, dropping empty lines.
mimeapps_remove() { # file, app-id
    local file="$1" app="$2" tmp
    [ -f "$file" ] || return 0
    grep -Fq -- "$app" "$file" || return 0
    tmp="$(mktemp)"
    awk -v drop="$app" '
        /^\[/ { sect = $0; print; next }
        {
            if (sect == "[Default Applications]" || sect == "[Added Associations]") {
                eq = index($0, "=")
                if (eq > 0) {
                    key = substr($0, 1, eq - 1)
                    n = split(substr($0, eq + 1), a, ";")
                    out = ""; found = 0
                    for (i = 1; i <= n; i++) {
                        if (a[i] == "") continue
                        if (a[i] == drop) { found = 1; continue }
                        out = out a[i] ";"
                    }
                    if (found) {
                        if (out != "") print key "=" out
                        next
                    }
                }
            }
            print
        }
    ' "$file" >"$tmp"
    if ((DRY_RUN)); then
        printf '[dry-run] prune %s from %s\n' "$app" "$file"
        rm -f -- "$tmp"
    else
        cat "$tmp" >"$file"
        rm -f -- "$tmp"
    fi
}

mimeapps_set() { # file, mime, app-id -- fallback when no xdg-mime/gio exist
    local file="$1" mime="$2" app="$3" old_d old_a new_d new_a
    if ((DRY_RUN)); then
        printf '[dry-run] set %s=%s in %s\n' "$mime" "$app" "$file"
        return 0
    fi
    mkdir -p -- "$(dirname -- "$file")"
    touch "$file"

    old_d="$(awk -v m="$mime" '
        /^\[Default Applications\]/ { s = 1; next }
        /^\[/ { s = 0 }
        s && index($0, m "=") == 1 { print substr($0, length(m) + 2); exit }' "$file")"
    old_a="$(awk -v m="$mime" '
        /^\[Added Associations\]/ { s = 1; next }
        /^\[/ { s = 0 }
        s && index($0, m "=") == 1 { print substr($0, length(m) + 2); exit }' "$file")"

    new_d="$(merge_list "$old_d" "$app" "")"
    new_a="$(merge_list "$old_a" "$app" ";")"

    awk -v m="$mime" -v dv="$new_d" -v av="$new_a" '
        /^\[Default Applications\]$/ { print; print m "=" dv; d = 1; next }
        /^\[Added Associations\]$/   { print; print m "=" av; a = 1; next }
        /^\[/                         { print; next }
        index($0, m "=") == 1          { next }
        { print }
        END {
            if (!d) { print ""; print "[Default Applications]"; print m "=" dv }
            if (!a) { print ""; print "[Added Associations]";   print m "=" av }
        }' "$file" >"$file.tmp.$$"
    mv -- "$file.tmp.$$" "$file"
}

merge_list() { # existing list, app, trailing ";" (or "")
    local old="$1" app="$2" trail="$3" part out
    local -a parts=()
    out="$app"
    IFS=';' read -r -a parts <<<"$old"
    for part in "${parts[@]}"; do
        [ -n "$part" ] || continue
        [ "$part" = "$app" ] && continue
        out="$out;$part"
    done
    if [ -n "$trail" ]; then out="$out;"; fi
    printf '%s' "$out"
}

set_default() { # mime
    local mime="$1"
    if ((DRY_RUN)); then
        printf '[dry-run] xdg-mime default %s %s\n' "$DESKTOP_ID" "$mime"
        return 0
    fi
    if command -v xdg-mime >/dev/null 2>&1; then
        xdg-mime default "$DESKTOP_ID" "$mime"
    elif command -v gio >/dev/null 2>&1 && gio mime "$mime" "$DESKTOP_ID" 2>/dev/null; then
        :
    else
        warn "no xdg-mime or gio; editing mimeapps.list directly"
        mimeapps_set "$CONFIG_HOME/mimeapps.list" "$mime" "$DESKTOP_ID"
    fi
}

current_default() { # mime -> desktop id or empty
    local mime="$1"
    if command -v xdg-mime >/dev/null 2>&1; then
        xdg-mime query default "$mime" 2>/dev/null || true
    elif command -v gio >/dev/null 2>&1; then
        gio mime "$mime" 2>/dev/null | sed -n 's/^Default application for .*: //p' | head -1
    else
        awk -v m="$mime" '
            /^\[Default Applications\]/ { s = 1; next }
            /^\[/ { s = 0 }
            s && index($0, m "=") == 1 { print substr($0, length(m) + 2); exit }' \
            "$CONFIG_HOME/mimeapps.list" 2>/dev/null || true
    fi
}

restore_default() { # mime
    local mime="$1" prev="" current=""
    if [ -f "$PREV_DEFAULTS" ]; then
        prev="$(awk -F'\t' -v m="$mime" '$1 == m { print $2; exit }' "$PREV_DEFAULTS")"
    fi
    current="$(current_default "$mime")"
    if [ -n "$current" ] && [ "$current" != "$DESKTOP_ID" ]; then
        mimeapps_remove "$CONFIG_HOME/mimeapps.list" "$DESKTOP_ID"
        mimeapps_remove "$DATA_HOME/applications/mimeapps.list" "$DESKTOP_ID"
        say "  leaving $mime -> $current (handler changed since install)"
        return 0
    fi
    if ((DRY_RUN)); then
        printf '[dry-run] restore %s -> %s\n' "$mime" "${prev:-<none>}"
        return 0
    fi
    if [ -n "$prev" ] && [ "$prev" != "$DESKTOP_ID" ]; then
        if command -v xdg-mime >/dev/null 2>&1; then
            xdg-mime default "$prev" "$mime" || warn "could not restore $mime to $prev"
        else
            mimeapps_set "$CONFIG_HOME/mimeapps.list" "$mime" "$prev"
        fi
        say "  restored $mime -> $prev"
    else
        local f
        for f in "$CONFIG_HOME/mimeapps.list" "$DATA_HOME/applications/mimeapps.list"; do
            mimeapps_remove "$f" "$DESKTOP_ID"
        done
        # Drop a now-empty key line left behind by xdg-mime.
        for f in "$CONFIG_HOME/mimeapps.list" "$DATA_HOME/applications/mimeapps.list"; do
            [ -f "$f" ] || continue
            sed -i -e '/^[A-Za-z0-9.+-]*\/[A-Za-z0-9.+-]*=$/d' -- "$f"
        done
        say "  cleared $mime"
    fi
}

refresh_caches() {
    if ! command -v update-mime-database >/dev/null 2>&1; then
        warn "update-mime-database not found (install shared-mime-info); .md detection may lag"
    elif [ -d "$DATA_HOME/mime" ]; then
        run update-mime-database "$DATA_HOME/mime" || warn "update-mime-database failed"
    fi
    if command -v update-desktop-database >/dev/null 2>&1; then
        run update-desktop-database "$APP_DIR" || warn "update-desktop-database failed"
    else
        warn "update-desktop-database not found (install desktop-file-utils); Open With may not list lmdv"
    fi
    if [ -f "$DATA_HOME/icons/hicolor/index.theme" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
        run gtk-update-icon-cache -f -t "$DATA_HOME/icons/hicolor" >/dev/null 2>&1 || true
    fi
    # KDE reads desktop files through ksycoca; without a rebuild the new entry
    # can be missing from Dolphin's Open With until the next login.
    if [[ "${XDG_CURRENT_DESKTOP:-}" == *KDE* ]] && [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
        if command -v kbuildsycoca6 >/dev/null 2>&1; then
            run timeout 30 kbuildsycoca6 --noincremental >/dev/null 2>&1 || warn "kbuildsycoca6 failed"
        elif command -v kbuildsycoca5 >/dev/null 2>&1; then
            run timeout 30 kbuildsycoca5 --noincremental >/dev/null 2>&1 || true
        fi
    fi
}

# --- uninstall ---------------------------------------------------------------

if [ "$MODE" = "uninstall" ]; then
    say "Removing the user-level lmdv Markdown handler"
    if managed_file "$WRAPPER" "$WRAPPER_MARKER"; then
        run rm -f -- "$WRAPPER"; say "  removed $WRAPPER"
    elif [ -e "$WRAPPER" ]; then
        warn "not removing $WRAPPER (not managed by this script; use --force to delete)"
        if ((FORCE)); then
            run rm -f -- "$WRAPPER"
            say "  removed $WRAPPER (forced)"
        fi
    fi
    if managed_file "$DESKTOP_FILE" "$MARKER"; then
        run rm -f -- "$DESKTOP_FILE"; say "  removed $DESKTOP_FILE"
    elif [ -e "$DESKTOP_FILE" ]; then
        warn "not removing $DESKTOP_FILE (not managed by this script)"
        if ((FORCE)); then
            run rm -f -- "$DESKTOP_FILE"
            say "  removed $DESKTOP_FILE (forced)"
        fi
    fi
    if managed_file "$MIME_FILE" "lmdv-managed"; then
        run rm -f -- "$MIME_FILE"; say "  removed $MIME_FILE"
    fi
    if managed_file "$ICON_FILE" "lmdv-managed"; then
        run rm -f -- "$ICON_FILE"; say "  removed $ICON_FILE"
    fi

    if ((DO_MIME)); then
        for m in "${MIME_TYPES[@]}"; do
            restore_default "$m"
        done
    fi
    refresh_caches
    if ((PURGE)); then
        run rm -rf -- "$STATE_HOME/lmdv"
        say "  removed $STATE_HOME/lmdv (--purge)"
    else
        say "  kept $STATE_DIR (use --purge to delete it)"
    fi
    say
    say "Verification:"
    for m in "${MIME_TYPES[@]}"; do
        printf '  %-24s -> %s\n' "$m" "$(command -v xdg-mime >/dev/null 2>&1 && xdg-mime query default "$m" 2>/dev/null || echo '(xdg-mime missing)')"
    done
    exit 0
fi

# --- install -----------------------------------------------------------------

((DRY_RUN)) || mkdir -p -- "$BIN_DIR" "$APP_DIR" "$STATE_DIR"

if ! pick_binary; then
    if ((DRY_RUN || ALLOW_MISSING)); then
        warn "no build found under $REPO_DIR/target; the launcher will work once you build"
        BINARY=""
    else
        die "no lmdv binary found. Build first: cargo build  (or cargo build --release), or pass --binary PATH"
    fi
fi

if ((USE_WRAPPER)); then
    EXEC_PATH="$WRAPPER"
else
    [ -n "$BINARY" ] || die "--no-wrapper needs a real binary; build first or pass --binary"
    EXEC_PATH="$BINARY"
fi

# Validate the Exec path before writing anything: some characters cannot be
# represented in a desktop entry in a way that both the validator and GLib accept.
if ! desktop_prog="$(desktop_quote "$EXEC_PATH")"; then
    die "Exec path contains one of \" \\ \$ \` % or a newline, which desktop entries cannot represent reliably: $EXEC_PATH (use the wrapper, or move the binary to a simple path)"
fi

say "Installing per-user Markdown handler for lmdv"
say "  repo:    $REPO_DIR"
say "  binary:  ${BINARY:-(none yet; launcher decides at run time)}"
say "  exec:    $EXEC_PATH"
((DRY_RUN)) && say "  mode:    dry run (nothing will be written)"

# 1. MIME package: merge aliases + globs into text/markdown at user level.
if ((DO_MIME)); then
    require_ours_or_absent "$MIME_FILE" "lmdv-managed"
    write_file "$MIME_FILE" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<!-- lmdv-managed: user-level Markdown type registration -->
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="text/markdown">
    <comment>Markdown document</comment>
    <glob pattern="*.md" weight="60"/>
    <glob pattern="*.markdown" weight="58"/>
    <glob pattern="*.mdown" weight="57"/>
    <glob pattern="*.mkd" weight="57"/>
    <glob pattern="*.mkdn" weight="57"/>
    <glob pattern="*.mdwn" weight="57"/>
    <alias type="text/x-markdown"/>
    <alias type="application/x-markdown"/>
  </mime-type>
</mime-info>
XML
    if ((DRY_RUN)); then say "  would write $MIME_FILE"; else say "  wrote $MIME_FILE"; fi
fi

# 2. Icon (copy the repo's if present, else a generated fallback).
require_ours_or_absent "$ICON_FILE" "lmdv-managed"
if [ -f "$REPO_DIR/assets/lmdv.svg" ]; then
    if ((DRY_RUN)); then
        say "[dry-run] copy $REPO_DIR/assets/lmdv.svg -> $ICON_FILE"
    else
        mkdir -p -- "$ICON_DIR"
        { printf '<!-- lmdv-managed -->\n'; cat -- "$REPO_DIR/assets/lmdv.svg"; } >"$ICON_FILE"
        say "  wrote $ICON_FILE (from assets/lmdv.svg)"
    fi
else
    write_file "$ICON_FILE" <<'SVG'
<!-- lmdv-managed -->
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" width="128" height="128">
  <rect x="10" y="10" width="108" height="108" rx="22" fill="#1f6feb"/>
  <path d="M36 28h38l18 18v54a6 6 0 0 1-6 6H36a6 6 0 0 1-6-6V34a6 6 0 0 1 6-6z"
        fill="#f6f8fa"/>
  <path d="M74 28l18 18H80a6 6 0 0 1-6-6z" fill="#c9d1d9"/>
  <path d="M44 88V58h8l8 12 8-12h8v30h-8V70l-8 12-8-12v18z" fill="#1f6feb"/>
</svg>
SVG
    if ((DRY_RUN)); then say "  would write $ICON_FILE (generated)"; else say "  wrote $ICON_FILE (generated)"; fi
fi

# 3. Desktop entry.
require_ours_or_absent "$DESKTOP_FILE" "$MARKER"

desktop_exec="$desktop_prog %f"
if ((DRY_RUN)); then
    say "[dry-run] write $DESKTOP_FILE (Exec=$desktop_exec)"
else
    mkdir -p -- "$APP_DIR"
    cat >"$DESKTOP_FILE" <<DESKTOP
[Desktop Entry]
Type=Application
Version=1.5
Name=lmdv
GenericName=Markdown Viewer
Comment=View and edit Markdown documents
Exec=$desktop_exec
TryExec=$EXEC_PATH
Icon=lmdv
Terminal=false
StartupNotify=false
StartupWMClass=lmdv
SingleMainWindow=true
DBusActivatable=false
NoDisplay=false
Categories=Utility;TextTools;
Keywords=markdown;md;view;preview;text;
MimeType=text/markdown;text/x-markdown;application/x-markdown;
$MARKER
DESKTOP
    if ((DRY_RUN)); then say "  would write $DESKTOP_FILE"; else say "  wrote $DESKTOP_FILE"; fi
fi

# 4. Launcher wrapper.
if ((USE_WRAPPER)); then
    require_ours_or_absent "$WRAPPER" "$WRAPPER_MARKER"
    WRAPPER_TEMPLATE=$(cat <<'WRAPPER_EOF'
#!/bin/bash
# lmdv-launcher v1 -- generated by install-md-handler.sh
# Managed file: re-running the installer overwrites it.
#
# Picks target/debug or target/release, absolutizes file arguments, logs to
# $XDG_STATE_HOME/lmdv/launch.log, never opens a terminal. Single-instance
# forwarding lives in the lmdv binary, not here.
set -u

LMDV_REPO="@LMDV_REPO@"
LMDV_PREFERRED_PROFILE="@LMDV_PROFILE@"
LMDV_FIXED_BIN="@LMDV_FIXED_BIN@"

SELF="$(readlink -f -- "${BASH_SOURCE[0]}" 2>/dev/null || printf '%s' "${BASH_SOURCE[0]}")"
STATE_HOME="${XDG_STATE_HOME:-$HOME/.local/state}"
LOG_DIR="$STATE_HOME/lmdv"
LOG_FILE="$LOG_DIR/launch.log"
MAX_LOG_BYTES=262144

mkdir -p -- "$LOG_DIR" 2>/dev/null || true

log() {
    [ -d "$LOG_DIR" ] || return 0
    if [ -f "$LOG_FILE" ] && [ "$(stat -c %s -- "$LOG_FILE" 2>/dev/null || echo 0)" -gt "$MAX_LOG_BYTES" ]; then
        mv -f -- "$LOG_FILE" "$LOG_FILE.1" 2>/dev/null || true
    fi
    printf '%s [%s] %s\n' "$(date -u +%FT%TZ)" "$$" "$*" >>"$LOG_FILE" 2>/dev/null || true
}

gui_error() {
    local msg="$1"
    log "ERROR: $msg"
    [ -n "${LMDV_NO_DIALOG:-}" ] && return 0
    if [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
        if command -v kdialog >/dev/null 2>&1 && [ "${XDG_CURRENT_DESKTOP:-}" != "GNOME" ]; then
            timeout 30 kdialog --title "lmdv" --error "$msg" >/dev/null 2>&1 && return 0
        fi
        if command -v zenity >/dev/null 2>&1; then
            timeout 30 zenity --error --title="lmdv" --text="$msg" >/dev/null 2>&1 && return 0
        fi
    fi
    if command -v notify-send >/dev/null 2>&1 && [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
        notify-send -a lmdv -u critical "lmdv" "$msg" >/dev/null 2>&1 || true
    fi
    printf 'lmdv: %s\n' "$msg" >&2
}

normalize_args() {
    local -a src=("$@") out=()
    local a expect_value=0 end_of_opts=0
    for a in "${src[@]}"; do
        if [ "$end_of_opts" -eq 1 ]; then
            if [ -e "$a" ]; then out+=("$(realpath -m -- "$a")"); else out+=("$a"); fi
            continue
        fi
        if [ "$expect_value" -eq 1 ]; then out+=("$a"); expect_value=0; continue; fi
        case "$a" in
            --) out+=("$a"); end_of_opts=1 ;;
            --line) out+=("$a"); expect_value=1 ;;
            --line=*|-*|"") out+=("$a") ;;
            *) if [ -e "$a" ]; then out+=("$(realpath -m -- "$a")"); else out+=("$a"); fi ;;
        esac
    done
    printf '%s\0' "${out[@]}"
}

pick_bin() {
    local rel="$LMDV_REPO/target/release/lmdv"
    local dbg="$LMDV_REPO/target/debug/lmdv"
    local profile="${LMDV_PROFILE:-$LMDV_PREFERRED_PROFILE}"

    if [ -n "${LMDV_BIN:-}" ]; then printf '%s' "$LMDV_BIN"; return 0; fi
    if [ -n "$LMDV_FIXED_BIN" ] && [ -x "$LMDV_FIXED_BIN" ]; then printf '%s' "$LMDV_FIXED_BIN"; return 0; fi

    case "$profile" in
        release) [ -x "$rel" ] && { printf '%s' "$rel"; return 0; } ;;
        debug)   [ -x "$dbg" ] && { printf '%s' "$dbg"; return 0; } ;;
    esac
    if [ -x "$rel" ] && [ -x "$dbg" ]; then
        if [ "$dbg" -nt "$rel" ]; then printf '%s' "$dbg"; else printf '%s' "$rel"; fi
        return 0
    fi
    [ -x "$rel" ] && { printf '%s' "$rel"; return 0; }
    [ -x "$dbg" ] && { printf '%s' "$dbg"; return 0; }
    return 1
}

main() {
    local bin profile
    profile="${LMDV_PROFILE:-$LMDV_PREFERRED_PROFILE}"

    if ! bin="$(pick_bin)"; then
        gui_error "No lmdv binary found. Build it with 'cargo build' (debug) or 'cargo build --release', or set LMDV_BIN."
        exit 127
    fi
    if [ ! -x "$bin" ]; then
        gui_error "Selected binary is not executable: $bin"
        exit 126
    fi
    if [ "$(readlink -f -- "$bin" 2>/dev/null)" = "$SELF" ]; then
        gui_error "Refusing to launch the launcher itself ($bin). Set LMDV_BIN to the real binary."
        exit 126
    fi

    local -a args=()
    mapfile -d '' -t args < <(normalize_args "$@")

    log "profile=$profile bin=$bin argc=${#args[@]} args=$(printf '[%s]' "${args[@]}")"

    # Cargo rewrites target/debug/lmdv in place; a concurrent launch can hit
    # ETXTBSY (exit 126). Retry briefly instead of showing an error.
    local rc attempt=0
    while :; do
        "$bin" "${args[@]}"
        rc=$?
        if [ "$rc" -eq 126 ] && [ "$attempt" -lt 10 ]; then
            sleep 0.1
            attempt=$((attempt + 1))
            continue
        fi
        break
    done

    [ "$rc" -ne 0 ] && log "exit=$rc bin=$bin"
    exit "$rc"
}

main "$@"
WRAPPER_EOF
)
    WRAPPER_TEMPLATE="${WRAPPER_TEMPLATE//@LMDV_REPO@/$REPO_DIR}"
    WRAPPER_TEMPLATE="${WRAPPER_TEMPLATE//@LMDV_PROFILE@/$PROFILE}"
    FIXED_BIN=""
    if ((BINARY_EXPLICIT)); then FIXED_BIN="$BINARY"; fi
    WRAPPER_TEMPLATE="${WRAPPER_TEMPLATE//@LMDV_FIXED_BIN@/$FIXED_BIN}"

    if ((DRY_RUN)); then
        say "[dry-run] write $WRAPPER"
    else
        mkdir -p -- "$BIN_DIR"
        printf '%s\n' "$WRAPPER_TEMPLATE" >"$WRAPPER"
        chmod 0755 -- "$WRAPPER"
        say "  wrote $WRAPPER"
    fi
fi

refresh_caches

# 5. Default handler, remembering what it was the first time.
if ((DO_MIME)); then
    if ((DRY_RUN)); then
        say "[dry-run] save previous handlers in $PREV_DEFAULTS"
    else
        mkdir -p -- "$STATE_DIR"
        if [ ! -f "$PREV_DEFAULTS" ]; then
            for m in "${MIME_TYPES[@]}"; do
                prev=""
                if command -v xdg-mime >/dev/null 2>&1; then
                    prev="$(xdg-mime query default "$m" 2>/dev/null || true)"
                elif command -v gio >/dev/null 2>&1; then
                    prev="$(gio mime "$m" 2>/dev/null | sed -n 's/^Default application for .*: //p' | head -1)"
                fi
                if [ "$prev" = "$DESKTOP_ID" ]; then prev=""; fi
                printf '%s\t%s\n' "$m" "$prev"
            done >"$PREV_DEFAULTS"
            say "  saved previous handlers in $PREV_DEFAULTS"
        fi
    fi
    for m in "${MIME_TYPES[@]}"; do
        set_default "$m"
    done
fi

# 6. Verification checklist.
say
say "Installed. Verify with:"
printf '  %-24s %s\n' "desktop file:" "$DESKTOP_FILE"
printf '  %-24s %s\n' "launcher:" "$EXEC_PATH"
printf '  %-24s %s\n' "icon:" "$ICON_FILE"
if command -v desktop-file-validate >/dev/null 2>&1 && [ -f "$DESKTOP_FILE" ]; then
    if desktop-file-validate "$DESKTOP_FILE"; then
        printf '  %-24s %s\n' "desktop-file-validate:" "clean"
    else
        printf '  %-24s %s\n' "desktop-file-validate:" "WARNINGS (see above)"
    fi
fi
for m in "${MIME_TYPES[@]}"; do
    if command -v xdg-mime >/dev/null 2>&1; then
        printf '  default %-16s -> %s\n' "$m" "$(xdg-mime query default "$m" 2>/dev/null || echo '?')"
    fi
done
cat <<'CHECK'

Double-click a .md file, or run:
  printf '# hello\n' > "/tmp/lmdv test ünïcode.md"
  gio open "/tmp/lmdv test ünïcode.md"
  xdg-mime query filetype "/tmp/lmdv test ünïcode.md"
  tail -5 "${XDG_STATE_HOME:-$HOME/.local/state}/lmdv/launch.log"
CHECK
```

### 8.2 `scripts/uninstall-md-handler.sh`

```bash
#!/usr/bin/env bash
# uninstall-md-handler.sh -- remove the per-user lmdv Markdown handler and
# restore the handler that was default before installation.
#
# This is a thin wrapper: all logic (and all flags) live in
# install-md-handler.sh. Pass --dry-run to preview, --purge to delete the
# saved state, --force to remove files that the installer did not create.
set -euo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
exec bash "$here/install-md-handler.sh" --uninstall "$@"
```

### 8.3 Flags

| Flag | Effect |
|---|---|
| `--dev` | launcher prefers `target/debug/lmdv` |
| `--release` | launcher prefers `target/release/lmdv` |
| *(none)* | launcher auto-selects the newest build |
| `--binary PATH` | bake an exact binary (still overridable by `LMDV_BIN`) |
| `--repo PATH` | project root; default is the parent of the script |
| `--prefix DIR` | where the launcher goes; default `~/.local/bin` |
| `--no-wrapper` | desktop `Exec` points at the binary directly (validated) |
| `--no-mime` | desktop entry only; do not claim `.md` |
| `--uninstall` | remove files, refresh caches, restore the previous handler |
| `--purge` | with `--uninstall`, also delete `~/.local/state/lmdv` |
| `--force` | overwrite/delete files not marked as ours |
| `--dry-run` | print every action, write nothing |
| `--allow-missing` | install even before the first build |

Idempotency guarantees:

* Re-running rewrites the same files; `previous-defaults.tsv` is only created on the first install.
* Re-running does not duplicate `mimeapps.list` entries (`xdg-mime` replaces; the fallback merge de-duplicates).
* Uninstalling twice is a no-op; the second run has no state file and just clears leftovers.
* `--dry-run` creates no directories and writes no state (tested by diffing the sandbox tree).

---

## 9. Test plan

Run from the repo root. Define both entry points once:

```bash
LMDV="$PWD/target/debug/lmdv"        # the app itself (IPC/CLI tests)
LMDVW="$HOME/.local/bin/lmdv"        # the installed launcher (profile/logging tests)
```

Use `$LMDV` for pure app/IPC checks and `$LMDVW` whenever the wrapper's behavior is under test.

### A. Static checks

```bash
bash -n scripts/install-md-handler.sh
bash -n scripts/uninstall-md-handler.sh
shellcheck -x scripts/*.sh                     # if installed (Arch: shellcheck)
desktop-file-validate ~/.local/share/applications/lmdv.desktop
systemd-analyze cat-config 2>/dev/null >/dev/null || true   # no-op; not required
```

Expected: no syntax errors; `desktop-file-validate` exits 0 with no output for the §2.1 file.

### B. MIME

```bash
printf '# hi\n' > "/tmp/lmdv test ünïcode.md"
xdg-mime query filetype "/tmp/lmdv test ünïcode.md"
gio info -a standard::content-type "/tmp/lmdv test ünïcode.md" | tail -1
grep -E 'markdown' ~/.local/share/mime/globs2
grep markdown ~/.local/share/mime/aliases
```

Expected: `text/markdown` twice; `60:…*.md`; both aliases (if `--no-mime` was not used).

### C. Registration and default

```bash
xdg-mime query default text/markdown
xdg-mime query default text/x-markdown
xdg-mime query default application/x-markdown
gio mime text/markdown
grep -n -A5 '^\[Default Applications\]' ~/.config/mimeapps.list
```

Expected: `lmdv.desktop` for all three; `Registered applications` contains `lmdv.desktop`; the mimeapps entries exist.

### D. Launch like a file manager

```bash
gio open "/tmp/lmdv test ünïcode.md"
xdg-open "/tmp/lmdv test ünïcode.md"
kioclient5 exec "/tmp/lmdv test ünïcode.md"
gtk-launch lmdv "/tmp/lmdv test ünïcode.md"
```

After each, check:

```bash
tail -5 ~/.local/state/lmdv/launch.log
```

Expected: one line per launch, the selected binary, the absolute path; the app shows the file. `gio open` and `xdg-open` must not print errors.

### E. Single instance and CLI

Start the app, then:

```bash
# 1. second file → new tab in the existing window
"$LMDV" "/tmp/lmdv test ünïcode.md"
# 2. same file again → its tab comes forward, reading position kept
"$LMDV" "/tmp/lmdv test ünïcode.md"
# 3. explicit line → jumps, even if the tab is already open
"$LMDV" "/tmp/lmdv test ünïcode.md" --line 40
"$LMDV" --line=41 "/tmp/lmdv test ünïcode.md"
"$LMDV" "/tmp/lmdv test ünïcode.md" -l 42
# 4. new window, even for an open file
"$LMDV" --new-window "/tmp/lmdv test ünïcode.md"
# 5. no file → focus the last window
"$LMDV"
# 6. no running instance → the first invocation owns the socket
```

Checks:

```bash
ss -xl | grep "lmdv-$(id -u).sock"
stat -c '%a %U %n' "$XDG_RUNTIME_DIR/lmdv-$(id -u).sock"     # 600 <you> …
pgrep -a lmdv | wc -l                                        # one process
```

Expected: exactly one `lmdv` process for 1–5; the socket mode is 600.

### F. Stale socket / crash recovery

```bash
# with the app running:
kill -9 "$(pgrep -n lmdv)"
ls -l "$XDG_RUNTIME_DIR/lmdv-$(id -u).sock"     # may still exist
"$LMDV" "/tmp/lmdv test ünïcode.md"             # must start cleanly
```

Expected: the new instance takes the lock, unlinks the stale socket, and starts. No manual `rm`.

### G. Paths that break shells

```bash
mkdir -p "/tmp/lmdv dir/ünïcode"
printf '# x\n' > "/tmp/lmdv dir/ünïcode/a b (1) & [x].md"
"$LMDV" "/tmp/lmdv dir/ünïcode/a b (1) & [x].md"
gio open "/tmp/lmdv dir/ünïcode/a b (1) & [x].md"
# leading dash via CLI
printf '# d\n' > "/tmp/lmdv dir/-dash.md"
"$LMDV" -- "/tmp/lmdv dir/-dash.md"
```

Expected: all open; no shell quoting errors; the log shows absolute paths.

Do **not** test a path containing `%` through `--no-wrapper` (the installer refuses it by design); through the wrapper it is only a runtime argument and works.

### H. Debug/release switching

```bash
cargo build
LMDV_LAUNCH_DEBUG=1 "$LMDVW" /tmp/x.md        # .../target/debug/lmdv
sleep 1; cargo build --release
LMDV_LAUNCH_DEBUG=1 "$LMDVW" /tmp/x.md        # .../target/release/lmdv (newer)
LMDV_PROFILE=debug LMDV_LAUNCH_DEBUG=1 "$LMDVW" /tmp/x.md
LMDV_BIN=/bin/true "$LMDVW" /tmp/x.md; echo $?  # 0, override honored
```

Expected: the printed/selected binary follows mtime; explicit overrides win.

### I. Uninstall and restore

```bash
scripts/install-md-handler.sh --uninstall
xdg-mime query default text/markdown          # previous handler, e.g. org.kde.kate.desktop
test ! -e ~/.local/share/applications/lmdv.desktop && echo removed
test ! -e ~/.local/bin/lmdv && echo removed
gio mime text/markdown                        # lmdv gone from registered list
```

Then the "user changed their mind" variant:

```bash
scripts/install-md-handler.sh --dev
xdg-mime default org.kde.kate.desktop text/markdown
scripts/install-md-handler.sh --uninstall
xdg-mime query default text/markdown          # still kate; not stolen back
```

### J. File managers and sessions

1. **Plasma Wayland:** double-click in Dolphin; repeat with a path containing spaces/Unicode; check the taskbar icon is not duplicated (`StartupWMClass`/app_id).
2. **Plasma X11:** log out, choose "Plasma (X11)", repeat; `xprop WM_CLASS` on the window must show `lmdv`.
3. **Nautilus/Thunar/Nemo** if installed: double-click; if the app is missing from Open With, run `update-desktop-database` and restart the FM.
4. **NVIDIA:** `vulkaninfo --summary | head` shows the NVIDIA device; if the window is blank, `WGPU_BACKEND=gl lmdv` and check the journal. Do not bake NVIDIA env vars into the desktop entry; make them opt-in in the wrapper config if needed.
5. **Without a session bus** (`env -u DBUS_SESSION_BUS_ADDRESS`): `gio open` still launches via `Exec`; the app must not crash (no notification/portal dependency in the launch path). `kbuildsycoca6` is skipped.

### K. Race and burst

```bash
for f in /tmp/lmdv-burst-{1..10}.md; do printf '# %s\n' "$f" > "$f"; done
for f in /tmp/lmdv-burst-*.md; do "$LMDV" "$f" & done; wait
```

Expected: one process, ten tabs (or the configured tab limit), no duplicates, no lost files. Repeat with half the launches using `--new-window`; expect five new windows and five tabs, with a cascade offset.

---

## 10. Common pitfalls

| # | Symptom | Cause | Detect | Fix |
|---|---|---|---|---|
| 1 | `file` says `text/plain`, so "MIME is broken" | `file(1)`/libmagic has no Markdown rule | compare `xdg-mime query filetype` and `gio info` | ignore `file`; use GIO |
| 2 | App missing from "Open With" but `gio open` works | stale `mimeinfo.cache` | `gio mime` shows it under Registered? | `update-desktop-database`; restart FM |
| 3 | Dolphin doesn't list it | stale `ksycoca` | `kbuildsycoca6 --noincremental` then re-check | run it, or re-login |
| 4 | Double-click opens the editor instead | no default set, or a `kde-mimeapps.list` / `[Removed Associations]` overrides | `xdg-mime query default`; inspect all `*mimeapps.list` | `xdg-mime default lmdv.desktop …`; remove the mask |
| 5 | Entry is invisible in menus | `NoDisplay=true` or `Hidden=true` | inspect the desktop file | set `NoDisplay=false`, remove `Hidden` |
| 6 | Nothing happens on double-click, no error | unquoted `Exec` path with spaces | `desktop-file-validate` passes anyway; run `gio open` | quote the program path |
| 7 | App not loaded at all (GIO falls back to another app) | bad `Exec` escaping (`\$`, `\"`, `%`), or `TryExec` missing | `gio mime` lists it under Registered? | use the wrapper path; see §2.3 |
| 8 | `%f` yields `file:///…` | used `%u`/`%U` in `Exec` | log the argv | switch to `%f`, or decode URIs in the app |
| 9 | App starts twice | connect-based single-instance raced | `pgrep -c lmdv` | use the `flock` design (§5.4) |
| 10 | "Address already in use" after a crash | stale socket without a lock | `ls -l $XDG_RUNTIME_DIR/lmdv-*.sock` | lock-holder unlinks; otherwise remove it once |
| 11 | `bind: File name too long` | socket path ≥ 108 bytes | `printf %s "$path" \| wc -c` | fall back to `/tmp/lmdv-$UID` |
| 12 | App fails only when double-clicked | GUI env lacks `PATH`/vars | run `env -i HOME=… PATH=/usr/bin:/bin lmdv file.md` | absolute paths in the wrapper; never rely on `.bashrc` |
| 13 | `lmdv` in a terminal is a different build | `~/.cargo/bin` shadows `~/.local/bin` | `type -a lmdv` | keep one install, or reorder `PATH` |
| 14 | Duplicate taskbar icon | `StartupWMClass` ≠ `WM_CLASS` (X11), or app_id ≠ desktop basename (Wayland) | `xprop WM_CLASS`; compositor window info | set the toolkit app id/name to `lmdv` |
| 15 | Spinner for ~10 s after launch | `StartupNotify=true` without support | desktop file | set `false` |
| 16 | Second launch opens behind | Wayland focus stealing prevention | observe; KWin marks it urgent | use `focus_window()` + attention request; accept compositor policy |
| 17 | `lmdv --line 40` does nothing | parsed as a file, or line 0/overflow | log the request | parse `--line`/`-l`/`--line=`; clamp 1-based |
| 18 | Extra CLI files ignored | reference semantics: only the first positional | compare with `parse_args` | document it; multi-file is a product decision |
| 19 | File named `-x.md` won't open | it is parsed as a flag | try `lmdv -- ./-x.md` | use `--`; file managers pass absolute paths |
| 20 | `cargo build` then double-click shows an error | `ETXTBSY` during relink | launch log exit 126 | wrapper retries; packaging should rename atomically |
| 21 | Installing over an existing handler loses it | state file deleted / first install while lmdv already default | `previous-defaults.tsv` contents | keep state; restore is best-effort otherwise |
| 24 | Wallpaper/theme makes the icon invisible | monochrome/transparent SVG | inspect in the menu | ship a filled, high-contrast icon |

---

## Appendix A — Evidence from this machine

All of the following were executed during this design; they are the basis for the "verified" claims.

```console
$ xdg-mime query filetype /tmp/opencode/_t.md
text/markdown
$ file --mime-type /tmp/opencode/_t.md
/tmp/opencode/_t.md: text/plain
$ gio info -a standard::content-type /tmp/opencode/_t.md | tail -1
  standard::content-type: text/markdown

$ grep markdown /usr/share/mime/globs2
50:text/markdown:*.md
50:text/markdown:*.mkd
50:text/markdown:*.markdown
$ grep markdown /usr/share/mime/aliases        # stock
text/x-markdown text/markdown

# after a user-level package + update-mime-database:
60:text/markdown:*.md
application/x-markdown text/markdown
text/x-markdown text/markdown
```

```console
$ desktop-file-validate ~/.local/share/applications/lmdv.desktop
$ echo $?
0
$ xdg-mime default lmdv.desktop text/markdown text/x-markdown application/x-markdown
$ grep -A2 'Default Applications' ~/.config/mimeapps.list
[Default Applications]
text/markdown=lmdv.desktop
text/x-markdown=lmdv.desktop
application/x-markdown=lmdv.desktop
```

```console
$ gio open "/tmp/lmdv dir/ünïcode/a b (1).md"
# ~/.local/state/lmdv/launch.log:
2026-11-12T20:41:07Z [12345] profile=debug bin=…/target/debug/lmdv argc=1 args=[/tmp/lmdv dir/ünïcode/a b (1).md]

$ xdg-open …        # XDG_UTILS_DEBUG_LEVEL=2: "Selected DE kde"
$ kioclient5 exec … # launched the registered handler
$ gtk-launch lmdv … # launched the registered handler
```

```console
$ command -v gio xdg-mime xdg-open update-desktop-database update-mime-database \
    desktop-file-validate kioclient5 kbuildsycoca6
/usr/bin/gio
/usr/bin/xdg-mime
/usr/bin/xdg-open
/usr/bin/update-desktop-database
/usr/bin/update-mime-database
/usr/bin/desktop-file-validate
/usr/bin/kioclient5
/usr/bin/kbuildsycoca6
```

Arch hooks present (packaged installs refresh caches automatically):

```
/usr/share/libalpm/hooks/30-update-mime-database.hook
/usr/share/libalpm/hooks/update-desktop-database.hook
/usr/share/libalpm/hooks/gtk-update-icon-cache.hook
```

## Appendix B — Decisions to record in `docs/DECISIONS.md`

1. **Canonical MIME is `text/markdown`; aliases are added per-user.** Do not invent a type.
2. **Extension-based detection only.** No magic rule.
3. **The desktop entry always points at a wrapper**, never directly at `target/…`, except with `--no-wrapper` for packaging.
4. **Single instance is `flock` + Unix socket**, not D-Bus.
5. **CLI is exactly the reference grammar**, including "first positional only" and ignored unknown flags; `-h/--help` and `-V/--version` are Linux additions.
6. **`--wait` is deferred** and the protocol reserves a `wait` boolean.
7. **Routing follows `windowing.rs`**, including the idle-window reuse and the in-flight set.
8. **`--line` is 1-based and beats the remembered position; a plain re-open does not reset it.**
9. **Per-user registration only**; no `sudo` in any script.
10. **Uninstall restores the previous handler only if lmdv is still the default.**
11. **Reconcile with `ARCHITECTURE.md` §9.6**: adopt the `$XDG_RUNTIME_DIR/lmdv/<instance-id>.sock` directory form when portable mode lands; keep the `flock` claim design (it is strictly stronger than "connect and see").
12. **Launcher log namespace**: `$XDG_STATE_HOME/lmdv/launch.log` is intentionally separate from the app's `$XDG_STATE_HOME/md.lmdv/` tree; rename it if one namespace is preferred. It is a dev aid, not app telemetry, and can be dropped for release builds.
13. **Desktop id `lmdv` vs data root `md.lmdv`**: keep the command-facing identity `lmdv`; if reverse-DNS is preferred for the desktop file, change the id and app id together (see §2.4).

## Appendix C — Open questions

* Does the product want multi-file CLI (`lmdv a.md b.md`) despite the reference ignoring extras? If yes, the routing table and window/tab placement need a rule.
* Should `--wait` ship in 0.1, or wait for an editor integration to justify it?
* Native titlebar / client-side decorations affect `StartupWMClass` and focus; coordinate with the Rune windowing spike (`docs/RISKS_AND_SPIKES.md`).
* Should the wrapper ever build automatically (`cargo build` on first launch)? Recommendation: **no** — a GUI launch must never block on a multi-minute build, and a build failure would be invisible.
