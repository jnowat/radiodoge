#!/usr/bin/env python3
"""Make local/CI release APKs buildable without a Play Store keystore in the repo.

Tauri's generated app/build.gradle.kts wires the release buildType to a
`signingConfigs.create("release")` block that reads ANDROID_KEY_STORE_* env
vars. With those unset, `assemble*Release` fails even though a debug-signed
release APK (R8/minified, stripped .so) is what we want for sideload testing.

When ANDROID_KEY_STORE_PATH is unset/empty, point the release buildType at the
built-in debug signingConfig instead. Real release keystores keep working when
the env vars are set (CI secrets or a local keystore path) — nothing is
committed.

Called from scripts/build-android.ps1 (and safe to call from CI) after
`tauri android init`.
"""
from __future__ import annotations

import os
import re
import sys
from pathlib import Path

path = Path("radiodoge-gui/src-tauri/gen/android/app/build.gradle.kts")
if not path.exists():
    print(f"WARNING: {path} not found - skipping release-signing patch", file=sys.stderr)
    sys.exit(0)

text = path.read_text(encoding="utf-8")
marker = "RADIODOGE_RELEASE_SIGNING_FALLBACK"
if marker in text:
    print("Release signing fallback already patched - no change")
    sys.exit(0)

keystore = (os.environ.get("ANDROID_KEY_STORE_PATH") or "").strip()
if keystore:
    print(f"ANDROID_KEY_STORE_PATH is set ({keystore}) - leaving release signing as generated")
    sys.exit(0)

# Replace the release buildType's signingConfig line to use debug when no keystore.
# Keep minify/proguard as Tauri generated them.
pat = re.compile(
    r'(getByName\("release"\)\s*\{\s*\n)(\s*)signingConfig\s*=\s*signingConfigs\.getByName\("release"\)',
    re.M,
)
m = pat.search(text)
if not m:
    print("WARNING: release signingConfig line not found - skipping", file=sys.stderr)
    sys.exit(0)

indent = m.group(2)
replacement = (
    m.group(1)
    + indent
    + f"// {marker}: no ANDROID_KEY_STORE_PATH — debug-sign the release APK for local/sideload builds\n"
    + indent
    + 'signingConfig = signingConfigs.getByName("debug")'
)
new_text, n = pat.subn(replacement, text, count=1)
if n != 1:
    print("WARNING: failed to patch release signingConfig", file=sys.stderr)
    sys.exit(0)

path.write_text(new_text, encoding="utf-8", newline="\n")
print(f"Patched {path}: release buildType uses debug signingConfig (no keystore secrets)")
