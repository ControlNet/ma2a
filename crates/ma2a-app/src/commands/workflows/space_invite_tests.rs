#[cfg(windows)]
use std::error::Error;
use std::path::Path;
#[cfg(windows)]
use std::{fs, process::Command};

use serde_json::json;

use super::{parse_duration_ms, retry_hint, should_show_retry_request_id, validate_request_id};

#[test]
fn the_default_invite_lifetime_is_five_minutes() {
    // Given / When / Then
    assert_eq!(
        parse_duration_ms(crate::cli::DEFAULT_INVITE_TTL).ok(),
        Some(300_000)
    );
}

#[test]
fn invite_lifetimes_stay_inside_the_protocol_bound() {
    // Given / When / Then
    assert_eq!(parse_duration_ms("30s").ok(), Some(30_000));
    assert_eq!(parse_duration_ms("1ms").ok(), Some(1));
    assert!(parse_duration_ms("6m").is_err());
    assert!(parse_duration_ms("0s").is_err());
    assert!(parse_duration_ms("5").is_err());
}

#[test]
fn explicit_accept_request_id_requires_32_lowercase_hex_chars() {
    let valid = "0123456789abcdef0123456789abcdef";
    assert_eq!(
        validate_request_id(valid.to_owned()).ok().as_deref(),
        Some(valid)
    );
    assert!(validate_request_id("0123".to_owned()).is_err());
    assert!(validate_request_id("0123456789ABCDEF0123456789ABCDEF".to_owned()).is_err());
}

#[test]
fn retry_hint_quotes_state_dir_and_falls_back_on_control_chars() {
    let id = "0123456789abcdef0123456789abcdef";
    #[cfg(not(windows))]
    assert_eq!(
        retry_hint(Path::new("/tmp/my state/o'ne"), id),
        format!(
            "Retry the exact same ticket with: ma2a --state-dir '/tmp/my state/o'\\''ne' space accept --request-id {id}"
        )
    );
    #[cfg(windows)]
    {
        for safe_dir in [r"C:\ma2a\my state", r"C:\ma2a\lab-01_a.b\my state"] {
            assert_eq!(
                retry_hint(Path::new(safe_dir), id),
                format!(
                    "Retry the exact same ticket with: ma2a --state-dir \"{safe_dir}\" space accept --request-id {id}"
                ),
                "expected quoted hint for {safe_dir}"
            );
        }
        for unsafe_dir in [
            r"C:\ma2a\state\",
            r"C:\",
            r"C:\ma2a\state $lab",
            r"C:\ma2a\state `lab",
            r#"C:\ma2a\state"lab"#,
            r"C:\ma2a\%TEMP%",
            "C:\\ma2a\\Lab \u{201C}East\u{201D}",
            "C:\\ma2a\\Lab \u{2018}East\u{2019}",
            r"C:\ma2a\Lab!TEMP!",
            r"C:\ma2a\Lab & East",
            r"C:\ma2a\Lab's",
            r"C:\ma2a\Lab^East",
            "C:\\ma2a\\caf\u{00e9}",
            "C:\\ma2a\\bad\tstate",
            "",
        ] {
            assert_eq!(
                retry_hint(Path::new(unsafe_dir), id),
                format!(
                    "Keep the original --state-dir and retry the exact same ticket with: ma2a space accept --request-id {id}"
                ),
                "expected fallback for {unsafe_dir}"
            );
        }
    }
    assert_eq!(
        retry_hint(Path::new("/tmp/bad\nstate"), id),
        format!(
            "Keep the original --state-dir and retry the exact same ticket with: ma2a space accept --request-id {id}"
        )
    );
}

