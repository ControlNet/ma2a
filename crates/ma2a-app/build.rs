//! Frontend build and embedding step for the MA2A binary.

use std::{
    env,
    error::Error,
    fmt::{self, Write as _},
    fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

#[derive(Debug)]
enum BuildError {
    CommandFailed {
        command: &'static str,
        status: ExitStatus,
        stderr: String,
    },
    EmptyDist {
        path: PathBuf,
    },
    Format(fmt::Error),
    Io {
        context: &'static str,
        source: io::Error,
    },
    MissingBun,
}

impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommandFailed {
                command,
                status,
                stderr,
            } => write!(
                formatter,
                "frontend command `{command}` failed with {status}. Run `cd web && bun install --frozen-lockfile && bun run build`. stderr: {stderr}"
            ),
            Self::EmptyDist { path } => write!(
                formatter,
                "frontend build produced no files in {}. Run `cd web && bun run build` and inspect Vite output",
                path.display()
            ),
            Self::Format(source) => write!(formatter, "generating embedded asset module: {source}"),
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
            Self::MissingBun => write!(
                formatter,
                "Bun is required to build embedded frontend assets. Install Bun 1.3.5, then run `cd web && bun install --frozen-lockfile && bun run build`"
            ),
        }
    }
}

impl Error for BuildError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Format(source) => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::CommandFailed { .. } | Self::EmptyDist { .. } | Self::MissingBun => None,
        }
    }
}

fn main() {
    if let Err(error) = build() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn build() -> Result<(), BuildError> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let web = manifest.join("../../web");
    for input in [
        "biome.jsonc",
        "bun.lock",
        "index.html",
        "package.json",
        "src",
        "tsconfig.json",
        "vite.config.ts",
        "vitest.config.ts",
    ] {
        println!("cargo::rerun-if-changed={}", web.join(input).display());
    }
    run_bun(&web, &["--version"], "bun --version")?;
    run_bun(
        &web,
        &["install", "--frozen-lockfile"],
        "bun install --frozen-lockfile",
    )?;
    run_bun(&web, &["run", "build"], "bun run build")?;

    let dist = web.join("dist");
    let mut assets = Vec::new();
    collect_assets(&dist, &dist, &mut assets)?;
    if assets.is_empty() {
        return Err(BuildError::EmptyDist { path: dist });
    }
    assets.sort_by(|left, right| left.0.cmp(&right.0));
    let generated = render_assets(&assets)?;
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or_else(|| BuildError::Io {
        context: "Cargo did not provide OUT_DIR",
        source: io::Error::new(io::ErrorKind::NotFound, "OUT_DIR is unset"),
    })?)
    .join("embedded_web.rs");
    fs::write(output, generated).map_err(|source| BuildError::Io {
        context: "writing generated embedded asset module",
        source,
    })
}

fn run_bun(directory: &Path, arguments: &[&str], label: &'static str) -> Result<(), BuildError> {
    let output = Command::new("bun")
        .args(arguments)
        .current_dir(directory)
        .output()
        .map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                BuildError::MissingBun
            } else {
                BuildError::Io {
                    context: "starting Bun frontend command",
                    source,
                }
            }
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(BuildError::CommandFailed {
            command: label,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn collect_assets(
    root: &Path,
    directory: &Path,
    assets: &mut Vec<(String, PathBuf)>,
) -> Result<(), BuildError> {
    for entry in fs::read_dir(directory).map_err(|source| BuildError::Io {
        context: "reading frontend dist directory",
        source,
    })? {
        let entry = entry.map_err(|source| BuildError::Io {
            context: "reading frontend dist entry",
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_assets(root, &path, assets)?;
        } else {
            let relative = path.strip_prefix(root).map_err(|source| BuildError::Io {
                context: "computing embedded frontend asset path",
                source: io::Error::other(source),
            })?;
            assets.push((relative.to_string_lossy().replace('\\', "/"), path));
        }
    }
    Ok(())
}

fn render_assets(assets: &[(String, PathBuf)]) -> Result<String, BuildError> {
    let mut generated = String::from("pub(crate) static WEB_ASSETS: &[(&str, &[u8])] = &[\n");
    for (relative, absolute) in assets {
        let absolute = absolute.display().to_string();
        writeln!(
            generated,
            "    ({relative:?}, include_bytes!({absolute:?})),"
        )
        .map_err(BuildError::Format)?;
    }
    generated.push_str("];\n");
    Ok(generated)
}
