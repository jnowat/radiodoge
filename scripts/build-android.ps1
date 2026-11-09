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

# Cargo android target dir (shared Bohr cache or default).
$cargoTarget = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo "target" }

function Install-JniLibsFromCargo {
    # Tauri tries to symlink libradiodoge_gui_lib.so into jniLibs; Windows without
    # Developer Mode rejects that. Copy instead so Gradle can package the APK.
    $map = @{
        "aarch64-linux-android"   = "arm64-v8a"
        "armv7-linux-androideabi" = "armeabi-v7a"
        "i686-linux-android"      = "x86"
        "x86_64-linux-android"    = "x86_64"
    }
    $profile = if ($Release) { "release" } else { "debug" }
    $jniRoot = Join-Path $repo "radiodoge-gui/src-tauri/gen/android/app/src/main/jniLibs"
    $copied = 0
    foreach ($triple in $map.Keys) {
        $src = Join-Path $cargoTarget "$triple\$profile\libradiodoge_gui_lib.so"
        if (-not (Test-Path $src)) { continue }
        $destDir = Join-Path $jniRoot $map[$triple]
        New-Item -ItemType Directory -Force -Path $destDir | Out-Null
        $dest = Join-Path $destDir "libradiodoge_gui_lib.so"
        # Remove a broken symlink left by tauri, then copy.
        if (Test-Path $dest) { Remove-Item -Force $dest }
        Copy-Item -Force $src $dest
        Write-Host "Copied JNI lib $($map[$triple]) from $src"
        $copied++
    }
    if ($copied -eq 0) { throw "No libradiodoge_gui_lib.so found under $cargoTarget for android triples." }
}

Push-Location radiodoge-gui
$tauriExit = 0
try {
    $env:CARGO_INCREMENTAL = "0"
    if ($Release) {
        Write-Host "== tauri android build --apk (release) =="
        npm run tauri -- android build --apk --verbose
    } else {
        Write-Host "== tauri android build --apk --debug =="
        npm run tauri -- android build --apk --debug --verbose
    }
    $tauriExit = $LASTEXITCODE
} finally { Pop-Location }

$apks = @(Get-ChildItem "radiodoge-gui/src-tauri/gen/android" -Recurse -Filter "*.apk" -ErrorAction SilentlyContinue)
if ($tauriExit -ne 0 -or $apks.Count -eq 0) {
    Write-Host "== symlink fallback: copy .so into jniLibs + gradle assemble =="
    Install-JniLibsFromCargo
    $gradlew = Join-Path $repo "radiodoge-gui/src-tauri/gen/android/gradlew.bat"
    if (-not (Test-Path $gradlew)) { throw "gradlew.bat missing; tauri android init may have failed." }
    Push-Location (Join-Path $repo "radiodoge-gui/src-tauri/gen/android")
    try {
        $task = if ($Release) { ":app:assembleRelease" } else { ":app:assembleDebug" }
        & .\gradlew.bat $task --no-daemon
        if ($LASTEXITCODE -ne 0) { throw "gradle $task failed with exit $LASTEXITCODE" }
    } finally { Pop-Location }
    $apks = @(Get-ChildItem "radiodoge-gui/src-tauri/gen/android" -Recurse -Filter "*.apk" -ErrorAction SilentlyContinue)
}

if ($apks.Count -eq 0) { throw "Build finished but no APK was found under gen/android." }
$apks | ForEach-Object { Write-Host "APK: $($_.FullName) ($([math]::Round($_.Length/1MB, 2)) MiB)" }

if ($OutDir) {
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    foreach ($a in $apks) {
        Copy-Item -Force $a.FullName (Join-Path $OutDir $a.Name)
        Write-Host "Copied $($a.Name) -> $OutDir"
    }
}

Write-Host "ANDROID_BUILD_OK"
