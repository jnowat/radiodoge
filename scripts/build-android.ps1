param(
    [switch]$Release,
    [string]$OutDir = ""
)
# Build a RadioDoge Android debug (default) or release APK on a developer machine.
# Mirrors .github/workflows/build-android.yml: icon gen → android init → JitPack +
# BLE manifest patches → tauri android build. Does not commit gen/android.
#
# Prerequisites: JDK 17+, Android SDK + NDK 27.x, ANDROID_HOME, Rust android targets.
# Run from the repo root:  powershell -File scripts/build-android.ps1

$ErrorActionPreference = "Stop"
$repo = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $repo

if (-not $env:ANDROID_HOME) {
    $guess = Join-Path $env:LOCALAPPDATA "Android\Sdk"
    if (Test-Path $guess) { $env:ANDROID_HOME = $guess }
}
if (-not $env:ANDROID_HOME -or -not (Test-Path $env:ANDROID_HOME)) {
    throw "ANDROID_HOME is not set and the default SDK path was not found."
}

# Prefer an installed NDK 27.x (CI pins 27.0.12077973; newer 27.x is fine locally).
if (-not $env:ANDROID_NDK_HOME -or -not (Test-Path $env:ANDROID_NDK_HOME)) {
    $ndkRoot = Join-Path $env:ANDROID_HOME "ndk"
    $pick = Get-ChildItem $ndkRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -like "27.*" } |
        Sort-Object Name -Descending |
        Select-Object -First 1
    if (-not $pick) { throw "No NDK 27.x under $ndkRoot. Install ndk;27.0.12077973 (or any 27.x)." }
    $env:ANDROID_NDK_HOME = $pick.FullName
    $env:NDK_HOME = $pick.FullName
}
Write-Host "ANDROID_HOME=$env:ANDROID_HOME"
Write-Host "ANDROID_NDK_HOME=$env:ANDROID_NDK_HOME"

Push-Location radiodoge-gui
try {
    if (-not (Test-Path node_modules)) { npm ci }
    Write-Host "== icons =="
    npm run tauri -- icon src-tauri/icons/doge-radio.png
    Write-Host "== android init =="
    npm run tauri -- android init --ci
} finally { Pop-Location }

Write-Host "== patch JitPack + BLE permissions =="
python .github/scripts/patch_jitpack.py
python .github/scripts/patch_manifest.py

Push-Location radiodoge-gui
try {
    $env:CARGO_INCREMENTAL = "0"
    if ($Release) {
        Write-Host "== tauri android build --apk (release) =="
        npm run tauri -- android build --apk --verbose
    } else {
        Write-Host "== tauri android build --apk --debug =="
        npm run tauri -- android build --apk --debug --verbose
    }
    if ($LASTEXITCODE -ne 0) { throw "tauri android build failed with exit $LASTEXITCODE" }
} finally { Pop-Location }

$apks = Get-ChildItem "radiodoge-gui/src-tauri/gen/android" -Recurse -Filter "*.apk" -ErrorAction SilentlyContinue
if (-not $apks) { throw "Build finished but no APK was found under gen/android." }
$apks | ForEach-Object { Write-Host "APK: $($_.FullName) ($([math]::Round($_.Length/1MB, 2)) MiB)" }

if ($OutDir) {
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    foreach ($a in $apks) {
        Copy-Item -Force $a.FullName (Join-Path $OutDir $a.Name)
        Write-Host "Copied $($a.Name) -> $OutDir"
    }
}

Write-Host "ANDROID_BUILD_OK"