#[test]
fn retry_request_id_is_shown_only_when_exact_retry_can_work() {
    assert!(should_show_retry_request_id(&json!({
        "error": "unavailable",
        "remediation": null,
    })));
    assert!(should_show_retry_request_id(&json!({
        "error": "internal",
        "remediation": "remote outcome may include side effects; inspect current state or retry the exact same request",
    })));
    assert!(!should_show_retry_request_id(&json!({
        "error": "unavailable",
        "remediation": "mutation outcome may include side effects; inspect current state; this request will not execute again",
    })));
    assert!(!should_show_retry_request_id(&json!({
        "error": "unavailable",
        "remediation": "mutation committed; completion failed; inspect current state; this request will not execute again",
    })));
    assert!(!should_show_retry_request_id(&json!({
        "error": "expired",
        "remediation": null,
    })));
}

#[cfg(windows)]
#[test]
fn windows_retry_hint_preserves_safe_state_dir_across_shells() -> Result<(), Box<dyn Error>> {
    const RECORDER_CS: &str = r#"
using System;
using System.IO;
using System.Text;
public static class Program {
    public static void Main(string[] args) {
        File.WriteAllText(
            Environment.GetEnvironmentVariable("MA2A_ARGV_LOG"),
            string.Join("\0", args),
            new UTF8Encoding(false)
        );
    }
}
"#;
    let id = "0123456789abcdef0123456789abcdef";
    let safe_dir = r"C:\ma2a\lab-01_a.b\my state";
    let hint = retry_hint(Path::new(safe_dir), id);
    let command = hint
        .strip_prefix("Retry the exact same ticket with: ")
        .ok_or("expected quoted retry hint")?;

    let base = std::env::temp_dir().join(format!("ma2a-retry-hint-{}", std::process::id()));
    let _stale = fs::remove_dir_all(&base);
    fs::create_dir_all(&base)?;

    let recorder_exe = base.join("ma2a.exe");
    let build_ps1 = base.join("build.ps1");
    let recorder_cs_path = base.join("recorder.cs");
    fs::write(&recorder_cs_path, RECORDER_CS)?;
    fs::write(
        &build_ps1,
        format!(
            "Add-Type -OutputAssembly '{}' -OutputType ConsoleApplication -Path '{}'\n",
            recorder_exe.display(),
            recorder_cs_path.display()
        ),
    )?;
    let build_status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&build_ps1)
        .status()?;
    assert!(
        build_status.success(),
        "building argv recorder failed: {build_status}"
    );

    let mut path_entries = vec![base.clone()];
    if let Some(existing) = std::env::var_os("PATH") {
        path_entries.extend(std::env::split_paths(&existing));
    }
    let path_env = std::env::join_paths(path_entries)?;

    let ps1 = base.join("run.ps1");
    let ps_log = base.join("ps.argv");
    fs::write(&ps1, format!("{command}\n"))?;
    let ps_status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&ps1)
        .env("PATH", &path_env)
        .env("MA2A_ARGV_LOG", &ps_log)
        .status()?;
    assert!(
        ps_status.success(),
        "PowerShell hint execution failed: {ps_status}"
    );
    let ps_args = fs::read_to_string(&ps_log)?;
    let expected = [
        "--state-dir",
        safe_dir,
        "space",
        "accept",
        "--request-id",
        id,
    ];
    assert_eq!(ps_args.split('\0').collect::<Vec<_>>(), expected);

    let cmd_file = base.join("run.cmd");
    let cmd_log = base.join("cmd.argv");
    fs::write(&cmd_file, format!("@echo off\r\n{command}\r\n"))?;
    let cmd_status = Command::new("cmd.exe")
        .args(["/D", "/V:ON", "/C"])
        .arg(&cmd_file)
        .env("PATH", &path_env)
        .env("MA2A_ARGV_LOG", &cmd_log)
        .status()?;
    assert!(
        cmd_status.success(),
        "cmd.exe hint execution failed: {cmd_status}"
    );
    let cmd_args = fs::read_to_string(&cmd_log)?;
    assert_eq!(cmd_args.split('\0').collect::<Vec<_>>(), expected);

    fs::remove_dir_all(&base)?;
    Ok(())
}
