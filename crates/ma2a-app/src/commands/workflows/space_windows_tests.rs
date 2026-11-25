use std::{error::Error, fs, process::Command};

use super::read_owner_only_invitation;

/// Gives SYSTEM's rule on the file in `MA2A_ACL_PATH` the inherit-only flag through the Win32 API:
/// .NET drops such a rule from a file's ACL, so `Set-Acl` cannot write it.
const INHERIT_ONLY_SYSTEM_RULE: &str = r#"
Add-Type -Namespace W -Name Sd -MemberDefinition '
[DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern bool ConvertStringSecurityDescriptorToSecurityDescriptorW(string s, int r, out IntPtr d, IntPtr l);
[DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern bool SetFileSecurityW(string f, int i, IntPtr d);'
$user = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$sd = [IntPtr]::Zero
if (-not [W.Sd]::ConvertStringSecurityDescriptorToSecurityDescriptorW("D:P(A;;FA;;;$user)(A;IO;FA;;;SY)", 1, [ref]$sd, [IntPtr]::Zero)) { exit 2 }
if (-not [W.Sd]::SetFileSecurityW($env:MA2A_ACL_PATH, 0x80000004, $sd)) { exit 3 }
"#;

#[test]
fn a_rule_for_children_alone_is_not_full_control_over_the_invite_file() -> Result<(), Box<dyn Error>>
{
    // Given
    let base = std::env::temp_dir().join(format!("ma2a-invite-acl-{}", std::process::id()));
    let _stale = fs::remove_dir_all(&base);
    fs::create_dir(&base)?;
    let (invitation, script) = (base.join("invite.ticket"), base.join("acl.ps1"));
    fs::write(&invitation, "ma2ainvite-test")?;
    fs::write(&script, INHERIT_ONLY_SYSTEM_RULE)?;
    ma2a_store::protect_private_file(&invitation, "test")?;
    assert!(read_owner_only_invitation(&invitation).is_ok());

    // When
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .env("MA2A_ACL_PATH", &invitation)
        .status()?;

    // Then
    assert!(status.success(), "setting the ACL failed: {status}");
    assert!(read_owner_only_invitation(&invitation).is_err());
    fs::remove_dir_all(&base)?;
    Ok(())
}
