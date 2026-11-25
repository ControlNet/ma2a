# Runs a command with SeBackupPrivilege and SeRestorePrivilege disabled, as in a CI job step.
# An upterm session enables both, and with them Windows skips file access checks.
param([string]$Command)
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Priv {
  [StructLayout(LayoutKind.Sequential, Pack=4)] public struct LUID { public uint Lo; public int Hi; }
  [StructLayout(LayoutKind.Sequential, Pack=4)] public struct TP { public int Count; public LUID Luid; public int Attr; }
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool OpenProcessToken(IntPtr h, int acc, out IntPtr tok);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool LookupPrivilegeValue(string s, string n, out LUID l);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool AdjustTokenPrivileges(IntPtr t, bool all, ref TP n, int len, IntPtr p, IntPtr r);
  [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
  public static void Drop(string name) {
    IntPtr tok; OpenProcessToken(GetCurrentProcess(), 0x28, out tok);
    TP tp = new TP(); tp.Count = 1; tp.Attr = 0; LookupPrivilegeValue(null, name, out tp.Luid);
    if (!AdjustTokenPrivileges(tok, false, ref tp, 0, IntPtr.Zero, IntPtr.Zero)) throw new Exception("adjust");
  }
}
"@
[Priv]::Drop("SeBackupPrivilege"); [Priv]::Drop("SeRestorePrivilege")
Invoke-Expression $Command
