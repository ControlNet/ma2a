//! Installed MA2A process entry point.

use std::{
    error::Error,
    ffi::OsStr,
    fmt,
    io::{self, Write as _},
};

mod embedded_web {
    include!(concat!(env!("OUT_DIR"), "/embedded_web.rs"));
}

#[derive(Debug)]
struct AppError(io::Error);

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "failed to write ma2a output: {}", self.0)
    }
}

impl Error for AppError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

fn main() -> Result<(), AppError> {
    std::hint::black_box(embedded_web::WEB_ASSETS);
    if std::env::args_os()
        .skip(1)
        .any(|argument| argument == OsStr::new("--version"))
    {
        writeln!(io::stdout().lock(), "ma2a {}", env!("CARGO_PKG_VERSION")).map_err(AppError)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::embedded_web;

    #[test]
    fn embedded_frontend_is_present_when_binary_is_built() {
        assert!(!embedded_web::WEB_ASSETS.is_empty());
    }
}
