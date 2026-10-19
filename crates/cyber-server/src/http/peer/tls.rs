//! The peer listener's TLS identity (`server-api` → Peer listener): an ECDSA P-256 key and
//! a self-signed certificate in `<state>/peer-tls/` (directory 0700, files 0600).
//!
//! Peers pin the SHA-256 of the SubjectPublicKeyInfo, not the certificate, so the
//! certificate is reissued from the kept key on every start (names and validity follow the
//! current configuration) and the fingerprint changes only with [`PeerIdentity::rotate`].
//! A key file that cannot be read is an error, never a reason to make a new key: a new key
//! would silently break every pairing.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};

use chrono::{Datelike, NaiveDate};
use rcgen::{
    CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ECDSA_P256_SHA256, PublicKeyData,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use sha2::{Digest, Sha256};

pub use rustls::ServerConfig;

/// The directory under the state directory that holds the identity.
pub const DIR: &str = "peer-tls";
const KEY_FILE: &str = "key.pem";
const CERT_FILE: &str = "cert.pem";
/// Certificates are valid for 10 years from yesterday (a day of slack for clock skew).
const VALID_DAYS: u64 = 3650;

/// What the certificate names: informational only, since clients pin the key and ignore
/// host names, so IP changes and DHCP renames do not break a pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectNames {
    /// The machine name shown to peers.
    pub machine: String,
    /// The listener's address; an unspecified IP (`0.0.0.0`, `::`) names `127.0.0.1`.
    pub address: SocketAddr,
}

impl SubjectNames {
    /// SANs: the machine, its `.local` name, `localhost` and the bound IP.
    fn alt_names(&self) -> Vec<String> {
        let ip = match self.address.ip() {
            ip if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            ip => ip,
        };
        let mut names = vec![
            self.machine.clone(),
            format!("{}.local", self.machine),
            "localhost".to_string(),
            ip.to_string(),
        ];
        names.dedup();
        names
    }
}

/// The key, the certificate issued for it, and the pinned fingerprint.
pub struct PeerIdentity {
    key_der: Vec<u8>,
    cert_der: Vec<u8>,
    fingerprint: String,
}

impl std::fmt::Debug for PeerIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerIdentity")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl PeerIdentity {
    /// Load the key in `dir`, or generate one on first use, and issue the certificate.
    pub fn load_or_create(dir: &Path, names: &SubjectNames) -> Result<Self, String> {
        prepare_dir(dir)?;
        let path = dir.join(KEY_FILE);
        let key = match std::fs::read_to_string(&path) {
            Ok(pem) => parse_key(&path, &pem)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => new_key(dir)?,
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        Self::issue(dir, &key, names)
    }

    /// Replace the key with a new one (`cyber peers invite --rotate-key`). Existing peers
    /// then fail with `PeerAuthError { reason: "fingerprint_mismatch" }` until re-added.
    pub fn rotate(dir: &Path, names: &SubjectNames) -> Result<Self, String> {
        prepare_dir(dir)?;
        let key = new_key(dir)?;
        Self::issue(dir, &key, names)
    }

    fn issue(dir: &Path, key: &KeyPair, names: &SubjectNames) -> Result<Self, String> {
        let cert = certificate_params(names)?
            .self_signed(key)
            .map_err(|error| format!("cannot issue the peer certificate: {error}"))?;
        write_private(&dir.join(CERT_FILE), &cert.pem())?;
        Ok(Self {
            key_der: key.serialize_der(),
            cert_der: cert.der().to_vec(),
            fingerprint: spki_fingerprint(&key.subject_public_key_info()),
        })
    }

    /// `sha256:<64 hex>` of the SubjectPublicKeyInfo.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// The DER certificate the listener presents.
    pub fn certificate_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// A TLS 1.3 server configuration presenting this identity, offering only HTTP/1.1.
    pub fn server_config(&self) -> Result<Arc<rustls::ServerConfig>, String> {
        CertSlot::new(self)?.server_config()
    }

    fn certified_key(&self) -> Result<Arc<CertifiedKey>, String> {
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_der.clone()));
        let chain = vec![CertificateDer::from(self.cert_der.clone())];
        CertifiedKey::from_der(chain, key, &provider())
            .map(Arc::new)
            .map_err(|error| format!("cannot load the peer key: {error}"))
    }
}

fn provider() -> rustls::crypto::CryptoProvider {
    rustls::crypto::aws_lc_rs::default_provider()
}

/// The certificate a running listener presents, replaced in place by
/// `cyber peers invite --rotate-key` so the next handshake shows the new key without a
/// restart.
#[derive(Debug)]
pub struct CertSlot {
    current: RwLock<Arc<CertifiedKey>>,
}

impl CertSlot {
    pub fn new(identity: &PeerIdentity) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            current: RwLock::new(identity.certified_key()?),
        }))
    }

    /// Present `identity` from the next handshake on.
    pub fn present(&self, identity: &PeerIdentity) -> Result<(), String> {
        let key = identity.certified_key()?;
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = key;
        Ok(())
    }

    /// A TLS 1.3 server configuration presenting this slot's certificate, offering only
    /// HTTP/1.1: the peer listener serves no HTTP/2 (see [`crate::http::serve_peer`]).
    pub fn server_config(self: &Arc<Self>) -> Result<Arc<rustls::ServerConfig>, String> {
        let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|error| format!("cannot configure peer TLS: {error}"))?
            .with_no_client_auth()
            .with_cert_resolver(Arc::clone(self) as Arc<dyn ResolvesServerCert>);
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    }
}

impl ResolvesServerCert for CertSlot {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(Arc::clone(
            &self.current.read().unwrap_or_else(PoisonError::into_inner),
        ))
    }
}

