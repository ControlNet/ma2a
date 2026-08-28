use std::{error::Error, fmt, io, path::PathBuf};

#[derive(Debug)]
pub(crate) enum XtaskError {
    Command {
        command: String,
        status: Option<i32>,
    },
    Io(io::Error),
    Manifest {
        path: PathBuf,
        message: String,
    },
    Policy {
        name: &'static str,
        violations: Vec<String>,
    },
    Usage {
        message: String,
    },
}

impl fmt::Display for XtaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command { command, status } => {
                write!(
                    formatter,
                    "command `{command}` failed with exit code {status:?}"
                )
            }
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Manifest { path, message } => {
                write!(formatter, "invalid manifest {}: {message}", path.display())
            }
            Self::Policy { name, violations } => {
                writeln!(formatter, "{name} violations:")?;
                for violation in violations {
                    writeln!(formatter, "- {violation}")?;
                }
                Ok(())
            }
            Self::Usage { message } => write!(formatter, "{message}"),
        }
    }
}

impl Error for XtaskError {}

impl From<io::Error> for XtaskError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
