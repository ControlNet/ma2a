# Runs a command with SeBackupPrivilege and SeRestorePrivilege disabled, as in a PowerShell CI
# step, and exits with the command's exit code. Git Bash enables both for itself and everything it
# starts, and with them Windows skips file access checks. The interactive workflow starts upterm
# from a Git Bash step, so every shell in the session has them, PowerShell included. A token that
# does not hold a privilege is left as it is.
param([string]$Command)
if (-not $Command) {
  [Console]::Error.WriteLine("drop-privileges: -Command is required")
  exit 2
}
try {
  if (-not ('Priv' -as [type])) {
    Add-Type @"
using System; using System.ComponentModel; using System.Runtime.InteropServices;
public static class Priv {
  [StructLayout(LayoutKind.Sequential, Pack=4)] public struct LUID { public uint Lo; public int Hi; }
  [StructLayout(LayoutKind.Sequential, Pack=4)] public struct TP { public int Count; public LUID Luid; public int Attr; }
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool OpenProcessToken(IntPtr h, int acc, out IntPtr tok);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool LookupPrivilegeValue(string s, string n, out LUID l);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool AdjustTokenPrivileges(IntPtr t, bool all, ref TP n, int len, IntPtr p, IntPtr r);
  [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
  public static void Drop(string name) {
    IntPtr tok;
    if (!OpenProcessToken(GetCurrentProcess(), 0x28, out tok)) throw new Win32Exception();
    try {
      TP tp = new TP(); tp.Count = 1; tp.Attr = 0;
      if (!LookupPrivilegeValue(null, name, out tp.Luid)) throw new Win32Exception();
      if (!AdjustTokenPrivileges(tok, false, ref tp, 0, IntPtr.Zero, IntPtr.Zero)) throw new Win32Exception();
    } finally { CloseHandle(tok); }
  }
}
"@
  }
  [Priv]::Drop("SeBackupPrivilege"); [Priv]::Drop("SeRestorePrivilege")
} catch {
  [Console]::Error.WriteLine("drop-privileges: $_")
  exit 1
}
# As in a CI step with `shell: pwsh`, a PowerShell error, such as a command that does not exist,
# a failed cmdlet or a throw, stops the command and exits with 1; only a native program sets
# $LASTEXITCODE.
$ErrorActionPreference = 'Stop'
$global:LASTEXITCODE = 0
try {
  Invoke-Expression $Command
} catch {
  [Console]::Error.WriteLine("drop-privileges: $_")
  exit 1
}
exit $LASTEXITCODE
