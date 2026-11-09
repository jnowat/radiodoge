param(
    [switch]$Release,
    [switch]$AllAbis,
    [string]$OutDir = ""
)
# Build a RadioDoge Android debug (default) or release APK on a developer machine.
# Mirrors .github/workflows/build-android.yml: icon gen → android init → JitPack +
# BLE + release-signing patches → tauri android build. Does not commit gen/android.
#
# -Release builds a minified release APK. Without ANDROID_KEY_STORE_* env vars the
# release build is signed with the Android debug keystore (sideload/testing only;
# no secrets in the repo). Set the env vars to use a real keystore.
# Default is arm64-only (phones); pass -AllAbis for every ABI Tauri builds.
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

$arm64Only = -not $AllAbis
$pkgVer = (Get-Content (Join-Path $repo "radiodoge-gui/package.json") -Raw |
    ConvertFrom-Json).version
Write-Host "RadioDoge version=$pkgVer  Release=$Release  Arm64Only=$arm64Only"

Push-Location radiodoge-gui
try {
    if (-not (Test-Path node_modules)) { npm ci }
    Write-Host "== icons =="
    npm run tauri -- icon src-tauri/icons/doge-radio.png
    Write-Host "== android init =="
    npm run tauri -- android init --ci
} finally { Pop-Location }

Write-Host "== patch JitPack + BLE permissions + release signing fallback =="
python .github/scripts/patch_jitpack.py
python .github/scripts/patch_manifest.py
python .github/scripts/patch_android_release_signing.py

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
    if ($arm64Only) {
        $map = @{ "aarch64-linux-android" = "arm64-v8a" }
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
        Write-Host "Copied JNI lib $($map[$triple]) from $src ($([math]::Round((Get-Item $src).Length/1MB, 2)) MiB)"
        $copied++
    }
    if ($copied -eq 0) { throw "No libradiodoge_gui_lib.so found under $cargoTarget for android triples." }
}

Push-Location radiodoge-gui
$tauriExit = 0
try {
    $env:CARGO_INCREMENTAL = "0"
    $tauriArgs = @("android", "build", "--apk", "--verbose")
    if (-not $Release) { $tauriArgs += "--debug" }
    if ($arm64Only) {
        $tauriArgs += @("--target", "aarch64", "--split-per-abi")
    }
    if ($Release) {
        Write-Host "== tauri android build --apk (release$(if ($arm64Only) { ', arm64' })) =="
    } else {
        Write-Host "== tauri android build --apk --debug$(if ($arm64Only) { ' (arm64)' }) =="
    }
    & npm.cmd run tauri -- @tauriArgs
    $tauriExit = $LASTEXITCODE
} finally { Pop-Location }

$apks = @(Get-ChildItem "radiodoge-gui/src-tauri/gen/android" -Recurse -Filter "*.apk" -ErrorAction SilentlyContinue)
if ($tauriExit -ne 0 -or $apks.Count -eq 0) {
    Write-Host "== Windows fallback: copy .so into jniLibs + gradle assemble (skip rustBuild*) =="
    Install-JniLibsFromCargo
    $gradlew = Join-Path $repo "radiodoge-gui/src-tauri/gen/android/gradlew.bat"
    if (-not (Test-Path $gradlew)) { throw "gradlew.bat missing; tauri android init may have failed." }
    Push-Location (Join-Path $repo "radiodoge-gui/src-tauri/gen/android")
    try {
        # Prefer arm64 (phones). Skip every rustBuild* task: tauri already compiled
        # the .so (or we copied it into jniLibs). Re-running rustBuild tries to
        # symlink into jniLibs and fails without Windows Developer Mode.
        $task = if ($Release) { ":app:assembleArm64Release" } else { ":app:assembleArm64Debug" }
        $skip = @(
            '-x', 'rustBuildUniversalDebug', '-x', 'rustBuildArm64Debug',
            '-x', 'rustBuildArmDebug', '-x', 'rustBuildX86Debug', '-x', 'rustBuildX86_64Debug',
            '-x', 'rustBuildUniversalRelease', '-x', 'rustBuildArm64Release',
            '-x', 'rustBuildArmRelease', '-x', 'rustBuildX86Release', '-x', 'rustBuildX86_64Release'
        )
        Write-Host "gradle $task (rustBuild* excluded)"
        & .\gradlew.bat $task @skip --no-daemon
        if ($LASTEXITCODE -ne 0) { throw "gradle $task failed with exit $LASTEXITCODE" }
    } finally { Pop-Location }
    $apks = @(Get-ChildItem "radiodoge-gui/src-tauri/gen/android" -Recurse -Filter "*.apk" -ErrorAction SilentlyContinue)
}

if ($apks.Count -eq 0) { throw "Build finished but no APK was found under gen/android." }

# Prefer arm64 APKs when several ABIs were produced.
$preferred = @($apks | Where-Object { $_.FullName -match 'arm64|aarch64' })
if ($preferred.Count -eq 0) { $preferred = $apks }
$preferred | ForEach-Object { Write-Host "APK: $($_.FullName) ($([math]::Round($_.Length/1MB, 2)) MiB)" }

if ($OutDir) {
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    $kind = if ($Release) { "release" } else { "debug" }
    foreach ($a in $preferred) {
        $abi = if ($a.FullName -match 'arm64|aarch64') { "arm64" }
               elseif ($a.FullName -match 'armeabi|armv7') { "armv7" }
               elseif ($a.FullName -match '[\\/]x86_64[\\/]|x86_64') { "x86_64" }
               elseif ($a.FullName -match '[\\/]x86[\\/]') { "x86" }
               else { "android" }
        $destName = "RadioDoge_${pkgVer}_${abi}-${kind}.apk"
        $destPath = Join-Path $OutDir $destName
        Copy-Item -Force $a.FullName $destPath
        Write-Host "Copied $($a.Name) -> $destPath"
    }
}

Write-Host "ANDROID_BUILD_OK"
