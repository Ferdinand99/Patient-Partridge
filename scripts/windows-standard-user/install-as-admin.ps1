# Installs the app the way an administrator or a deployment tool does, in folders outside Program
# Files and outside any profile, so that the files belong to the administrators. Run as the runner's
# administrator, after prepare.ps1 and before the checks of the standard user (run-as-user.ps1),
# which open documents with these copies.
#
#   C:\Apps\pp-msi    the MSI, with INSTALLDIR set (what the install page or a tool does)
#   C:\Apps\pp-nsis   the NSIS installer, run by the administrator with the folder given
#   C:\Apps\pp-bare   a copy of the MSI folder without the entry for the engine's package (control)
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

function Show-Access([string]$folder) {
  Log "--- access list of $folder ---"
  Log ((& icacls $folder) -join "`n")
  Log "owner: $((Get-Acl -LiteralPath $folder).Owner)"
}

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
  $acl = (& icacls $folder) -join "`n"
  if ($acl -notmatch 'ALL APPLICATION PACKAGES|S-1-15-2-1') { throw "$folder has no entry for all application packages." }
}
if (((& icacls 'C:\Apps\pp-bare') -join "`n") -match 'ALL APPLICATION PACKAGES|S-1-15-2-1') {
  throw 'The control folder has the entry; it would prove nothing.'
}
# The files of the standard user's profile are not involved: it only has to read and run these.
& icacls 'C:\Apps' | Out-Null
Log 'Installed outside the profile.'
