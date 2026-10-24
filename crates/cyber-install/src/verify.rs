//! Release integrity (`installation-upgrade` → Self-update integrity): a strict
//! `SHA256SUMS` parser, the archive digest check, and the Sigstore bundle check over
//! `SHA256SUMS` against the trusted root embedded in the binary.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use cyber_core::version::ReleaseSource;
use sha2::{Digest, Sha256};
use sigstore_verify::trust_root::{SIGSTORE_PRODUCTION_TRUSTED_ROOT, TrustedRoot};
use sigstore_verify::types::Bundle;
use sigstore_verify::{VerificationPolicy, Verifier};

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("invalid SHA256SUMS line {line}: {reason}")]
    Sums { line: usize, reason: String },
    #[error("signature verification failed: {0}")]
    Signature(String),
    #[error("checksum verification failed: {0}")]
    Checksum(String),
    #[error("no release for {0}")]
    Missing(String),
    #[error("trusted root: {0}")]
    TrustedRoot(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The parsed `SHA256SUMS`: file name → SHA-256.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sums(BTreeMap<String, [u8; 32]>);

impl Sums {
    /// Lines of exactly `<64 lowercase hex>  <name>`, newline-terminated. A name holds no
    /// whitespace or path separator and appears once; anything else is refused.
    pub fn parse(text: &str) -> Result<Self, VerifyError> {
        let Some(body) = text.strip_suffix('\n') else {
            return Err(sums_error(0, "missing final newline"));
        };
        let mut sums = BTreeMap::new();
        for (index, line) in body.split('\n').enumerate() {
            let number = index + 1;
            let (digest, name) = parse_line(line).map_err(|reason| sums_error(number, reason))?;
            if sums.insert(name.to_string(), digest).is_some() {
                return Err(sums_error(number, &format!("duplicate entry for {name}")));
            }
        }
        Ok(Self(sums))
    }

    pub fn get(&self, name: &str) -> Option<&[u8; 32]> {
        self.0.get(name)
    }

    /// `path`'s SHA-256 must equal the line for `name`.
    pub fn check_file(&self, name: &str, path: &Path) -> Result<(), VerifyError> {
        let expected = self
            .get(name)
            .ok_or_else(|| VerifyError::Missing(name.to_string()))?;
        if sha256_file(path)? != *expected {
            return Err(VerifyError::Checksum(name.to_string()));
        }
        Ok(())
    }
}

fn sums_error(line: usize, reason: &str) -> VerifyError {
    VerifyError::Sums {
        line,
        reason: reason.to_string(),
    }
}

fn parse_line(line: &str) -> Result<([u8; 32], &str), &'static str> {
    let (hex, name) = line
        .split_once("  ")
        .ok_or("expected \"<sha256>  <name>\"")?;
    let digest = decode_hex(hex).ok_or("expected 64 lowercase hex digits")?;
    let bad_name = name.is_empty()
        || name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\'));
    if bad_name || name == "." || name == ".." {
        return Err("invalid file name");
    }
    Ok((digest, name))
}

fn decode_hex(hex: &str) -> Option<[u8; 32]> {
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut digest = [0u8; 32];
    let (pairs, _) = bytes.as_chunks::<2>();
    for (byte, [high, low]) in digest.iter_mut().zip(pairs) {
        *byte = (nibble(*high)? << 4) | nibble(*low)?;
    }
    Some(digest)
}

/// The SHA-256 of a file, read in chunks.
pub fn sha256_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hasher.finalize().into());
        }
        hasher.update(&buffer[..read]);
    }
}

/// The trusted root a release is checked against.
pub struct Trust {
    root: TrustedRoot,
    /// Debug trust hook only: a throwaway root has no transparency log or CT log, so
    /// their inclusion proof and SCT are not checked. Release builds have no such field.
    #[cfg(debug_assertions)]
    relaxed: bool,
}

impl Trust {
    /// The Sigstore public-good root embedded in this binary.
    pub fn production() -> Result<Self, VerifyError> {
        let root = TrustedRoot::from_json(SIGSTORE_PRODUCTION_TRUSTED_ROOT)
            .map_err(|e| VerifyError::TrustedRoot(e.to_string()))?;
        Ok(Self {
            root,
            #[cfg(debug_assertions)]
            relaxed: false,
        })
    }

    /// The production root, or the debug trust hook's root when the source names one.
    pub fn for_source(source: &ReleaseSource) -> Result<Self, VerifyError> {
        match &source.test_trusted_root {
            None => Self::production(),
            Some(path) => Self::test_root(path),
        }
    }

    #[cfg(debug_assertions)]
    fn test_root(path: &Path) -> Result<Self, VerifyError> {
        let root = TrustedRoot::from_file(path)
            .map_err(|e| VerifyError::TrustedRoot(format!("{}: {e}", path.display())))?;
        Ok(Self {
            root,
            relaxed: true,
        })
    }

    /// `ReleaseSource::resolve` never sets a test root in a release build; fail closed
    /// should one appear anyway.
    #[cfg(not(debug_assertions))]
    fn test_root(path: &Path) -> Result<Self, VerifyError> {
        Err(VerifyError::TrustedRoot(format!(
            "{}: a test trusted root is refused in a release build",
            path.display()
        )))
    }

