# Rune patches

Numbered patches applied by `scripts/apply-rune-patches.sh` with `patch -p1`
from `vendor/rune`. Paths are relative to `vendor/rune`.

`scripts/vendor-rune.sh` formats a fresh copy with the repository's
`rustfmt.toml` before patches are applied, so the patch context is
deterministic. Do not run `cargo fmt --all` from the repository root: it
follows path dependencies into `vendor/rune`; use `scripts/fmt-check.sh` for
lmdv's own crates.

Applied:

- `0001-text-inline-object.patch` — `Span::image` + `Span::rise`, inline image
  geometry for math, and `InlineImage::label` (the words a screen reader says
  where the image draws).
- `0002-close-request-veto.patch` — `on_close_request` before hiding.
- `0003-a11y-inline-object.patch` — the accessibility text speaks an inline
  image's label instead of the U+FFFC object marker; character lengths and word
  starts stay consistent with the spoken value.

Planned:

- `0004-text-hit.patch` — only if preview double-click needs a text hit callback.
- `0005-window-theme.patch` — optional native frame theme.
- `0006-linux-frame.patch` — deferred client-side decorations/resize.

Generate a patch from a pristine tree and the working vendor tree:

```bash
diff -ruN --label a/crates/rune/src/... --label b/crates/rune/src/... \
  /tmp/opencode/rune/crates/rune/src/... vendor/rune/crates/rune/src/...
```

Keep each patch focused; re-apply on every Rune rev bump and rerun the goldens.