/// `sha256:` and the lowercase hex SHA-256 of a DER SubjectPublicKeyInfo.
pub fn spki_fingerprint(spki_der: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(spki_der))
}

/// The fingerprint of a DER certificate's SubjectPublicKeyInfo, as peers pin it.
pub fn certificate_fingerprint(cert_der: &[u8]) -> Result<String, String> {
    let der = CertificateDer::from(cert_der);
    let cert = webpki::EndEntityCert::try_from(&der)
        .map_err(|error| format!("not an X.509 certificate: {error}"))?;
    Ok(spki_fingerprint(&cert.subject_public_key_info()))
}

fn certificate_params(names: &SubjectNames) -> Result<CertificateParams, String> {
    let mut params = CertificateParams::new(names.alt_names())
        .map_err(|error| format!("cannot name the peer certificate: {error}"))?;
    let mut subject = DistinguishedName::new();
    subject.push(DnType::CommonName, format!("cyber peer {}", names.machine));
    params.distinguished_name = subject;
    let start = chrono::Utc::now().date_naive() - chrono::Days::new(1);
    let (year, month, day) = ymd(start);
    params.not_before = rcgen::date_time_ymd(year, month, day);
    let (year, month, day) = ymd(start + chrono::Days::new(VALID_DAYS));
    params.not_after = rcgen::date_time_ymd(year, month, day);
    Ok(params)
}

fn ymd(date: NaiveDate) -> (i32, u8, u8) {
    // Month and day always fit in a `u8`.
    (date.year(), date.month() as u8, date.day() as u8)
}

fn parse_key(path: &Path, pem: &str) -> Result<KeyPair, String> {
    let key = KeyPair::from_pem(pem)
        .map_err(|error| format!("cannot read the peer key {}: {error}", path.display()))?;
    if key.algorithm() != &PKCS_ECDSA_P256_SHA256 {
        return Err(format!(
            "the peer key {} is not ECDSA P-256; remove it and re-pair peers",
            path.display()
        ));
    }
    Ok(key)
}

fn new_key(dir: &Path) -> Result<KeyPair, String> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .map_err(|error| format!("cannot generate the peer key: {error}"))?;
    write_private(&dir.join(KEY_FILE), &key.serialize_pem())?;
    Ok(key)
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    cyber_core::trust::write_private_atomic(path, text.as_bytes())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// Create `dir` readable only by the owner.
fn prepare_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot restrict {}: {error}", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> SubjectNames {
        SubjectNames {
            machine: "desk".into(),
            address: "0.0.0.0:4748".parse().unwrap(),
        }
    }

    #[test]
    fn the_fingerprint_is_the_spki_hash_of_the_presented_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let identity = PeerIdentity::load_or_create(dir.path(), &names()).unwrap();
        let fingerprint = identity.fingerprint();
        assert!(
            fingerprint.starts_with("sha256:") && fingerprint.len() == 71,
            "{fingerprint}"
        );
        assert_eq!(
            certificate_fingerprint(identity.certificate_der()).unwrap(),
            fingerprint
        );
        identity.server_config().unwrap();
    }

    #[test]
    fn the_fingerprint_is_stable_across_certificate_reissue_with_the_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let first = PeerIdentity::load_or_create(dir.path(), &names()).unwrap();
        std::fs::remove_file(dir.path().join(CERT_FILE)).unwrap();
        let renamed = SubjectNames {
            machine: "workstation".into(),
            address: "192.168.1.20:4748".parse().unwrap(),
        };
        let second = PeerIdentity::load_or_create(dir.path(), &renamed).unwrap();
        assert_ne!(first.certificate_der(), second.certificate_der());
        assert_eq!(first.fingerprint(), second.fingerprint());
        assert!(dir.path().join(CERT_FILE).exists());
    }

    #[test]
    fn rotation_makes_a_new_key_that_later_loads_keep() {
        let dir = tempfile::tempdir().unwrap();
        let first = PeerIdentity::load_or_create(dir.path(), &names()).unwrap();
        let rotated = PeerIdentity::rotate(dir.path(), &names()).unwrap();
        assert_ne!(first.fingerprint(), rotated.fingerprint());
        let reloaded = PeerIdentity::load_or_create(dir.path(), &names()).unwrap();
        assert_eq!(reloaded.fingerprint(), rotated.fingerprint());
    }

    #[test]
    fn an_unreadable_key_fails_instead_of_replacing_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(KEY_FILE), "not a key").unwrap();
        let error = PeerIdentity::load_or_create(dir.path(), &names()).unwrap_err();
        assert!(error.contains("cannot read the peer key"), "{error}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(KEY_FILE)).unwrap(),
            "not a key"
        );
        let other = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
        std::fs::write(dir.path().join(KEY_FILE), other.serialize_pem()).unwrap();
        let error = PeerIdentity::load_or_create(dir.path(), &names()).unwrap_err();
        assert!(error.contains("not ECDSA P-256"), "{error}");
    }

    #[test]
    fn the_certificate_names_the_machine_and_the_bound_ip() {
        assert_eq!(
            names().alt_names(),
            ["desk", "desk.local", "localhost", "127.0.0.1"]
        );
        let bound = SubjectNames {
            machine: "desk".into(),
            address: "[fd00::5]:4748".parse().unwrap(),
        };
        assert_eq!(bound.alt_names()[3], "fd00::5");
    }

    #[cfg(unix)]
    #[test]
    fn identity_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let tls = dir.path().join(DIR);
        PeerIdentity::load_or_create(&tls, &names()).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&tls), 0o700);
        assert_eq!(mode(&tls.join(KEY_FILE)), 0o600);
        assert_eq!(mode(&tls.join(CERT_FILE)), 0o600);
    }
}