    /// The certificate identity must equal `identity` and its OIDC issuer `issuer`; the
    /// chain, the SCT and the transparency-log inclusion are all required.
    fn policy(&self, identity: &str, issuer: &str) -> VerificationPolicy {
        let policy = VerificationPolicy::new(identity, issuer);
        #[cfg(debug_assertions)]
        if self.relaxed {
            return policy.skip_tlog_unsafe().skip_sct();
        }
        policy
    }

    /// Verify the Sigstore `bundle` (JSON) over `blob`.
    pub fn verify(
        &self,
        blob: &[u8],
        bundle: &[u8],
        identity: &str,
        issuer: &str,
    ) -> Result<(), VerifyError> {
        let signature = |e: &dyn std::fmt::Display| VerifyError::Signature(e.to_string());
        let text = std::str::from_utf8(bundle).map_err(|e| signature(&e))?;
        let bundle = Bundle::from_json(text).map_err(|e| signature(&e))?;
        let verifier = Verifier::new(&self.root).map_err(|e| signature(&e))?;
        verifier
            .verify(blob, &bundle, &self.policy(identity, issuer))
            .map(|_| ())
            .map_err(|e| signature(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOB: &[u8] = include_bytes!("../tests/fixtures/cosign-v3-blob.txt");
    const BUNDLE: &[u8] = include_bytes!("../tests/fixtures/cosign-v3-blob.sigstore.json");
    const IDENTITY: &str = "w.vollprecht@gmail.com";
    const ISSUER: &str = "https://github.com/login/oauth";

    fn hex(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    #[test]
    fn the_real_cosign_bundle_verifies_under_the_production_policy() {
        let trust = Trust::production().unwrap();
        trust.verify(BLOB, BUNDLE, IDENTITY, ISSUER).unwrap();
    }

    #[test]
    fn a_changed_blob_identity_or_issuer_fails() {
        let trust = Trust::production().unwrap();
        let failed = |blob: &[u8], identity: &str, issuer: &str| {
            let err = trust.verify(blob, BUNDLE, identity, issuer).unwrap_err();
            assert!(matches!(err, VerifyError::Signature(_)), "{err}");
            err.to_string()
        };
        let mut changed = BLOB.to_vec();
        changed[0] ^= 1;
        failed(&changed, IDENTITY, ISSUER);
        failed(BLOB, "someone@example.com", ISSUER);
        failed(
            BLOB,
            IDENTITY,
            "https://token.actions.githubusercontent.com",
        );
        assert!(failed(BLOB, IDENTITY, "x").starts_with("signature verification failed"));
        let err = trust.verify(BLOB, b"{}", IDENTITY, ISSUER).unwrap_err();
        assert!(matches!(err, VerifyError::Signature(_)), "{err}");
    }

    #[test]
    fn sums_parse_strict_lines() {
        let text = format!(
            "{}  cyber-1.0.0-x.tar.gz\n{}  SHA256SUMS.sigstore.json\n",
            hex('a'),
            hex('0')
        );
        let sums = Sums::parse(&text).unwrap();
        assert_eq!(sums.get("cyber-1.0.0-x.tar.gz"), Some(&[0xaa; 32]));
        assert_eq!(sums.get("SHA256SUMS.sigstore.json"), Some(&[0; 32]));
        assert_eq!(sums.get("other"), None);
    }

    #[test]
    fn malformed_or_duplicate_sums_are_refused() {
        let bad = [
            String::new(),
            format!("{}  a", hex('a')),
            format!("{}  a\n\n", hex('a')),
            format!("{} a\n", hex('a')),
            format!("{} *a\n", hex('a')),
            format!("{}  a\n", hex('A')),
            format!("{}  a\n", "a".repeat(63)),
            format!("{}  a b\n", hex('a')),
            format!("{}  ../a\n", hex('a')),
            format!("{}  a\\b\n", hex('a')),
            format!("{}  a\r\n", hex('a')),
            format!("{}  a\n{}  a\n", hex('a'), hex('b')),
        ];
        for text in bad {
            let err = Sums::parse(&text).unwrap_err();
            assert!(matches!(err, VerifyError::Sums { .. }), "{text:?}: {err}");
        }
        let err = Sums::parse(&format!("{}  a\n{}  a\n", hex('a'), hex('b'))).unwrap_err();
        assert!(
            err.to_string().contains("line 2: duplicate entry for a"),
            "{err}"
        );
    }

    #[test]
    fn check_file_compares_the_digest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive");
        std::fs::write(&path, b"archive").unwrap();
        let digest: [u8; 32] = Sha256::digest(b"archive").into();
        let line: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        let sums = Sums::parse(&format!("{line}  good\n{}  bad\n", hex('0'))).unwrap();
        sums.check_file("good", &path).unwrap();
        let err = sums.check_file("bad", &path).unwrap_err();
        assert_eq!(err.to_string(), "checksum verification failed: bad");
        let err = sums.check_file("absent", &path).unwrap_err();
        assert_eq!(err.to_string(), "no release for absent");
    }
}
