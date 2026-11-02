# Puts a Windows runner in one state of the firewall, to see what the engine does in it (the job
# "Windows, firewall state" in .github/workflows/ci.yml). The state is the first argument. Everything
# that is done and what the system answered goes to logs/state.txt. A state that the runner does not
# allow is said so in the log and is not an error: the job then proves nothing about that state.
param([Parameter(Mandatory)][string]$State)

$ErrorActionPreference = 'Continue'
New-Item -ItemType Directory -Force logs | Out-Null
$log = Join-Path $PWD 'logs/state.txt'
function Say($text) { $text | Tee-Object -FilePath $log -Append }

function Show-Firewall($title) {
  Say "--- $title"
  Say (Get-NetFirewallProfile | Format-Table Name, Enabled, DefaultOutboundAction | Out-String)
  Say (Get-Service MpsSvc, BFE | Format-Table Name, Status, StartType | Out-String)
}

# The package the app puts the engine in (the name is the one in container.rs).
function Get-PackageSid {
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class PartridgePackage {
  [DllImport("userenv.dll", CharSet = CharSet.Unicode)]
  static extern int DeriveAppContainerSidFromAppContainerName(string name, out IntPtr sid);
  [DllImport("advapi32.dll", CharSet = CharSet.Unicode)]
  static extern bool ConvertSidToStringSidW(IntPtr sid, out IntPtr text);
  public static string Sid(string name) {
    IntPtr sid; IntPtr text;
    if (DeriveAppContainerSidFromAppContainerName(name, out sid) != 0) throw new Exception("derive failed");
    if (!ConvertSidToStringSidW(sid, out text)) throw new Exception("convert failed");
    return Marshal.PtrToStringUni(text);
  }
}
'@
  return [PartridgePackage]::Sid('Partridge.Engine.Sandbox')
}

# Tries every way a hosted runner might offer to stop a service.
function Stop-ServiceEveryWay($name) {
  try { Stop-Service $name -Force -ErrorAction Stop; Say "Stop-Service $name: stopped" } catch { Say "Stop-Service ${name}: $_" }
  Say ("sc.exe stop ${name}: " + (sc.exe stop $name | Out-String))
  Say ("net stop ${name}: " + (cmd /c "net stop $name /y 2>&1" | Out-String))
  # As the system account, through a scheduled task.
  $out = Join-Path $PWD "logs/system-stop-$name.txt"
  $task = "pp-stop-$name"
  schtasks.exe /create /tn $task /tr "cmd /c sc stop $name > `"$out`" 2>&1" /sc once /st 23:59 /ru SYSTEM /f | Out-Null
  schtasks.exe /run /tn $task | Out-Null
  Start-Sleep -Seconds 8
  if (Test-Path $out) { Say ("as the system account, sc stop ${name}: " + (Get-Content $out -Raw)) } else { Say "as the system account: no answer" }
  schtasks.exe /delete /tn $task /f | Out-Null
}

Show-Firewall 'before'
Say (Get-CimInstance -Namespace root/SecurityCenter2 -ClassName FirewallProduct -ErrorAction SilentlyContinue | Format-List displayName, productState | Out-String)

switch ($State) {
  'default' { }
  'profiles-off' {
    Set-NetFirewallProfile -Profile Domain, Public, Private -Enabled False
  }
  'outbound-allowed' {
    # The default for outgoing traffic is "allow" in every profile, which is what some firewall
    # suites set.
    Set-NetFirewallProfile -Profile Domain, Public, Private -DefaultOutboundAction Allow
    New-NetFirewallRule -DisplayName 'allow all out' -Direction Outbound -Action Allow | Out-Null
  }
  'package-allowed' {
    # A rule that lets the package of the engine out: what an administrator or a firewall product
    # can do for one package.
    $sid = Get-PackageSid
    Say "package SID: $sid"
    New-NetFirewallRule -DisplayName 'allow the engine package' -Direction Outbound -Action Allow -Package $sid | Out-Null
    New-NetFirewallRule -DisplayName 'allow the engine package in' -Direction Inbound -Action Allow -Package $sid | Out-Null
  }
  'loopback-exempt' {
    # The loopback exemption that developers of AppContainer programs use.
    $sid = Get-PackageSid
    Say "package SID: $sid"
    Say (CheckNetIsolation.exe LoopbackExempt -a "-p=$sid" | Out-String)
    Say (CheckNetIsolation.exe LoopbackExempt -s | Out-String)
  }
  'firewall-service-stopped' { Stop-ServiceEveryWay 'MpsSvc' }
  'filtering-engine-stopped' { Stop-ServiceEveryWay 'BFE' }
  default { throw "unknown state $State" }
}

Show-Firewall 'after'
