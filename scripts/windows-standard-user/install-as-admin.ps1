# Installs the app the way an administrator or a deployment tool does, in folders outside Program
# Files and outside any profile, so that the files belong to the administrators. Run as the runner's
# administrator, after prepare.ps1 and before the checks of the standard user (run-as-user.ps1),
# which open documents with these copies.
#
#   C:\Apps\pp-msi    the MSI, with INSTALLDIR set (what the install page or a tool does)
#   C:\Apps\pp-nsis   the NSIS installer, run by the administrator with the folder given
#   C:\Apps\pp-bare   a copy of the MSI folder without the entry for the engine's package (control)
#
# Before those, each installer is run once into a folder of its own and removed again, to show what
# the installers leave: an entry for the engine's package on the folder, the engine program and the
# PDFium library, without inheritance and on nothing else, and no entry on the folder after the
# uninstall. A file of the user in the folder, made after the install, never gets an entry.
#
# The NSIS installer of this app is a per-user one (it has no per-machine mode); run by an
# administrator into a folder outside the profile it leaves the same kind of files as the MSI.
$ErrorActionPreference = 'Stop'
$log = 'C:\pp\out\admin-install.log'
function Log([string]$text) {
  Write-Host $text
  Add-Content -LiteralPath $log -Value $text -Encoding UTF8
}

$msi = Get-ChildItem 'C:\pp\in' -Filter '*.msi' | Select-Object -First 1
$setup = Get-ChildItem 'C:\pp\in' -Filter '*-setup.exe' | Select-Object -First 1
if (-not $msi -or -not $setup) { throw 'The installers are missing in C:\pp\in.' }
New-Item -ItemType Directory -Force -Path 'C:\Apps' | Out-Null

# The package of the engine (see container.rs); icacls names "all application packages" in words.
$package = 'S-1-15-2-1452645665-1071814252-341713739-2096083442-1079832312-2911523698-3580489556'
function Get-Access([string]$path) {
  return ((& icacls $path) -join "`n")
}
function Test-PackageEntry([string]$path) {
  return (Get-Access $path) -match [regex]::Escape($package)
}
function Test-AllPackagesEntry([string]$path) {
  return (Get-Access $path) -match 'ALL APPLICATION PACKAGES|S-1-15-2-1:'
}
# What the entry for the package on a path allows and how it is inherited, for example '(RX)'.
function Get-PackageRights([string]$path) {
  $line = (& icacls $path) | Where-Object { $_ -match [regex]::Escape($package) } | Select-Object -First 1
  if (-not $line) { return $null }
  return ($line -replace ('^.*' + [regex]::Escape($package) + ':'), '').Trim()
}
# What an install leaves in a folder, and nowhere else; or, with $installed false, what is left
# after the uninstall (nothing).
function Assert-PackageAccess([string]$folder, [bool]$installed) {
  $engine = Join-Path $folder 'partridge-engine.exe'
  $library = Join-Path $folder 'pdfium.dll'
  if ($installed) {
    foreach ($path in @($folder, $engine, $library)) {
      $rights = Get-PackageRights $path
      Log "entry for the package on ${path}: $rights"
      if ($rights -ne '(RX)') { throw "$path should have the entry for the package with (RX) only, without inheritance: '$rights'." }
      if (Test-AllPackagesEntry $path) { throw "$path has an entry for all application packages." }
    }
    # A file of the user beside the install must stay unreadable to an AppContainer: no entry for
    # the package and none for all application packages, on a file, a folder and a file in it.
    $mine = Join-Path $folder 'user-file-beside-the-install.txt'
    Set-Content -LiteralPath $mine -Value 'private' -Encoding ASCII
    $subfolder = Join-Path $folder 'user-folder'
    New-Item -ItemType Directory -Force -Path $subfolder | Out-Null
    $inside = Join-Path $subfolder 'inside.txt'
    Set-Content -LiteralPath $inside -Value 'private' -Encoding ASCII
    foreach ($path in @($mine, $subfolder, $inside)) {
      if (Test-PackageEntry $path) { throw "$path has an entry for the package of the engine." }
      if (Test-AllPackagesEntry $path) { throw "$path has an entry for all application packages." }
    }
    Log "The files of the user in $folder have no entry for any application package."
  } else {
    if (Test-PackageEntry $folder) { throw "$folder still has the entry for the package after the uninstall." }
    foreach ($item in Get-ChildItem -LiteralPath $folder -Recurse -Force -ErrorAction SilentlyContinue) {
      if (Test-PackageEntry $item.FullName) { throw "$($item.FullName) still has the entry for the package after the uninstall." }
    }
    Log "No entry for the package is left in $folder."
  }
}

function Show-Access([string]$folder) {
  Log "--- access list of $folder ---"
  Log ((& icacls $folder) -join "`n")
  Log "owner: $((Get-Acl -LiteralPath $folder).Owner)"
}

