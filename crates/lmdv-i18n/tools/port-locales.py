#!/usr/bin/env python3
"""Port the upstream TypeScript locale dictionaries to flat JSON catalogs.

The upstream files are TypeScript wrappers around a plain object:

    // en.ts
    export default { "key": "value", ... } as const;

    // it.ts
    import type { Dict } from "../i18n";
    const it: Dict = { "key": "value", ... };
    export default it;

This script extracts the object literal without evaluating TypeScript (no
`tsc`, no `node_modules`): it walks the braces with a small scanner that is
aware of strings, escapes and `//` / `/* */` comments, drops comments and
trailing commas, and parses the result as JSON. Values, escapes, Unicode and
`{placeholder}` braces are preserved byte for byte (JSON re-escapes only what
JSON requires).

Usage:
    python3 crates/lmdv-i18n/tools/port-locales.py \
        path/to/upstream/src/locales locales/app

The six expected source ids are fixed by the port; a missing file is an error.
Exit status is non-zero when a file cannot be parsed, a key repeats, or a value
is not a string.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

LOCALES = ("en", "it", "de", "fr", "es", "zh-CN")


class PortError(Exception):
    pass


def strip_comments_and_trailing_commas(text: str) -> str:
    """Remove comments and trailing commas outside strings."""
    out: list[str] = []
    i = 0
    n = len(text)
    while i < n:
        ch = text[i]
        if ch == '"':
            # Copy the string verbatim, escapes included.
            start = i
            i += 1
            while i < n:
                if text[i] == "\\":
                    i += 2
                    continue
                if text[i] == '"':
                    i += 1
                    break
                i += 1
            else:
                raise PortError("unterminated string")
            out.append(text[start:i])
        elif ch == "/" and i + 1 < n and text[i + 1] == "/":
            while i < n and text[i] != "\n":
                i += 1
            out.append("\n")
        elif ch == "/" and i + 1 < n and text[i + 1] == "*":
            end = text.find("*/", i + 2)
            if end < 0:
                raise PortError("unterminated block comment")
            i = end + 2
        else:
            out.append(ch)
            i += 1

    cleaned = "".join(out)
    # Drop a comma whose next non-space character closes the object/array.
    result: list[str] = []
    i = 0
    n = len(cleaned)
    while i < n:
        ch = cleaned[i]
        if ch == '"':
            start = i
            i += 1
            while i < n:
                if cleaned[i] == "\\":
                    i += 2
                    continue
                if cleaned[i] == '"':
                    i += 1
                    break
                i += 1
            result.append(cleaned[start:i])
            continue
        if ch == ",":
            j = i + 1
            while j < n and cleaned[j] in " \t\r\n":
                j += 1
            if j < n and cleaned[j] in "}]":
                result.append(" ")
                i += 1
                continue
        result.append(ch)
        i += 1
    return "".join(result)


def object_literal(source: str) -> str:
    """The body of the top-level dictionary object, comments removed."""
    marker = "export default {"
    at = source.find(marker)
    if at >= 0:
        start = at + len(marker) - 1
    else:
        marker = ": Dict = {"
        at = source.find(marker)
        if at < 0:
            raise PortError("no `export default {` or `: Dict = {` object found")
        start = at + len(marker) - 1

    depth = 0
    i = start
    n = len(source)
    while i < n:
        ch = source[i]
        if ch == '"':
            i += 1
            while i < n:
                if source[i] == "\\":
                    i += 2
                    continue
                if source[i] == '"':
                    break
                i += 1
            if i >= n:
                raise PortError("unterminated string")
            i += 1
            continue
        if ch == "/" and i + 1 < n and source[i + 1] == "/":
            while i < n and source[i] != "\n":
                i += 1
            continue
        if ch == "/" and i + 1 < n and source[i + 1] == "*":
            end = source.find("*/", i + 2)
            if end < 0:
                raise PortError("unterminated block comment")
            i = end + 2
            continue
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return source[start + 1 : i]
        i += 1
    raise PortError("unterminated object literal")


def duplicate_check(pairs: list[tuple[str, object]]) -> dict[str, object]:
    seen: dict[str, object] = {}
    for key, value in pairs:
        if key in seen:
            raise PortError(f"duplicate key {key!r}")
        seen[key] = value
    return seen


def port_file(path: Path) -> dict[str, str]:
    source = path.read_text(encoding="utf-8")
    body = object_literal(source)
    cleaned = strip_comments_and_trailing_commas("{" + body + "}")
    try:
        value = json.loads(cleaned, object_pairs_hook=duplicate_check)
    except json.JSONDecodeError as error:
        raise PortError(f"{path}: {error}") from error
    if not isinstance(value, dict):
        raise PortError(f"{path}: the dictionary is not an object")
    for key, message in value.items():
        if not isinstance(key, str) or not isinstance(message, str):
            raise PortError(f"{path}: {key!r} does not map to a string")
    return value


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    source_dir = Path(argv[1])
    out_dir = Path(argv[2])
    out_dir.mkdir(parents=True, exist_ok=True)

    catalogs: dict[str, dict[str, str]] = {}
    for locale in LOCALES:
        path = source_dir / f"{locale}.ts"
        if not path.is_file():
            print(f"missing source dictionary: {path}", file=sys.stderr)
            return 1
        catalogs[locale] = port_file(path)

    reference_keys = set(catalogs["en"])
    for locale, catalog in catalogs.items():
        missing = sorted(reference_keys - set(catalog))
        extra = sorted(set(catalog) - reference_keys)
        if missing or extra:
            print(
                f"{locale}: {len(missing)} missing key(s), {len(extra)} extra key(s)",
                file=sys.stderr,
            )
            for key in missing:
                print(f"  missing {key}", file=sys.stderr)
            for key in extra:
                print(f"  extra   {key}", file=sys.stderr)
            return 1

    for locale, catalog in catalogs.items():
        target = out_dir / f"{locale}.json"
        target.write_text(
            json.dumps(catalog, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        print(f"{target}: {len(catalog)} keys")

    print(f"all {len(catalogs)} locales share {len(reference_keys)} keys")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv))
    except PortError as error:
        print(error, file=sys.stderr)
        raise SystemExit(1)
