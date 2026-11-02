# Creates the standard user and the folders the checks use. Run as the runner's administrator.
#
# The user is a local account in the Users group only. Its name has a letter outside ASCII, so the
# profile folder (C:\Users\<name>) has one too. The password is made here, masked in the log, and
# handed to the later steps through the job's environment. It exists for this job only.
$ErrorActionPreference = 'Stop'

$Root = 'C:\pp'
# "testbruk" + e with acute accent + "r". Written as code points: these files stay ASCII, because
# Windows PowerShell 5.1 reads a file without a byte order mark in the system's code page.
$name = 'testbruk' + [char]0x00E9 + 'r'

$alphabet = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789'.ToCharArray()
$bytes = New-Object byte[] 24
$generator = [System.Security.Cryptography.RandomNumberGenerator]::Create()
$generator.GetBytes($bytes)
# The suffix guarantees that the password meets the complexity rule of the system.
$password = (-join ($bytes | ForEach-Object { $alphabet[$_ % $alphabet.Length] })) + '-9aZ'
Write-Host "::add-mask::$password"

# Without a byte order mark: the runner reads this file as UTF-8 and Windows PowerShell 5.1 would
# write one with -Encoding utf8.
[System.IO.File]::AppendAllText($env:GITHUB_ENV, "PP_USER=$name`n", (New-Object System.Text.UTF8Encoding($false)))
[System.IO.File]::AppendAllText($env:GITHUB_ENV, "PP_PASSWORD=$password`n", (New-Object System.Text.UTF8Encoding($false)))

if (Get-LocalUser -Name $name -ErrorAction SilentlyContinue) { Remove-LocalUser -Name $name }
$secure = ConvertTo-SecureString $password -AsPlainText -Force
New-LocalUser -Name $name -Password $secure -PasswordNeverExpires -AccountNeverExpires `
  -FullName 'Standard user for the Partridge checks' | Out-Null
Add-LocalGroupMember -SID 'S-1-5-32-545' -Member $name
$sid = (Get-LocalUser -Name $name).SID.Value

Write-Host "Created the local user '$name' ($sid)."
Write-Host 'Members of Users:'
Get-LocalGroupMember -SID 'S-1-5-32-545' | Format-Table Name, PrincipalSource -AutoSize | Out-String | Write-Host
Write-Host 'Members of Administrators:'
Get-LocalGroupMember -SID 'S-1-5-32-544' | Format-Table Name, PrincipalSource -AutoSize | Out-String | Write-Host
$admins = @(Get-LocalGroupMember -SID 'S-1-5-32-544' | ForEach-Object { $_.SID.Value })
if ($admins -contains $sid) { throw 'The new user is an administrator.' }

# Folders: the user can read what the job puts here, and write only to 'out'.
foreach ($sub in 'in', 'docs', 'pdfium', 'scripts', 'build', 'out') {
  New-Item -ItemType Directory -Force -Path (Join-Path $Root $sub) | Out-Null
}
& icacls $Root /grant "*${sid}:(OI)(CI)(RX)" | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'icacls failed on the root folder.' }
# The engine runs in an AppContainer, which can open only what is open to "all application
# packages" or to its own package. A program in Program Files already is; the test programs built
# here are owned by the administrator, which the standard user cannot change, so they get the
# entry that Program Files has. (An app installed by the user itself, as in step 2 of the checks,
# is owned by the user, and the app adds the entry for its package itself.)
& icacls $Root /grant '*S-1-15-2-1:(OI)(CI)(RX)' | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'icacls failed to allow the application packages to read the root folder.' }
& icacls (Join-Path $Root 'out') /grant "*${sid}:(OI)(CI)(M)" | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'icacls failed on the output folder.' }

Copy-Item -Path (Join-Path $PSScriptRoot '*.ps1') -Destination (Join-Path $Root 'scripts') -Force
Write-Host "Folders ready under $Root."
