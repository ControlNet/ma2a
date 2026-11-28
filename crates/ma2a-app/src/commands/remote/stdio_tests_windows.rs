//! Windows regression tests for [`LocalStdout`] console Unicode and redirected byte identity.

#[path = "stdio_tests_windows_pipe.rs"]
mod pipe;

use std::{
    error::Error,
    process::{Command, Stdio},
};

use tokio::io::AsyncWriteExt as _;

use super::{LocalStdout, StdioGuard};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const CONSOLE_CHILD_ENV: &str = "MA2A_TEST_STDIO_WIN_CONSOLE_CHILD";
const REDIRECT_CHILD_ENV: &str = "MA2A_TEST_STDIO_WIN_REDIRECT_CHILD";
const CONSOLE_TEST_NAME: &str =
    "commands::remote::stdio::tests::console_stdout_preserves_utf8_under_legacy_code_page";
const REDIRECT_TEST_NAME: &str = "commands::remote::stdio::tests::redirected_stdout_preserves_raw_bytes_and_uses_unbuffered_path";

const CONSOLE_PROBE_SCRIPT: &str = r#"
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Ma2aConTest {
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool AllocConsole();
    [DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr CreateConsoleScreenBuffer(uint access, uint share, IntPtr sa, uint flags, IntPtr data);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool SetHandleInformation(IntPtr h, uint mask, uint flags);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr GetStdHandle(int which);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool SetStdHandle(int which, IntPtr h);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool SetConsoleOutputCP(uint cp);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern uint GetConsoleOutputCP();
    [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern bool ReadConsoleOutputCharacterW(IntPtr h, StringBuilder buf, uint len, uint coord, out uint read);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool CloseHandle(IntPtr h);
}
'@
[void][Ma2aConTest]::AllocConsole()
$rw = [uint32]3221225472
$h = [Ma2aConTest]::CreateConsoleScreenBuffer($rw, 3, [IntPtr]::Zero, 1, [IntPtr]::Zero)
if ($h -eq [IntPtr]::Zero -or $h -eq [IntPtr](-1)) { throw "CreateConsoleScreenBuffer failed" }
if (-not [Ma2aConTest]::SetHandleInformation($h, 1, 1)) { throw "SetHandleInformation failed" }
$origCp = [Ma2aConTest]::GetConsoleOutputCP()
$origOut = [Ma2aConTest]::GetStdHandle(-11)
try {
    if (-not [Ma2aConTest]::SetConsoleOutputCP(437)) { throw "SetConsoleOutputCP(437) failed" }
    if ([Ma2aConTest]::GetConsoleOutputCP() -ne 437) { throw "code page is not 437" }
    if (-not [Ma2aConTest]::SetStdHandle(-11, $h)) { throw "SetStdHandle failed" }
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $env:MA2A_TEST_EXE
    $psi.Arguments = "--exact $($env:MA2A_TEST_NAME) --nocapture"
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $false
    $psi.RedirectStandardError = $true
    $psi.EnvironmentVariables["MA2A_TEST_STDIO_WIN_CONSOLE_CHILD"] = "1"
    $p = [System.Diagnostics.Process]::Start($psi)
    $err = $p.StandardError.ReadToEnd()
    $p.WaitForExit()
    if ($p.ExitCode -ne 0) { throw "child exited $($p.ExitCode): $err" }
    $sb = New-Object System.Text.StringBuilder 512
    $read = [uint32]0
    if (-not [Ma2aConTest]::ReadConsoleOutputCharacterW($h, $sb, 512, 0, [ref]$read)) { throw "ReadConsoleOutputCharacterW failed" }
    $buf = $sb.ToString(0, [int]$read)
    $start = $buf.IndexOf("MA2A_CON_START:")
    $end = $buf.IndexOf(":MA2A_CON_END")
    if ($start -lt 0 -or $end -le $start) { throw "markers missing in console buffer: $buf" }
    $slice = $buf.Substring($start + 15, $end - ($start + 15))
    $codes = ($slice.ToCharArray() | ForEach-Object { "{0:X4}" -f [int]$_ }) -join ","
    [void][Ma2aConTest]::SetStdHandle(-11, $origOut)
    Write-Output "CODES=$codes"
} finally {
    [void][Ma2aConTest]::SetStdHandle(-11, $origOut)
    if ($origCp -ne 0) { [void][Ma2aConTest]::SetConsoleOutputCP($origCp) }
    [void][Ma2aConTest]::CloseHandle($h)
}
"#;

fn run_console_child() -> TestResult {
    let local = LocalStdout::open();
    assert!(
        matches!(local, LocalStdout::Buffered(_)),
        "expected console-aware stdout for Windows console handle, got {local:?}"
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let stdio = StdioGuard::default();
        let mut stdout = stdio.stdout();
        stdout
            .write_all("MA2A_CON_START:caf\u{00e9}:MA2A_CON_END\n".as_bytes())
            .await?;
        stdout.flush().await?;
        Ok(())
    })
}

fn run_redirect_child() -> TestResult {
    let local = LocalStdout::open();
    assert!(
        matches!(local, LocalStdout::Unbuffered(_)),
        "expected unbuffered stdout for redirected handle, got {local:?}"
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let stdio = StdioGuard::default();
        let mut stdout = stdio.stdout();
        stdout
            .write_all(b"MA2A_RAW_START:caf\xc3\xa9\xff\xfe:MA2A_RAW_END\n")
            .await?;
        stdout.flush().await?;
        Ok(())
    })
}

