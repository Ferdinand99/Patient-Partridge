# Runs the checks as the standard user and waits for them. The user is started with
# Start-Process -Credential (the secondary logon service): the process gets a logon of that user
# and is put on the window station and desktop of this step's session. What the checks print is
# written to a file by the checks themselves; this script prints it afterwards.
param(
  [int]$TimeoutSeconds = 1500
)
$ErrorActionPreference = 'Stop'
$Root = 'C:\pp'
. (Join-Path $PSScriptRoot 'session-info.ps1')

Write-Host '--- the step that starts the checks (the runner account) ---'
Write-Host (Get-SessionInfo)
Write-Host "Secondary logon service: $((Get-Service seclogon).Status)"

$secure = ConvertTo-SecureString $env:PP_PASSWORD -AsPlainText -Force
$credential = New-Object System.Management.Automation.PSCredential("$($env:COMPUTERNAME)\$($env:PP_USER)", $secure)
$shell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
$arguments = @(
  '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
  '-File', (Join-Path $Root 'scripts\standard-user-checks.ps1'), '-Root', $Root
)
$log = Join-Path $Root 'out\child.log'
Remove-Item -LiteralPath $log -Force -ErrorAction SilentlyContinue

$process = Start-Process -FilePath $shell -ArgumentList $arguments -Credential $credential `
  -LoadUserProfile -WorkingDirectory $Root -PassThru
Write-Host "Started the checks as '$($env:PP_USER)' (process $($process.Id))."
$finished = $process.WaitForExit($TimeoutSeconds * 1000)
if (-not $finished) {
  Write-Host "The checks did not finish within $TimeoutSeconds seconds."
  Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
}

Write-Host '--- what the checks wrote ---'
if (Test-Path -LiteralPath $log) {
  Get-Content -LiteralPath $log -Encoding UTF8 | Write-Host
} else {
  Write-Host '(nothing: the checks never started or could not write)'
}
if (-not $finished) { exit 1 }
Write-Host "The checks ended with exit code $($process.ExitCode)."
exit $process.ExitCode
