use std::process::Command;

pub(crate) fn open(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");
    #[cfg(windows)]
    let mut command = Command::new("explorer.exe");
    command.arg(url).spawn().map(|_child| ())
}
