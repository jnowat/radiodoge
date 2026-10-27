param([string]$Base = 'origin/0.0.1-Beta-1')
# Pre-push gate: rustfmt, tests, clippy, svelte-check, JS unit tests, privacy scan.
# Run from the repo root. Exit 0 only if every step passes.
$ErrorActionPreference = 'Continue'
$fail = @()
cargo fmt --all -- --check 2>&1 | Select-String '^Diff in' | select -First 10 | Out-Host
if ($LASTEXITCODE -ne 0) { $fail += 'cargo fmt' }
$t = cargo test --workspace --lib --bins --tests 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { $fail += 'cargo test'; $t | Select-String 'FAILED|panicked|error' | select -First 15 | Out-Host }
$n = ([regex]::Matches($t, 'test result: ok\. (\d+) passed') | % { [int]$_.Groups[1].Value } | Measure-Object -Sum).Sum
"tests passed: $n"
$c = cargo clippy --workspace --all-targets -- -D warnings 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { $fail += 'clippy'; $c | Select-String '^error' -Context 0,6 | select -First 6 | Out-Host }
Push-Location radiodoge-gui
if (-not (Test-Path node_modules)) { npm ci --silent 2>&1 | Out-Null }
npx svelte-kit sync 2>&1 | Out-Null
$s = npx svelte-check --threshold error 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { $fail += 'svelte-check' }
($s -split "`n" | Select-String 'svelte-check found') -join ''
$u = npm run --silent test:unit 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { $fail += 'test:unit'; $u | Select-String 'not ok|Error' | select -First 10 | Out-Host }
(($u -split "`n" | Select-String '^. (pass|fail) ') -join ' ').Trim()
Pop-Location
# Privacy scan: block WIF-shaped private keys added in this branch.
$d = git diff "$Base...HEAD" | Select-String -Pattern '^\+.*\b[5KLQ][1-9A-HJ-NP-Za-km-z]{50,51}\b'
if ($d) { $fail += 'PRIVACY'; $d | Out-Host }
if ($fail) { "GATE FAILED: $($fail -join ', ')"; exit 1 } else { 'GATE OK'; exit 0 }
