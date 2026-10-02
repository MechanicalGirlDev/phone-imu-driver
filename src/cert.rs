//! Self-signed TLS certificate generation and persistence.
//!
//! On first start, `phone-imu.crt` / `phone-imu.key` (PEM) are generated in `cert_dir` and
//! reused afterwards. The phone only has to accept the certificate warning once on first
//! access (the exception also applies to wss because the origin is the same). The SAN
//! contains localhost and the detected LAN IP. If the PC's IP changes, delete `cert_dir`
//! and the certificate is regenerated on the next start.

use core::net::IpAddr;
use std::path::{Path, PathBuf};

/// Failure to prepare the certificate.
#[derive(Debug, thiserror::Error)]
pub enum CertError {
    /// The certificate directory could not be created.
    #[error("failed to create cert dir {dir}: {source}")]
    CreateDir {
        /// The directory that could not be created.
        dir: PathBuf,
        /// The underlying OS error.
        source: std::io::Error,
    },
    /// A certificate or key file could not be read or written.
    #[error("failed to read/write {path}: {source}")]
    Io {
        /// The certificate or key file that could not be read or written.
        path: PathBuf,
        /// The underlying OS error.
        source: std::io::Error,
    },
    #[error("certificate generation failed: {0}")]
    /// `rcgen` failed to produce a self-signed certificate.
    Generate(#[from] rcgen::Error),
}

/// A PEM-encoded certificate and private key pair.
#[derive(Debug, Clone)]
pub struct CertPair {
    /// The certificate chain, PEM encoded.
    pub cert_pem: String,
    /// The private key, PEM encoded.
    pub key_pem: String,
}

/// Loads the certificate from `cert_dir`, or generates a self-signed one and saves it.
pub fn load_or_generate(cert_dir: &Path) -> Result<CertPair, CertError> {
    let cert_path = cert_dir.join("phone-imu.crt");
    let key_path = cert_dir.join("phone-imu.key");

    if cert_path.is_file() && key_path.is_file() {
        let cert_pem = std::fs::read_to_string(&cert_path).map_err(|source| CertError::Io {
            path: cert_path.clone(),
            source,
        })?;
        let key_pem = std::fs::read_to_string(&key_path).map_err(|source| CertError::Io {
            path: key_path.clone(),
            source,
        })?;
        tracing::info!("using existing TLS cert {}", cert_path.display());
        return Ok(CertPair { cert_pem, key_pem });
    }

    std::fs::create_dir_all(cert_dir).map_err(|source| CertError::CreateDir {
        dir: cert_dir.to_path_buf(),
        source,
    })?;

    // SAN: localhost + loopback + the detected LAN IP. Browsers still show a warning (the
    // fate of any self-signed certificate), but a matching SAN makes the "accept the
    // exception" flow straightforward.
    let mut sans: Vec<String> = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    if let Some(ip) = primary_local_ip() {
        sans.push(ip.to_string());
    }
    tracing::info!("generating self-signed TLS cert (SAN: {sans:?})");

    let cert = rcgen::generate_simple_self_signed(sans)?;
    let pair = CertPair {
        cert_pem: cert.cert.pem(),
        key_pem: cert.signing_key.serialize_pem(),
    };

    std::fs::write(&cert_path, &pair.cert_pem).map_err(|source| CertError::Io {
        path: cert_path.clone(),
        source,
    })?;
    std::fs::write(&key_path, &pair.key_pem).map_err(|source| CertError::Io {
        path: key_path.clone(),
        source,
    })?;
    tracing::info!("wrote {} / {}", cert_path.display(), key_path.display());
    Ok(pair)
}

/// Detects one local IP of the outbound route using the UDP connect trick (nothing is sent).
/// Returns None when that fails, for example on an offline machine (the SAN is then
/// localhost only).
pub fn primary_local_ip() -> Option<IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr().ok().map(|a| a.ip())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn generates_then_reuses_cert() {
        let dir = std::env::temp_dir().join(format!("phone-imu-cert-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let first = load_or_generate(&dir).unwrap();
        assert!(first.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(first.key_pem.contains("PRIVATE KEY"));

        // The second call reads the same content back instead of generating a new one.
        let second = load_or_generate(&dir).unwrap();
        assert_eq!(first.cert_pem, second.cert_pem);
        assert_eq!(first.key_pem, second.key_pem);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