fn extract_marked<'a>(bytes: &'a [u8], start: &[u8], end: &[u8]) -> TestResult<&'a [u8]> {
    let from = bytes
        .windows(start.len())
        .position(|window| window == start)
        .ok_or("missing start marker")?
        + start.len();
    let rest = bytes.get(from..).ok_or("start past end")?;
    let len = rest
        .windows(end.len())
        .position(|window| window == end)
        .ok_or("missing end marker")?;
    rest.get(..len).ok_or_else(|| "slice out of range".into())
}

#[test]
fn console_stdout_preserves_utf8_under_legacy_code_page() -> TestResult {
    if std::env::var_os(CONSOLE_CHILD_ENV).is_some() {
        return run_console_child();
    }

    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            CONSOLE_PROBE_SCRIPT,
        ])
        .env("MA2A_TEST_EXE", std::env::current_exe()?)
        .env("MA2A_TEST_NAME", CONSOLE_TEST_NAME)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "console probe failed ({:?}): stdout={stdout:?} stderr={stderr:?}",
        output.status
    );
    let codes = stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("CODES="))
        .ok_or_else(|| format!("missing CODES= line in {stdout:?}"))?;
    assert_eq!(
        codes, "0063,0061,0066,00E9",
        "expected UTF-16 code units for \"café\" on CP437 console, got {codes:?}"
    );
    Ok(())
}

#[test]
fn redirected_stdout_preserves_raw_bytes_and_uses_unbuffered_path() -> TestResult {
    if std::env::var_os(REDIRECT_CHILD_ENV).is_some() {
        return run_redirect_child();
    }

    let expected = b"caf\xc3\xa9\xff\xfe";
    // 1. Redirected pipe.
    let output = Command::new(std::env::current_exe()?)
        .args(["--exact", REDIRECT_TEST_NAME, "--nocapture"])
        .env(REDIRECT_CHILD_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    assert!(
        output.status.success(),
        "pipe child failed: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        extract_marked(&output.stdout, b"MA2A_RAW_START:", b":MA2A_RAW_END")?,
        expected
    );

    // 2. Redirected file.
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let path = std::env::temp_dir().join(format!(
        "ma2a-win-stdout-redirect-{:016x}.bin",
        u64::from_le_bytes(nonce)
    ));
    let file = std::fs::File::create(&path)?;
    let status = Command::new(std::env::current_exe()?)
        .args(["--exact", REDIRECT_TEST_NAME, "--nocapture"])
        .env(REDIRECT_CHILD_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::piped())
        .status()?;
    let content = std::fs::read(&path);
    let _ = std::fs::remove_file(&path);
    assert!(status.success(), "file child failed: {status:?}");
    assert_eq!(
        extract_marked(&content?, b"MA2A_RAW_START:", b":MA2A_RAW_END")?,
        expected
    );
    Ok(())
}
