#!/usr/bin/env python3
"""Offline fixtures for patch_android_release_signing.py (no Gradle required)."""
from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(Path(__file__).resolve().parent))

spec = importlib.util.spec_from_file_location(
    "patch_android_release_signing",
    Path(__file__).resolve().parent / "patch_android_release_signing.py",
)
mod = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(mod)

_repo = Path(__file__).resolve().parents[2]
FIXTURES = _repo / "scripts" / "fixtures" / "android-gradle"
if not FIXTURES.exists():
    FIXTURES = Path(__file__).resolve().parent / "fixtures"


def load(name: str) -> str:
    return (FIXTURES / name).read_text(encoding="utf-8")


def main() -> None:
    current = load("tauri-current-build.gradle.kts")
    legacy = load("tauri-legacy-build.gradle.kts")

    # Current Tauri: no signingConfig → insert debug
    out, action = mod.patch_text(current, keystore=None)
    assert mod.MARKER in out, action
    assert 'signingConfig = signingConfigs.getByName("debug")' in out
    assert 'getByName("release")' in out
    # Idempotent
    out2, action2 = mod.patch_text(out, keystore=None)
    assert action2 == "already patched"
    assert out2 == out

    # Legacy: replace release→release with debug
    out_l, action_l = mod.patch_text(legacy, keystore=None)
    assert 'signingConfig = signingConfigs.getByName("debug")' in out_l
    assert 'signingConfigs.getByName("release")' not in out_l.split('getByName("release")', 1)[1].split("isMinifyEnabled", 1)[0]
    assert mod.MARKER in out_l

    # With keystore on current template: inject signingConfigs + release wiring
    out_k, action_k = mod.patch_text(current, keystore=r"C:\keys\store.jks")
    assert 'create("release")' in out_k
    assert 'signingConfig = signingConfigs.getByName("release")' in out_k
    assert mod.MARKER in out_k
    assert "ANDROID_KEY_STORE_PATH" in out_k

    print("patch_android_release_signing fixtures OK")


if __name__ == "__main__":
    main()
