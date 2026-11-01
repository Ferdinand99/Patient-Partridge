# Runs as the standard user (see run-as-user.ps1). It checks who it is, installs the app with the
# NSIS installer, opens PDFs with the installed app (one in a folder with a space and a file name
# with letters outside ASCII, in a profile folder that has one too), and runs the engine's
# sandbox tests. Everything it finds goes to C:\pp\out\child.log; the exit code is 1 when a
# check failed. This file is ASCII only (Windows PowerShell 5.1 reads it in the system code page):
# letters outside ASCII are made from code points.
param(
  [Parameter(Mandatory)][string]$Root
)
$ErrorActionPreference = 'Continue'
$out = Join-Path $Root 'out'
$log = Join-Path $out 'child.log'
$script:failed = New-Object System.Collections.ArrayList

function Log([string]$text) {
  Add-Content -LiteralPath $log -Value $text -Encoding UTF8
}
function Check([string]$name, [bool]$ok, [string]$detail = '') {
  $tag = if ($ok) { 'PASS' } else { 'FAIL' }
  Log "[$tag] $name $detail"
  if (-not $ok) { [void]$script:failed.Add($name) }
}

. (Join-Path $Root 'scripts\session-info.ps1')

try {
  # ---- 1. Who this is --------------------------------------------------------------------------
  Log '=== 1. Identity of the process that runs the checks ==='
  Log (Get-SessionInfo)
  $groups = (& whoami /groups | Out-String)
  Log '--- whoami /groups ---'
  Log $groups
  Log '--- whoami /priv ---'
  Log ((& whoami /priv | Out-String))

  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = New-Object Security.Principal.WindowsPrincipal($identity)
  Check 'the user is not in Administrators' (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))
  Check 'the token has no Administrators SID (not even deny-only)' ($groups -notmatch 'S-1-5-32-544')
  Check 'the token is not elevated' ([PPProbe]::Elevated() -eq 0) "(TokenElevation $([PPProbe]::Elevated()))"
  Check 'the integrity level is Medium' ($groups -match 'Mandatory Label\\Medium Mandatory Level')
  # What an administrator could do and this user must not be able to.
  $probeFile = Join-Path $env:SystemRoot 'pp-standard-user-probe.txt'
  $wrote = $true
  try { Set-Content -LiteralPath $probeFile -Value 'x' -ErrorAction Stop } catch { $wrote = $false }
  if ($wrote) { Remove-Item -LiteralPath $probeFile -Force -ErrorAction SilentlyContinue }
  Check 'the user cannot write to the Windows folder' (-not $wrote)
  $machineKey = 'HKLM:\SOFTWARE\PPStandardUserProbe'
  $wroteMachine = $true
  try { New-Item -Path $machineKey -ErrorAction Stop | Out-Null } catch { $wroteMachine = $false }
  if ($wroteMachine) { Remove-Item -Path $machineKey -ErrorAction SilentlyContinue }
  Check 'the user cannot write to HKLM' (-not $wroteMachine)
  Check 'the user name has a letter outside ASCII'($env:USERNAME -match '[^\x00-\x7F]') "($env:USERNAME)"
  Check 'the profile folder has a letter outside ASCII' ($env:USERPROFILE -match '[^\x00-\x7F]') "($env:USERPROFILE)"

  # ---- 2. Install, as this user -----------------------------------------------------------------
  Log ''
  Log '=== 2. Install with the NSIS installer, as this user ==='
  $installer = Get-ChildItem -LiteralPath (Join-Path $Root 'in') -Filter '*-setup.exe' | Select-Object -First 1
  Check 'an installer is there' ($null -ne $installer)
  $installed = $null
  if ($installer) {
    Log "installer: $($installer.FullName) ($($installer.Length) bytes)"
    $setup = Start-Process -FilePath $installer.FullName -ArgumentList '/S' -PassThru
    $done = $setup.WaitForExit(300000)
    Check 'the installer ended' $done
    if ($done) { Log "installer exit code: $($setup.ExitCode)" }
    if (-not $done) { Stop-Process -Id $setup.Id -Force -ErrorAction SilentlyContinue }
    # The installer may hand the work to a copy of itself: wait for the program to appear.
    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline -and -not $installed) {
      $installed = Get-ChildItem -LiteralPath $env:LOCALAPPDATA -Recurse -Filter 'patient-partridge.exe' -ErrorAction SilentlyContinue |
        Select-Object -First 1
      if (-not $installed) { Start-Sleep -Seconds 2 }
    }
  }
  Check 'the program was installed in the profile of the user' ($null -ne $installed)
  if (-not $installed) { throw 'The program was not installed; nothing more to run.' }
  $exe = $installed.FullName
  $folder = $installed.DirectoryName
  Log "installed program: $exe"
  Log (Get-ChildItem -LiteralPath $folder | Select-Object Name, Length | Format-Table -AutoSize | Out-String)
  Check 'the install folder is under the profile of the user' ($exe.StartsWith($env:USERPROFILE, [StringComparison]::OrdinalIgnoreCase))
  Check 'the engine is installed next to the program' (Test-Path -LiteralPath (Join-Path $folder 'partridge-engine.exe'))
  Check 'the PDF library is installed' ((Get-ChildItem -LiteralPath $folder -Recurse -Filter 'pdfium.dll' -ErrorAction SilentlyContinue | Measure-Object).Count -gt 0)

  # ---- 3. Open PDFs with the installed app ------------------------------------------------------
  Log ''
  Log '=== 3. Open a PDF with the installed program ==='
  $ae = [string][char]0x00E6
  $oe = [string][char]0x00F8
  $aa = [string][char]0x00E5
  $docs = Join-Path $env:USERPROFILE 'Mine dokumenter'
  New-Item -ItemType Directory -Force -Path $docs | Out-Null
  $plain = Join-Path $Root 'docs\plain.pdf'
  $unicodePdf = Join-Path $docs ("rapport $($ae)$($oe)$($aa).pdf")
  Copy-Item -LiteralPath $plain -Destination $unicodePdf -Force
  $asciiPdf = Join-Path $Root 'docs\plain.pdf'

  function Open-InApp([string]$pdf, [string]$tag, [string]$reportFolder, [int]$waitSeconds = 90, [bool]$expectDrawn = $true) {
    Log "--- case '$tag': $pdf ---"
    $report = Join-Path $reportFolder "report-$tag.json"
    Remove-Item -LiteralPath $report -Force -ErrorAction SilentlyContinue
    $env:PARTRIDGE_MEASURE_REPORT = $report
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $app = Start-Process -FilePath $exe -ArgumentList ('"' + $pdf + '"') -PassThru
    $engineSeen = $null
    $reported = $false
    while ($clock.Elapsed.TotalSeconds -lt $waitSeconds) {
      if ((Test-Path -LiteralPath $report) -and ((Get-Item -LiteralPath $report).Length -gt 0)) { $reported = $true; break }
      if ($app.HasExited) { break }
      if (-not $engineSeen) {
        $e = Get-Process -Name 'partridge-engine' -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($e) { $engineSeen = "pid $($e.Id), session $($e.SessionId)" }
      }
      Start-Sleep -Milliseconds 200
    }
    $drawn = $false
    if ($reported) {
      Start-Sleep -Milliseconds 300
      $text = Get-Content -LiteralPath $report -Raw -Encoding UTF8
      Log "report: $text"
      try {
        $json = $text | ConvertFrom-Json
        $drawn = ($json.pages -gt 0) -and ($json.first_page_ms -gt 0)
      } catch { Log "the report could not be read: $_" }
    }
    Log "engine process seen: $engineSeen; app session: $($app.SessionId); waited $([int]$clock.Elapsed.TotalMilliseconds) ms"
    if ($expectDrawn) {
      Check "case '$tag': the first page was drawn" $drawn
    } else {
      Check "case '$tag': no first page is reported (negative control)" (-not $drawn)
    }
    # The app closes itself a few seconds after it has reported.
    if ($reported -and -not $app.WaitForExit(30000)) {
      Log 'the app did not close itself'
    }
    if (-not $app.HasExited) {
      if (-not $drawn) { Log 'the app was still running after the wait' }
      & taskkill /T /F /PID $app.Id | Out-Null
    } elseif (-not $reported) {
      Log "the app ended without a report, exit code $($app.ExitCode)"
    }
    if (-not $drawn -and $expectDrawn) {
      Log 'The application events of the last minutes:'
      try {
        Get-WinEvent -FilterHashtable @{ LogName = 'Application'; StartTime = (Get-Date).AddMinutes(-3) } -MaxEvents 15 -ErrorAction Stop |
          ForEach-Object { Log "  $($_.TimeCreated) $($_.ProviderName): $(($_.Message -split "`n")[0])" }
      } catch { Log "  (cannot read the events: $_)" }
      $data = Join-Path $env:LOCALAPPDATA 'com.ferdinand99.patientpartridge'
      if (Test-Path -LiteralPath $data) {
        Log (Get-ChildItem -LiteralPath $data | Select-Object Name | Format-Table -AutoSize | Out-String)
      }
    }
    Remove-Item Env:\PARTRIDGE_MEASURE_REPORT -ErrorAction SilentlyContinue
  }

  # The report is written into the profile folder too: a path with a letter outside ASCII.
  Open-InApp $unicodePdf 'unicode-path' $env:USERPROFILE
  Open-InApp $asciiPdf 'plain-path' $out
  # A control for the checks themselves: a file that is not a PDF must not look like a drawn page.
  $notPdf = Join-Path $docs 'not a pdf.pdf'
  Set-Content -LiteralPath $notPdf -Value 'this is not a PDF file' -Encoding ascii
  Open-InApp $notPdf 'not-a-pdf' $out 20 $false

  # ---- 4. The engine's own sandbox tests, as this user -------------------------------------------
  Log ''
  Log '=== 4. The sandbox tests of the engine, as this user ==='
  Log (Get-SessionInfo)
  $env:PARTRIDGE_PDFIUM_DIR = Join-Path $Root 'pdfium'
  foreach ($test in (Get-Content -LiteralPath (Join-Path $Root 'tests.txt'))) {
    $name = [IO.Path]::GetFileNameWithoutExtension($test)
    $stdout = Join-Path $out "test-$name.txt"
    $stderr = Join-Path $out "test-$name.err.txt"
    Log "--- $name ---"
    $run = Start-Process -FilePath $test -ArgumentList '--nocapture' -PassThru -NoNewWindow `
      -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $null = $run.Handle   # keeps the exit code readable after the process has ended
    $finished = $run.WaitForExit(900000)
    if (-not $finished) { Stop-Process -Id $run.Id -Force -ErrorAction SilentlyContinue }
    $summary = @(Get-Content -LiteralPath $stderr -ErrorAction SilentlyContinue) + @(Get-Content -LiteralPath $stdout -ErrorAction SilentlyContinue) |
      Where-Object { $_ -match '^test result|^test .* FAILED|panicked' } | Select-Object -First 12
    foreach ($line in $summary) { Log "  $line" }
    Check "tests '$name' pass" ($finished -and $run.ExitCode -eq 0) "(exit code $($run.ExitCode))"
  }
}
catch {
  Log "STOPPED: $($_ | Out-String)"
  [void]$script:failed.Add('the checks stopped early')
}

Log ''
if ($script:failed.Count -gt 0) {
  Log "FAILED: $($script:failed.Count) check(s): $($script:failed -join '; ')"
  exit 1
}
Log 'ALL CHECKS PASSED'
exit 0