# ---- What the installers leave, and what the uninstall takes away ----------------------------------
# The MSI, into a folder of its own. The folder stays after the uninstall because of the user file.
$install = Start-Process -FilePath msiexec.exe -Wait -PassThru -ArgumentList @(
  '/i', "`"$($msi.FullName)`"", '/qn', '/norestart', 'INSTALLDIR=C:\Apps\pp-msi-gone\', '/l*v', '"C:\pp\out\msi-gone.log"')
Log "msiexec exit code (install, folder to be removed again): $($install.ExitCode)"
if ($install.ExitCode -ne 0 -and $install.ExitCode -ne 3010) { throw "The MSI did not install (exit code $($install.ExitCode))." }
Show-Access 'C:\Apps\pp-msi-gone'
Assert-PackageAccess 'C:\Apps\pp-msi-gone' $true
$remove = Start-Process -FilePath msiexec.exe -Wait -PassThru -ArgumentList @(
  '/x', "`"$($msi.FullName)`"", '/qn', '/norestart', '/l*v', '"C:\pp\out\msi-gone-remove.log"')
Log "msiexec exit code (uninstall): $($remove.ExitCode)"
if (-not (Test-Path 'C:\Apps\pp-msi-gone\user-file-beside-the-install.txt')) { throw 'The uninstall removed the file of the user.' }
Show-Access 'C:\Apps\pp-msi-gone'
Assert-PackageAccess 'C:\Apps\pp-msi-gone' $false

# The NSIS installer the same way. The uninstaller runs from the folder and waits (_?=).
$install = Start-Process -FilePath $setup.FullName -Wait -PassThru -ArgumentList @('/S', '/D=C:\Apps\pp-nsis-gone')
Log "NSIS installer exit code (folder to be removed again): $($install.ExitCode)"
$deadline = (Get-Date).AddSeconds(60)
while ((Get-Date) -lt $deadline -and -not (Test-Path 'C:\Apps\pp-nsis-gone\pdfium.dll')) { Start-Sleep -Seconds 2 }
if (-not (Test-Path 'C:\Apps\pp-nsis-gone\pdfium.dll')) { throw 'The NSIS installer did not install into C:\Apps\pp-nsis-gone.' }
Show-Access 'C:\Apps\pp-nsis-gone'
Assert-PackageAccess 'C:\Apps\pp-nsis-gone' $true
$uninstaller = Get-ChildItem 'C:\Apps\pp-nsis-gone' -Filter 'uninstall*.exe' | Select-Object -First 1
if (-not $uninstaller) { throw 'The NSIS installer left no uninstaller.' }
$remove = Start-Process -FilePath $uninstaller.FullName -Wait -PassThru -ArgumentList @('/S', '_?=C:\Apps\pp-nsis-gone')
Log "NSIS uninstall exit code: $($remove.ExitCode)"
if (-not (Test-Path 'C:\Apps\pp-nsis-gone\user-file-beside-the-install.txt')) { throw 'The uninstall removed the file of the user.' }
Show-Access 'C:\Apps\pp-nsis-gone'
Assert-PackageAccess 'C:\Apps\pp-nsis-gone' $false

# ---- MSI -----------------------------------------------------------------------------------------
$msiLog = 'C:\pp\out\msi-install.log'
$install = Start-Process -FilePath msiexec.exe -Wait -PassThru -ArgumentList @(
  '/i', "`"$($msi.FullName)`"", '/qn', '/norestart', 'INSTALLDIR=C:\Apps\pp-msi\', '/l*v', "`"$msiLog`"")
Log "msiexec exit code: $($install.ExitCode)"
if ($install.ExitCode -ne 0 -and $install.ExitCode -ne 3010) {
  Get-Content -LiteralPath $msiLog -Tail 60 | ForEach-Object { Log $_ }
  throw "The MSI did not install (exit code $($install.ExitCode))."
}
Log ((Select-String -LiteralPath $msiLog -Pattern 'PackageAccess|icacls' | ForEach-Object { $_.Line }) -join "`n")
if (-not (Test-Path 'C:\Apps\pp-msi\patient-partridge.exe')) {
  Log ((Get-ChildItem 'C:\Apps' -Recurse -Depth 2 | Select-Object -ExpandProperty FullName) -join "`n")
  throw 'The MSI did not install into C:\Apps\pp-msi.'
}
Show-Access 'C:\Apps\pp-msi'

# ---- NSIS ----------------------------------------------------------------------------------------
# The folder goes last and without quotes, as the installer wants it.
$install = Start-Process -FilePath $setup.FullName -Wait -PassThru -ArgumentList @('/S', '/D=C:\Apps\pp-nsis')
Log "NSIS installer exit code: $($install.ExitCode)"
$deadline = (Get-Date).AddSeconds(60)
while ((Get-Date) -lt $deadline -and -not (Test-Path 'C:\Apps\pp-nsis\patient-partridge.exe')) { Start-Sleep -Seconds 2 }
if (-not (Test-Path 'C:\Apps\pp-nsis\patient-partridge.exe')) { throw 'The NSIS installer did not install into C:\Apps\pp-nsis.' }
Show-Access 'C:\Apps\pp-nsis'

# ---- the control: the same files without the entry ------------------------------------------------
Copy-Item -LiteralPath 'C:\Apps\pp-msi' -Destination 'C:\Apps\pp-bare' -Recurse
Show-Access 'C:\Apps\pp-bare'

foreach ($folder in 'C:\Apps\pp-msi', 'C:\Apps\pp-nsis') {
  foreach ($file in $folder, (Join-Path $folder 'partridge-engine.exe'), (Join-Path $folder 'pdfium.dll')) {
    if (-not (Test-PackageEntry $file)) { throw "$file has no entry for the package of the engine." }
    if (Test-AllPackagesEntry $file) { throw "$file has an entry for all application packages." }
  }
}
# The control is a copy, which takes the access list of its new folder: it has the files and no entry.
foreach ($file in 'C:\Apps\pp-bare', 'C:\Apps\pp-bare\partridge-engine.exe', 'C:\Apps\pp-bare\pdfium.dll') {
  if (Test-PackageEntry $file) { throw "The control has the entry on $file; it would prove nothing." }
}
# The files of the standard user's profile are not involved: it only has to read and run these.
& icacls 'C:\Apps' | Out-Null
Log 'Installed outside the profile.'
