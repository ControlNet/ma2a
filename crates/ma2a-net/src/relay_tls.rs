use std::{
    error::Error,
    fmt,
    fs::{self, File},
    io::Read as _,
    path::PathBuf,
    sync::Arc,
};

use rustls::ServerConfig;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject as _};
use x509_parser::parse_x509_certificate;
use zeroize::Zeroizing;

const MAX_CERTIFICATE_FILE_BYTES: u64 = 1_048_576;
const MAX_PRIVATE_KEY_FILE_BYTES: u64 = 262_144;

/// Operator-supplied native TLS certificate and private-key paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeRelayTlsConfig {
    certificate_path: PathBuf,
    private_key_path: PathBuf,
}

impl NativeRelayTlsConfig {
    /// Creates path-only TLS configuration without loading private material.
    pub const fn new(certificate_path: PathBuf, private_key_path: PathBuf) -> Self {
        Self {
            certificate_path,
            private_key_path,
        }
    }

    /// Returns the certificate chain path.
    pub fn certificate_path(&self) -> &std::path::Path {
        &self.certificate_path
    }

    /// Returns the TLS private-key path.
    pub fn private_key_path(&self) -> &std::path::Path {
        &self.private_key_path
    }

    /// Loads bounded current TLS material and verifies key correspondence.
    ///
    /// # Errors
    /// Returns [`RelayTlsError`] for unreadable, expired, malformed, mismatched, or insecure files.
    pub fn load_server_config(&self) -> Result<ServerConfig, RelayTlsError> {
        let certificate_bytes = read_bounded(&self.certificate_path, TlsFileKind::Certificate)?;
        let private_key_bytes = Zeroizing::new(read_bounded(
            &self.private_key_path,
            TlsFileKind::PrivateKey,
        )?);
        let certificates = CertificateDer::pem_slice_iter(&certificate_bytes)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| RelayTlsError::InvalidCertificate)?;
        if certificates.is_empty() {
            return Err(RelayTlsError::InvalidCertificate);
        }
        for certificate in &certificates {
            let (remainder, parsed) = parse_x509_certificate(certificate.as_ref())
                .map_err(|_| RelayTlsError::InvalidCertificate)?;
            if !remainder.is_empty() {
                return Err(RelayTlsError::InvalidCertificate);
            }
            if !parsed.validity().is_valid() {
                return Err(RelayTlsError::CertificateNotCurrent);
            }
        }
        let private_key = PrivateKeyDer::from_pem_slice(&private_key_bytes)
            .map_err(|_| RelayTlsError::InvalidPrivateKey)?;
        let builder =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|_| RelayTlsError::TlsConfiguration)?
                .with_no_client_auth();
        builder
            .with_single_cert(certificates, private_key)
            .map_err(|_| RelayTlsError::KeyMismatch)
    }
}

/// Fail-closed native relay TLS material error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RelayTlsError {
    /// Certificate file cannot be safely read within its bound.
    CertificateFile,
    /// Private-key file cannot be safely read within its bound.
    PrivateKeyFile,
    /// Private-key ownership or permission bits are unsafe.
    InsecurePrivateKeyPermissions,
    /// Certificate PEM or DER is malformed.
    InvalidCertificate,
    /// Certificate is not currently valid.
    CertificateNotCurrent,
    /// Private-key PEM is malformed or unsupported.
    InvalidPrivateKey,
    /// Certificate and private key do not correspond.
    KeyMismatch,
    /// Rustls protocol configuration failed.
    TlsConfiguration,
}

impl fmt::Display for RelayTlsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CertificateFile => {
                formatter.write_str("relay TLS certificate file is unavailable or oversized")
            }
            Self::PrivateKeyFile => {
                formatter.write_str("relay TLS private-key file is unavailable or oversized")
            }
            Self::InsecurePrivateKeyPermissions => formatter.write_str(
                "relay TLS private key must be a current-user-owned regular file with mode 0600",
            ),
            Self::InvalidCertificate => {
                formatter.write_str("relay TLS certificate chain is invalid")
            }
            Self::CertificateNotCurrent => {
                formatter.write_str("relay TLS certificate chain is not currently valid")
            }
            Self::InvalidPrivateKey => formatter.write_str("relay TLS private key is invalid"),
            Self::KeyMismatch => {
                formatter.write_str("relay TLS certificate and private key do not match")
            }
            Self::TlsConfiguration => {
                formatter.write_str("relay TLS protocol configuration is unavailable")
            }
        }
    }
}

