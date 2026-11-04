# Installs the stable app and the Windows beta on one computer, with the silent NSIS installers, and
# checks that they live side by side: another install folder, Start menu entry, uninstall entry,
# program name, data folder and single-instance lock, no .pdf association and no update address for
# the beta, and that removing one leaves the other working. Each app opens a PDF through
# scripts/smoke.mjs (it must draw the first page) and is also started and left open while the other
# is installed or removed. It also checks the sandbox of the engine and the signer in each copy: the
# installer's entries for the engine's AppContainer package on the folder and the programs, and a
# signature made with the engine and signer of each copy. Run from the root of the repository (the job in
# .github/workflows/windows-beta.yml does). This file is ASCII only.
param(
  [Parameter(Mandatory)][string]$Stable,   # NSIS installer of the stable app
  [Parameter(Mandatory)][string]$Beta,     # NSIS installer of the beta
  [Parameter(Mandatory)][string]$Pdf,      # a PDF to open
  [Parameter(Mandatory)][string]$Out,      # a folder for logs and pictures
  [Parameter(Mandatory)][string]$SignDriver, # crates/engine/examples/sign_installed.rs, built
  [Parameter(Mandatory)][string]$Certificate # a PKCS#12 file of the tests (password "test")
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$checksFile = Join-Path $Out 'checks.txt'

$StableName = 'Patient Partridge'
$BetaName = 'Patient Partridge Beta'
$StableId = 'com.ferdinand99.patientpartridge'
$BetaId = 'com.ferdinand99.patientpartridge.beta'
$StableProgram = 'patient-partridge.exe'
$BetaProgram = 'patient-partridge-beta.exe'
$failed = New-Object System.Collections.ArrayList

function Say([string]$text) {
  Write-Host $text
  Add-Content -LiteralPath $checksFile -Value $text -Encoding UTF8
}
function Check([string]$name, [bool]$ok, [string]$detail = '') {
  $tag = if ($ok) { 'PASS' } else { 'FAIL' }
  Say "[$tag] $name $detail"
  if (-not $ok) { [void]$failed.Add($name) }
}

$uninstallRoot = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
function Get-Entry([string]$name) {
  if (-not (Test-Path $uninstallRoot)) { return $null }
  Get-ChildItem $uninstallRoot | ForEach-Object { Get-ItemProperty -LiteralPath $_.PSPath } |
    Where-Object { $_.DisplayName -eq $name } | Select-Object -First 1
}
function First-Quoted([string]$text) {
  if ($text -match '^\s*"([^"]+)"') { return $Matches[1] }
  return $text.Trim()
}
function Install-App([string]$installer, [string]$name) {
  Say "--- install $name from $installer"
  $p = Start-Process -FilePath $installer -ArgumentList '/S' -PassThru
  if (-not $p.WaitForExit(300000)) { Stop-Process -Id $p.Id -Force; throw "The installer of $name did not end." }
  $deadline = (Get-Date).AddSeconds(60)
  while (-not (Get-Entry $name) -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 1 }
  $entry = Get-Entry $name
  if (-not $entry) { throw "$name has no uninstall entry after the installation (installer exit code $($p.ExitCode))." }
  $dir = if ($entry.InstallLocation) { First-Quoted $entry.InstallLocation } else { Split-Path (First-Quoted $entry.UninstallString) -Parent }
  Say "installed in $dir (installer exit code $($p.ExitCode))"
  $script:InstalledIn = [string]$dir
}
function Uninstall-App([string]$name) {
  Say "--- uninstall $name"
  $entry = Get-Entry $name
  if (-not $entry) { throw "$name has no uninstall entry." }
  $uninstaller = First-Quoted $entry.UninstallString
  $dir = Split-Path $uninstaller -Parent
  # `_?=` keeps the uninstaller in its folder, so that this call waits until it is done.
  $p = Start-Process -FilePath $uninstaller -ArgumentList '/S', "_?=$dir" -PassThru
  if (-not $p.WaitForExit(300000)) { Stop-Process -Id $p.Id -Force; throw "The uninstaller of $name did not end." }
  Say "uninstaller exit code $($p.ExitCode)"
  Start-Sleep -Seconds 2
  Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
}
function Start-Open([string]$exe) {
  Remove-Item Env:\PARTRIDGE_MEASURE_REPORT -ErrorAction SilentlyContinue
  Say "start: $exe"
  return Start-Process -FilePath $exe -ArgumentList ('"' + $Pdf + '"') -PassThru
}
function Stop-Tree([int]$id) {
  & taskkill /T /F /PID $id 2>&1 | Out-Null
}
function Wait-Window([System.Diagnostics.Process]$proc, [string]$identifier) {
  # The web view makes its profile folder when the window has started; give it a few seconds more.
  $profile = Join-Path $env:LOCALAPPDATA "$identifier\EBWebView"
  $deadline = (Get-Date).AddSeconds(150)
  while ((Get-Date) -lt $deadline -and -not (Test-Path -LiteralPath $profile) -and -not $proc.HasExited) { Start-Sleep -Seconds 1 }
  Start-Sleep -Seconds 10
}
function Smoke([string]$exe, [string]$label) {
  Say "--- smoke test: $label opens the PDF ($exe)"
  $text = & node scripts/smoke.mjs --bin $exe --pdf $Pdf --runs 1 2>&1 | Out-String
  $code = $LASTEXITCODE
  Say $text.Trim()
  Check "$label opens a PDF and draws the first page" ($code -eq 0) "(exit code $code)"
}
function Find-Shortcut([string]$name) {
  $roots = @((Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'), [Environment]::GetFolderPath('Desktop'))
  $found = @()
  foreach ($root in $roots) {
    if (Test-Path -LiteralPath $root) {
      $found += @(Get-ChildItem -LiteralPath $root -Recurse -Filter "$name.lnk" -ErrorAction SilentlyContinue)
    }
  }
  return $found
}
function Shortcut-Target([string]$file) {
  $shell = New-Object -ComObject WScript.Shell
  return $shell.CreateShortcut($file).TargetPath
}
function Registry-Mentions([string]$text) {
  & reg query 'HKCU\Software\Classes' /s /f $text 2>&1 | Out-Null
  return ($LASTEXITCODE -eq 0)
}
function File-Has([string]$file, [string]$text) {
  $bytes = [IO.File]::ReadAllBytes($file)
  return [Text.Encoding]::GetEncoding(28591).GetString($bytes).Contains($text)
}
# The AppContainer package of the engine and the signer (container.rs): the installer gives it read
# and execute on the install folder, without inheritance, and on the programs the sandboxed
# processes open by path. Both copies use the same package (its name is fixed); each has the entries
# on its own files.
$Package = 'S-1-15-2-1452645665-1071814252-341713739-2096083442-1079832312-2911523698-3580489556'
function Package-Rights([string]$path) {
  $line = (& icacls $path) | Where-Object { $_ -match [regex]::Escape($Package) } | Select-Object -First 1
  if (-not $line) { return $null }
  return ($line -replace ('^.*' + [regex]::Escape($Package) + ':'), '').Trim()
}
function Check-Package-Entries([string]$dir, [string]$label) {
  foreach ($name in @('', 'partridge-engine.exe', 'partridge-signer.exe', 'pdfium.dll')) {
    $path = if ($name) { Join-Path $dir $name } else { $dir }
    $rights = Package-Rights $path
    Check "the $label install gives the engine's package read and execute on $(if ($name) { $name } else { 'the folder' })" ($rights -eq '(RX)') "('$rights')"
  }
}
function Sign-With([string]$dir, [string]$label, [string]$tag) {
  $signed = Join-Path $Out "signed-$tag.pdf"
  Remove-Item -LiteralPath $signed -Force -ErrorAction SilentlyContinue
  Say "--- signing with the engine and the signer in $dir"
  $text = & $SignDriver $dir $Pdf $Certificate test $signed 2>&1 | Out-String
  $code = $LASTEXITCODE
  Say $text.Trim()
  Check "the $label engine and signer sign a PDF in their sandboxes" ($code -eq 0 -and (Test-Path -LiteralPath $signed)) "(exit code $code)"
}
function Local-Data([string]$identifier) { Join-Path $env:LOCALAPPDATA $identifier }
function Roaming-Data([string]$identifier) { Join-Path $env:APPDATA $identifier }
function List-Data([string]$identifier) {
  foreach ($base in @((Roaming-Data $identifier), (Local-Data $identifier))) {
    Say "data folder $base exists: $(Test-Path -LiteralPath $base)"
    if (Test-Path -LiteralPath $base) {
      Get-ChildItem -LiteralPath $base | ForEach-Object { Say "    $($_.Name)" }
    }
  }
}
function Save-Icon([string]$exe, [string]$png) {
  Add-Type -AssemblyName System.Drawing
  $icon = [System.Drawing.Icon]::ExtractAssociatedIcon($exe)
  $bitmap = $icon.ToBitmap()
  $bitmap.Save($png, [System.Drawing.Imaging.ImageFormat]::Png)
  # Pixels in the violet of the ribbon (Hedgerow brand colour).
  $violet = 0
  for ($y = 0; $y -lt $bitmap.Height; $y++) {
    for ($x = 0; $x -lt $bitmap.Width; $x++) {
      $c = $bitmap.GetPixel($x, $y)
      if ($c.A -gt 200 -and [Math]::Abs($c.R - 0x66) -lt 40 -and [Math]::Abs($c.G - 0x56) -lt 40 -and [Math]::Abs($c.B - 0xe8) -lt 40) { $violet++ }
    }
  }
  $bitmap.Dispose()
  return $violet
}

try {
  Say '=== 0. A clean computer: neither app is installed'
  Check 'the stable app is not installed yet' ($null -eq (Get-Entry $StableName))
  Check 'the beta is not installed yet' ($null -eq (Get-Entry $BetaName))
  Check 'the beta has no data folder yet' (-not (Test-Path -LiteralPath (Local-Data $BetaId)) -and -not (Test-Path -LiteralPath (Roaming-Data $BetaId)))

  Say ''
  Say '=== 1. Install the stable app and start it, then install the beta while it runs'
  Install-App $Stable $StableName
  $stableDir = $script:InstalledIn
  $stableExe = Join-Path $stableDir $StableProgram
  Check 'the stable program is installed' (Test-Path -LiteralPath $stableExe) "($stableExe)"
  $stableApp = Start-Open $stableExe
  Wait-Window $stableApp $StableId
  Check 'the stable app is running' (-not $stableApp.HasExited)
  # Control of the check for the .pdf association below: the stable app must show up in the registry.
  Check 'control: the stable app registers its file type' (Registry-Mentions $StableProgram)

  Install-App $Beta $BetaName
  $betaDir = $script:InstalledIn
  $betaExe = Join-Path $betaDir $BetaProgram
  Check 'the beta program is installed' (Test-Path -LiteralPath $betaExe) "($betaExe)"
  Check 'installing the beta did not end the running stable app' (-not $stableApp.HasExited)

  Say ''
  Say '=== 2. They are two installations'
  Check 'another install folder' ($stableDir -ne $betaDir) "($stableDir | $betaDir)"
  Check 'the beta folder is named after the beta' ((Split-Path $betaDir -Leaf) -eq $BetaName)
  Check 'the stable folder is not the beta folder or inside it' (-not $stableDir.StartsWith($betaDir, [StringComparison]::OrdinalIgnoreCase))
  Check 'both programs are there, with another name' ((Test-Path -LiteralPath $stableExe) -and (Test-Path -LiteralPath $betaExe) -and ($StableProgram -ne $BetaProgram))
  foreach ($pair in @(@($stableDir, 'stable'), @($betaDir, 'beta'))) {
    Check "the engine is installed next to the $($pair[1]) program" (Test-Path -LiteralPath (Join-Path $pair[0] 'partridge-engine.exe'))
    Check "the PDF library is installed with the $($pair[1]) program" ((Get-ChildItem -LiteralPath $pair[0] -Recurse -Filter 'pdfium.dll' -ErrorAction SilentlyContinue | Measure-Object).Count -gt 0)
    Check "the uninstaller is installed with the $($pair[1]) program" (Test-Path -LiteralPath (Join-Path $pair[0] 'uninstall.exe'))
  }
  Check 'the signer is installed next to each program' ((Test-Path -LiteralPath (Join-Path $stableDir 'partridge-signer.exe')) -and (Test-Path -LiteralPath (Join-Path $betaDir 'partridge-signer.exe')))
  Check-Package-Entries $stableDir 'stable'
  Check-Package-Entries $betaDir 'beta'
  $stableEntry = Get-Entry $StableName
  $betaEntry = Get-Entry $BetaName
  Check 'two uninstall entries, each with its own name' ($stableEntry -and $betaEntry -and ($stableEntry.PSChildName -ne $betaEntry.PSChildName)) "($($stableEntry.PSChildName) | $($betaEntry.PSChildName))"
  Check 'the beta entry has a beta version' ($betaEntry.DisplayVersion -match '-beta\.\d+$') "($($betaEntry.DisplayVersion))"
  Check 'the stable entry has no beta mark' ($stableEntry.DisplayVersion -notmatch 'beta') "($($stableEntry.DisplayVersion))"
  $stableLinks = @(Find-Shortcut $StableName)
  $betaLinks = @(Find-Shortcut $BetaName)
  Check 'a Start menu entry for each app' ($stableLinks.Count -gt 0 -and $betaLinks.Count -gt 0) "($($stableLinks.FullName -join ', ') | $($betaLinks.FullName -join ', '))"
  if ($betaLinks.Count -gt 0) {
    Check 'the beta entry starts the beta program' ((Shortcut-Target $betaLinks[0].FullName) -eq $betaExe) "($(Shortcut-Target $betaLinks[0].FullName))"
  }
  if ($stableLinks.Count -gt 0) {
    Check 'the stable entry starts the stable program' ((Shortcut-Target $stableLinks[0].FullName) -eq $stableExe) "($(Shortcut-Target $stableLinks[0].FullName))"
  }
  Check 'no file type for the beta (nothing in the registry names the beta program)' (-not (Registry-Mentions $BetaProgram))
  Check 'the stable app keeps its file type' (Registry-Mentions $StableProgram)

  Say ''
  Say '=== 3. The update address and the icon'
  $address = 'releases/latest/download/latest.json'
  $key = 'dW50cnVzdGVk'
  Check 'control: the configuration is visible in the stable program (public key)' (File-Has $stableExe $key)
  Check 'control: the stable program has the update address' (File-Has $stableExe $address)
  Check 'the configuration is visible in the beta program (public key)' (File-Has $betaExe $key)
  Check 'the beta program has no update address' (-not (File-Has $betaExe $address))
  Check 'the beta program has its identifier' (File-Has $betaExe $BetaId)
  $betaViolet = Save-Icon $betaExe (Join-Path $Out 'icon-beta.png')
  $stableViolet = Save-Icon $stableExe (Join-Path $Out 'icon-stable.png')
  Check 'the icon of the beta program is not the icon of the stable program' ((Get-FileHash (Join-Path $Out 'icon-beta.png')).Hash -ne (Get-FileHash (Join-Path $Out 'icon-stable.png')).Hash)
  Check 'the icon of the beta program has the violet ribbon' ($betaViolet -gt $stableViolet -and $betaViolet -ge 8) "(violet pixels: beta $betaViolet, stable $stableViolet)"

  Say ''
  Say '=== 4. Both run at the same time, with data folders of their own'
  Check 'the stable app made its data folder' (Test-Path -LiteralPath (Local-Data $StableId))
  Set-Content -LiteralPath (Join-Path (Local-Data $StableId) 'marker-of-the-stable-app.txt') -Value 'stable' -Encoding ascii
  Check 'the beta still has no data folder before it has run' (-not (Test-Path -LiteralPath (Local-Data $BetaId)))
  # The stable app is running (its single-instance lock is held): the beta must start next to it.
  Smoke $betaExe 'the beta (next to the running stable app)'
  Check 'the stable app was not disturbed by the beta' (-not $stableApp.HasExited)
  Check 'the beta made its own data folder' (Test-Path -LiteralPath (Local-Data $BetaId))
  Check 'the folders are not the same' ((Local-Data $StableId) -ne (Local-Data $BetaId))
  Check 'the stable marker is not in the data of the beta' (-not (Test-Path -LiteralPath (Join-Path (Local-Data $BetaId) 'marker-of-the-stable-app.txt')))
  Check 'the stable marker is not in the roaming data of the beta' (-not (Test-Path -LiteralPath (Join-Path (Roaming-Data $BetaId) 'marker-of-the-stable-app.txt')))
  List-Data $StableId
  List-Data $BetaId
  Stop-Tree $stableApp.Id
  Start-Sleep -Seconds 3
  Smoke $stableExe 'the stable app (next to the installed beta)'
  Sign-With $betaDir 'beta' 'beta'
  Sign-With $stableDir 'stable' 'stable'

  Say ''
  Say '=== 5. Remove the beta while the stable app runs'
  $stableApp = Start-Open $stableExe
  Wait-Window $stableApp $StableId
  Check 'the stable app is running' (-not $stableApp.HasExited)
  Uninstall-App $BetaName
  Check 'the beta has no uninstall entry' ($null -eq (Get-Entry $BetaName))
  Check 'the beta program is gone' (-not (Test-Path -LiteralPath $betaExe))
  Check 'the beta Start menu entry is gone' (@(Find-Shortcut $BetaName).Count -eq 0)
  Check 'the stable app is still installed' ((Test-Path -LiteralPath $stableExe) -and $null -ne (Get-Entry $StableName) -and @(Find-Shortcut $StableName).Count -gt 0)
  Check 'the stable app kept running' (-not $stableApp.HasExited)
  Check 'the stable data is still there' (Test-Path -LiteralPath (Join-Path (Local-Data $StableId) 'marker-of-the-stable-app.txt'))
  Check-Package-Entries $stableDir 'stable (after the beta was removed)'
  Stop-Tree $stableApp.Id
  Start-Sleep -Seconds 3
  Smoke $stableExe 'the stable app (after the beta was removed)'

  Say ''
  Say '=== 6. Install the beta again and remove the stable app while the beta runs'
  Install-App $Beta $BetaName
  $betaDir = $script:InstalledIn
  $betaExe = Join-Path $betaDir $BetaProgram
  $betaApp = Start-Open $betaExe
  Wait-Window $betaApp $BetaId
  Check 'the beta is running' (-not $betaApp.HasExited)
  Uninstall-App $StableName
  Check 'the stable app has no uninstall entry' ($null -eq (Get-Entry $StableName))
  Check 'the stable program is gone' (-not (Test-Path -LiteralPath $stableExe))
  Check 'the stable Start menu entry is gone' (@(Find-Shortcut $StableName).Count -eq 0)
  Check 'the beta is still installed' ((Test-Path -LiteralPath $betaExe) -and $null -ne (Get-Entry $BetaName) -and @(Find-Shortcut $BetaName).Count -gt 0)
  Check 'the beta kept running' (-not $betaApp.HasExited)
  Check-Package-Entries $betaDir 'beta (after the stable app was removed)'
  Stop-Tree $betaApp.Id
  Start-Sleep -Seconds 3
  Smoke $betaExe 'the beta (after the stable app was removed)'
  Sign-With $betaDir 'beta (after the stable app was removed)' 'beta-alone'
  Uninstall-App $BetaName
  Check 'the beta is removed' ($null -eq (Get-Entry $BetaName) -and -not (Test-Path -LiteralPath $betaExe))
}
catch {
  Say "STOPPED: $($_ | Out-String)"
  [void]$failed.Add('the checks stopped early')
}
finally {
  foreach ($name in @('patient-partridge', 'patient-partridge-beta')) {
    Get-Process -Name $name -ErrorAction SilentlyContinue | ForEach-Object { Stop-Tree $_.Id }
  }
}

Say ''
if ($failed.Count -gt 0) {
  Say "FAILED: $($failed.Count) check(s): $($failed -join '; ')"
  exit 1
}
Say 'ALL CHECKS PASSED'
exit 0
