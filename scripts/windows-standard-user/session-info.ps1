# Describes the session, window station and desktop of the process that dot-sources this file, and
# whether its token is elevated. Used by the step that starts the checks (an administrator) and by
# the checks that run as the standard user, so the two can be compared in the log.

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class PPProbe {
  [DllImport("user32.dll", SetLastError=true)] static extern IntPtr GetProcessWindowStation();
  [DllImport("user32.dll", SetLastError=true)] static extern IntPtr GetThreadDesktop(uint threadId);
  [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
  [DllImport("user32.dll", SetLastError=true, CharSet=CharSet.Unicode, EntryPoint="GetUserObjectInformationW")]
  static extern bool GetName(IntPtr h, int index, StringBuilder buffer, int length, out int needed);
  [DllImport("user32.dll", SetLastError=true, EntryPoint="GetUserObjectInformationW")]
  static extern bool GetFlags(IntPtr h, int index, int[] buffer, int length, out int needed);
  [DllImport("advapi32.dll", SetLastError=true)]
  static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
  [DllImport("advapi32.dll", SetLastError=true)]
  static extern bool GetTokenInformation(IntPtr token, int cls, out int info, int length, out int needed);
  [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);

  static string NameOf(IntPtr h) {
    if (h == IntPtr.Zero) return "(none)";
    StringBuilder b = new StringBuilder(256);
    int needed;
    return GetName(h, 2, b, b.Capacity * 2, out needed) ? b.ToString() : "(error " + Marshal.GetLastWin32Error() + ")";
  }
  public static string Station() { return NameOf(GetProcessWindowStation()); }
  public static string Desktop() { return NameOf(GetThreadDesktop(GetCurrentThreadId())); }
  // WSF_VISIBLE: the window station is the one that is shown on a screen (an interactive one).
  public static string StationVisible() {
    IntPtr h = GetProcessWindowStation();
    int[] flags = new int[3];
    int needed;
    if (h == IntPtr.Zero || !GetFlags(h, 1, flags, 12, out needed)) return "unknown";
    return (flags[2] & 1) != 0 ? "yes" : "no";
  }
  static int TokenValue(int cls) {
    IntPtr token;
    if (!OpenProcessToken(GetCurrentProcess(), 8, out token)) return -1;
    int value, needed;
    bool ok = GetTokenInformation(token, cls, out value, 4, out needed);
    CloseHandle(token);
    return ok ? value : -1;
  }
  // TokenElevation: 1 when the token is elevated (full administrator rights).
  public static int Elevated() { return TokenValue(20); }
  // TokenElevationType: 1 default (no split token), 2 full, 3 limited.
  public static int ElevationType() { return TokenValue(18); }
}
'@

function Get-SessionInfo {
  $p = [System.Diagnostics.Process]::GetCurrentProcess()
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $lines = @(
    "user: $($identity.Name)",
    "process session id: $($p.SessionId)",
    "window station: $([PPProbe]::Station()) (visible: $([PPProbe]::StationVisible()))",
    "desktop: $([PPProbe]::Desktop())",
    "token elevated: $([PPProbe]::Elevated()) (elevation type $([PPProbe]::ElevationType()))",
    "SESSIONNAME: $($env:SESSIONNAME)"
  )
  return ($lines -join [Environment]::NewLine)
}