impl Error for RelayTlsError {}

#[derive(Clone, Copy)]
enum TlsFileKind {
    Certificate,
    PrivateKey,
}

impl TlsFileKind {
    const fn maximum(self) -> u64 {
        match self {
            Self::Certificate => MAX_CERTIFICATE_FILE_BYTES,
            Self::PrivateKey => MAX_PRIVATE_KEY_FILE_BYTES,
        }
    }

    const fn error(self) -> RelayTlsError {
        match self {
            Self::Certificate => RelayTlsError::CertificateFile,
            Self::PrivateKey => RelayTlsError::PrivateKeyFile,
        }
    }
}

fn read_bounded(path: &std::path::Path, kind: TlsFileKind) -> Result<Vec<u8>, RelayTlsError> {
    let maximum = kind.maximum();
    let error = kind.error();
    let mut file = open_without_following_links(path, error)?;
    let metadata = file.metadata().map_err(|_| error)?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(error);
    }
    if matches!(kind, TlsFileKind::PrivateKey) {
        validate_private_key(path, &metadata)?;
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error)?;
    if u64::try_from(bytes.len()).map_or(true, |length| length > maximum) {
        return Err(error);
    }
    Ok(bytes)
}

#[cfg(unix)]
fn open_without_following_links(
    path: &std::path::Path,
    error: RelayTlsError,
) -> Result<File, RelayTlsError> {
    use rustix::fs::{Mode, OFlags};

    rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| error)
}

#[cfg(windows)]
fn open_without_following_links(
    path: &std::path::Path,
    error: RelayTlsError,
) -> Result<File, RelayTlsError> {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| error)?;
    if file.metadata().map_err(|_| error)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(error);
    }
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn open_without_following_links(
    path: &std::path::Path,
    error: RelayTlsError,
) -> Result<File, RelayTlsError> {
    File::open(path).map_err(|_| error)
}

#[cfg(unix)]
fn validate_private_key(
    _path: &std::path::Path,
    metadata: &fs::Metadata,
) -> Result<(), RelayTlsError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let mode = metadata.permissions().mode() & 0o777;
    let current_user = rustix::process::geteuid().as_raw();
    if !metadata.is_file() || metadata.uid() != current_user || mode != 0o600 {
        return Err(RelayTlsError::InsecurePrivateKeyPermissions);
    }
    Ok(())
}

#[cfg(windows)]
fn validate_private_key(
    path: &std::path::Path,
    metadata: &fs::Metadata,
) -> Result<(), RelayTlsError> {
    if !metadata.is_file() {
        return Err(RelayTlsError::InsecurePrivateKeyPermissions);
    }
    let script = "$u=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value;$s='S-1-5-18';$a=Get-Acl -LiteralPath $args[0];$o=$a.Owner;$os=(New-Object Security.Principal.NTAccount($o)).Translate([Security.Principal.SecurityIdentifier]).Value;if($os-ne $u){exit 2};if(!$a.AreAccessRulesProtected){exit 3};$r=@($a.Access|Where-Object {!$_.IsInherited});if($r.Count-ne 2){exit 4};foreach($x in $r){$sid=$x.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value;if(($sid-ne $u)-and($sid-ne $s)){exit 5};if($x.AccessControlType-ne 'Allow' -or (($x.FileSystemRights-band [Security.AccessControl.FileSystemRights]::FullControl)-eq 0)){exit 6}}";
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .arg(path)
        .status()
        .map_err(|_| RelayTlsError::InsecurePrivateKeyPermissions)?;
    if status.success() {
        Ok(())
    } else {
        Err(RelayTlsError::InsecurePrivateKeyPermissions)
    }
}

#[cfg(not(any(unix, windows)))]
fn validate_private_key(
    _path: &std::path::Path,
    metadata: &fs::Metadata,
) -> Result<(), RelayTlsError> {
    metadata
        .is_file()
        .then_some(())
        .ok_or(RelayTlsError::InsecurePrivateKeyPermissions)
}
