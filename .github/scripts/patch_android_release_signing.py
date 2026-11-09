#!/usr/bin/env python3
"""Ensure release APKs are signed without committing keystore secrets.

Current Tauri `android init` generates a release buildType with *no*
`signingConfig` (so `assemble*Release` emits an *-unsigned.apk). Older
templates wired release to `signingConfigs.create("release")` backed by
`ANDROID_KEY_STORE_*` env vars — which also yields an unusable APK when
those vars are unset.

This patch runs after `tauri android init` (from scripts/build-android.ps1):

* No `ANDROID_KEY_STORE_PATH` → release buildType uses the built-in
  `debug` signingConfig (sideload/testing only).
* `ANDROID_KEY_STORE_PATH` set → ensure a `signingConfigs.create("release")`
  block from the env vars and point release at it (Play/CI keystores).

Idempotent via the RADIODOGE_RELEASE_SIGNING_FALLBACK marker.
"""
from __future__ import annotations

import os
import re
import sys
from pathlib import Path

MARKER = "RADIODOGE_RELEASE_SIGNING_FALLBACK"
DEFAULT_PATH = Path("radiodoge-gui/src-tauri/gen/android/app/build.gradle.kts")

RELEASE_SIGNING_CONFIG_BLOCK = """\
    signingConfigs {
        create("release") {
            storeFile = providers.environmentVariable("ANDROID_KEY_STORE_PATH").orNull?.let { file(it) }
            storePassword = providers.environmentVariable("ANDROID_KEY_STORE_PASSWORD").orNull
            keyAlias = providers.environmentVariable("ANDROID_KEY_ALIAS").orNull
            keyPassword = providers.environmentVariable("ANDROID_KEY_PASSWORD").orNull
        }
    }

"""


def patch_text(text: str, *, keystore: str | None) -> tuple[str, str]:
    """Return (new_text, action_description). Raises ValueError on failure."""
    if MARKER in text:
        return text, "already patched"

    # Find the release buildType block opener.
    release_open = re.search(
        r'getByName\("release"\)\s*\{[ \t]*\n',
        text,
    )
    if not release_open:
        raise ValueError('getByName("release") { ... } not found')

    # Indentation of properties inside the release block (line after opener).
    after = text[release_open.end() :]
    indent_m = re.match(r"([ \t]*)\S", after)
    indent = indent_m.group(1) if indent_m else "            "

    if keystore:
        # Ensure signingConfigs.create("release") exists, then wire release to it.
        out = text
        if 'create("release")' not in out or "signingConfigs" not in out.split('getByName("release")')[0]:
            # Insert signingConfigs block just inside android { ... }
            android_open = re.search(r"android\s*\{\s*\n", out)
            if not android_open:
                raise ValueError("android { ... } not found")
            out = out[: android_open.end()] + RELEASE_SIGNING_CONFIG_BLOCK + out[android_open.end() :]
            # Re-find release opener after insertion.
            release_open = re.search(r'getByName\("release"\)\s*\{[ \t]*\n', out)
            if not release_open:
                raise ValueError("release block lost after signingConfigs insert")
            after = out[release_open.end() :]
            indent_m = re.match(r"([ \t]*)\S", after)
            indent = indent_m.group(1) if indent_m else "            "

        line = (
            f"{indent}// {MARKER}: ANDROID_KEY_STORE_PATH set — use release keystore\n"
            f'{indent}signingConfig = signingConfigs.getByName("release")\n'
        )
        # Replace existing signingConfig line if present, else insert after opener.
        block_start = release_open.end()
        # Limit search to this buildType block (until a line with only closing brace at buildTypes level is hard;
        # replace first signingConfig= inside release or insert).
        release_slice = out[block_start:]
        end_rel = release_slice.find("\n        }")
        if end_rel < 0:
            end_rel = release_slice.find("\n    }")
        region = release_slice if end_rel < 0 else release_slice[:end_rel]
        if re.search(r"signingConfig\s*=", region):
            region2 = re.sub(
                r"[ \t]*signingConfig\s*=\s*signingConfigs\.getByName\(\"[^\"]+\"\)[ \t]*\n?",
                line,
                region,
                count=1,
            )
            out = out[:block_start] + region2 + out[block_start + len(region) :]
        else:
            out = out[:block_start] + line + out[block_start:]
        return out, "release signingConfig → release keystore (env)"

    # No keystore: debug-sign release.
    line = (
        f"{indent}// {MARKER}: no ANDROID_KEY_STORE_PATH — debug-sign release for local/sideload builds\n"
        f'{indent}signingConfig = signingConfigs.getByName("debug")\n'
    )
    block_start = release_open.end()
    release_slice = text[block_start:]
    # Prefer replacing an existing signingConfig assignment inside release.
    m = re.search(
        r"[ \t]*signingConfig\s*=\s*signingConfigs\.getByName\(\"[^\"]+\"\)[ \t]*\n?",
        release_slice,
    )
    if m and (release_slice.find("\n        }") < 0 or m.start() < release_slice.find("\n        }")):
        out = text[: block_start + m.start()] + line + text[block_start + m.end() :]
        return out, "release signingConfig → debug (replaced)"

    out = text[:block_start] + line + text[block_start:]
    return out, "release signingConfig → debug (inserted)"


def main(argv: list[str]) -> int:
    path = Path(argv[1]) if len(argv) > 1 else DEFAULT_PATH
    if not path.exists():
        print(f"WARNING: {path} not found - skipping release-signing patch", file=sys.stderr)
        return 0

    text = path.read_text(encoding="utf-8")
    keystore = (os.environ.get("ANDROID_KEY_STORE_PATH") or "").strip() or None
    try:
        new_text, action = patch_text(text, keystore=keystore)
    except ValueError as e:
        print(f"WARNING: {e} - skipping", file=sys.stderr)
        return 0

    if action == "already patched":
        print("Release signing fallback already patched - no change")
        return 0

    path.write_text(new_text, encoding="utf-8", newline="\n")
    print(f"Patched {path}: {action}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
