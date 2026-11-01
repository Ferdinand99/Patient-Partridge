# Builds the test programs of the engine and its client into C:\pp\build, a folder the standard
# user may read (prepare.ps1 sets that before anything is built there, so every new file inherits
# it). The programs are run in place by the checks: they find the engine and the self-test program
# by paths that were fixed when they were built. Writes the list of test programs to C:\pp\tests.txt.
$ErrorActionPreference = 'Stop'
$Root = 'C:\pp'
$env:CARGO_TARGET_DIR = Join-Path $Root 'build\target'

$lines = & cargo test --locked --no-run --message-format=json -p partridge-engine -p partridge-engine-client
if ($LASTEXITCODE -ne 0) { throw "cargo test --no-run failed ($LASTEXITCODE)" }

$tests = $lines |
  Where-Object { $_ -like '{*' } |
  ForEach-Object { $_ | ConvertFrom-Json } |
  Where-Object {
    $_.reason -eq 'compiler-artifact' -and $_.executable -and $_.profile.test -and
    ($_.target.kind -contains 'test')
  } |
  ForEach-Object { $_.executable } |
  Sort-Object -Unique
if (-not $tests) { throw 'No test programs were built.' }

$selftest = Join-Path $env:CARGO_TARGET_DIR 'debug\examples\engine_selftest.exe'
if (-not (Test-Path $selftest)) { throw "Missing $selftest" }
$engine = Join-Path $env:CARGO_TARGET_DIR 'debug\partridge-engine.exe'
if (-not (Test-Path $engine)) { throw "Missing $engine" }

Set-Content -Path (Join-Path $Root 'tests.txt') -Value $tests -Encoding ascii
Write-Host 'Test programs:'
$tests | ForEach-Object { Write-Host "  $_" }
